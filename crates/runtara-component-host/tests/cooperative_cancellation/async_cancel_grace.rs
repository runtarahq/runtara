//! Qualify the optional standard async-cancel ABI before adopting it in the
//! emitter. The production engine is deliberately unchanged by these tests.
use super::*;
use runtara_component_host::{CallContext, HostState};
use std::{sync::mpsc, thread::JoinHandle, time::Instant};
use wasmtime::{Config, Engine, UpdateDeadline};

#[derive(Clone, Copy)]
enum Cleanup {
    Acknowledge,
    Return,
    PendingIo,
    CpuLoop,
}

fn source(cleanup: Cleanup) -> String {
    include_str!("async-cancel-grace.wat")
        .replace(
            "{{CLEANUP}}",
            if matches!(cleanup, Cleanup::CpuLoop) {
                "(loop $spin (br $spin))"
            } else {
                ""
            },
        )
        .replace(
            "{{RESOLVE}}",
            if matches!(cleanup, Cleanup::Return) {
                "(call $return)"
            } else {
                "(call $cancelled)"
            },
        )
        .replace(
            "{{RESOLUTION}}",
            if matches!(cleanup, Cleanup::Return) {
                "2"
            } else {
                "4"
            },
        )
        .replace(
            "{{GRACE_MS}}",
            if matches!(cleanup, Cleanup::Acknowledge | Cleanup::Return) {
                "60000"
            } else {
                "100"
            },
        )
}

