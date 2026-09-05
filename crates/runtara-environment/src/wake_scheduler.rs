// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wake scheduler for durable sleep.
//!
//! Polls for sleeping instances and relaunches them when their wake time
//! arrives. Queries the `sleep_until` column via Core's Persistence trait.
//!
//! Two properties keep a large backlog from taking days to clear: the poll
//! interval is the *idle* wait, so a batch that comes back full is followed
//! immediately by the next one; and a batch is relaunched concurrently rather
//! than one `await` at a time. Selection and claiming happen in a single
//! statement (`claim_sleeping_instances_due`), which is what makes overlapping
//! polls safe.

use runtara_core::instance_handlers::{
    InstanceHandlerState, SignalAck, SignalType, handle_signal_ack,
};
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::db;
use crate::handlers::DrainController;
use crate::launch_dispatcher::{DEFAULT_LAUNCH_QUEUE_TIMEOUT, LaunchLifecycleObservers};
use crate::launch_queue::{
    EnqueueOutcome, EnqueueRequest, LaunchKind, LaunchQueueError, LaunchRepository,
};

/// Wake scheduler configuration.
#[derive(Debug, Clone)]
pub struct WakeSchedulerConfig {
    /// How long to wait before polling again **when there was no more work**.
    ///
    /// A poll that fills its batch does not wait at all — see
    /// [`WakeScheduler::run`]. This is the idle interval, not a rate limit.
    pub poll_interval: Duration,
    /// Maximum wakes to claim per poll
    pub batch_size: i64,
    /// Maximum wakes launched concurrently within a batch
    pub concurrency: usize,
    /// How long to wait before offering a failed wake again.
    ///
    /// Restoring to now instead lets a batch that fails for a persistent reason
    /// spin: released, immediately reclaimed, failed, released.
    pub failed_wake_retry_delay: Duration,
    /// How long a claimed instance stays hidden before it becomes due again.
    ///
    /// The claim leases rather than clears, so this is the worst-case delay
    /// before a batch that was claimed by a process that then died gets picked
    /// up by someone else. It has to comfortably exceed how long a launch
    /// takes, or a slow launch would be claimed twice.
    pub claim_lease: Duration,
}

impl Default for WakeSchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            batch_size: 200,
            concurrency: default_wake_concurrency(),
            claim_lease: Duration::from_secs(300),
            failed_wake_retry_delay: Duration::from_secs(5),
        }
    }
}

/// Whether the scheduler should poll again immediately instead of waiting.
///
/// A poll that filled its batch means instances were still due when the limit
/// was hit, so the next batch is already waiting. Sleeping there is what caps
/// the scheduler at `batch_size / poll_interval` wakes per second regardless of
/// backlog; going straight back lets the drain run at the speed the host can
/// actually sustain.
fn should_poll_again(claimed: usize, batch_size: i64) -> bool {
    batch_size > 0 && claimed >= batch_size as usize
}

/// Default in-batch wake concurrency: eight per core, bounded.
///
/// Relaunching a suspended instance is mostly CPU — instantiate the component,
/// replay to the checkpoint — so the useful ceiling tracks core count rather
/// than an IO-bound fan-out. The upper bound keeps a large host from launching
/// an unbounded surge into the runner after a long outage.
pub(crate) fn default_wake_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_mul(8))
        .unwrap_or(8)
        .clamp(1, 512)
}

/// Clamp in-batch concurrency to what the database pool can actually serve.
///
/// Every wake does several round trips — read the image, read the timeout,
/// launch, register the container — so a wake in flight holds a pooled
/// connection for most of its life. Running more wakes than the pool has
/// connections does not go faster; it starves the scheduler's own claim query
/// and surfaces as `pool timed out while waiting for an open connection`. A
/// couple of connections are left over for the claim itself and for the other
/// workers sharing this pool.
///
/// This matters most where the core count and the pool size disagree: eight
/// per core is 128 on a sixteen-core host, against a pool of ten.
pub(crate) fn concurrency_within_pool(requested: usize, pool_max_connections: usize) -> usize {
    let usable = pool_max_connections.saturating_sub(2).max(1);
    requested.min(usable).max(1)
}

