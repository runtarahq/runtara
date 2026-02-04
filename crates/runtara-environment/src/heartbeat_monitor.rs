// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for detecting and failing stale instances.
//!
//! Instances that are registered as running but haven't sent any events
//! (checkpoints, heartbeats, custom events) within the configured timeout
//! are marked as failed. This prevents instances from getting stuck in
//! the "running" state when:
//! - The container crashes without sending a failed event
//! - Network issues prevent event delivery
//! - The process is killed externally
//!
//! The monitor queries Core's `instance_events` table to find the most recent
//! activity for each running container and marks those without recent activity
//! as failed.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use runtara_core::persistence::Persistence;
use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::container_registry::ContainerRegistry;

/// Configuration for the heartbeat monitor.
///
/// # Timeout Design
///
/// The heartbeat timeout should be **significantly shorter** than the QUIC idle timeout:
/// - **QUIC idle timeout** (default: 600s): When the connection is truly idle, QUIC closes it
/// - **Heartbeat timeout** (default: 120s): When an instance crashes/hangs without sending events
///
/// This ensures:
/// 1. Crashed instances are detected and failed within 2 minutes
/// 2. Long-running healthy instances (that send checkpoints/events) never time out
/// 3. The QUIC connection stays alive for legitimate long-running workflows (up to 10 minutes of idle time)
///
/// If heartbeat_timeout >= QUIC idle timeout, you'll get false positives where healthy
/// instances are marked as failed due to connection timeout rather than actual crashes.
#[derive(Debug, Clone)]
pub struct HeartbeatMonitorConfig {
    /// How often to check for stale instances.
    pub poll_interval: Duration,
    /// Maximum time since last heartbeat before marking as failed.
    ///
    /// **IMPORTANT**: This must be significantly shorter than the QUIC idle timeout
    /// (default: 120s vs 600s QUIC timeout).
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
    /// Last activity timestamp from instance_events table (any event counts as activity).
    last_activity: Option<DateTime<Utc>>,
}

/// Information about an orphaned instance.
///
/// An orphaned instance is one that is marked as "running" in Core's persistence
/// but is not being tracked in this Environment's container_registry.
#[derive(Debug)]
struct OrphanedInstance {
    instance_id: String,
    tenant_id: String,
    started_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
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

        // Check 1: Containers in container_registry with stale heartbeats
        let stale_containers = self.get_stale_containers(cutoff).await?;

        // Check 2: Running instances in Core that are not being tracked locally
        let orphaned_instances = self.get_orphaned_running_instances(cutoff).await?;

        let total_stale = stale_containers.len() + orphaned_instances.len();
        if total_stale == 0 {
            debug!("No stale instances found");
            return Ok(());
        }

        info!(
            stale_containers = stale_containers.len(),
            orphaned_instances = orphaned_instances.len(),
            "Found stale instances to fail"
        );

        // Process stale containers (those in container_registry)
        for container in stale_containers {
            if let Err(e) = self.fail_stale_instance(&container).await {
                error!(
                    instance_id = %container.instance_id,
                    error = %e,
                    "Failed to mark stale instance as failed"
                );
            }
        }

        // Process orphaned instances (running in Core but not tracked locally)
        for instance in orphaned_instances {
            if let Err(e) = self.fail_orphaned_instance(&instance).await {
                error!(
                    instance_id = %instance.instance_id,
                    error = %e,
                    "Failed to mark orphaned instance as failed"
                );
            }
        }