// An independent, bounded epoch source also runs while WASM monopolizes its
// executor thread. Always join it; the proof must not leak background tickers.
struct EpochTicker(mpsc::Sender<()>, Option<JoinHandle<()>>);
impl EpochTicker {
    fn new(engine: Engine) -> Self {
        let (send, receive) = mpsc::channel();
        Self(
            send,
            Some(std::thread::spawn(move || {
                while matches!(
                    receive.recv_timeout(Duration::from_millis(5)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    engine.increment_epoch();
                }
            })),
        )
    }
}
impl Drop for EpochTicker {
    fn drop(&mut self) {
        let _ = self.0.send(());
        self.1.take().unwrap().join().unwrap();
    }
}

struct PendingIo(Arc<AtomicUsize>);
impl Drop for PendingIo {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[test]
fn async_cancel_requires_an_additional_engine_capability() -> anyhow::Result<()> {
    let engine = runtara_component_host::build_engine(&runtara_component_host::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: false,
    })?;
    let error = Component::new(&engine, source(Cleanup::Acknowledge))
        .err()
        .expect("production engine must reject the unqualified ABI extension");
    let message = format!("{error:#}");
    assert!(
        message.contains(
            "async `subtask.cancel` requires the component model more async builtins feature"
        ),
        "{message}"
    );
    Ok(())
}

async fn run(cleanup: Cleanup) -> anyhow::Result<()> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_more_async_builtins(true);
    config.epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let component = Component::new(&engine, source(cleanup))?;
    let mut linker = runtara_component_host::build_linker(&engine)?;
    let trace = Trace::default();
    let active = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let cancel_returned = Arc::new(Notify::new());
    linker.root().func_wrap_concurrent("request", {
        let active = active.clone();
        let started = started.clone();
        move |_, (): ()| {
            let active = active.clone();
            let started = started.clone();
            Box::pin(async move {
                active.fetch_add(1, Ordering::SeqCst);
                let _guard = PendingIo(active);
                started.notify_one();
                std::future::pending::<wasmtime::Result<()>>().await
            })
        }
    })?;
    linker
        .root()
        .func_wrap_concurrent("ready", move |_, (): ()| {
            let started = started.clone();
            Box::pin(async move {
                started.notified().await;
                Ok(())
            })
        })?;
    linker.root().func_wrap_concurrent("cleanup", {
        let active = active.clone();
        let cancel_returned = cancel_returned.clone();
        move |_, (): ()| {
            let active = active.clone();
            let cancel_returned = cancel_returned.clone();
            Box::pin(async move {
                // The original request must already be disposed before cleanup.
                assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                let _guard = PendingIo(active);
                if matches!(cleanup, Cleanup::PendingIo) {
                    std::future::pending::<()>().await;
                } else {
                    // A synchronous cancel cannot resolve this fixture: its
                    // parent must regain control before cleanup may complete.
                    cancel_returned.notified().await;
                }
                Ok(())
            })
        }
    })?;
    linker.root().func_wrap("trace", {
        let trace = trace.clone();
        move |_, (event,): (u32,)| {
            trace.lock().unwrap().push(event);
            if event == 40 {
                cancel_returned.notify_one();
            }
            Ok(())
        }
    })?;
    let mut store = Store::new(
        &engine,
        HostState::new(Arc::new(CallContext::placeholder_for_metadata())),
    );
    let yields = Arc::new(AtomicUsize::new(0));
    let interrupted = Arc::new(AtomicUsize::new(0));
    let watchdog_start = Instant::now();
    store.set_epoch_deadline(1);
    store.epoch_deadline_callback({
        let yields = yields.clone();
        let interrupted = interrupted.clone();
        move |_| {
            if watchdog_start.elapsed() >= Duration::from_millis(500) {
                interrupted.fetch_add(1, Ordering::SeqCst);
                Ok(UpdateDeadline::Interrupt)
            } else {
                yields.fetch_add(1, Ordering::SeqCst);
                Ok(UpdateDeadline::Yield(1))
            }
        }
    });
    let _ticker = EpochTicker::new(engine.clone());
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let run = instance.get_typed_func::<(), (u32,)>(&mut store, "run")?;
    let rounds = if matches!(cleanup, Cleanup::Acknowledge | Cleanup::Return) {
        2
    } else {
        1
    };
    for _ in 0..rounds {
        trace.lock().unwrap().clear();
        let result = tokio::time::timeout(Duration::from_secs(3), run.call_async(&mut store, ()))
            .await
            .expect("independent epoch watchdog must bound the proof");
        let observed = trace.lock().unwrap().clone();
        match cleanup {
            Cleanup::Acknowledge | Cleanup::Return => {
                assert_eq!(result?, (99,));
                assert_eq!(observed, [10, 20, 30, 40, 50, 60]);
                assert_eq!(active.load(Ordering::SeqCst), 0);
                assert_eq!(interrupted.load(Ordering::SeqCst), 0);
            }
            Cleanup::PendingIo => {
                let error = result.expect_err("unresolved cleanup cannot report success");
                assert_eq!(
                    error.downcast_ref::<wasmtime::Trap>(),
                    Some(&wasmtime::Trap::UnreachableCodeReached)
                );
                assert_eq!(observed, [10, 20, 30, 40, 70]);
                assert_eq!(interrupted.load(Ordering::SeqCst), 0);
                assert_eq!(active.load(Ordering::SeqCst), 1);
            }
            Cleanup::CpuLoop => {
                let error = result.unwrap_err();
                assert_eq!(
                    error.downcast_ref::<wasmtime::Trap>(),
                    Some(&wasmtime::Trap::Interrupt)
                );
                assert_eq!(observed, [10, 20, 30]);
                assert!(yields.load(Ordering::SeqCst) > 0);
                assert_eq!(interrupted.load(Ordering::SeqCst), 1);
            }
        }
    }
    drop(store);
    assert_eq!(
        active.load(Ordering::SeqCst),
        0,
        "Store teardown must dispose pending cleanup I/O"
    );
    Ok(())
}

#[tokio::test]
async fn async_cancel_allows_cleanup_ack_and_repeated_instance_use() -> anyhow::Result<()> {
    run(Cleanup::Acknowledge).await
}

#[tokio::test]
async fn async_cancel_allows_return_during_cleanup() -> anyhow::Result<()> {
    run(Cleanup::Return).await
}

#[tokio::test]
async fn async_cancel_allows_guest_grace_during_pending_cleanup_io() -> anyhow::Result<()> {
    run(Cleanup::PendingIo).await
}

#[tokio::test]
async fn async_cancel_still_needs_independent_abort_for_cpu_cleanup() -> anyhow::Result<()> {
    run(Cleanup::CpuLoop).await
}