/// Wake scheduler that runs as a background task.
pub struct WakeScheduler {
    pool: PgPool,
    /// Core persistence layer for querying sleeping instances.
    persistence: Arc<dyn Persistence>,
    config: WakeSchedulerConfig,
    shutdown: Arc<Notify>,
    drain: DrainController,
    launch_notifier: Arc<Notify>,
    lifecycle_observers: LaunchLifecycleObservers,
}

impl WakeScheduler {
    /// Create a new wake scheduler.
    ///
    /// The scheduler queries `sleep_until` from Core's instances table
    /// via the provided persistence layer.
    pub fn new(
        pool: PgPool,
        persistence: Arc<dyn Persistence>,
        config: WakeSchedulerConfig,
    ) -> Self {
        let pool_max = pool.options().get_max_connections() as usize;
        let mut config = config;
        let bounded = concurrency_within_pool(config.concurrency, pool_max);
        if bounded != config.concurrency {
            info!(
                requested = config.concurrency,
                bounded,
                pool_max_connections = pool_max,
                "Wake concurrency clamped to the database pool"
            );
            config.concurrency = bounded;
        }
        Self {
            pool,
            persistence,
            config,
            shutdown: Arc::new(Notify::new()),
            drain: DrainController::new(),
            launch_notifier: Arc::new(Notify::new()),
            lifecycle_observers: LaunchLifecycleObservers::default(),
        }
    }

    /// Attach an externally-managed drain controller so spawned monitors
    /// observe the same drain state.
    pub fn with_drain(mut self, drain: DrainController) -> Self {
        self.drain = drain;
        self
    }

    /// Attach the shared dispatcher notification and lifecycle observer holder.
    pub fn with_launch_control(
        mut self,
        launch_notifier: Arc<Notify>,
        lifecycle_observers: LaunchLifecycleObservers,
    ) -> Self {
        self.launch_notifier = launch_notifier;
        self.lifecycle_observers = lifecycle_observers;
        self
    }

    /// Get a handle to signal shutdown.
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    /// Run the wake scheduler loop.
    ///
    /// `poll_interval` is the **idle** interval, not a rate limit: a poll that
    /// fills its batch means more instances are already overdue, so the next
    /// poll starts immediately. Sleeping a fixed interval regardless of backlog
    /// caps the scheduler at `batch_size / poll_interval` wakes per second no
    /// matter how much work is waiting or how idle the host is.
    pub async fn run(self) {
        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            batch_size = self.config.batch_size,
            concurrency = self.config.concurrency,
            "Wake scheduler started"
        );

        // Batches are woken concurrently, and those tasks outlive the borrow,
        // so the scheduler is shared rather than borrowed. `run(self)` keeps
        // its signature so callers are unaffected.
        let this = Arc::new(self);
        let batch_size = this.config.batch_size;

