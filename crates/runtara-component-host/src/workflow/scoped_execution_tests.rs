use super::super::test_support::{Ticker, bounded, parent_wat, spec};
use super::*;
use crate::execution_host::{
    Entry, ExecutionError, InvocationLauncher, PreparedInvocation, StartRequest,
};
use crate::isolated_tasks::{IsolatedTasks, TaskError};
use std::sync::atomic::AtomicUsize;

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Default)]
struct Signals {
    started: Notify,
    ready: Notify,
    dropped: Arc<AtomicBool>,
    cleanup_entered: Notify,
    cleanup_release: Notify,
    cleanup_done: AtomicBool,
    calls: AtomicUsize,
}
struct Launcher {
    signals: Arc<Signals>,
    hold_cleanup: bool,
    fail_cleanup: bool,
}
impl InvocationLauncher for Launcher {
    fn prepare(&self, request: StartRequest) -> Result<PreparedInvocation, ExecutionError> {
        if request.binding != "child" {
            return Err(ExecutionError::InvalidBinding);
        }
        if request.context.path != "parent/step" {
            return Err(ExecutionError::InvalidContext);
        }
        let pending = request.entry == Entry::Capability("pending".into());
        let signals = self.signals.clone();
        let run = Box::new(move |_token| -> crate::execution_host::ExecutionFuture {
            Box::pin(async move {
                signals.calls.fetch_add(1, Ordering::AcqRel);
                if pending {
                    let _dropped = Dropped(signals.dropped.clone());
                    signals.started.notify_one();
                    std::future::pending::<()>().await;
                    unreachable!();
                }
                signals.started.notified().await;
                signals.ready.notify_one();
                InvokeExit::Completed(request.input)
            })
        });
        if !pending {
            return Ok(PreparedInvocation::leaf(run));
        }
        let signals = self.signals.clone();
        let (hold, fail) = (self.hold_cleanup, self.fail_cleanup);
        Ok(PreparedInvocation {
            run,
            cleanup: Some(Box::pin(async move {
                signals.cleanup_entered.notify_one();
                if hold {
                    signals.cleanup_release.notified().await;
                }
                signals.cleanup_done.store(true, Ordering::Release);
                if fail {
                    Err(TaskError::WorkerLost)
                } else {
                    Ok(())
                }
            })),
        })
    }
}
struct Fixture {
    executor: Arc<WorkflowExecutor>,
    context: Arc<ExecutionContext>,
    signals: Arc<Signals>,
    tasks: Arc<IsolatedTasks>,
    _ticker: Ticker,
}
impl Fixture {
    fn new(hold_cleanup: bool, fail_cleanup: bool) -> Self {
        let engine = crate::build_engine(&crate::EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let executor = Arc::new(WorkflowExecutor::new(engine.clone()).unwrap());
        let signals = Arc::new(Signals::default());
        let tasks = Arc::new(IsolatedTasks::new(engine.clone(), 8, 1024 * 1024).unwrap());
        let context = ExecutionContext::new(
            tasks.clone(),
            Arc::new(Launcher {
                signals: signals.clone(),
                hold_cleanup,
                fail_cleanup,
            }),
            8,
        )
        .unwrap();
        Self {
            executor,
            context,
            signals,
            tasks,
            _ticker: Ticker::new(engine),
        }
    }

    fn pre(&self, root_exit: &str) -> InstancePre<WorkflowState> {
        let component = Component::new(self.executor.engine(), parent_wat(root_exit)).unwrap();
        self.executor.linker.instantiate_pre(&component).unwrap()
    }

    fn run(
        &self,
        root_exit: &str,
        spec: WorkflowRunSpec,
    ) -> tokio::task::JoinHandle<InvokeRunResult> {
        let pre = self.pre(root_exit);
        let executor = self.executor.clone();
        let context = self.context.clone();
        tokio::spawn(async move {
            executor
                .execute_invoke_with_context(&pre, spec, vec![], None, context)
                .await
        })
    }
}
#[tokio::test]
async fn production_parent_cancels_child_and_returns_its_recovery() {
    let fx = Fixture::new(false, false);
    let result = bounded(fx.run("", spec())).await.unwrap();
    assert!(
        matches!(result.exit, InvokeExit::Completed(ref bytes) if bytes == b"42"),
        "{result:?}"
    );
    assert_eq!(fx.signals.calls.load(Ordering::Acquire), 2);
    assert!(fx.signals.dropped.load(Ordering::Acquire));
    assert!(fx.signals.cleanup_done.load(Ordering::Acquire));
    assert_eq!(fx.tasks.retained_result_bytes(), 0);
    assert!(matches!(
        fx.tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::Closed)
    ));
}

