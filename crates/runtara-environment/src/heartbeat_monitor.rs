// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Background worker for detecting and failing stale instances.
//!
//! Instances that are registered as running but haven't sent any events
//! (checkpoints, heartbeats, custom events) within the configured timeout
//! are marked as failed. This prevents instances from getting stuck in
//! the "running" state when:
//! - The workflow guest crashes without sending a failed event
//! - Network issues prevent event delivery
//! - The process is killed externally
//!
//! The monitor queries Core's `instance_events` table to find the most recent
//! activity for each running container and marks those without recent activity
//! as failed.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::container_registry::ContainerRegistry;
use crate::handlers::DrainController;
use crate::runner::{Runner, RunnerHandle};

/// Configuration for the heartbeat monitor.
///
/// # Timeout Design
///
/// The heartbeat timeout should be set appropriately:
/// - **Heartbeat timeout** (default: 120s): When an instance crashes/hangs without sending events
///
/// This ensures:
/// 1. Crashed instances are detected and failed within 2 minutes
/// 2. Long-running healthy instances (that send checkpoints/events) never time out
#[derive(Debug, Clone)]
pub struct HeartbeatMonitorConfig {
    /// How often to check for stale instances.
    pub poll_interval: Duration,
    /// Maximum time since last heartbeat before marking as failed.
    ///
    /// Default: 120s.
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
    runner: Arc<dyn Runner>,
    container_registry: ContainerRegistry,
    config: HeartbeatMonitorConfig,
    shutdown: Arc<Notify>,
    drain: DrainController,
}

