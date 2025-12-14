// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wake scheduler for durable sleep.
//!
//! Periodically polls the wake_queue table and relaunches instances
//! when their wake time arrives.

use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::container_registry::{ContainerInfo, ContainerRegistry};
use crate::db;
use crate::handlers::spawn_container_monitor;
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
    runner: Arc<dyn Runner>,
    image_registry: ImageRegistry,
    config: WakeSchedulerConfig,
    shutdown: Arc<Notify>,
}

impl WakeScheduler {
    /// Create a new wake scheduler.
    pub fn new(pool: PgPool, runner: Arc<dyn Runner>, config: WakeSchedulerConfig) -> Self {
        let image_registry = ImageRegistry::new(pool.clone());
        Self {
            pool,
            runner,
            image_registry,
            config,
            shutdown: Arc::new(Notify::new()),
        }
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
        let wakes = db::get_pending_wakes(&self.pool, self.config.batch_size).await?;

        if wakes.is_empty() {
            debug!("No pending wakes to process");
            return Ok(());
        }

        info!(count = wakes.len(), "Processing pending wakes");

        for wake in wakes {
            if let Err(e) = self.wake_instance(&wake).await {
                error!(
                    instance_id = %wake.instance_id,
                    error = %e,
                    "Failed to wake instance"
                );
                // Continue processing other wakes
            }
        }

        Ok(())
    }

    /// Wake a single instance.
    async fn wake_instance(&self, wake: &db::WakeEntry) -> crate::error::Result<()> {
        info!(
            instance_id = %wake.instance_id,
            checkpoint_id = %wake.checkpoint_id,
            "Waking instance"
        );

        // Get the image to find bundle path
        let image = self
            .image_registry
            .get(&wake.image_id)
            .await?
            .ok_or_else(|| crate::error::Error::ImageNotFound(wake.image_id.to_string()))?;

        // Ensure bundle exists
        let bundle_path = image
            .bundle_path
            .as_ref()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                crate::error::Error::ImageNotFound(format!(
                    "Image '{}' has no bundle",
                    wake.image_id
                ))
            })?;

        // Build launch options (using the shared image bundle)
        let options = LaunchOptions {
            instance_id: wake.instance_id.clone(),
            tenant_id: wake.tenant_id.clone(),
            bundle_path,
            input: serde_json::json!({}), // Input was already consumed on first run
            timeout: Duration::from_secs(300), // Default timeout
            runtara_core_addr: self.config.core_addr.clone(),
            checkpoint_id: Some(wake.checkpoint_id.clone()),
        };

        // Launch the instance
        match self.runner.launch_detached(&options).await {
            Ok(handle) => {
                info!(
                    instance_id = %wake.instance_id,
                    handle_id = %handle.handle_id,
                    "Instance woken successfully"
                );

                // Register in container registry
                let container_registry = ContainerRegistry::new(self.pool.clone());
                let container_info = ContainerInfo {
                    container_id: handle.handle_id.clone(),
                    instance_id: wake.instance_id.clone(),
                    tenant_id: wake.tenant_id.clone(),
                    binary_path: image.binary_path.clone(),
                    bundle_path: image.bundle_path.clone(),
                    started_at: handle.started_at,
                    pid: None,
                    timeout_seconds: Some(300), // Default timeout for woken instances
                };
                if let Err(e) = container_registry.register(&container_info).await {
                    warn!(error = %e, "Failed to register container (instance still running)");
                }

                // Update instance status to running
                db::update_instance_status(
                    &self.pool,
                    &wake.instance_id,
                    "running",
                    Some(&wake.checkpoint_id),
                )
                .await?;

                // Spawn background task to monitor container and process output when done
                spawn_container_monitor(
                    self.pool.clone(),
                    self.runner.clone(),
                    handle,
                    wake.tenant_id.clone(),
                    self.config.data_dir.clone(),
                );

                // Remove from wake queue
                db::remove_wake(&self.pool, &wake.instance_id).await?;
            }
            Err(e) => {
                warn!(
                    instance_id = %wake.instance_id,
                    error = %e,
                    "Failed to wake instance"
                );
                return Err(e.into());
            }
        }

        Ok(())
    }
}