        loop {
            // Check for shutdown without waiting, so a saturated scheduler
            // still stops promptly between batches. `biased` polls the signal
            // first; the ready() arm makes the whole select non-blocking.
            let stopping = tokio::select! {
                biased;
                _ = this.shutdown.notified() => true,
                _ = std::future::ready(()) => false,
            };
            if stopping {
                info!("Wake scheduler shutting down");
                break;
            }

            let claimed = match Arc::clone(&this).process_pending_wakes().await {
                Ok(n) => n,
                Err(e) => {
                    error!(error = %e, "Failed to process pending wakes");
                    0
                }
            };

            // A full batch means more work is already due; go straight back
            // for it. Anything less means we drained the queue.
            if should_poll_again(claimed, batch_size) {
                tokio::task::yield_now().await;
                continue;
            }

            tokio::select! {
                _ = this.shutdown.notified() => {
                    info!("Wake scheduler shutting down");
                    break;
                }
                _ = tokio::time::sleep(this.config.poll_interval) => {}
            }
        }
    }

    /// Claim a batch of due instances and wake them concurrently.
    ///
    /// Returns how many were claimed, which [`WakeScheduler::run`] uses to
    /// decide whether more work is waiting.
    async fn process_pending_wakes(self: Arc<Self>) -> crate::error::Result<usize> {
        // While draining, suspended instances are being stamped with
        // `sleep_until = now` so they relaunch after restart. Relaunching
        // them in this (shutting-down) process would defeat the drain.
        if self.drain.is_draining() {
            debug!("Draining; skipping wake processing");
            return Ok(0);
        }

        // Claims as it selects: back-to-back polls would otherwise keep
        // re-selecting rows whose per-instance claim had not landed yet.
        // Every record returned is already owned by this caller.
        let sleeping_instances = self
            .persistence
            .claim_sleeping_instances_due(
                self.config.batch_size,
                chrono::Utc::now()
                    + chrono::Duration::from_std(self.config.claim_lease)
                        .unwrap_or_else(|_| chrono::Duration::seconds(300)),
            )
            .await
            .map_err(|e| crate::error::Error::Other(format!("Core persistence error: {}", e)))?;

        if sleeping_instances.is_empty() {
            debug!("No sleeping instances due for wake");
            return Ok(0);
        }

        let claimed = sleeping_instances.len();
        info!(count = claimed, "Processing sleeping instances");

        // Relaunching is mostly CPU-bound, so a batch is worth spreading over
        // the cores rather than awaiting one instance at a time.
        let permits = Arc::new(tokio::sync::Semaphore::new(self.config.concurrency.max(1)));
        let mut tasks = tokio::task::JoinSet::new();

        for instance in sleeping_instances {
            let scheduler = Arc::clone(&self);
            let permits = Arc::clone(&permits);
            tasks.spawn(async move {
                // Semaphore is never closed, so acquire cannot fail.
                let _permit = permits
                    .acquire_owned()
                    .await
                    .expect("wake semaphore closed");

                // Re-check the drain after waiting for a permit. The check at
                // the top of this function covers the whole batch, but a batch
                // queues behind `concurrency` permits and drain snapshots the
                // container registry the moment it sets its flag — so a launch
                // that starts after that snapshot registers a container nobody
                // is going to signal, and it survives into teardown. Release
                // the claim instead so a restart picks it up promptly rather
                // than after the lease expires.
                if scheduler.drain.is_draining() {
                    debug!(
                        instance_id = %instance.instance_id,
                        "Drain began while this wake was queued; releasing the claim"
                    );
                    if let Err(e) = scheduler
                        .persistence
                        .set_instance_sleep(&instance.instance_id, chrono::Utc::now())
                        .await
                    {
                        warn!(
                            instance_id = %instance.instance_id,
                            error = %e,
                            "Failed to release a wake claim abandoned to drain; \
                             it will retry when the claim lease expires"
                        );
                    }
                    return;
                }

                if let Err(e) = scheduler.wake_instance(&instance).await {
                    error!(
                        instance_id = %instance.instance_id,
                        error = %e,
                        "Failed to wake instance"
                    );
                    // One failure must not abandon the rest of the batch.
                }
            });
        }

        while let Some(joined) = tasks.join_next().await {
            if let Err(e) = joined {
                error!(error = %e, "Wake task panicked");
            }
        }

        Ok(claimed)
    }

    /// Whether a `cancel` signal is waiting for this instance.
    ///
    /// A read failure is reported as "no cancel": relaunching an instance that
    /// turns out to be cancelled is recoverable (the guest observes the signal
    /// at its next poll), whereas refusing to wake on a transient database
    /// error would strand a healthy sleeper.
    async fn cancel_pending(&self, instance_id: &str) -> bool {
        match self.persistence.get_pending_signal(instance_id).await {
            // `acknowledged_at` is re-checked even though `get_pending_signal`
            // already filters on `acknowledged_at IS NULL`: defence in depth.
            // Waking is destructive enough that a regression in that predicate
            // must not silently re-cancel a handled run. The check is free.
            Ok(Some(signal)) => signal.signal_type == "cancel" && signal.acknowledged_at.is_none(),
            Ok(None) => false,
            Err(e) => {
                warn!(
                    instance_id = %instance_id,
                    error = %e,
                    "Failed to read pending signals before wake; relaunching anyway"
                );
                false
            }
        }
    }

    /// Acknowledge the pending cancel and drive the instance to `cancelled`.
    ///
    /// Reuses core's ack path rather than writing the status directly, so a
    /// cancel resolved here is indistinguishable from one a running guest
    /// acknowledged: same signal acknowledgement, same terminal status.
    async fn cancel_without_launch(&self, instance_id: &str) -> crate::error::Result<()> {
        let state = InstanceHandlerState::new(self.persistence.clone());
        handle_signal_ack(
            &state,
            SignalAck {
                instance_id: instance_id.to_string(),
                signal_type: SignalType::SignalCancel as i32,
                acknowledged: true,
            },
        )
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to cancel woken instance: {e}")))
    }

    /// Wake an already-claimed instance, releasing the claim if it fails.
    ///
    /// The claim happened in `claim_sleeping_instances_due`, so `sleep_until`
    /// is already leased into the future and the wake scan can no longer see
    /// this instance. Any failure therefore has to put it back in the candidate
    /// set, or the instance sleeps until the lease expires. That restore lives
    /// here rather than at each error site so a new early return in the inner
    /// function cannot silently strand a sleeper.
    ///
    /// It restores to a short delay rather than to now: a batch whose launches
    /// all fail for a persistent reason — a missing image, say — would
    /// otherwise be reclaimed the instant it is released, and the scheduler
    /// polls again immediately after a full batch, so the same rows spin
    /// through claim, fail and restore as fast as Postgres will take them.
    /// When a failed wake should be offered again.
    ///
    /// Short enough that a transient failure retries promptly, long enough that
    /// a persistent one cannot spin: see `wake_instance`.
    fn retry_deadline(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
            + chrono::Duration::from_std(self.config.failed_wake_retry_delay)
                .unwrap_or_else(|_| chrono::Duration::seconds(5))
    }

    async fn wake_instance(
        &self,
        instance: &runtara_core::persistence::InstanceRecord,
    ) -> crate::error::Result<()> {
        let result = self.wake_claimed_instance(instance).await;

        if result.is_err()
            && let Err(restore_err) = self
                .persistence
                .set_instance_sleep(&instance.instance_id, self.retry_deadline())
                .await
        {
            warn!(
                instance_id = %instance.instance_id,
                error = %restore_err,
                "Failed to restore sleep_until after a failed wake; instance may not retry"
            );
        }

        result
    }

    /// Queue a claimed instance for durable runner handoff.
    ///
    /// See [`WakeScheduler::wake_instance`] for the claim-release contract
    /// wrapping this. The scheduler intentionally does not call a runner: a
    /// full runner is dispatcher work, not a reason to leave wake tasks parked
    /// on in-memory permits.
    async fn wake_claimed_instance(
        &self,
        instance: &runtara_core::persistence::InstanceRecord,
    ) -> crate::error::Result<()> {
        info!(
            instance_id = %instance.instance_id,
            checkpoint_id = ?instance.checkpoint_id,
            "Waking instance"
        );

        // The instance is already claimed: `claim_sleeping_instances_due`
        // cleared `sleep_until` on exactly the rows it returned, which is what
        // takes them out of the wake candidate set and stops a concurrent poll
        // — or a second Environment sharing this Core DB — from launching a
        // duplicate guest for the same (possibly non-idempotent) in-flight
        // step. Every early return below must therefore either drive the
        // instance to terminal or re-stamp `sleep_until`, or it is stranded.

        // A cancel that arrived while the instance slept has nobody to observe
        // it: the guest is not running, and a relaunch replays into a checkpoint
        // HIT that skips the poll sites entirely. Drive it to terminal here
        // instead of starting a process only to cancel it — the claim above
        // already took this row out of the wake candidate set.
        if self.cancel_pending(&instance.instance_id).await {
            info!(
                instance_id = %instance.instance_id,
                "Cancel signal pending at wake time; cancelling instead of relaunching"
            );
            self.cancel_without_launch(&instance.instance_id).await?;
            self.lifecycle_observers.notify_instance_released(
                instance.tenant_id.clone(),
                instance.instance_id.clone(),
                "cancelled",
            );
            return Ok(());
        }

        let image_id = match db::get_instance_image_with_env(&self.pool, &instance.instance_id)
            .await?
        {
            Some((image_id, _)) => image_id,
            None => {
                let message = "Instance has no associated image";
                warn!(instance_id = %instance.instance_id, "Failing wake without image association");
                self.persistence
                    .complete_instance(
                        CompleteInstanceParams::new(&instance.instance_id, "failed")
                            .with_error(message),
                    )
                    .await?;
                self.lifecycle_observers.notify_instance_released(
                    instance.tenant_id.clone(),
                    instance.instance_id.clone(),
                    "launch_failed",
                );
                return Ok(());
            }
        };

        let repository = LaunchRepository::new(self.pool.clone());
        let request = EnqueueRequest::immediate(
            uuid::Uuid::new_v4().to_string(),
            instance.instance_id.clone(),
            instance.tenant_id.clone(),
            image_id,
            LaunchKind::Wake,
            DEFAULT_LAUNCH_QUEUE_TIMEOUT,
        );
        match repository.enqueue(request).await {
            Ok(EnqueueOutcome::Enqueued(launch)) | Ok(EnqueueOutcome::Existing(launch)) => {
                // The core wake claim is no longer needed once a durable launch
                // generation owns this handoff. If this clear is interrupted,
                // another scan only observes the same active generation and
                // cannot start a duplicate guest.
                if let Err(error) = self
                    .persistence
                    .clear_instance_sleep(&instance.instance_id)
                    .await
                {
                    warn!(instance_id = %instance.instance_id, error = %error, "Failed to clear wake claim after queuing launch");
                }
                self.launch_notifier.notify_one();
                info!(
                    instance_id = %instance.instance_id,
                    launch_id = %launch.launch_id,
                    "Wake durably queued for dispatcher"
                );
                Ok(())
            }
            Ok(EnqueueOutcome::SingleInstanceActive) => {
                // A parked approval intentionally owns no active lease. If a
                // new trigger won the same workflow scope first, keep this
                // wake durable and try again later rather than turning the
                // healthy suspended instance into a failure or silently
                // discarding its due wake.
                self.persistence
                    .set_instance_sleep(&instance.instance_id, self.retry_deadline())
                    .await?;
                info!(
                    instance_id = %instance.instance_id,
                    "Deferring wake while single-instance workflow has active work"
                );
                Ok(())
            }
            Err(LaunchQueueError::InvalidLaunchTarget { .. }) => {
                let message = "Wake has no valid tenant-scoped image binding";
                warn!(instance_id = %instance.instance_id, "Failing wake with invalid image binding");
                self.persistence
                    .complete_instance(
                        CompleteInstanceParams::new(&instance.instance_id, "failed")
                            .with_error(message),
                    )
                    .await?;
                self.lifecycle_observers.notify_instance_released(
                    instance.tenant_id.clone(),
                    instance.instance_id.clone(),
                    "launch_failed",
                );
                Ok(())
            }
            Err(error) => Err(crate::error::Error::Other(format!(
                "Failed to enqueue durable wake: {error}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_batch_polls_again_immediately() {
        // The backlog case: the batch filled, so more is due right now.
        assert!(should_poll_again(200, 200));
        assert!(should_poll_again(201, 200));
    }

    #[test]
    fn partial_batch_waits_for_the_idle_interval() {
        // Fewer than requested means the due queue is drained; waiting here is
        // what keeps an idle scheduler from spinning on the database.
        assert!(!should_poll_again(199, 200));
        assert!(!should_poll_again(1, 200));
    }

    #[test]
    fn empty_batch_waits() {
        assert!(!should_poll_again(0, 200));
    }

    #[test]
    fn non_positive_batch_size_never_spins() {
        // A misconfigured batch size must not turn the loop into a busy wait.
        assert!(!should_poll_again(0, 0));
        assert!(!should_poll_again(5, 0));
        assert!(!should_poll_again(5, -1));
    }

    #[test]
    fn concurrency_is_bounded_by_the_pool() {
        // Eight per core on a sixteen-core host is 128, against the runtime
        // pool's ten connections: the excess would starve the claim query.
        assert_eq!(concurrency_within_pool(128, 10), 8);
        // Headroom is left for the claim and the other workers on this pool.
        assert_eq!(concurrency_within_pool(100, 32), 30);
        // A request that already fits is untouched.
        assert_eq!(concurrency_within_pool(4, 32), 4);
        // Never zero, however small the pool: that would stall the scheduler.
        assert_eq!(concurrency_within_pool(8, 1), 1);
        assert_eq!(concurrency_within_pool(8, 2), 1);
    }

    #[test]
    fn default_config_is_not_rate_limited_by_the_poll_interval() {
        let config = WakeSchedulerConfig::default();
        assert_eq!(config.batch_size, 200);
        assert!(config.concurrency >= 1);
        // The interval still exists, but only as the idle wait.
        assert_eq!(config.poll_interval, Duration::from_secs(5));
    }
}