#[tokio::test]
async fn production_root_success_and_trap_wait_for_unreleased_child_cleanup() {
    for root_exit in ["i32.const 42 return", "unreachable"] {
        let fx = Fixture::new(true, false);
        let run = fx.run(root_exit, spec());
        bounded(fx.signals.cleanup_entered.notified()).await;
        assert!(fx.signals.dropped.load(Ordering::Acquire));
        assert!(
            !run.is_finished(),
            "root published before descendant cleanup"
        );
        fx.signals.cleanup_release.notify_one();
        let result = bounded(run).await.unwrap();
        if root_exit == "unreachable" {
            assert!(
                matches!(result.exit, InvokeExit::Trapped { .. }),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(result.exit, InvokeExit::Completed(_)),
                "{result:?}"
            );
        }
        assert!(fx.signals.cleanup_done.load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn cleanup_failure_overrides_successful_root_result() {
    let fx = Fixture::new(false, true);
    let result = bounded(fx.run("i32.const 42 return", spec()))
        .await
        .unwrap();
    assert!(
        matches!(result.exit, InvokeExit::Trapped { ref reason } if reason.contains("descendant cleanup failed")),
        "{result:?}"
    );
}

#[tokio::test]
async fn abandoned_root_future_still_reaps_owned_children() {
    let fx = Fixture::new(true, false);
    // CPU loop yields at epochs; abort must also trigger an epoch to stop it.
    let run = fx.run("(loop $forever (br $forever))", spec());
    bounded(fx.signals.ready.notified()).await;
    run.abort();
    assert!(bounded(run).await.unwrap_err().is_cancelled());
    bounded(fx.signals.cleanup_entered.notified()).await;
    assert!(fx.signals.dropped.load(Ordering::Acquire));
    fx.signals.cleanup_release.notify_one();
    // Observe the *supervisor's* completed teardown, not an independent shutdown.
    bounded(async {
        while !fx.signals.cleanup_done.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test]
async fn legacy_execution_has_no_launcher_even_when_artifact_imports_tasks() {
    let fx = Fixture::new(false, false);
    let result = bounded(fx.executor.execute_invoke(&fx.pre(""), spec(), vec![])).await;
    assert!(
        matches!(result.exit, InvokeExit::Trapped { .. }),
        "{result:?}"
    );
    assert_eq!(fx.signals.calls.load(Ordering::Acquire), 0);
    fx.context.shutdown().await.unwrap();
}

#[tokio::test]
async fn root_timeout_and_cancel_reap_cpu_and_pending_join_children() {
    for timeout in [false, true] {
        let fx = Fixture::new(true, false);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut config = spec();
        config.cancel = Some(cancel.clone());
        if timeout {
            config.timeout = Duration::from_millis(150);
        }
        let root_exit = if timeout {
            "(loop $forever (br $forever))"
        } else {
            "(call $join (local.get $pending) (i32.const 512)) unreachable"
        };
        let run = fx.run(root_exit, config);
        bounded(fx.signals.ready.notified()).await;
        if !timeout {
            cancel.store(true, Ordering::Release);
        }
        bounded(fx.signals.cleanup_entered.notified()).await;
        assert!(fx.signals.dropped.load(Ordering::Acquire));
        assert!(!run.is_finished());
        fx.signals.cleanup_release.notify_one();
        let result = bounded(run).await.unwrap();
        if timeout {
            assert!(matches!(result.exit, InvokeExit::Timeout), "{result:?}");
        } else {
            assert!(matches!(result.exit, InvokeExit::Cancelled), "{result:?}");
        }
        assert_eq!(fx.tasks.retained_result_bytes(), 0);
    }
}

#[tokio::test]
async fn prestart_cancel_does_not_admit_children_and_closes_scope() {
    let fx = Fixture::new(false, false);
    let mut config = spec();
    config.cancel = Some(Arc::new(AtomicBool::new(true)));
    let result = bounded(fx.run("", config)).await.unwrap();
    assert!(matches!(result.exit, InvokeExit::Cancelled), "{result:?}");
    assert_eq!(fx.signals.calls.load(Ordering::Acquire), 0);
    assert!(matches!(
        fx.tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::Closed)
    ));
}

struct Gate {
    entered: Arc<Notify>,
    dropped: Arc<AtomicBool>,
    mode: &'static str,
}
#[async_trait]
impl WorkflowStartConfirmation for Gate {
    async fn confirm_before_instantiate(&self) -> Result<()> {
        let _dropped = Dropped(self.dropped.clone());
        self.entered.notify_one();
        match self.mode {
            "pending" => std::future::pending::<()>().await,
            "panic" => panic!("fixture gate panic"),
            _ => anyhow::bail!("fixture gate closed"),
        }
        Ok(())
    }
}

#[tokio::test]
async fn failed_panicked_and_abandoned_start_gates_close_scope_without_guest_entry() {
    for mode in ["closed", "panic", "pending"] {
        let fx = Fixture::new(false, false);
        let entered = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(Gate {
            entered: entered.clone(),
            dropped: dropped.clone(),
            mode,
        });
        let executor = fx.executor.clone();
        let context = fx.context.clone();
        let pre = fx.pre("");
        let run = tokio::spawn(async move {
            executor
                .execute_invoke_with_context(&pre, spec(), vec![], Some(gate), context)
                .await
        });
        bounded(entered.notified()).await;
        if mode == "pending" {
            run.abort();
            assert!(bounded(run).await.unwrap_err().is_cancelled());
            bounded(async {
                while !dropped.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            // Wait for the supervisor to close admission; release any probe
            // admitted during the brief worker-to-cleanup handoff.
            bounded(async {
                loop {
                    match fx.tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }) {
                        Err(TaskError::Closed) => break,
                        Ok(id) => fx.tasks.release(id).await.unwrap(),
                        other => panic!("{other:?}"),
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
        } else {
            let result = bounded(run).await.unwrap();
            assert!(
                matches!(result.exit, InvokeExit::Trapped { ref reason } if reason.contains(if mode == "panic" { "worker lost" } else { "gate closed" })),
                "{result:?}"
            );
            assert!(dropped.load(Ordering::Acquire));
        }
        assert_eq!(fx.signals.calls.load(Ordering::Acquire), 0);
    }
}

#[tokio::test]
async fn isolated_lifecycle_entry_uses_task_and_root_cancellation_with_separate_cleanup() {
    for cancel_root in [false, true] {
        let fx = Fixture::new(true, false);
        let registry = IsolatedTasks::new(fx.executor.engine().clone(), 2, 1024).unwrap();
        let executor = fx.executor.clone();
        let pre = fx.pre("(call $join (local.get $pending) (i32.const 512)) unreachable");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut config = spec();
        config.cancel = Some(cancel.clone());
        let context = fx.context.clone();
        let child = registry
            .spawn_scoped(
                move |token| async move {
                    executor
                        .execute_isolated_workflow(&pre, config, vec![], token, Some(context))
                        .await
                        .exit
                },
                fx.context.clone().into_cleanup(),
            )
            .unwrap();
        bounded(fx.signals.ready.notified()).await;
        if cancel_root {
            cancel.store(true, Ordering::Release);
        } else {
            registry.cancel(child).unwrap();
        }
        bounded(fx.signals.cleanup_entered.notified()).await;
        assert!(fx.signals.dropped.load(Ordering::Acquire));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), registry.join(child))
                .await
                .is_err()
        );
        fx.signals.cleanup_release.notify_one();
        assert!(matches!(
            bounded(registry.join(child)).await.unwrap().outcome(),
            InvokeExit::Cancelled
        ));
        assert!(fx.signals.cleanup_done.load(Ordering::Acquire));
        registry.shutdown().await.unwrap();
    }
}

#[path = "terminal_publication_tests.rs"]
mod terminal_publication_tests;
