use super::*;
use std::sync::{Mutex, atomic::AtomicUsize};
use tokio::sync::Notify;

#[derive(Clone, Copy)]
enum Cleanup {
    Complete,
    Pending,
    CpuLoop,
    CpuBody,
    ReturnAfterAlarmFires,
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn run(cleanup: Cleanup) -> Result<()> {
    let engine = crate::engine::build_engine(&crate::engine::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: true,
    })?;
    let _ticker = test_support::Ticker::new(engine.clone());
    let mut executor = WorkflowExecutor::new(engine.clone())?;
    let trace = Arc::new(Mutex::new(Vec::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(Notify::new());
    executor.linker.root().func_wrap_concurrent("request", {
        let active = active.clone();
        let ready = ready.clone();
        move |_, (): ()| {
            let active = active.clone();
            let ready = ready.clone();
            Box::pin(async move {
                active.fetch_add(1, Ordering::SeqCst);
                let _guard = Active(active);
                ready.notify_one();
                std::future::pending::<wasmtime::Result<()>>().await
            })
        }
    })?;
    executor
        .linker
        .root()
        .func_wrap_concurrent("ready", move |_, (): ()| {
            let ready = ready.clone();
            Box::pin(async move {
                ready.notified().await;
                Ok(())
            })
        })?;
    executor.linker.root().func_wrap_concurrent("cleanup", {
        let active = active.clone();
        move |_, (): ()| {
            let active = active.clone();
            Box::pin(async move {
                assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                let _guard = Active(active);
                if matches!(cleanup, Cleanup::Complete | Cleanup::ReturnAfterAlarmFires) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                } else {
                    std::future::pending::<()>().await;
                }
                Ok(())
            })
        }
    })?;
    executor.linker.root().func_wrap("trace", {
        let trace = trace.clone();
        move |ctx, (event,): (u32,)| {
            trace.lock().unwrap().push(event);
            if event == 60 && matches!(cleanup, Cleanup::ReturnAfterAlarmFires) {
                // Deterministically place expiry immediately before the guest
                // returns. No further loop/epoch check is required by its code.
                drop(ctx.data().cleanup_alarm.arm(0));
            }
            Ok(())
        }
    })?;
    let (agent, _) = include_str!("../../tests/cooperative_cancellation/async-cancel-grace.wat")
        .split_once("  (component $workflow")
        .unwrap();
    let agent = agent
        .replace("(export \"sleep\" (func async (param \"ms\" u64))))", "(export \"sleep\" (func async (param \"ms\" u64)))\n(export \"abort-after\" (func async (param \"ms\" u64))))")
        .replace("{{CLEANUP}}", if matches!(cleanup, Cleanup::CpuLoop) { "(loop $spin (br $spin))" } else { "" })
        .replace("{{RESOLVE}}", "(call $cancelled)");
    let agent = if matches!(cleanup, Cleanup::CpuBody) {
        agent.replace("(func (export \"run\") (result i32)",
            "(func (export \"run\") (result i32) (call $trace (i32.const 15)) (loop $body (br $body))")
    } else {
        agent
    };
    let source =
        agent + include_str!("../../tests/cooperative_cancellation/cleanup-alarm-parent.wat");
    let component = Component::new(&engine, source)?;
    let pre = executor.linker.instantiate_pre(&component)?;
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        executor.execute_invoke(
            &pre,
            tests::run_spec(Duration::from_secs(3)),
            b"{}".to_vec(),
        ),
    )
    .await?;
    let observed = trace.lock().unwrap().clone();
    if matches!(cleanup, Cleanup::Complete) {
        assert!(
            matches!(result.exit, InvokeExit::Completed(ref bytes) if bytes == b"42"),
            "{:?}",
            result.exit
        );
        assert_eq!(observed, [10, 20, 30, 50, 40, 60]);
    } else if matches!(cleanup, Cleanup::ReturnAfterAlarmFires) {
        assert!(
            matches!(result.exit, InvokeExit::CleanupAborted),
            "{:?}",
            result.exit
        );
        assert_eq!(observed, [10, 20, 30, 50, 40, 60]);
    } else if matches!(cleanup, Cleanup::CpuBody) {
        assert!(
            matches!(result.exit, InvokeExit::CleanupAborted),
            "{:?}",
            result.exit
        );
        assert_eq!(
            observed,
            [15],
            "parent cannot select cancellation after this call starts"
        );
    } else {
        assert!(
            matches!(result.exit, InvokeExit::CleanupAborted),
            "{:?}",
            result.exit
        );
        assert_eq!(observed, [10, 20, 30]);
    }
    assert_eq!(
        active.load(Ordering::SeqCst),
        0,
        "result is published only after Store cleanup"
    );
    Ok(())
}

