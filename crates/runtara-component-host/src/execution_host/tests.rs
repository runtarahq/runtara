use super::*;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use wasmtime::{
    Engine, Store,
    component::{Component, Instance, TypedFunc},
};

const INTERFACE: &str = include_str!("interface.wat");

struct State {
    table: ResourceTable,
    context: Option<Arc<ExecutionContext>>,
}
impl ExecutionView for State {
    fn execution_table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
    fn execution_context(&self) -> Option<&Arc<ExecutionContext>> {
        self.context.as_ref()
    }
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
                        })
                    }
                    Entry::Workflow => InvokeExit::Suspended(vec![
                        WorkflowWake::At(1234),
                        WorkflowWake::OnSignal(crate::lifecycle::SignalWait {
                            checkpoint_id: "nested/wait".into(),
                            deadline_ms: None,
                        }),
                        WorkflowWake::OnResume,
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
    engine: Arc<Engine>,
    linker: Linker<State>,
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
            Arc::new(IsolatedTasks::new(engine.clone(), slots, 1024 * 1024).unwrap()),
            Arc::new(Launcher {
                calls: calls.clone(),
                started: started.clone(),
                dropped: dropped.clone(),
            }),
            64,
        )
        .unwrap();
        let mut linker = Linker::new(&engine);
        add_execution_to_linker(&mut linker).unwrap();
        Self {
            engine,
            linker,
            context,
            calls,
            started,
            dropped,
        }
    }
    async fn proxy(&self) -> (Store<State>, Instance) {
        let component = Component::new(
            &self.engine,
            include_str!("proxy.wat").replace("{{INTERFACE}}", INTERFACE),
        )
        .unwrap();
        let mut store = Store::new(
            &self.engine,
            State {
                table: ResourceTable::new(),
                context: Some(self.context.clone()),
            },
        );
        store.set_epoch_deadline(1 << 40);
        let instance = self
            .linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        (store, instance)
    }
}

type Start = TypedFunc<
    (String, Entry, Vec<u8>, InvocationContext),
    (Result<Resource<TaskHandle>, ExecutionError>,),
>;
type Join = TypedFunc<(Resource<TaskHandle>,), (Result<TaskOutcome, ExecutionError>,)>;
type Cancel = TypedFunc<(Resource<TaskHandle>,), (Result<CancelStatus, ExecutionError>,)>;
type Release = TypedFunc<(Resource<TaskHandle>,), (Result<(), ExecutionError>,)>;

fn func<P, R>(store: &mut Store<State>, instance: &Instance, name: &str) -> TypedFunc<P, R>
where
    P: wasmtime::component::ComponentNamedList + wasmtime::component::Lower,
    R: wasmtime::component::ComponentNamedList + wasmtime::component::Lift,
{
    let interface = instance.get_export_index(&mut *store, None, "api").unwrap();
    let index = instance
        .get_export_index(&mut *store, Some(&interface), name)
        .unwrap();
    instance.get_typed_func(store, index).unwrap()
}
fn args(entry: Entry, bytes: Vec<u8>) -> (String, Entry, Vec<u8>, InvocationContext) {
    (
        "child".into(),
        entry,
        bytes,
        InvocationContext {
            path: "parent/step".into(),
            attempt: 7,
        },
    )
}

#[tokio::test]
async fn wire_roundtrip_preserves_owned_bytes_error_metadata_and_wake_sets() {
    let fx = Fixture::new(4);
    let (mut store, instance) = fx.proxy().await;
    let start: Start = func(&mut store, &instance, "start");
    let join: Join = func(&mut store, &instance, "join");
    let release: Release = func(&mut store, &instance, "release");
    for entry in [
        Entry::Capability("echo".into()),
        Entry::Capability("failed".into()),
        Entry::Workflow,
        Entry::Capability("timeout".into()),
        Entry::Capability("trap".into()),
    ] {
        let (result,) = start
            .call_async(&mut store, args(entry.clone(), vec![7; 16385]))
            .await
            .unwrap();
        let handle = result.unwrap();
        let (outcome,) = join
            .call_async(&mut store, (Resource::new_borrow(handle.rep()),))
            .await
            .unwrap();
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
                assert_eq!(wakes.len(), 3);
                assert_eq!(wakes[0], WorkflowWake::At(1234));
                assert!(
                    matches!(&wakes[1], WorkflowWake::OnSignal(s) if s.checkpoint_id == "nested/wait" && s.deadline_ms.is_none())
                );
                assert_eq!(wakes[2], WorkflowWake::OnResume);
            }
            Entry::Capability(name) if name == "timeout" => {
                assert_eq!(outcome, Ok(TaskOutcome::TimedOut))
            }
            _ => assert_eq!(outcome, Ok(TaskOutcome::Trapped("fixture trap".into()))),
        }
        for _ in 0..2 {
            assert_eq!(
                release
                    .call_async(&mut store, (Resource::new_borrow(handle.rep()),))
                    .await
                    .unwrap()
                    .0,
                Ok(())
            );
        }
        assert_eq!(
            join.call_async(&mut store, (Resource::new_borrow(handle.rep()),))
                .await
                .unwrap()
                .0,
            Err(ExecutionError::InvalidTask)
        );
    }
    assert_eq!(fx.calls.lock().unwrap().len(), 5);
    assert!(
        fx.calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, _, c)| c.attempt == 7)
    );
    drop(store);
    fx.context.shutdown().await.unwrap();
}

