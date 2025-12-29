// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for detecting and failing stale instances.
//!
//! Instances that are registered as running but haven't sent heartbeats
//! within the configured timeout are marked as failed. This prevents
//! instances from getting stuck in the "running" state when:
//! - The container crashes without sending a failed event
//! - Network issues prevent heartbeat delivery
//! - The process is killed externally
//!
//! The monitor queries for running containers that lack recent heartbeats
//! and marks them as failed in the Core persistence layer.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use runtara_core::persistence::Persistence;
use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::container_registry::ContainerRegistry;

/// Configuration for the heartbeat monitor.
#[derive(Debug, Clone)]
pub struct HeartbeatMonitorConfig {
    /// How often to check for stale instances.
    pub poll_interval: Duration,
    /// Maximum time since last heartbeat before marking as failed.
    pub heartbeat_timeout: Duration,
}

impl Default for HeartbeatMonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(30), // Check every 30 seconds
            heartbeat_timeout: Duration::from_secs(120), // 2 minutes without heartbeat = stale
        }
    }
}

/// Background worker that monitors for stale instances.
pub struct HeartbeatMonitor {
    pool: PgPool,
    core_persistence: Arc<dyn Persistence>,
    container_registry: ContainerRegistry,
    config: HeartbeatMonitorConfig,
    shutdown: Arc<Notify>,
}

/// Information about a stale container.
#[derive(Debug)]
struct StaleContainer {
    instance_id: String,
    started_at: DateTime<Utc>,
    last_heartbeat: Option<DateTime<Utc>>,
}

impl HeartbeatMonitor {
    /// Create a new heartbeat monitor.
    pub fn new(
        pool: PgPool,
        core_persistence: Arc<dyn Persistence>,
        config: HeartbeatMonitorConfig,
    ) -> Self {
        let container_registry = ContainerRegistry::new(pool.clone());
        Self {
            pool,
            core_persistence,
            container_registry,
            config,
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Get a handle that can be used to signal shutdown.
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    /// Run the heartbeat monitor loop.
    ///
    /// This will periodically check for stale instances and mark them as failed.
    /// The loop exits when the shutdown signal is received.
    pub async fn run(&self) {
        info!(
            poll_interval_secs = self.config.poll_interval.as_secs(),
            heartbeat_timeout_secs = self.config.heartbeat_timeout.as_secs(),
            "Heartbeat monitor started"
        );

        loop {
            tokio::select! {
                biased;

                _ = self.shutdown.notified() => {
                    info!("Heartbeat monitor received shutdown signal");
                    break;
                }

                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.check_stale_instances().await {
                        error!(error = %e, "Failed to check stale instances");
                    }
                }
            }
        }

        info!("Heartbeat monitor stopped");
    }

    /// Check for stale instances and mark them as failed.
    async fn check_stale_instances(&self) -> crate::error::Result<()> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(self.config.heartbeat_timeout)
                .map_err(|e| crate::error::Error::Other(format!("Invalid duration: {}", e)))?;

        let stale_containers = self.get_stale_containers(cutoff).await?;

        if stale_containers.is_empty() {
            debug!("No stale instances found");
            return Ok(());
        }

        info!(
            count = stale_containers.len(),
            "Found stale instances to fail"
        );

        for container in stale_containers {
            if let Err(e) = self.fail_stale_instance(&container).await {
                error!(
                    instance_id = %container.instance_id,
                    error = %e,
                    "Failed to mark stale instance as failed"
                );
            }
        }

        Ok(())
    }

    /// Get containers that are registered but haven't sent heartbeats recently.
    ///
    /// A container is considered stale if:
    /// 1. It's in the container_registry (meaning it was launched and is expected to be running)
    /// 2. Either:
    ///    - It has never sent a heartbeat, OR
    ///    - Its last heartbeat is older than the cutoff time
    async fn get_stale_containers(
        &self,
        cutoff: DateTime<Utc>,
    ) -> crate::error::Result<Vec<StaleContainer>> {
        // Query for containers that are registered but have stale/missing heartbeats
        let stale: Vec<StaleContainer> =
            sqlx::query_as::<_, (String, DateTime<Utc>, Option<DateTime<Utc>>)>(
                r#"
            SELECT
                cr.instance_id,
                cr.started_at,
                ch.last_heartbeat
            FROM container_registry cr
            LEFT JOIN container_heartbeats ch ON cr.instance_id = ch.instance_id
            WHERE
                -- Never received a heartbeat and container started before cutoff
                (ch.last_heartbeat IS NULL AND cr.started_at < $1)
                OR
                -- Last heartbeat is older than cutoff
                (ch.last_heartbeat IS NOT NULL AND ch.last_heartbeat < $1)
            "#,
            )
            .bind(cutoff)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|(instance_id, started_at, last_heartbeat)| StaleContainer {
                instance_id,
                started_at,
                last_heartbeat,
            })
            .collect();

        Ok(stale)
    }

    /// Mark a stale instance as failed.
    async fn fail_stale_instance(&self, container: &StaleContainer) -> crate::error::Result<()> {
        let error_message = match container.last_heartbeat {
            Some(last_hb) => format!(
                "Instance stale: no heartbeat since {} (timeout: {}s)",
                last_hb.format("%Y-%m-%d %H:%M:%S UTC"),
                self.config.heartbeat_timeout.as_secs()
            ),
            None => format!(
                "Instance stale: no heartbeat received since start at {} (timeout: {}s)",
                container.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
                self.config.heartbeat_timeout.as_secs()
            ),
        };

        warn!(
            instance_id = %container.instance_id,
            started_at = %container.started_at,
            last_heartbeat = ?container.last_heartbeat,
            "Marking stale instance as failed"
        );

        // Mark instance as failed in Core persistence
        self.core_persistence
            .complete_instance(&container.instance_id, None, Some(&error_message))
            .await
            .map_err(|e| crate::error::Error::Other(format!("Core persistence error: {}", e)))?;

        // Clean up container registry entry
        self.container_registry
            .cleanup(&container.instance_id)
            .await?;

        info!(
            instance_id = %container.instance_id,
            "Stale instance marked as failed and cleaned up"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = HeartbeatMonitorConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(30));
        assert_eq!(config.heartbeat_timeout, Duration::from_secs(120));
    }

    #[test]
    fn test_config_custom() {
        let config = HeartbeatMonitorConfig {
            poll_interval: Duration::from_secs(60),
            heartbeat_timeout: Duration::from_secs(300),
        };
        assert_eq!(config.poll_interval, Duration::from_secs(60));
        assert_eq!(config.heartbeat_timeout, Duration::from_secs(300));
    }
}