#[tokio::test]
async fn production_alarm_aborts_cpu_cleanup_during_synchronous_cancel() -> Result<()> {
    run(Cleanup::CpuLoop).await
}

#[tokio::test]
async fn production_alarm_bounds_cpu_work_before_parent_can_select_timeout() -> Result<()> {
    run(Cleanup::CpuBody).await
}
#[tokio::test]
async fn production_alarm_aborts_pending_cleanup_during_synchronous_cancel() -> Result<()> {
    run(Cleanup::Pending).await
}
#[tokio::test]
async fn standard_cancellation_disarms_alarm_and_preserves_the_same_store() -> Result<()> {
    run(Cleanup::Complete).await
}

#[tokio::test]
async fn latched_cleanup_abort_rejects_a_late_successful_return() -> Result<()> {
    run(Cleanup::ReturnAfterAlarmFires).await
}

const ALARM_LOOP: &str = r#"(component
  (import "runtara:host-io/timers@0.1.0" (instance $timers
    (export "abort-after" (func async (param "ms" u64)))))
  (core func $alarm (canon lower (func $timers "abort-after") async))
  (core module $m
    (import "h" "alarm" (func $alarm (param i64) (result i32)))
    (func (export "run") (result i32)
      (drop (call $alarm (i64.const 100)))
      (loop $spin (br $spin))
      (i32.const 0)))
  (core instance $m (instantiate $m (with "h" (instance (export "alarm" (func $alarm))))))
  (func $run (result (result)) (canon lift (core func $m "run")))
  (instance $cli (export "run" (func $run)))
  (export "wasi:cli/run@0.2.3" (instance $cli))
  (func (export "probe") async (result u32) (canon lift (core func $m "run"))))"#;

#[tokio::test]
async fn production_alarm_also_bounds_command_execution() -> Result<()> {
    let engine = crate::engine::build_engine(&crate::engine::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: true,
    })?;
    let _ticker = test_support::Ticker::new(engine.clone());
    let executor = WorkflowExecutor::new(engine.clone())?;
    let prepared = executor
        .prepare_precompiled(Component::new(&engine, ALARM_LOOP)?)
        .await?;
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        executor.execute(
            prepared.command().unwrap(),
            tests::run_spec(Duration::from_secs(3)),
        ),
    )
    .await?;
    assert!(
        matches!(result.exit, WorkflowExit::CleanupAborted),
        "{:?}",
        result.exit
    );
    Ok(())
}

#[tokio::test]
async fn production_alarm_also_bounds_dispatcher_execution() -> Result<()> {
    let engine = crate::engine::build_engine(&crate::engine::EngineConfig {
        cache_dir: None,
        enable_epoch_interruption: true,
    })?;
    let _ticker = test_support::Ticker::new(engine.clone());
    let linker = crate::build_linker(&engine)?;
    let mut store = Store::new(
        &engine,
        crate::HostState::new(Arc::new(crate::CallContext::placeholder_for_metadata())),
    );
    let component = Component::new(&engine, ALARM_LOOP)?;
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let probe = instance.get_typed_func::<(), (u32,)>(&mut store, "probe")?;
    let outcome = tokio::time::timeout(
        Duration::from_secs(4),
        crate::dispatcher::call_with_guards(&mut store, Duration::from_secs(3), probe, ()),
    )
    .await?;
    assert!(
        matches!(outcome, crate::dispatcher::GuardOutcome::Trapped(error) if error.is::<crate::cleanup_alarm::CleanupGraceExpired>())
    );
    Ok(())
}