#[tokio::test]
async fn wire_cancel_join_destroys_pending_future_and_keeps_sibling() {
    let fx = Fixture::new(2);
    let (mut store, instance) = fx.proxy().await;
    let start: Start = func(&mut store, &instance, "start");
    let join: Join = func(&mut store, &instance, "join");
    let cancel: Cancel = func(&mut store, &instance, "request-cancel");
    let handle = start
        .call_async(
            &mut store,
            args(Entry::Capability("pending".into()), vec![]),
        )
        .await
        .unwrap()
        .0
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), fx.started.notified())
        .await
        .unwrap();
    let sibling = start
        .call_async(&mut store, args(Entry::Capability("echo".into()), vec![9]))
        .await
        .unwrap()
        .0
        .unwrap();
    let status = cancel
        .call_async(&mut store, (Resource::new_borrow(handle.rep()),))
        .await
        .unwrap()
        .0
        .unwrap();
    assert_eq!(status, CancelStatus::Requested);
    assert_eq!(
        join.call_async(&mut store, (Resource::new_borrow(handle.rep()),))
            .await
            .unwrap()
            .0,
        Ok(TaskOutcome::Cancelled)
    );
    assert!(fx.dropped.load(Ordering::Acquire));
    assert_eq!(
        join.call_async(&mut store, (Resource::new_borrow(sibling.rep()),))
            .await
            .unwrap()
            .0,
        Ok(TaskOutcome::Completed(vec![9]))
    );
    assert_eq!(
        cancel
            .call_async(&mut store, (Resource::new_borrow(sibling.rep()),))
            .await
            .unwrap()
            .0,
        Ok(CancelStatus::AlreadyTerminal)
    );
    drop(store);
    fx.context.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_context_binding_capacity_and_missing_runtime_are_explicit() {
    let fx = Fixture::new(0);
    let (mut store, instance) = fx.proxy().await;
    let start: Start = func(&mut store, &instance, "start");
    let mut input = args(Entry::Workflow, vec![]);
    input.0 = "foreign-package".into();
    assert_eq!(
        start.call_async(&mut store, input).await.unwrap().0.err(),
        Some(ExecutionError::InvalidBinding)
    );
    let mut input = args(Entry::Workflow, vec![]);
    input.3.path = "other-parent/step".into();
    assert_eq!(
        start.call_async(&mut store, input).await.unwrap().0.err(),
        Some(ExecutionError::InvalidContext)
    );
    assert_eq!(
        start
            .call_async(&mut store, args(Entry::Workflow, vec![]))
            .await
            .unwrap()
            .0
            .err(),
        Some(ExecutionError::Capacity)
    );
    fx.context.shutdown().await.unwrap();
    assert_eq!(
        start
            .call_async(&mut store, args(Entry::Workflow, vec![]))
            .await
            .unwrap()
            .0
            .err(),
        Some(ExecutionError::Closed)
    );
    store.data_mut().context = None;
    assert_eq!(
        start
            .call_async(&mut store, args(Entry::Workflow, vec![]))
            .await
            .unwrap()
            .0
            .err(),
        Some(ExecutionError::Unavailable)
    );
    assert!(fx.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn host_resource_cannot_be_transplanted_to_another_parent_context() {
    let fx = Fixture::new(1);
    let (mut store, instance) = fx.proxy().await;
    let start: Start = func(&mut store, &instance, "start");
    let handle = start
        .call_async(
            &mut store,
            args(Entry::Capability("pending".into()), vec![]),
        )
        .await
        .unwrap()
        .0
        .unwrap();
    let other = Fixture::new(1);
    store.data_mut().context = Some(other.context.clone());
    let join: Join = func(&mut store, &instance, "join");
    let error = join
        .call_async(&mut store, (Resource::new_borrow(handle.rep()),))
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("another parent"));
    drop(store);
    fx.context.shutdown().await.unwrap();
    other.context.shutdown().await.unwrap();
}

async fn run_parent(early_exit: &str) -> (Fixture, Result<u32, String>) {
    let mut fx = Fixture::new(2);
    let started = fx.started.clone();
    fx.linker
        .root()
        .func_wrap_concurrent("wait-started", move |_accessor, (): ()| {
            let started = started.clone();
            Box::pin(async move {
                started.notified().await;
                Ok(())
            })
        })
        .unwrap();
    let wat = include_str!("parent.wat")
        .replace("{{INTERFACE}}", INTERFACE)
        .replace("{{EARLY_EXIT}}", early_exit);
    let component = Component::new(&fx.engine, wat).unwrap();
    let mut store = Store::new(
        &fx.engine,
        State {
            table: ResourceTable::new(),
            context: Some(fx.context.clone()),
        },
    );
    store.set_epoch_deadline(1 << 40);
    let instance = fx
        .linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let run = instance
        .get_typed_func::<(), (u32,)>(&mut store, "run")
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), run.call_async(&mut store, ()))
        .await
        .expect("parent failed to settle")
        .map(|(value,)| value)
        .map_err(|error| format!("{error:#}"));
    drop(store);
    fx.context.shutdown().await.unwrap();
    (fx, result)
}

