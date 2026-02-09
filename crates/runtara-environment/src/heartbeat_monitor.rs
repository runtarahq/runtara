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
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use runtara_core::persistence::Persistence;
use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::container_registry::ContainerRegistry;
use crate::runner::{Runner, RunnerHandle};

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
    runner: Arc<dyn Runner>,
    container_registry: ContainerRegistry,
    config: HeartbeatMonitorConfig,
    shutdown: Arc<Notify>,
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
    /// Process ID (if known), used to build RunnerHandle for stopping.
    pid: Option<i32>,
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
        }
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

        // Immediately kill any surviving processes from a previous run.
        // After a platform restart, container_registry entries survive in PostgreSQL
        // but the processes may still be running as zombies.
        if let Err(e) = self.kill_surviving_processes().await {
            error!(error = %e, "Failed to clean up surviving processes on startup");
        }

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
        let stale: Vec<StaleContainer> = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                DateTime<Utc>,
                Option<DateTime<Utc>>,
                Option<i32>,
            ),
        >(
            r#"
            SELECT
                cr.instance_id,
                cr.container_id,
                cr.tenant_id,
                cr.started_at,
                (SELECT MAX(ie.created_at) FROM instance_events ie WHERE ie.instance_id = cr.instance_id) as last_activity,
                cr.pid
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
        .map(
            |(instance_id, container_id, tenant_id, started_at, last_activity, pid)| {
                StaleContainer {
                    instance_id,
                    container_id,
                    tenant_id,
                    started_at,
                    last_activity,
                    pid,
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
            pid = ?container.pid,
            started_at = %container.started_at,
            last_activity = ?container.last_activity,
            "Failing stale instance"
        );

        // Step 1: Try runner.stop() (uses crun kill + crun delete)
        let handle = RunnerHandle {
            handle_id: container.container_id.clone(),
            instance_id: container.instance_id.clone(),
            tenant_id: container.tenant_id.clone(),
            started_at: container.started_at,
            spawned_pid: container.pid.map(|p| p as u32),
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

        // Step 2: Direct PID kill as backup + confirmation
        let pid_confirmed_dead = self.kill_and_confirm_pid(container.pid).await;

        // Step 3: Mark process_killed in container_registry
        self.container_registry
            .mark_process_killed(&container.instance_id)
            .await?;

        // Step 4: Build error message with kill evidence
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
            "{} [pid={:?}, container_id={}, runner_stopped={}, pid_confirmed_dead={}]",
            base_msg, container.pid, container.container_id, runner_stopped, pid_confirmed_dead
        );

        // Step 5: Mark instance as failed in Core persistence with termination tracking
        self.core_persistence
            .complete_instance_with_termination_if_running(
                &container.instance_id,
                "failed",
                Some("heartbeat_timeout"),
                None, // exit_code
                None, // output
                Some(&error_message),
                None, // stderr
                None, // checkpoint_id
            )
            .await
            .map_err(|e| crate::error::Error::Other(format!("Core persistence error: {}", e)))?;

        // Step 6: Clean up container registry entry
        self.container_registry
            .cleanup(&container.instance_id)
            .await?;

        info!(
            instance_id = %container.instance_id,
            pid = ?container.pid,
            runner_stopped = runner_stopped,
            pid_confirmed_dead = pid_confirmed_dead,
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

    /// Mark an orphaned instance as failed.
    ///
    /// Orphaned instances have no container_registry entry, so no PID is available.
    /// We can only mark them as failed in Core persistence.
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
            "Instance orphaned: {} but not tracked by Environment (timeout: {}s) [no_pid_available]",
            started_desc,
            self.config.heartbeat_timeout.as_secs()
        );

        warn!(
            instance_id = %instance.instance_id,
            tenant_id = %instance.tenant_id,
            started_at = ?instance.started_at,
            created_at = %instance.created_at,
            "Marking orphaned instance as failed (no PID to kill)"
        );

        // Mark instance as failed in Core persistence with termination tracking
        self.core_persistence
            .complete_instance_with_termination_if_running(
                &instance.instance_id,
                "failed",
                Some("orphaned"),
                None, // exit_code
                None, // output
                Some(&error_message),
                None, // stderr
                None, // checkpoint_id
            )
            .await
            .map_err(|e| crate::error::Error::Other(format!("Core persistence error: {}", e)))?;

        info!(
            instance_id = %instance.instance_id,
            "Orphaned instance marked as failed"
        );

        Ok(())
    }

    // =========================================================================
    // PID Killing Utilities
    // =========================================================================

    /// Kill surviving processes from a previous run.
    ///
    /// Called on startup to handle the platform-restart edge case:
    /// container_registry entries with PIDs survive in PostgreSQL,
    /// but the processes may still be running as zombies.
    async fn kill_surviving_processes(&self) -> crate::error::Result<()> {
        let unkilled = self.container_registry.get_unkilled_containers().await?;

        if unkilled.is_empty() {
            debug!("No surviving processes to clean up on startup");
            return Ok(());
        }

        info!(
            count = unkilled.len(),
            "Found containers with unconfirmed process kills, cleaning up"
        );

        for container in &unkilled {
            let pid_confirmed_dead = self.kill_and_confirm_pid(container.pid).await;

            // Mark process as killed in container_registry
            self.container_registry
                .mark_process_killed(&container.instance_id)
                .await?;

            let error_message = format!(
                "Process killed on startup (previous run) [pid={:?}, container_id={}, pid_confirmed_dead={}]",
                container.pid, container.container_id, pid_confirmed_dead
            );

            // Mark instance as failed in Core persistence
            self.core_persistence
                .complete_instance_with_termination_if_running(
                    &container.instance_id,
                    "failed",
                    Some("crashed"),
                    None, // exit_code
                    None, // output
                    Some(&error_message),
                    None, // stderr
                    None, // checkpoint_id
                )
                .await
                .map_err(|e| {
                    crate::error::Error::Other(format!("Core persistence error: {}", e))
                })?;

            // Clean up container registry entry
            self.container_registry
                .cleanup(&container.instance_id)
                .await?;

            info!(
                instance_id = %container.instance_id,
                pid = ?container.pid,
                pid_confirmed_dead = pid_confirmed_dead,
                "Cleaned up surviving process from previous run"
            );
        }

        Ok(())
    }

    /// Send SIGKILL to a PID and confirm the process is dead.
    ///
    /// Returns true if the process is confirmed dead (either was already dead
    /// or was successfully killed). Returns false if the process could not be
    /// confirmed dead (e.g. no PID available, or kill failed and /proc still exists).
    async fn kill_and_confirm_pid(&self, pid: Option<i32>) -> bool {
        let Some(pid) = pid else {
            return false;
        };

        // Send SIGKILL directly
        let kill_result = signal::kill(Pid::from_raw(pid), Signal::SIGKILL);
        match &kill_result {
            Ok(()) => {
                debug!(pid = pid, "Sent SIGKILL to process");
            }
            Err(nix::errno::Errno::ESRCH) => {
                debug!(pid = pid, "Process already dead (ESRCH)");
                return true;
            }
            Err(e) => {
                warn!(pid = pid, error = %e, "Failed to send SIGKILL to process");
            }
        }

        // Wait briefly for the process to die
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Confirm via /proc/{pid}
        let alive = std::path::Path::new(&format!("/proc/{}", pid)).exists();
        if alive {
            warn!(pid = pid, "Process still alive after SIGKILL");
        }

        !alive
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
