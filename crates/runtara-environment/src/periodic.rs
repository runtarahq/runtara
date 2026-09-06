// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The shutdown-aware polling loop this crate's periodic workers share.
//!
//! Run-dir cleanup, database cleanup, image cleanup and the heartbeat monitor
//! each had their own copy
//! of the same forty lines: an eager pass raced against the shutdown signal,
//! then a `biased` select between that signal and a sleep. The three differed
//! only in what they logged and which pass they called, so a fix to the loop —
//! the `biased` that makes shutdown win a ready race, say — had to be made
//! three times or it was made once and drifted.
//!
//! One thing this moved: tracing takes an event's target from `module_path!()`
//! at the callsite, so the lines emitted here now arrive under
//! `runtara_environment::periodic` rather than under each worker's own module.
//! The rendered text is unchanged and still names the worker, and every log
//! filter in this repo is crate-level (`runtara_environment=info`), so nothing
//! in tree is affected — but the split is deliberate and worth knowing: a
//! worker's `disabled` and `started` lines stay in its own module
//! because they carry per-worker configuration fields, while the shutdown,
//! stopped and pass-failure lines come from here. A module-scoped filter set
//! on one worker will therefore see it start and not see it stop.

use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{error, info};

/// One worker's polling loop.
///
/// The caller keeps its own "disabled" check and its startup log line, because
/// those name configuration this loop knows nothing about. Everything from the
/// first pass to the final "stopped" line is here.
pub(crate) struct PeriodicLoop<'a> {
    /// Worker name, used verbatim to open each lifecycle log line.
    pub name: &'static str,
    /// How long to wait between passes.
    pub poll_interval: Duration,
    /// Fires once when the runtime wants this worker to stop.
    pub shutdown: &'a Notify,
    /// Run a pass immediately instead of waiting out the first interval.
    ///
    /// Retention workers want this: a host that restarts more often than the
    /// interval would otherwise never enforce retention at all. The heartbeat
    /// monitor does not: staleness is measured against a timeout, so a scan at
    /// t=0 can say nothing a scan one interval later cannot.
    pub eager_first_pass: bool,
    /// Logged when a pass returns `Err`. The pass keeps running after one.
    pub pass_error: &'static str,
}

impl PeriodicLoop<'_> {
    /// Poll `pass` until the shutdown signal arrives.
    ///
    /// `biased` in both selects so a shutdown signal that is already ready wins
    /// against a ready timer, and so the eager pass cannot delay shutdown: a
    /// cleanup against an unreachable database can hang for a long time, and
    /// racing it here is what stops that from holding up the whole runtime.
    pub(crate) async fn run<F, Fut, E>(self, pass: F)
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<(), E>>,
        E: Display,
    {
        if self.eager_first_pass {
            tokio::select! {
                biased;

                _ = self.shutdown.notified() => {
                    info!("{} received shutdown signal during eager pass", self.name);
                    return;
                }

                result = pass() => {
                    if let Err(error) = result {
                        error!(error = %error, "{}", self.pass_error);
                    }
                }
            }
        }

        loop {
            tokio::select! {
                biased;

                _ = self.shutdown.notified() => {
                    info!("{} received shutdown signal", self.name);
                    break;
                }

                _ = tokio::time::sleep(self.poll_interval) => {
                    if let Err(error) = pass().await {
                        error!(error = %error, "{}", self.pass_error);
                    }
                }
            }
        }

        info!("{} stopped", self.name);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// The eager pass runs before the first interval elapses.
    ///
    /// This is the property retention depends on: a host restarting more often
    /// than `poll_interval` would otherwise never run a pass at all.
    #[tokio::test(start_paused = true)]
    async fn an_eager_pass_runs_before_the_first_interval() {
        let shutdown = Arc::new(Notify::new());
        let passes = Arc::new(AtomicUsize::new(0));

        let counter = passes.clone();
        let signal = shutdown.clone();
        let worker = tokio::spawn(async move {
            PeriodicLoop {
                name: "Test worker",
                poll_interval: Duration::from_secs(3600),
                shutdown: &signal,
                eager_first_pass: true,
                pass_error: "test pass failed",
            }
            .run(|| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), std::io::Error>(())
                }
            })
            .await;
        });

        // Let the eager pass run without advancing anywhere near the interval.
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(
            passes.load(Ordering::SeqCst),
            1,
            "the eager pass must not wait for the interval"
        );

        shutdown.notify_one();
        worker.await.expect("worker must not panic");
    }

    /// Without the eager flag the loop waits, which is what a monitor wants.
    #[tokio::test(start_paused = true)]
    async fn without_the_eager_flag_nothing_runs_before_the_interval() {
        let shutdown = Arc::new(Notify::new());
        let passes = Arc::new(AtomicUsize::new(0));

        let counter = passes.clone();
        let signal = shutdown.clone();
        let worker = tokio::spawn(async move {
            PeriodicLoop {
                name: "Test worker",
                poll_interval: Duration::from_secs(3600),
                shutdown: &signal,
                eager_first_pass: false,
                pass_error: "test pass failed",
            }
            .run(|| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), std::io::Error>(())
                }
            })
            .await;
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(passes.load(Ordering::SeqCst), 0);

        shutdown.notify_one();
        worker.await.expect("worker must not panic");
    }

    /// A failing pass is logged, not fatal: the next tick still happens.
    #[tokio::test(start_paused = true)]
    async fn a_failing_pass_does_not_end_the_loop() {
        let shutdown = Arc::new(Notify::new());
        let passes = Arc::new(AtomicUsize::new(0));

        let counter = passes.clone();
        let signal = shutdown.clone();
        let worker = tokio::spawn(async move {
            PeriodicLoop {
                name: "Test worker",
                poll_interval: Duration::from_secs(10),
                shutdown: &signal,
                eager_first_pass: true,
                pass_error: "test pass failed",
            }
            .run(|| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err::<(), std::io::Error>(std::io::Error::other("always fails"))
                }
            })
            .await;
        });

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(passes.load(Ordering::SeqCst), 1, "eager pass ran");

        for expected in 2..=3 {
            tokio::time::advance(Duration::from_secs(11)).await;
            tokio::task::yield_now().await;
            assert_eq!(
                passes.load(Ordering::SeqCst),
                expected,
                "a failed pass must not stop the loop"
            );
        }

        shutdown.notify_one();
        worker.await.expect("worker must not panic");
    }

    /// Shutdown wins a race it is already ready for, which is what `biased`
    /// buys: a due timer must not get one more pass in on the way out.
    #[tokio::test(start_paused = true)]
    async fn a_ready_shutdown_beats_a_due_timer() {
        let shutdown = Arc::new(Notify::new());
        let passes = Arc::new(AtomicUsize::new(0));

        // Signal before the worker ever polls, so both branches are ready the
        // first time through the select.
        shutdown.notify_one();

        let counter = passes.clone();
        let signal = shutdown.clone();
        PeriodicLoop {
            name: "Test worker",
            poll_interval: Duration::ZERO,
            shutdown: &signal,
            eager_first_pass: false,
            pass_error: "test pass failed",
        }
        .run(|| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok::<(), std::io::Error>(())
            }
        })
        .await;

        assert_eq!(
            passes.load(Ordering::SeqCst),
            0,
            "a pending shutdown must win against a zero-length sleep"
        );
    }
}