#[tokio::test]
async fn parent_wasm_starts_joins_cancels_releases_and_runs_its_recovery() {
    let (fx, result) = run_parent("").await;
    assert_eq!(result, Ok(42));
    assert_eq!(fx.calls.lock().unwrap().len(), 2);
    assert!(fx.dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn parent_trap_drops_owned_resources_and_shutdown_reaps_child() {
    let (fx, result) = run_parent("unreachable").await;
    assert!(result.unwrap_err().contains("unreachable"));
    assert_eq!(fx.calls.lock().unwrap().len(), 1);
    assert!(fx.dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn resource_destructor_cancels_and_reaps_without_an_explicit_release() {
    let (fx, result) = run_parent("local.get $pending call $drop i32.const 17 return").await;
    assert_eq!(result, Ok(17));
    assert_eq!(fx.calls.lock().unwrap().len(), 1);
    assert!(fx.dropped.load(Ordering::Acquire));
}

#[cfg(feature = "component-integration-tests")]
#[path = "real_agents.rs"]
mod real_agents;

#[tokio::test]
async fn released_but_undropped_resources_remain_charged() {
    let mut fx = Fixture::new(1);
    fx.context =
        ExecutionContext::new(fx.context.tasks.clone(), fx.context.launcher.clone(), 1).unwrap();
    let (mut store, instance) = fx.proxy().await;
    let start: Start = func(&mut store, &instance, "start");
    let release: Release = func(&mut store, &instance, "release");
    let handle = start
        .call_async(&mut store, args(Entry::Capability("echo".into()), vec![]))
        .await
        .unwrap()
        .0
        .unwrap();
    release
        .call_async(&mut store, (Resource::new_borrow(handle.rep()),))
        .await
        .unwrap()
        .0
        .unwrap();
    assert_eq!(
        start
            .call_async(&mut store, args(Entry::Capability("echo".into()), vec![]))
            .await
            .unwrap()
            .0
            .err(),
        Some(ExecutionError::Capacity)
    );
    // The proxy transferred this owned resource to its host caller. The host
    // must delete its table entry, just as canonical resource.drop would do.
    drop(store.data_mut().table.delete(handle).unwrap());
    let next = start
        .call_async(&mut store, args(Entry::Capability("echo".into()), vec![]))
        .await
        .unwrap()
        .0;
    assert!(next.is_ok());
    drop(store);
    fx.context.shutdown().await.unwrap();
}

#[tokio::test]
async fn scoped_wire_join_waits_for_cleanup_and_surfaces_cleanup_failure() {
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
                cleanup: Some(Box::pin(async move {
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
        let (mut store, instance) = fx.proxy().await;
        let start: Start = func(&mut store, &instance, "start");
        let join: Join = func(&mut store, &instance, "join");
        let handle = start
            .call_async(&mut store, args(Entry::Workflow, vec![]))
            .await
            .unwrap()
            .0
            .unwrap();
        let result = join
            .call_async(&mut store, (Resource::new_borrow(handle.rep()),))
            .await
            .unwrap()
            .0;
        assert!(cleaned.load(Ordering::Acquire));
        assert_eq!(
            result,
            if fail {
                Err(ExecutionError::WorkerLost)
            } else {
                Ok(TaskOutcome::Completed(vec![42]))
            }
        );
        drop(store);
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
