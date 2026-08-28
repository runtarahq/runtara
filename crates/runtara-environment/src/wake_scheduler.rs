// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wake scheduler for durable sleep.
//!
//! Periodically polls for sleeping instances and relaunches them
//! when their wake time arrives. Queries `sleep_until` column via
//! Core's Persistence trait.

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
    /// How often to poll for pending wakes
    pub poll_interval: Duration,
    /// Maximum wakes to process per poll
    pub batch_size: i64,
    /// Core address to pass to instances
    pub core_addr: String,
    /// Data directory
    pub data_dir: std::path::PathBuf,
}

impl Default for WakeSchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            batch_size: 10,
            core_addr: "127.0.0.1:8001".to_string(),
            data_dir: std::path::PathBuf::from(".data"),
        }
    }
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
    pub async fn run(self) {
        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            batch_size = self.config.batch_size,
            "Wake scheduler started"
        );

        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    info!("Wake scheduler shutting down");
                    break;
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.process_pending_wakes().await {
                        error!(error = %e, "Failed to process pending wakes");
                    }
                }
            }
        }
    }

    /// Process pending wakes.
    async fn process_pending_wakes(&self) -> crate::error::Result<()> {
        // While draining, suspended instances are being stamped with
        // `sleep_until = now` so they relaunch after restart. Relaunching
        // them in this (shutting-down) process would defeat the drain.
        if self.drain.is_draining() {
            debug!("Draining; skipping wake processing");
            return Ok(());
        }

        let sleeping_instances = self
            .persistence
            .get_sleeping_instances_due(self.config.batch_size)
            .await
            .map_err(|e| crate::error::Error::Other(format!("Core persistence error: {}", e)))?;

        if sleeping_instances.is_empty() {
            debug!("No sleeping instances due for wake");
            return Ok(());
        }

        info!(
            count = sleeping_instances.len(),
            "Processing sleeping instances"
        );

        for instance in sleeping_instances {
            if let Err(e) = self.wake_instance(&instance).await {
                error!(
                    instance_id = %instance.instance_id,
                    error = %e,
                    "Failed to wake instance"
                );
                // Continue processing other wakes
            }
        }

        Ok(())
    }

    /// Whether a `cancel` signal is waiting for this instance.
    ///
    /// A read failure is reported as "no cancel": relaunching an instance that
    /// turns out to be cancelled is recoverable (the guest observes the signal
    /// at its next poll), whereas refusing to wake on a transient database
    /// error would strand a healthy sleeper.
    async fn cancel_pending(&self, instance_id: &str) -> bool {
        match self.persistence.get_pending_signal(instance_id).await {
            // Both backends already filter acknowledged rows, so this re-checks
            // what the query guarantees: waking is destructive enough that a
            // regression there must not silently re-cancel a handled run.
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

    /// Wake an instance.
    async fn wake_instance(
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
            runtara_core_addr: self.config.core_addr.clone(),
            checkpoint_id,
            env: stored_env, // Restore env from initial launch
        };

        // Atomically claim the instance before launching. The wake-scan SELECT
        // requires `sleep_until IS NOT NULL`, so this conditional clear removes
        // the row from the candidate set. A concurrent poll — or a second
        // Environment sharing this Core DB — that also selected this instance
        // gets `false` here and skips, so a duplicate guest never runs the same
        // (possibly non-idempotent) in-flight step twice.
        match self
            .persistence
            .claim_sleeping_instance(&instance.instance_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                info!(
                    instance_id = %instance.instance_id,
                    "Instance already claimed by another waker; skipping"
                );
                return Ok(());
            }
            Err(e) => {
                warn!(
                    instance_id = %instance.instance_id,
                    error = %e,
                    "Failed to claim instance for wake; will retry next poll"
                );
                return Err(e.into());
            }
        }

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
                // The claim cleared sleep_until; the launch never started, so
                // re-stamp it (status is still 'suspended') to make the wake
                // scan re-select this instance on a later poll instead of
                // stranding it.
                if let Err(restore_err) = self
                    .persistence
                    .set_instance_sleep(&instance.instance_id, chrono::Utc::now())
                    .await
                {
                    warn!(
                        instance_id = %instance.instance_id,
                        error = %restore_err,
                        "Failed to restore sleep_until after launch failure; instance may not retry"
                    );
                }
                return Err(e.into());
            }
        }

        Ok(())
    }
}