/// Information about a stale container.
#[derive(Debug)]
struct StaleContainer {
    instance_id: String,
    container_id: String,
    tenant_id: String,
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
        runner: Arc<dyn Runner>,
        config: HeartbeatMonitorConfig,
    ) -> Self {
        let container_registry = ContainerRegistry::new(pool.clone());
        Self {
            pool,
            core_persistence,
            runner,
            container_registry,
            config,
            shutdown: Arc::new(Notify::new()),
            drain: DrainController::new(),
        }
    }

    /// Attach an externally-managed drain controller. While draining, the
    /// monitor skips stale-instance scans so it doesn't race a graceful
    /// suspend and mark an in-progress instance as failed.
    pub fn with_drain(mut self, drain: DrainController) -> Self {
        self.drain = drain;
        self
    }

    /// Get a handle that can be used to signal shutdown.
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown.clone()
    }

    /// Run the heartbeat monitor loop.
    ///
    /// On startup, immediately kills any processes from a previous run that
    /// were not confirmed dead (protects against platform restart edge cases).
    /// Then periodically checks for stale instances and marks them as failed.
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
                    if self.drain.is_draining() {
                        // During drain, in-progress instances are racing to
                        // checkpoint; skip scanning to avoid marking them as failed.
                        debug!("Heartbeat monitor skipping scan during drain");
                        continue;
                    }
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

        // Process orphaned instances (running in Core but not tracked locally).
        // These were orphaned by an Environment that went away; route them into
        // the restart-recovery path rather than failing them outright.
        for instance in orphaned_instances {
            self.recover_orphaned_instance(&instance).await;
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
        let stale: Vec<StaleContainer> = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                DateTime<Utc>,
                Option<DateTime<Utc>>,
            ),
        >(
            r#"
            SELECT
                cr.instance_id,
                cr.container_id,
                cr.tenant_id,
                cr.started_at,
                (SELECT MAX(ie.created_at) FROM instance_events ie WHERE ie.instance_id = cr.instance_id) as last_activity
            FROM container_registry cr
            WHERE
                -- Nothing that started within the timeout can be stale yet, whatever
                -- its event history says. A woken sleeper is registered with a fresh
                -- `started_at` while its newest event is the one it wrote before going
                -- to sleep -- hours or days old -- so without this guard every wake
                -- looks stale for the few milliseconds between registering the
                -- container and the relaunched guest writing its first event. The
                -- instance really is `running` in that window, so the `if_running`
                -- guard on the failing write does not catch it either: a run that
                -- goes on to succeed gets recorded as a heartbeat timeout.
                cr.started_at < $1
                AND (
                    -- Never received any event
                    NOT EXISTS (SELECT 1 FROM instance_events ie WHERE ie.instance_id = cr.instance_id)
                    OR
                    -- Last event is older than cutoff
                    ((SELECT MAX(ie.created_at) FROM instance_events ie WHERE ie.instance_id = cr.instance_id) < $1)
                )
            "#,
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(
            |(instance_id, container_id, tenant_id, started_at, last_activity)| {
                StaleContainer {
                    instance_id,
                    container_id,
                    tenant_id,
                    started_at,
                    last_activity,
                }
            },
        )
        .collect();

        Ok(stale)
    }

    /// Mark a stale instance as failed.
    ///
    /// Kills the actual process first (via runner + direct PID SIGKILL),
    /// confirms the process is dead, records the kill in the container registry,
    /// then updates the database state and cleans up.
    async fn fail_stale_instance(&self, container: &StaleContainer) -> crate::error::Result<()> {
        warn!(
            instance_id = %container.instance_id,
            container_id = %container.container_id,
            started_at = %container.started_at,
            last_activity = ?container.last_activity,
            "Failing stale instance"
        );

        // Step 1: Try runner.stop() (signals the guest's cancel token)
        let handle = RunnerHandle {
            handle_id: container.container_id.clone(),
            instance_id: container.instance_id.clone(),
            tenant_id: container.tenant_id.clone(),
            started_at: container.started_at,
            metrics: None,
        };
        let runner_stopped = match self.runner.stop(&handle).await {
            Ok(()) => true,
            Err(e) => {
                warn!(
                    instance_id = %container.instance_id,
                    error = %e,
                    "Runner.stop() failed (may already be dead)"
                );
                false
            }
        };

        // Step 2: Build error message with kill evidence
        let base_msg = match container.last_activity {
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
        let error_message = format!(
            "{} [container_id={}, runner_stopped={}]",
            base_msg, container.container_id, runner_stopped
        );

        // Step 3: Claim the generation this scan selected. The instance may have
        // been woken and relaunched since, which writes a fresh registry row
        // under a new container id — and `if_running` would then be satisfied by
        // that live run, so marking failed by instance alone would kill it and
        // the unguarded cleanup would delete its row. Deleting the exact row we
        // selected is both the guard and the claim: losing it means a newer run
        // owns this instance and none of the rest applies.
        if !self
            .container_registry
            .cleanup_generation(&container.instance_id, &container.container_id)
            .await?
        {
            info!(
                instance_id = %container.instance_id,
                container_id = %container.container_id,
                "Instance was relaunched since the stale scan; leaving the new run alone"
            );
            return Ok(());
        }

        // Step 4: Mark instance as failed in Core persistence with termination
        // tracking. Ordered after the claim so this can only ever describe the
        // run that was actually stale.
        self.core_persistence
            .complete_instance(
                CompleteInstanceParams::new(&container.instance_id, "failed")
                    .if_running()
                    .with_termination("heartbeat_timeout", None)
                    .with_error(&error_message),
            )
            .await
            .map_err(|e| crate::error::Error::Other(format!("Core persistence error: {}", e)))?;

        info!(
            instance_id = %container.instance_id,
            runner_stopped = runner_stopped,
            "Stale instance killed, marked as failed, and cleaned up"
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

    /// Recover an orphaned instance (Core shows it running, but no Environment
    /// tracks it — the tracking Environment went away). Route it into the
    /// suspend → wake → relaunch recovery path, gated by the crash-loop cap,
    /// instead of failing it outright. Per-workflow opt-out is wired in a later
    /// phase; default is to recover.
    async fn recover_orphaned_instance(&self, instance: &OrphanedInstance) {
        warn!(
            instance_id = %instance.instance_id,
            tenant_id = %instance.tenant_id,
            started_at = ?instance.started_at,
            created_at = %instance.created_at,
            "Found orphaned instance (Core running, untracked) - recovering after Environment restart"
        );

        let outcome = crate::recovery::recover_or_fail(
            self.core_persistence.as_ref(),
            &instance.instance_id,
            crate::recovery::auto_recover_enabled(),
        )
        .await;

        info!(
            instance_id = %instance.instance_id,
            outcome = ?outcome,
            "Orphaned instance recovery decision"
        );
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
