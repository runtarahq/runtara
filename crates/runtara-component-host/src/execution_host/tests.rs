use super::*;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[tokio::test]
async fn a_dropped_task_keeps_its_owners_teardown_disposition() {
    struct WaitingLauncher(
        Arc<Mutex<Option<TeardownDisposition>>>,
        Arc<tokio::sync::Notify>,
    );
    impl InvocationLauncher for WaitingLauncher {
        fn prepare(&self, _: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
            let observed = self.0.clone();
            let entered = self.1.clone();
            Ok(PreparedInvocation {
                run: Box::new(move |_| {
                    Box::pin(async move {
                        entered.notify_one();
                        std::future::pending().await
                    })
                }),
                cleanup: Some(TaskCleanup::with_disposition(
                    move |disposition| async move {
                        *observed.lock().unwrap() = Some(disposition);
                        Ok(())
                    },
                )),
                lifecycle: None,
            })
        }
    }
    let mut fx = Fixture::new(1);
    let disposition = Arc::new(Mutex::new(None));
    let entered = Arc::new(tokio::sync::Notify::new());
    fx.context = ExecutionContext::new(
        fx.context.tasks.clone(),
        Arc::new(WaitingLauncher(disposition.clone(), entered.clone())),
        1,
    )
    .unwrap();
    let task = fx
        .context
        .start_task(request(Entry::Workflow, vec![]))
        .unwrap();
    entered.notified().await;
    drop(task);
    fx.context
        .shutdown_with(TeardownDisposition::Resumable)
        .await
        .unwrap();
    assert_eq!(
        *disposition.lock().unwrap(),
        Some(TeardownDisposition::Resumable)
    );
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct Launcher {
    calls: Arc<Mutex<Vec<(String, Entry, InvocationContext)>>>,
    started: Arc<tokio::sync::Notify>,
    dropped: Arc<AtomicBool>,
}
impl InvocationLauncher for Launcher {
    fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
        if request.context.path != "parent/step" {
            return Err(ExecutionError::InvalidContext);
        }
        if request.binding != "child" {
            return Err(ExecutionError::InvalidBinding);
        }
        let calls = self.calls.clone();
        let started = self.started.clone();
        let dropped = self.dropped.clone();
        Ok(PreparedInvocation::leaf(Box::new(move |_token| {
            Box::pin(async move {
                let _guard = Dropped(dropped);
                let pending = request.entry == Entry::Capability("pending".into());
                let mode = request.entry.clone();
                calls
                    .lock()
                    .unwrap()
                    .push((request.binding, request.entry, request.context));
                started.notify_one();
                if pending {
                    std::future::pending::<()>().await;
                }
                match mode {
                    Entry::Capability(ref name) if name == "failed" => {
                        InvokeExit::Failed(WorkflowErrorInfo {
                            code: "RATE_LIMIT".into(),
                            message: "retry later".into(),
                            category: "transient".into(),
                            severity: "warning".into(),
                            retryable: true,
                            retry_after_ms: Some(1234),
                            attributes: Some("{}".into()),
                            details: None,
                        })
                    }
                    Entry::Workflow => InvokeExit::Suspended(vec![
                        WorkflowWake::At(1234),
                        WorkflowWake::OnSignal(crate::lifecycle::SignalWait {
                            checkpoint_id: "nested/wait".into(),
                            deadline_ms: None,
                        }),
                        WorkflowWake::OnResume,
                        WorkflowWake::Instances("nested/instances".into()),
                    ]),
                    Entry::Capability(ref name) if name == "timeout" => InvokeExit::Timeout,
                    Entry::Capability(ref name) if name == "trap" => InvokeExit::Trapped {
                        reason: "fixture trap".into(),
                    },
                    _ => InvokeExit::Completed(request.input),
                }
            })
        })))
    }
}

