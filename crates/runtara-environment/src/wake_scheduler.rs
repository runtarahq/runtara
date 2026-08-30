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
use runtara_core::persistence::Persistence;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::container_registry::{ContainerInfo, ContainerRegistry};
use crate::db;
use crate::handlers::{DrainController, default_instance_timeout, spawn_container_monitor};
use crate::image_registry::ImageRegistry;
use crate::runner::{LaunchOptions, Runner};

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
    /// Core address to pass to instances
    pub core_addr: String,
    /// Data directory
    pub data_dir: std::path::PathBuf,
}

impl Default for WakeSchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            batch_size: 200,
            concurrency: default_wake_concurrency(),
            core_addr: "127.0.0.1:8001".to_string(),
            data_dir: std::path::PathBuf::from(".data"),
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

/// Wake scheduler that runs as a background task.
pub struct WakeScheduler {
    pool: PgPool,
    /// Core persistence layer for querying sleeping instances.
    persistence: Arc<dyn Persistence>,
    runner: Arc<dyn Runner>,
    image_registry: ImageRegistry,
    config: WakeSchedulerConfig,
    shutdown: Arc<Notify>,
    drain: DrainController,
}

impl WakeScheduler {
    /// Create a new wake scheduler.
    ///
    /// The scheduler queries `sleep_until` from Core's instances table
    /// via the provided persistence layer.
    pub fn new(
        pool: PgPool,
        persistence: Arc<dyn Persistence>,
        runner: Arc<dyn Runner>,
        config: WakeSchedulerConfig,
    ) -> Self {
        let image_registry = ImageRegistry::new(pool.clone());
        Self {
            pool,
            persistence,
            runner,
            image_registry,
            config,
            shutdown: Arc::new(Notify::new()),
            drain: DrainController::new(),
        }
    }

    /// Attach an externally-managed drain controller so spawned monitors
    /// observe the same drain state.
    pub fn with_drain(mut self, drain: DrainController) -> Self {
        self.drain = drain;
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
            .claim_sleeping_instances_due(self.config.batch_size)
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
    /// is already NULL and the wake scan can no longer see this instance. Any
    /// failure therefore has to put it back in the candidate set, or the
    /// instance sleeps forever. That restore lives here rather than at each
    /// error site so a new early return in the inner function cannot silently
    /// strand a sleeper.
    async fn wake_instance(
        &self,
        instance: &runtara_core::persistence::InstanceRecord,
    ) -> crate::error::Result<()> {
        let result = self.wake_claimed_instance(instance).await;

        if result.is_err()
            && let Err(restore_err) = self
                .persistence
                .set_instance_sleep(&instance.instance_id, chrono::Utc::now())
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

    /// Relaunch a claimed instance. See [`WakeScheduler::wake_instance`] for
    /// the claim-release contract wrapping this.
    async fn wake_claimed_instance(
        &self,
        instance: &runtara_core::persistence::InstanceRecord,
    ) -> crate::error::Result<()> {
        info!(
            instance_id = %instance.instance_id,
            checkpoint_id = ?instance.checkpoint_id,
            "Waking instance"
        );

        // Instances suspended mid-step during a graceful drain may have no
        // checkpoint at all. The workflow model is replay-from-start with
        // checkpoints as a result cache, so relaunching without one is
        // valid — completed durable steps replay from cache, the rest
        // re-execute.
        let checkpoint_id = instance.checkpoint_id.clone();
        if checkpoint_id.is_none() {
            info!(
                instance_id = %instance.instance_id,
                "No checkpoint recorded; relaunching from the start"
            );
        }

        // Look up image_id and stored env from instance_images table
        let (image_id, stored_env) =
            db::get_instance_image_with_env(&self.pool, &instance.instance_id)
                .await?
                .ok_or_else(|| {
                    crate::error::Error::Other(format!(
                        "No image association found for instance '{}'",
                        instance.instance_id
                    ))
                })?;

        // Get the image to find its wasm artifact
        let image = self
            .image_registry
            .get(&image_id)
            .await?
            .ok_or_else(|| crate::error::Error::ImageNotFound(image_id.clone()))?;

        let wasm_path = std::path::PathBuf::from(&image.binary_path);

        // Honor the per-instance timeout persisted at first launch so a workflow
        // that durably sleeps longer than the old hardcoded 300s isn't force-killed
        // on relaunch; fall back to the configured default when none was persisted.
        let timeout = db::get_instance_timeout_seconds(&self.pool, &instance.instance_id)
            .await
            .ok()
            .flatten()
            .map(|s| Duration::from_secs(s as u64))
            .unwrap_or_else(default_instance_timeout);

        // Build launch options with restored env
        let options = LaunchOptions {
            instance_id: instance.instance_id.clone(),
            tenant_id: instance.tenant_id.clone(),
            wasm_path,
            input: serde_json::json!({}), // Input was already consumed on first run
            timeout,
            checkpoint_id,
            env: stored_env, // Restore env from initial launch
        };

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
            return Ok(());
        }

        // Launch the instance
        match self.runner.launch_detached(&options).await {
            Ok(handle) => {
                info!(
                    instance_id = %instance.instance_id,
                    handle_id = %handle.handle_id,
                    "Instance woken successfully"
                );

                // Register in container registry
                let container_registry = ContainerRegistry::new(self.pool.clone());
                let container_info = ContainerInfo {
                    container_id: handle.handle_id.clone(),
                    instance_id: instance.instance_id.clone(),
                    tenant_id: instance.tenant_id.clone(),
                    binary_path: image.binary_path.clone(),
                    started_at: handle.started_at,
                    timeout_seconds: Some(options.timeout.as_secs() as i64),
                };
                if let Err(e) = container_registry.register(&container_info).await {
                    warn!(error = %e, "Failed to register container (instance still running)");
                }

                // sleep_until was already cleared atomically by the claim above.

                // Spawn background task to monitor container
                spawn_container_monitor(
                    self.pool.clone(),
                    self.runner.clone(),
                    handle,
                    self.persistence.clone(),
                    options.timeout,
                    self.drain.clone(),
                );
            }
            Err(e) => {
                warn!(
                    instance_id = %instance.instance_id,
                    error = %e,
                    "Failed to wake instance"
                );
                // The launch never started; the caller re-stamps sleep_until so
                // the wake scan re-selects this instance on a later poll.
                return Err(e.into());
            }
        }

        Ok(())
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
    fn default_config_is_not_rate_limited_by_the_poll_interval() {
        let config = WakeSchedulerConfig::default();
        assert_eq!(config.batch_size, 200);
        assert!(config.concurrency >= 1);
        // The interval still exists, but only as the idle wait.
        assert_eq!(config.poll_interval, Duration::from_secs(5));
    }
}
