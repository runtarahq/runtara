//! Whole-execution emergency alarms. Guest scopes own standard async call
//! handles; this module owns only the native clock and one latched abort bit.
//! It cannot start an Agent, identify a scope, or choose a workflow successor.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    sync::Notify,
    task::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub(crate) struct CleanupAlarmState(Arc<Inner>);

#[derive(Default)]
struct Inner {
    expired: AtomicBool,
    wake: Notify,
}

#[derive(Debug)]
pub(crate) struct CleanupGraceExpired;

impl std::fmt::Display for CleanupGraceExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cooperative cleanup grace expired; whole execution aborted")
    }
}
impl std::error::Error for CleanupGraceExpired {}

struct Alarm {
    armed: Arc<Mutex<bool>>,
    timer: JoinHandle<()>,
}

impl Drop for Alarm {
    fn drop(&mut self) {
        // Linearize disposal against expiry. abort() alone cannot prevent a
        // timer already running on another worker from setting the latch later.
        *self.armed.lock().unwrap_or_else(|p| p.into_inner()) = false;
        self.timer.abort();
    }
}

impl CleanupAlarmState {
    pub(crate) fn expired(&self) -> bool {
        self.0.expired.load(Ordering::Acquire)
    }

    pub(crate) async fn wait(&self) {
        loop {
            let notified = self.0.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.expired() {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn arm(
        &self,
        ms: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = wasmtime::Result<()>> + Send>> {
        if ms == 0 {
            self.0.expired.store(true, Ordering::Release);
            self.0.wake.notify_waiters();
            return Box::pin(async { Err(wasmtime::Error::new(CleanupGraceExpired)) });
        }
        // Begin the clock before returning the host future to the Component
        // Model scheduler. CPU-bound guest work may prevent its next poll.
        let Some(deadline) = Instant::now().checked_add(Duration::from_millis(ms)) else {
            return Box::pin(async {
                Err(wasmtime::Error::msg("cleanup alarm deadline overflow"))
            });
        };
        let armed = Arc::new(Mutex::new(true));
        let timer = tokio::spawn({
            let armed = armed.clone();
            let state = self.clone();
            async move {
                tokio::time::sleep_until(deadline).await;
                let armed = armed.lock().unwrap_or_else(|p| p.into_inner());
                if *armed {
                    state.0.expired.store(true, Ordering::Release);
                    state.0.wake.notify_waiters();
                }
            }
        });
        let alarm = Alarm { armed, timer };
        let state = self.clone();
        Box::pin(async move {
            let _alarm = alarm;
            state.wait().await;
            Err(wasmtime::Error::new(CleanupGraceExpired))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn expiry_is_independent_of_polling_the_component_future() {
        let state = CleanupAlarmState::default();
        let alarm = state.arm(100);
        tokio::time::advance(Duration::from_millis(99)).await;
        assert!(!state.expired());
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(state.expired());
        assert!(alarm.await.unwrap_err().is::<CleanupGraceExpired>());
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_an_unpolled_alarm_disarms_and_releases_its_native_timer() {
        let state = CleanupAlarmState::default();
        drop(state.arm(10));
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;
        assert!(!state.expired());
        assert_eq!(Arc::strong_count(&state.0), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_one_alarm_preserves_another_and_expiry_is_latched() {
        let state = CleanupAlarmState::default();
        let early = state.arm(10);
        let later = state.arm(100);
        tokio::time::advance(Duration::from_millis(5)).await;
        drop(early);
        tokio::time::advance(Duration::from_millis(94)).await;
        assert!(!state.expired());
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(state.expired());
        drop(later);
        assert!(
            state.expired(),
            "late disposal cannot undo a selected abort"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn zero_grace_aborts_synchronously_and_maximum_grace_can_be_cancelled() {
        let maximum = CleanupAlarmState::default();
        let mut alarm = maximum.arm(u64::MAX);
        tokio::select! {
            biased;
            result = &mut alarm => panic!("maximum grace must remain pending: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        drop(alarm);
        tokio::task::yield_now().await;
        assert!(!maximum.expired());
        assert_eq!(Arc::strong_count(&maximum.0), 1);
        let zero = CleanupAlarmState::default();
        drop(zero.arm(0));
        assert!(zero.expired());
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_disposal_does_not_keep_store_state_or_stale_alarms_alive() {
        let state = CleanupAlarmState::default();
        for _ in 0..1000 {
            drop(state.arm(100));
        }
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(!state.expired());
        // Tokio reaps aborted tasks when it services their ready queue. A
        // single yield need not drain 1,000 tasks under its cooperative budget.
        for _ in 0..1000 {
            if Arc::strong_count(&state.0) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(Arc::strong_count(&state.0), 1);
    }
}