        Ok(())
    }

    /// Get containers that are registered but haven't sent any events recently.
    ///
    /// A container is considered stale if:
    /// 1. It's in the container_registry (meaning it was launched and is expected to be running)
    /// 2. Either:
    ///    - It has never sent any event (checkpoint, heartbeat, custom), OR
    ///    - Its last event is older than the cutoff time
    ///
    /// This queries Core's `instance_events` table to find the most recent activity
    /// for each container, treating any event as proof of life.
    async fn get_stale_containers(
        &self,
        cutoff: DateTime<Utc>,
    ) -> crate::error::Result<Vec<StaleContainer>> {
        // Query for containers that are registered but have no recent events.
        // We join container_registry with instance_events to find the last activity.
        let stale: Vec<StaleContainer> =
            sqlx::query_as::<_, (String, DateTime<Utc>, Option<DateTime<Utc>>)>(
                r#"
            SELECT
                cr.instance_id,
                cr.started_at,
                (SELECT MAX(ie.created_at) FROM instance_events ie WHERE ie.instance_id = cr.instance_id) as last_activity
            FROM container_registry cr
            WHERE
                -- Never received any event and container started before cutoff
                (NOT EXISTS (SELECT 1 FROM instance_events ie WHERE ie.instance_id = cr.instance_id) AND cr.started_at < $1)
                OR
                -- Last event is older than cutoff
                ((SELECT MAX(ie.created_at) FROM instance_events ie WHERE ie.instance_id = cr.instance_id) < $1)
            "#,
            )
            .bind(cutoff)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|(instance_id, started_at, last_activity)| StaleContainer {
                instance_id,
                started_at,
                last_activity,
            })
            .collect();

        Ok(stale)
    }

    /// Mark a stale instance as failed.
    async fn fail_stale_instance(&self, container: &StaleContainer) -> crate::error::Result<()> {
        let error_message = match container.last_activity {
            Some(last_event) => format!(
                "Instance stale: no activity since {} (timeout: {}s)",
                last_event.format("%Y-%m-%d %H:%M:%S UTC"),
                self.config.heartbeat_timeout.as_secs()
            ),
            None => format!(
                "Instance stale: no activity received since start at {} (timeout: {}s)",
                container.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
                self.config.heartbeat_timeout.as_secs()
            ),
        };

        warn!(
            instance_id = %container.instance_id,
            started_at = %container.started_at,
            last_activity = ?container.last_activity,
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

    /// Get instances that are running in Core but not tracked in container_registry.
    ///
    /// These are "orphaned" instances - Core thinks they're running, but we have
    /// no local record of them. This can happen when:
    /// - Environment was restarted while instances were running
    /// - Container registry entry was lost or never created
    /// - Multiple Environment instances share the same Core database
    async fn get_orphaned_running_instances(
        &self,
        cutoff: DateTime<Utc>,
    ) -> crate::error::Result<Vec<OrphanedInstance>> {
        // Get all running instances from Core
        let running_instances = self
            .core_persistence
            .list_instances(None, Some("running"), 1000, 0)
            .await
            .map_err(|e| crate::error::Error::Other(format!("Core persistence error: {}", e)))?;

        if running_instances.is_empty() {
            return Ok(vec![]);
        }

        // Get all instance IDs we're tracking locally
        let tracked_ids: std::collections::HashSet<String> =
            sqlx::query_scalar::<_, String>("SELECT instance_id FROM container_registry")
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect();

        // Filter to find orphaned instances:
        // - Running in Core
        // - Not tracked locally
        // - Started before the cutoff time (to avoid racing with new launches)
        let orphaned: Vec<OrphanedInstance> = running_instances
            .into_iter()
            .filter(|inst| {
                // Not tracked locally
                if tracked_ids.contains(&inst.instance_id) {
                    return false;
                }

                // Check if it's old enough to be considered orphaned
                // Use started_at if available, otherwise created_at
                let started = inst.started_at.unwrap_or(inst.created_at);
                started < cutoff
            })
            .map(|inst| OrphanedInstance {
                instance_id: inst.instance_id,
                tenant_id: inst.tenant_id,
                started_at: inst.started_at,
                created_at: inst.created_at,
            })
            .collect();

        Ok(orphaned)
    }

    /// Mark an orphaned instance as failed.
    async fn fail_orphaned_instance(
        &self,
        instance: &OrphanedInstance,
    ) -> crate::error::Result<()> {
        let started_desc = match instance.started_at {
            Some(started) => format!("started at {}", started.format("%Y-%m-%d %H:%M:%S UTC")),
            None => format!(
                "created at {}",
                instance.created_at.format("%Y-%m-%d %H:%M:%S UTC")
            ),
        };

        let error_message = format!(
            "Instance orphaned: {} but not tracked by Environment (timeout: {}s)",
            started_desc,
            self.config.heartbeat_timeout.as_secs()
        );

        warn!(
            instance_id = %instance.instance_id,
            tenant_id = %instance.tenant_id,
            started_at = ?instance.started_at,
            created_at = %instance.created_at,
            "Marking orphaned instance as failed"
        );

        // Mark instance as failed in Core persistence
        self.core_persistence
            .complete_instance(&instance.instance_id, None, Some(&error_message))
            .await
            .map_err(|e| crate::error::Error::Other(format!("Core persistence error: {}", e)))?;

        info!(
            instance_id = %instance.instance_id,
            "Orphaned instance marked as failed"
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
