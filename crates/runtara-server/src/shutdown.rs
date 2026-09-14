// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Graceful shutdown coordinator for the server process.
//!
//! Owns the server-wide shutdown signal that:
//!
//! 1. Stops new intake — background workers (trigger, compilation, cron,
//!    cleanup) observe the flag and exit at the next loop boundary.
//!    [`ShutdownCoordinator::drain_intake`] then waits up to
//!    `RUNTARA_SHUTDOWN_INTAKE_GRACE_MS` for them to actually return, so a
//!    worker that is mid-launch finishes rather than being aborted when the
//!    process drops the runtime. Only workers spawned through
//!    [`ShutdownCoordinator::spawn_intake`] are waited on.
//! 2. Drains active synchronous executions — the DashMap of
//!    `CancellationHandle`s is walked, each `cancel_flag` is flipped, and a
//!    `Shutdown` signal is written via the `RuntimeClient` so the SDK
//!    suspends at its next checkpoint.
//! 3. Force-stops stragglers after `RUNTARA_SHUTDOWN_GRACE_MS` so deploys
//!    are bounded.
//!
//! Intake is drained before executions on purpose: [`drain_executions`] takes
//! its list of executions up front, so a trigger worker still launching would
//! slip past it and die with the process.
//!
//! The actual orchestration lives in [`ShutdownCoordinator::drain`]; workers
//! only need a read-only handle via [`ShutdownSignal`].
//!
//! [`drain_executions`]: ShutdownCoordinator::drain_executions

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::ShutdownGrace;
use crate::runtime_client::RuntimeClient;
use crate::types::CancellationHandle;

/// Default grace period for waiting on in-flight executions to reach a
/// checkpoint before force-stopping them.
pub const DEFAULT_SHUTDOWN_GRACE_MS: u64 = 60_000;

/// Default grace period for intake workers (trigger/compilation/cron/cleanup)
/// to finish their current unit of work.
pub const DEFAULT_INTAKE_GRACE_MS: u64 = 5_000;

