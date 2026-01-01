// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wake scheduler for durable sleep.
//!
//! Periodically polls for sleeping instances and relaunches them
//! when their wake time arrives. Queries `sleep_until` column via
//! Core's Persistence trait.

use runtara_core::persistence::Persistence;
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
    /// Core persistence layer for querying sleeping instances.
    persistence: Arc<dyn Persistence>,
    runner: Arc<dyn Runner>,
    image_registry: ImageRegistry,
    config: WakeSchedulerConfig,
    shutdown: Arc<Notify>,
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

        // Get checkpoint_id from instance
        let checkpoint_id = instance.checkpoint_id.clone().ok_or_else(|| {
            crate::error::Error::Other(format!(
                "Instance '{}' has no checkpoint to resume from",
                instance.instance_id
            ))
        })?;

        // Look up image_id from instance_images table
        let image_id = db::get_instance_image_id(&self.pool, &instance.instance_id)
            .await?
            .ok_or_else(|| {
                crate::error::Error::Other(format!(
                    "No image association found for instance '{}'",
                    instance.instance_id
                ))
            })?;

        // Get the image to find bundle path
        let image = self
            .image_registry
            .get(&image_id)
            .await?
            .ok_or_else(|| crate::error::Error::ImageNotFound(image_id.clone()))?;

        // Ensure bundle exists
        let bundle_path = image
            .bundle_path
            .as_ref()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                crate::error::Error::ImageNotFound(format!("Image '{}' has no bundle", image_id))
            })?;

        // Build launch options
        let options = LaunchOptions {
            instance_id: instance.instance_id.clone(),
            tenant_id: instance.tenant_id.clone(),
            bundle_path,
            input: serde_json::json!({}), // Input was already consumed on first run
            timeout: Duration::from_secs(300),
            runtara_core_addr: self.config.core_addr.clone(),
            checkpoint_id: Some(checkpoint_id.clone()),
        };

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
                    bundle_path: image.bundle_path.clone(),
                    started_at: handle.started_at,
                    pid: None,
                    timeout_seconds: Some(300),
                };
                if let Err(e) = container_registry.register(&container_info).await {
                    warn!(error = %e, "Failed to register container (instance still running)");
                }

                // Clear sleep_until via Core persistence
                if let Err(e) = self
                    .persistence
                    .clear_instance_sleep(&instance.instance_id)
                    .await
                {
                    warn!(error = %e, "Failed to clear sleep_until");
                }

                // Spawn background task to monitor container
                spawn_container_monitor(
                    self.pool.clone(),
                    self.runner.clone(),
                    handle,
                    instance.tenant_id.clone(),
                    self.config.data_dir.clone(),
                    Some(self.persistence.clone()),
                );
            }
            Err(e) => {
                warn!(
                    instance_id = %instance.instance_id,
                    error = %e,
                    "Failed to wake instance"
                );
                return Err(e.into());
            }
        }

        Ok(())
    }
}
