use super::*;
use crate::{EngineConfig, build_engine};
use std::time::Duration;
use tokio::sync::oneshot;

fn registry(slots: usize, bytes: usize) -> IsolatedTasks {
    IsolatedTasks::new(
        build_engine(&EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap(),
        slots,
        bytes,
    )
    .unwrap()
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn cancellation_before_first_poll_never_calls_factory() {
    let tasks = registry(1, 1024);
    let started = Arc::new(AtomicBool::new(false));
    let flag = started.clone();
    let id = tasks
        .spawn(move |_| {
            flag.store(true, Ordering::Release);
            async { InvokeExit::Completed(vec![1]) }
        })
        .unwrap();
    assert_eq!(tasks.cancel(id), Ok(CancelResult::Requested));
    assert_eq!(tasks.cancel(id), Ok(CancelResult::AlreadyRequested));
    assert!(matches!(
        tasks.join(id).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    assert!(!started.load(Ordering::Acquire));
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_wins_when_completion_is_ready_but_not_published() {
    let tasks = registry(1, 1024);
    let (tx, rx) = oneshot::channel();
    let id = tasks
        .spawn(move |_| async move {
            rx.await.unwrap();
            InvokeExit::Completed(vec![1])
        })
        .unwrap();
    tokio::task::yield_now().await;
    tx.send(()).unwrap();
    tasks.cancel(id).unwrap();
    assert!(matches!(
        tasks.join(id).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn committed_completion_survives_late_cancel() {
    let tasks = registry(1, 1024);
    let id = tasks
        .spawn(|_| async { InvokeExit::Completed(vec![7]) })
        .unwrap();
    let result = tasks.join(id).await.unwrap();
    assert!(matches!(result.outcome(),InvokeExit::Completed(b) if b==&[7]));
    assert_eq!(tasks.cancel(id), Ok(CancelResult::AlreadyTerminal));
    tasks.release(id).await.unwrap();
    tasks.release(id).await.unwrap();
    assert!(matches!(tasks.join(id).await, Err(TaskError::UnknownTask)));
}

#[tokio::test]
async fn cancel_drops_pending_execution_before_join_and_keeps_sibling() {
    let tasks = registry(2, 1024);
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(dropped.clone());
    let (tx, rx) = oneshot::channel();
    let id = tasks
        .spawn(move |_| async move {
            let _guard = guard;
            tx.send(()).unwrap();
            std::future::pending().await
        })
        .unwrap();
    let sibling = tasks
        .spawn(|_| async { InvokeExit::Completed(vec![9]) })
        .unwrap();
    rx.await.unwrap();
    tasks.cancel(id).unwrap();
    assert!(matches!(
        tasks.join(id).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    assert!(dropped.load(Ordering::Acquire));
    assert!(
        matches!(tasks.join(sibling).await.unwrap().outcome(),InvokeExit::Completed(b) if b==&[9])
    );
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn handles_are_owner_scoped_and_never_reused() {
    let a = registry(1, 1024);
    let b = registry(1, 1024);
    let first = a
        .spawn(|_| async { InvokeExit::Completed(vec![]) })
        .unwrap();
    assert_eq!(b.cancel(first), Err(TaskError::WrongOwner));
    assert_eq!(b.release(first).await, Err(TaskError::WrongOwner));
    a.release(first).await.unwrap();
    let next = a
        .spawn(|_| async { InvokeExit::Completed(vec![]) })
        .unwrap();
    assert_ne!(first, next);
    assert_eq!(a.cancel(first), Err(TaskError::UnknownTask));
    a.shutdown().await.unwrap();
}

#[tokio::test]
async fn retained_handles_bound_admission_and_release_returns_capacity() {
    let tasks = registry(1, 1024);
    let first = tasks
        .spawn(|_| async { InvokeExit::Completed(vec![1]) })
        .unwrap();
    tasks.join(first).await.unwrap();
    assert_eq!(
        tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::AtCapacity)
    );
    tasks.release(first).await.unwrap();
    tasks
        .spawn(|_| async { InvokeExit::Completed(vec![]) })
        .unwrap();
    tasks.shutdown().await.unwrap();
    assert_eq!(
        tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::Closed)
    );
}

#[tokio::test]
async fn result_budget_is_aggregate_and_charged_until_last_reader_drops() {
    let tasks = registry(2, 8);
    let first = tasks
        .spawn(|_| async { InvokeExit::Completed(vec![0; 8]) })
        .unwrap();
    let result = tasks.join(first).await.unwrap();
    tasks.release(first).await.unwrap();
    assert_eq!(tasks.retained_result_bytes(), 8);
    let second = tasks
        .spawn(|_| async { InvokeExit::Completed(vec![0; 8]) })
        .unwrap();
    assert!(
        matches!(tasks.join(second).await.unwrap().outcome(),InvokeExit::Trapped {reason} if reason.contains("budget"))
    );
    drop(result);
    assert_eq!(tasks.retained_result_bytes(), 0);
    tasks.release(second).await.unwrap();
    let third = tasks
        .spawn(|_| async { InvokeExit::Completed(vec![0; 8]) })
        .unwrap();
    assert!(matches!(
        tasks.join(third).await.unwrap().outcome(),
        InvokeExit::Completed(_)
    ));
    tasks.shutdown().await.unwrap();
    assert_eq!(tasks.retained_result_bytes(), 0);
}

#[tokio::test]
async fn result_budget_counts_reserved_capacity_not_only_payload_length() {
    let tasks = registry(1, 8);
    let id = tasks
        .spawn(|_| async {
            let mut bytes = Vec::with_capacity(1024);
            bytes.push(1);
            InvokeExit::Completed(bytes)
        })
        .unwrap();
    assert!(matches!(
        tasks.join(id).await.unwrap().outcome(),
        InvokeExit::Trapped { .. }
    ));
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn worker_panic_is_terminal_and_reaped() {
    let tasks = registry(1, 1024);
    let id = tasks
        .spawn(|_| async { panic!("test worker panic") })
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), tasks.join(id))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result.outcome(), InvokeExit::Trapped { .. }));
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_reaps_pending_children_and_is_idempotent() {
    let tasks = registry(2, 1024);
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(dropped.clone());
    let (tx, rx) = oneshot::channel();
    tasks
        .spawn(move |_| async move {
            let _guard = guard;
            tx.send(()).unwrap();
            std::future::pending().await
        })
        .unwrap();
    rx.await.unwrap();
    tasks.shutdown().await.unwrap();
    tasks.shutdown().await.unwrap();
    assert!(dropped.load(Ordering::Acquire));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_non_cooperative_wasm_is_interrupted_and_its_store_dropped() {
    assert_wasm_is_stopped(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn infinite_wasm_initializer_is_interrupted_and_its_store_dropped() {
    assert_wasm_is_stopped(true, false).await;
}

#[test]
fn spawn_outside_runtime_is_an_error_without_poisoning_registry() {
    let tasks = registry(1, 1024);
    assert_eq!(
        tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::NoRuntime)
    );
    assert_eq!(tasks.retained_result_bytes(), 0);
}

async fn assert_wasm_is_stopped(initializer: bool, nested: bool) {
    let children = Arc::new(registry(2, 1024));
    let engine = children.engine.clone();
    let entry = if initializer {
        "(start $busy) (func (export \"run\"))"
    } else {
        "(export \"run\" (func $busy))"
    };
    let module = wasmtime::Module::new(
        &engine,
        format!(
            r#"(module
        (import "host" "started" (func $started))
        (func $busy (call $started) (loop $loop br $loop)) {entry})"#
        ),
    )
    .unwrap();
    let (tx, rx) = oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(dropped.clone());
    let child_factory = move |cancel: TaskCancellation| async move {
        let mut store = wasmtime::Store::new(&engine, (cancel, Some(tx), guard));
        store.epoch_deadline_callback(|ctx| {
            Ok(if ctx.data().0.is_requested() {
                wasmtime::UpdateDeadline::Interrupt
            } else {
                wasmtime::UpdateDeadline::Yield(1)
            })
        });
        store.set_epoch_deadline(1);
        let mut linker = wasmtime::Linker::new(&engine);
        linker
            .func_wrap(
                "host",
                "started",
                |mut caller: wasmtime::Caller<
                    '_,
                    (TaskCancellation, Option<oneshot::Sender<()>>, Dropped),
                >| {
                    caller.data_mut().1.take().unwrap().send(()).unwrap();
                },
            )
            .unwrap();
        let instance = match linker.instantiate_async(&mut store, &module).await {
            Ok(instance) => instance,
            Err(error) => {
                return InvokeExit::Trapped {
                    reason: error.to_string(),
                };
            }
        };
        let run = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .unwrap();
        match run.call_async(&mut store, ()).await {
            Ok(()) => InvokeExit::Completed(vec![]),
            Err(e) => InvokeExit::Trapped {
                reason: e.to_string(),
            },
        }
    };
    let (tasks, id) = if nested {
        let parents = Arc::new(IsolatedTasks::new(children.engine.clone(), 2, 1024).unwrap());
        let (run_children, cleanup_children) = (children.clone(), children.clone());
        let id = parents
            .spawn_scoped(
                move |_| async move {
                    run_children.spawn(child_factory).unwrap();
                    std::future::pending().await
                },
                Box::pin(async move { cleanup_children.shutdown().await }),
            )
            .unwrap();
        (parents, id)
    } else {
        let id = children.spawn(child_factory).unwrap();
        (children, id)
    };
    let sibling = tasks
        .spawn(|_| async { InvokeExit::Completed(vec![42]) })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .unwrap()
        .unwrap();
    tasks.cancel(id).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), tasks.join(id))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result.outcome(), InvokeExit::Cancelled));
    assert!(dropped.load(Ordering::Acquire));
    assert!(
        matches!(tasks.join(sibling).await.unwrap().outcome(),InvokeExit::Completed(b) if b==&[42])
    );
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn scoped_cancel_waits_for_cleanup_before_publishing() {
    let tasks = registry(2, 1024);
    let (cleanup_started, started) = oneshot::channel();
    let (finish_cleanup, finish) = oneshot::channel();
    let id = tasks
        .spawn_scoped(
            |_| async { std::future::pending().await },
            Box::pin(async move {
                cleanup_started.send(()).unwrap();
                finish.await.unwrap();
                Ok(())
            }),
        )
        .unwrap();
    tasks.cancel(id).unwrap();
    started.await.unwrap();
    assert!(
        tasks.task(id).unwrap().result.borrow().is_none(),
        "published before descendant cleanup"
    );
    let sibling = tasks
        .spawn(|_| async { InvokeExit::Completed(vec![9]) })
        .unwrap();
    assert!(
        matches!(tasks.join(sibling).await.unwrap().outcome(), InvokeExit::Completed(bytes) if bytes == &[9])
    );
    finish_cleanup.send(()).unwrap();
    assert!(matches!(
        tasks.join(id).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn scoped_prestart_cancel_runs_cleanup_without_running_factory() {
    let tasks = registry(1, 1024);
    let called = Arc::new(AtomicBool::new(false));
    let cleaned = Arc::new(AtomicBool::new(false));
    let (run_flag, cleanup_flag) = (called.clone(), cleaned.clone());
    let id = tasks
        .spawn_scoped(
            move |_| {
                run_flag.store(true, Ordering::Release);
                async { InvokeExit::Completed(vec![]) }
            },
            Box::pin(async move {
                cleanup_flag.store(true, Ordering::Release);
                Ok(())
            }),
        )
        .unwrap();
    tasks.cancel(id).unwrap();
    assert!(matches!(
        tasks.join(id).await.unwrap().outcome(),
        InvokeExit::Cancelled
    ));
    assert!(!called.load(Ordering::Acquire));
    assert!(cleaned.load(Ordering::Acquire));
    tasks.shutdown().await.unwrap();
}

#[tokio::test]
async fn scoped_parent_panic_reaps_grandchildren_before_returning_trap() {
    let parents = registry(1, 1024);
    let children = Arc::new(registry(1, 1024));
    let dropped = Arc::new(AtomicBool::new(false));
    let (run_children, cleanup_children) = (children.clone(), children.clone());
    let guard = Dropped(dropped.clone());
    let id = parents
        .spawn_scoped(
            move |_| async move {
                let (tx, rx) = oneshot::channel();
                run_children
                    .spawn(move |_| async move {
                        let _guard = guard;
                        tx.send(()).unwrap();
                        std::future::pending().await
                    })
                    .unwrap();
                rx.await.unwrap();
                panic!("parent failed with an active child");
            },
            Box::pin(async move { cleanup_children.shutdown().await }),
        )
        .unwrap();
    let result = parents.join(id).await.unwrap();
    assert!(matches!(result.outcome(), InvokeExit::Trapped { .. }));
    assert!(dropped.load(Ordering::Acquire));
    assert_eq!(
        children.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::Closed)
    );
    parents.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_or_panicking_cleanup_never_publishes_a_guest_outcome() {
    for panic in [false, true] {
        let tasks = registry(1, 1024);
        let id = tasks
            .spawn_scoped(
                |_| async { InvokeExit::Completed(vec![1]) },
                Box::pin(async move {
                    assert!(!panic, "cleanup panic");
                    Err(TaskError::WorkerLost)
                }),
            )
            .unwrap();
        tasks.cancel(id).unwrap();
        assert!(matches!(tasks.join(id).await, Err(TaskError::WorkerLost)));
        assert_eq!(tasks.shutdown().await, Err(TaskError::WorkerLost));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_parent_reaps_non_cooperative_wasm_grandchildren() {
    assert_wasm_is_stopped(false, true).await;
    assert_wasm_is_stopped(true, true).await;
}

#[tokio::test]
async fn repeated_and_concurrent_shutdown_cannot_forget_failed_cleanup() {
    let tasks = registry(2, 1024);
    let id = tasks
        .spawn_scoped(
            |_| async { InvokeExit::Completed(vec![1]) },
            Box::pin(async { Err(TaskError::WorkerLost) }),
        )
        .unwrap();
    assert!(matches!(tasks.join(id).await, Err(TaskError::WorkerLost)));
    let (first, concurrent) = tokio::join!(tasks.shutdown(), tasks.shutdown());
    assert_eq!(first, Err(TaskError::WorkerLost));
    assert_eq!(concurrent, Err(TaskError::WorkerLost));
    assert_eq!(tasks.shutdown().await, Err(TaskError::WorkerLost));
    assert_eq!(
        tasks.spawn(|_| async { InvokeExit::Completed(vec![]) }),
        Err(TaskError::Closed)
    );
    assert_eq!(tasks.retained_result_bytes(), 0);
}
