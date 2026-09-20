//! Concurrent timers shared by workflow and agent components.
use anyhow::Result;
use std::time::Duration;
use wasmtime::component::Linker;

pub(crate) trait HostIoContext {
    fn timers_allowed(&self) -> bool {
        true
    }
    fn cleanup_alarm(&self) -> Option<&crate::cleanup_alarm::CleanupAlarmState> {
        None
    }
}

pub(crate) fn add_host_io_to_linker<T: HostIoContext + Send + 'static>(
    linker: &mut Linker<T>,
) -> Result<()> {
    // Concurrent timer for in-window retry backoff: a sleep is just
    // another waitable in the window's set, so item backoffs overlap
    // instead of serializing through assembly.
    let mut timers = linker.instance("runtara:host-io/timers@0.1.0")?;
    timers.func_wrap_concurrent("sleep", |accessor, (ms,): (u64,)| {
        let allowed = accessor.with(|mut access| access.get().timers_allowed());
        Box::pin(async move {
            wasmtime::ensure!(allowed, "Timers are disabled in trusted execution");
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        })
    })?;
    // Platform contract: abort-after: async func(ms: u64). It has no normal
    // success result: expiry ends the whole execution. The guest owns its
    // standard subtask handle and cancels it after cleanup. Zero expires at
    // registration; cancellation cannot clear an already selected abort.
    // No host scope IDs, registries or graph routing are involved. Ordinary
    // sleep retains its existing semantics, including in older artifacts.
    timers.func_wrap_concurrent("abort-after", |accessor, (ms,): (u64,)| {
        accessor.with(|mut access| match access.get().cleanup_alarm() {
            Some(alarm) => alarm.arm(ms),
            None => Box::pin(async {
                Err(wasmtime::Error::msg(
                    "cleanup alarms unavailable for this execution",
                ))
            }),
        })
    })?;
    Ok(())
}