/// Read-only view of the shutdown flag given to background workers so they
/// can check it at loop boundaries. Clone freely — all copies share the
/// same atomic.
#[derive(Debug, Clone)]
pub struct ShutdownSignal {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl ShutdownSignal {
    /// Create a new signal in the not-shutting-down state.
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Returns `true` when shutdown has been requested.
    pub fn is_shutting_down(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves once shutdown has been requested. Useful with
    /// `axum::serve(..).with_graceful_shutdown(signal.wait())`.
    pub async fn wait(self) {
        if self.is_shutting_down() {
            return;
        }
        self.notify.notified().await;
    }

    /// Flip this signal directly (waking any `wait()`-ers). Idempotent.
    ///
    /// Used for the internal API server's *own* signal, which is held back
    /// until the environment drain completes so in-flight guest agent calls
    /// (object-model / agents / proxy) keep succeeding while running instances
    /// reach a checkpoint and suspend.
    pub fn trigger(&self) {
        if !self.flag.swap(true, Ordering::SeqCst) {
            self.notify.notify_waiters();
        }
    }
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Orchestrates server shutdown across intake workers and running executions.
pub struct ShutdownCoordinator {
    signal: ShutdownSignal,
    running_executions: Arc<DashMap<Uuid, CancellationHandle>>,
    runtime_client: Option<Arc<RuntimeClient>>,
    grace: Duration,
    intake_grace: Duration,
    /// Handles of the intake workers spawned through [`spawn_intake`], so
    /// [`drain_intake`] has something to wait on. Every other background task
    /// stays a plain `tokio::spawn`: waiting on a loop that never reads the
    /// shutdown flag would burn the whole grace on every shutdown.
    ///
    /// A `std::sync::Mutex` is enough — `JoinSet::spawn` is not async and the
    /// guard is never held across an await.
    ///
    /// [`spawn_intake`]: ShutdownCoordinator::spawn_intake
    /// [`drain_intake`]: ShutdownCoordinator::drain_intake
    intake_workers: Mutex<JoinSet<()>>,
}

impl ShutdownCoordinator {
    /// Create a new coordinator with the grace periods the host already
    /// parsed. The variables behind them are read in [`crate::config`], with
    /// the rest of the configuration, so a malformed value stops the process
    /// before it opens a pool or recovers any instance.
    pub fn new(
        running_executions: Arc<DashMap<Uuid, CancellationHandle>>,
        runtime_client: Option<Arc<RuntimeClient>>,
        grace: ShutdownGrace,
    ) -> Self {
        Self {
            signal: ShutdownSignal::new(),
            running_executions,
            runtime_client,
            grace: grace.executions,
            intake_grace: grace.intake,
            intake_workers: Mutex::new(JoinSet::new()),
        }
    }

    /// Get a cloneable handle to the shutdown signal for workers.
    pub fn signal(&self) -> ShutdownSignal {
        self.signal.clone()
    }

    /// Returns the configured grace period for execution drain.
    pub fn grace(&self) -> Duration {
        self.grace
    }

    /// Returns the configured grace period for intake workers.
    pub fn intake_grace(&self) -> Duration {
        self.intake_grace
    }

    /// Flip the shutdown flag. Idempotent.
    pub fn request_shutdown(&self) {
        if !self.signal.flag.swap(true, Ordering::SeqCst) {
            self.signal.notify.notify_waiters();
            info!("Shutdown requested");
        }
    }

    /// Spawn a background worker that observes [`ShutdownSignal`], keeping its
    /// handle so [`drain_intake`] can wait for it to return.
    ///
    /// Use this for anything that stops itself on the shutdown flag. A task
    /// that loops forever regardless — a pool monitor, say — should stay a
    /// plain `tokio::spawn`, or every shutdown pays the full intake grace
    /// waiting for something that is never going to exit.
    ///
    /// [`drain_intake`]: ShutdownCoordinator::drain_intake
    pub fn spawn_intake<F>(&self, worker: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        // A poisoned registry is still a perfectly usable `JoinSet`, and this
        // runs on the shutdown path where a panic would take out the orderly
        // shutdown entirely. Recover the guard instead of unwrapping.
        self.intake_workers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .spawn(worker);
    }

    /// Wait up to `RUNTARA_SHUTDOWN_INTAKE_GRACE_MS` for every worker spawned
    /// through [`spawn_intake`] to return.
    ///
    /// Call this after [`request_shutdown`] — the workers only start winding
    /// down once the flag is set. Workers that park on [`ShutdownSignal::wait`]
    /// return at once; those that check [`ShutdownSignal::is_shutting_down`] at
    /// a loop boundary take up to one iteration, which is what the grace is
    /// for.
    ///
    /// Stragglers still running when the budget expires are **detached, not
    /// aborted**. Dropping a `JoinSet` aborts everything still in it, which
    /// would be a regression: before this wait existed these were detached
    /// `tokio::spawn`s that kept running until the process dropped the runtime,
    /// so they survived the execution drain and the embedded shutdown after it.
    /// Killing them at the intake grace would cut that short — the compilation
    /// worker takes its request off Valkey with a destructive `BLPOP` and no
    /// redelivery, so an abort mid-compile loses the request outright.
    /// Detaching keeps the old lifetime and makes this wait a pure improvement.
    ///
    /// [`spawn_intake`]: ShutdownCoordinator::spawn_intake
    /// [`request_shutdown`]: ShutdownCoordinator::request_shutdown
    pub async fn drain_intake(&self) {
        let mut workers = {
            let mut registry = self
                .intake_workers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *registry)
        };

        let total = workers.len();
        if total == 0 {
            info!("No intake workers to drain");
            return;
        }

        info!(
            count = total,
            grace_ms = self.intake_grace.as_millis(),
            "Waiting for intake workers to stop"
        );

        let deadline = tokio::time::Instant::now() + self.intake_grace;
        let mut stopped = 0usize;
        loop {
            match tokio::time::timeout_at(deadline, workers.join_next()).await {
                // A worker returned — or panicked, which we surface rather
                // than let a silent count make shutdown look orderly.
                Ok(Some(result)) => {
                    if let Err(e) = result {
                        warn!(error = %e, "Intake worker did not exit cleanly");
                    }
                    stopped += 1;
                }
                Ok(None) => {
                    info!(count = total, "All intake workers stopped");
                    return;
                }
                Err(_) => {
                    // Hand the stragglers back to the runtime rather than
                    // letting `workers` drop and abort them: they keep running
                    // through the execution drain and the embedded shutdown,
                    // exactly as they did before this wait existed.
                    workers.detach_all();
                    warn!(
                        stragglers = total - stopped,
                        of = total,
                        grace_ms = self.intake_grace.as_millis(),
                        "Intake grace expired; remaining workers left running until process exit"
                    );
                    return;
                }
            }
        }
    }

    /// Drain active synchronous executions. For each entry in the running
    /// executions map:
    ///
    /// 1. Set the per-execution `cancel_flag`.
    /// 2. If a `RuntimeClient` is configured, call
    ///    [`RuntimeClient::signal_shutdown`] so the environment writes a
    ///    `"shutdown"` signal via core (the SDK picks it up at next checkpoint).
    ///
    /// Then poll the DashMap every 250 ms until it's empty or the grace
    /// period expires.
    pub async fn drain_executions(&self) {
        if self.running_executions.is_empty() {
            info!("No running executions to drain");
            return;
        }

        info!(
            count = self.running_executions.len(),
            grace_secs = self.grace.as_secs(),
            "Signalling running executions"
        );

        // Collect ids up front so we don't race the map mutating under us.
        let ids: Vec<Uuid> = self
            .running_executions
            .iter()
            .map(|entry| *entry.key())
            .collect();

        for id in &ids {
            if let Some(entry) = self.running_executions.get(id) {
                entry.cancel_flag.store(true, Ordering::SeqCst);
            }
            if let Some(client) = self.runtime_client.as_ref()
                && let Err(e) = client.signal_shutdown(*id).await
            {
                warn!(
                    execution_id = %id,
                    error = %e,
                    "Failed to write shutdown signal via runtime client"
                );
            }
        }

        let deadline = tokio::time::Instant::now() + self.grace;
        let poll_interval = Duration::from_millis(250);
        while tokio::time::Instant::now() < deadline {
            if self.running_executions.is_empty() {
                info!("All executions drained gracefully");
                return;
            }
            tokio::time::sleep(poll_interval).await;
        }

        warn!(
            stragglers = self.running_executions.len(),
            "Grace period expired; remaining executions will be force-stopped downstream"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ShutdownGrace;
    use std::sync::atomic::AtomicUsize;

    /// A coordinator with no running executions and the given intake budget.
    fn coordinator(intake: Duration) -> ShutdownCoordinator {
        ShutdownCoordinator::new(
            Arc::new(DashMap::new()),
            None,
            ShutdownGrace {
                executions: Duration::from_millis(DEFAULT_SHUTDOWN_GRACE_MS),
                intake,
            },
        )
    }

    /// The ordinary path: workers that park on the signal come back as soon as
    /// the flag flips, so the drain costs nothing like the grace.
    #[tokio::test(start_paused = true)]
    async fn drain_intake_returns_once_every_worker_stops() {
        let coord = coordinator(Duration::from_secs(30));
        let stopped = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let signal = coord.signal();
            let stopped = Arc::clone(&stopped);
            coord.spawn_intake(async move {
                signal.wait().await;
                stopped.fetch_add(1, Ordering::SeqCst);
            });
        }

        // Let the workers reach `wait()` before the flag flips; `wait()` parks
        // on `notify_waiters`, which only wakes receivers already waiting.
        tokio::task::yield_now().await;
        coord.request_shutdown();

        let start = tokio::time::Instant::now();
        coord.drain_intake().await;

        assert_eq!(
            stopped.load(Ordering::SeqCst),
            3,
            "every worker should have run to completion"
        );
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "drain should return on the last worker, not on the grace"
        );
    }

    /// The reason the grace exists: a worker that only notices the flag at a
    /// loop boundary is given time to get there instead of being aborted
    /// mid-iteration.
    #[tokio::test(start_paused = true)]
    async fn drain_intake_waits_for_a_worker_that_polls_at_a_loop_boundary() {
        let coord = coordinator(Duration::from_secs(30));
        let finished = Arc::new(AtomicBool::new(false));

        let signal = coord.signal();
        let worker_finished = Arc::clone(&finished);
        coord.spawn_intake(async move {
            loop {
                // Stands in for a unit of work that ignores the signal while
                // it is in flight.
                tokio::time::sleep(Duration::from_secs(5)).await;
                if signal.is_shutting_down() {
                    break;
                }
            }
            worker_finished.store(true, Ordering::SeqCst);
        });

        tokio::task::yield_now().await;
        coord.request_shutdown();
        coord.drain_intake().await;

        assert!(
            finished.load(Ordering::SeqCst),
            "worker should have finished its iteration inside the grace"
        );
    }

    /// A worker that never stops must not hold shutdown open: the wait is
    /// bounded by the grace.
    #[tokio::test(start_paused = true)]
    async fn drain_intake_gives_up_on_a_straggler_at_the_grace() {
        let intake = Duration::from_secs(5);
        let coord = coordinator(intake);

        coord.spawn_intake(async {
            // Ignores the flag entirely, and outlives any plausible grace.
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });

        tokio::task::yield_now().await;
        coord.request_shutdown();

        let start = tokio::time::Instant::now();
        coord.drain_intake().await;
        let waited = start.elapsed();

        assert!(
            waited >= intake && waited < Duration::from_secs(3600),
            "drain should return at the grace ({intake:?}), waited {waited:?}"
        );
    }

    /// Giving up on a straggler must not kill it. Before the grace existed
    /// these were detached `tokio::spawn`s that ran until the process dropped
    /// the runtime — past the execution drain and the embedded shutdown. A
    /// `JoinSet` aborts on drop, so `drain_intake` has to detach what is left;
    /// otherwise this wait would *shorten* a straggler's life and, for the
    /// compilation worker, lose the request its destructive `BLPOP` took.
    #[tokio::test(start_paused = true)]
    async fn drain_intake_leaves_a_straggler_running_rather_than_aborting_it() {
        let intake = Duration::from_secs(5);
        let coord = coordinator(intake);

        // Observable from outside the JoinSet: an aborted task never reaches
        // the store, a detached one does.
        let ran_to_completion = Arc::new(AtomicBool::new(false));
        let worker_flag = Arc::clone(&ran_to_completion);
        coord.spawn_intake(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            worker_flag.store(true, Ordering::SeqCst);
        });

        tokio::task::yield_now().await;
        coord.request_shutdown();
        coord.drain_intake().await;

        assert!(
            !ran_to_completion.load(Ordering::SeqCst),
            "precondition: the straggler is still mid-work when the drain gives up"
        );

        // Stand in for the rest of shutdown, which the worker used to outlive.
        tokio::time::sleep(Duration::from_secs(120)).await;

        assert!(
            ran_to_completion.load(Ordering::SeqCst),
            "straggler was aborted at the grace instead of being left to finish"
        );
    }

    /// With nothing tracked — nothing spawned, or a second call after the
    /// first took the registry — the drain returns immediately instead of
    /// spending another full grace period.
    #[tokio::test(start_paused = true)]
    async fn drain_intake_is_a_no_op_without_tracked_workers() {
        let coord = coordinator(Duration::from_secs(30));

        let start = tokio::time::Instant::now();
        coord.drain_intake().await;
        coord.drain_intake().await;

        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "an empty registry should not wait at all"
        );
    }
}