struct Fixture {
    context: Arc<ExecutionContext>,
    calls: Arc<Mutex<Vec<(String, Entry, InvocationContext)>>>,
    started: Arc<tokio::sync::Notify>,
    dropped: Arc<AtomicBool>,
}
impl Fixture {
    fn new(slots: usize) -> Self {
        let engine = crate::build_engine(&crate::EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let started = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let context = ExecutionContext::new(
            Arc::new(IsolatedTasks::new(engine, slots, 1024 * 1024).unwrap()),
            Arc::new(Launcher {
                calls: calls.clone(),
                started: started.clone(),
                dropped: dropped.clone(),
            }),
            64,
        )
        .unwrap();
        Self {
            context,
            calls,
            started,
            dropped,
        }
    }
}

fn request(entry: Entry, input: Vec<u8>) -> StartRequest {
    StartRequest {
        binding: "child".into(),
        entry,
        input,
        context: InvocationContext {
            path: "parent/step".into(),
            attempt: 7,
        },
    }
}

#[tokio::test]
async fn outcomes_keep_owned_bytes_error_metadata_and_wake_sets() {
    let fx = Fixture::new(4);
    for entry in [
        Entry::Capability("echo".into()),
        Entry::Capability("failed".into()),
        Entry::Workflow,
        Entry::Capability("timeout".into()),
        Entry::Capability("trap".into()),
    ] {
        let task = fx
            .context
            .start_task(request(entry.clone(), vec![7; 16385]))
            .unwrap();
        let outcome = task.join().await;
        match entry {
            Entry::Capability(name) if name == "echo" => {
                assert_eq!(outcome, Ok(TaskOutcome::Completed(vec![7; 16385])))
            }
            Entry::Capability(name) if name == "failed" => {
                let Ok(TaskOutcome::Failed(error)) = outcome else {
                    panic!("{outcome:?}")
                };
                assert_eq!(error.retry_after_ms, Some(1234));
                assert_eq!(error.code, "RATE_LIMIT");
                assert!(error.retryable);
            }
            Entry::Workflow => {
                let Ok(TaskOutcome::Suspended(wakes)) = outcome else {
                    panic!("{outcome:?}")
                };
                assert_eq!(wakes.len(), 4);
                assert_eq!(wakes[0], WorkflowWake::At(1234));
                assert!(
                    matches!(&wakes[1], WorkflowWake::OnSignal(s) if s.checkpoint_id == "nested/wait" && s.deadline_ms.is_none())
                );
                assert_eq!(wakes[2], WorkflowWake::OnResume);
                assert_eq!(wakes[3], WorkflowWake::Instances("nested/instances".into()));
            }
            Entry::Capability(name) if name == "timeout" => {
                assert_eq!(outcome, Ok(TaskOutcome::TimedOut))
            }
            _ => assert_eq!(outcome, Ok(TaskOutcome::Trapped("fixture trap".into()))),
        }
        for _ in 0..2 {
            assert_eq!(task.release().await, Ok(()));
        }
        assert_eq!(task.join().await, Err(ExecutionError::InvalidTask));
    }
    assert_eq!(fx.calls.lock().unwrap().len(), 5);
    assert!(
        fx.calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, _, c)| c.attempt == 7)
    );
    fx.context.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_destroys_the_pending_child_and_keeps_its_sibling() {
    let fx = Fixture::new(2);
    let task = fx
        .context
        .start_task(request(Entry::Capability("pending".into()), vec![]))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), fx.started.notified())
        .await
        .unwrap();
    let sibling = fx
        .context
        .start_task(request(Entry::Capability("echo".into()), vec![9]))
        .unwrap();
    assert_eq!(task.request_cancel(), Ok(CancelStatus::Requested));
    assert_eq!(task.join().await, Ok(TaskOutcome::Cancelled));
    assert!(fx.dropped.load(Ordering::Acquire));
    assert_eq!(sibling.join().await, Ok(TaskOutcome::Completed(vec![9])));
    assert_eq!(sibling.request_cancel(), Ok(CancelStatus::AlreadyTerminal));
    drop((task, sibling));
    fx.context.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_context_binding_and_capacity_are_explicit() {
    let fx = Fixture::new(0);
    let mut input = request(Entry::Workflow, vec![]);
    input.binding = "foreign-package".into();
    assert_eq!(
        fx.context.start_task(input).err(),
        Some(ExecutionError::InvalidBinding)
    );
    let mut input = request(Entry::Workflow, vec![]);
    input.context.path = "other-parent/step".into();
    assert_eq!(
        fx.context.start_task(input).err(),
        Some(ExecutionError::InvalidContext)
    );
    assert_eq!(
        fx.context
            .start_task(request(Entry::Workflow, vec![]))
            .err(),
        Some(ExecutionError::Capacity)
    );
    fx.context.shutdown().await.unwrap();
    assert_eq!(
        fx.context
            .start_task(request(Entry::Workflow, vec![]))
            .err(),
        Some(ExecutionError::Closed)
    );
    assert!(fx.calls.lock().unwrap().is_empty());
}

#[cfg(feature = "component-integration-tests")]
#[path = "real_agents.rs"]
mod real_agents;

#[tokio::test]
async fn a_released_task_stays_charged_until_it_is_dropped() {
    let mut fx = Fixture::new(1);
    fx.context =
        ExecutionContext::new(fx.context.tasks.clone(), fx.context.launcher.clone(), 1).unwrap();
    let task = fx
        .context
        .start_task(request(Entry::Capability("echo".into()), vec![]))
        .unwrap();
    task.release().await.unwrap();
    assert_eq!(
        fx.context
            .start_task(request(Entry::Capability("echo".into()), vec![]))
            .err(),
        Some(ExecutionError::Capacity)
    );
    drop(task);
    let next = fx
        .context
        .start_task(request(Entry::Capability("echo".into()), vec![]));
    assert!(next.is_ok());
    drop(next);
    fx.context.shutdown().await.unwrap();
}

#[tokio::test]
async fn join_waits_for_cleanup_and_surfaces_its_failure() {
    struct ScopedLauncher {
        cleaned: Arc<AtomicBool>,
        fail: bool,
    }
    impl InvocationLauncher for ScopedLauncher {
        fn prepare(&self, _: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
            let cleaned = self.cleaned.clone();
            let fail = self.fail;
            Ok(PreparedInvocation {
                lifecycle: None,
                run: Box::new(|_| Box::pin(async { InvokeExit::Completed(vec![42]) })),
                cleanup: Some(crate::isolated_tasks::TaskCleanup::new(async move {
                    cleaned.store(true, Ordering::Release);
                    if fail {
                        Err(TaskError::WorkerLost)
                    } else {
                        Ok(())
                    }
                })),
            })
        }
    }
    for fail in [false, true] {
        let mut fx = Fixture::new(1);
        let cleaned = Arc::new(AtomicBool::new(false));
        fx.context = ExecutionContext::new(
            fx.context.tasks.clone(),
            Arc::new(ScopedLauncher {
                cleaned: cleaned.clone(),
                fail,
            }),
            1,
        )
        .unwrap();
        let task = fx
            .context
            .start_task(request(Entry::Workflow, vec![]))
            .unwrap();
        let result = task.join().await;
        assert!(cleaned.load(Ordering::Acquire));
        assert_eq!(
            result,
            if fail {
                Err(ExecutionError::WorkerLost)
            } else {
                Ok(TaskOutcome::Completed(vec![42]))
            }
        );
        drop(task);
        assert_eq!(
            fx.context.shutdown().await,
            if fail {
                Err(ExecutionError::WorkerLost)
            } else {
                Ok(())
            }
        );
    }
}
