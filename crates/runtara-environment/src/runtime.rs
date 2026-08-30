// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embeddable runtime for runtara-environment.
//!
//! This module provides [`EnvironmentRuntime`] which allows embedding runtara-environment
//! into an existing tokio application instead of running it as a standalone server.
//!
//! # Basic Example (External Core)
//!
//! When running with an external runtara-core server:
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use runtara_environment::runtime::EnvironmentRuntime;
//! use runtara_environment::runner::build_runner;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let pool = sqlx::PgPool::connect("postgres://...").await?;
//!     let runner = build_runner(persistence.clone())?;
//!
//!     let runtime = EnvironmentRuntime::builder()
//!         .pool(pool)
//!         .runner(runner)
//!         .core_addr("127.0.0.1:8001")  // External Core server
//!         .build()?
//!         .start()
//!         .await?;
//!
//!     // ... run your application ...
//!
//!     runtime.shutdown().await?;
//!     Ok(())
//! }
//! ```
//!

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use sqlx::PgPool;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::cleanup_worker::{CleanupWorker, CleanupWorkerConfig};
use crate::container_registry::ContainerRegistry;
use crate::db_cleanup_worker::{DbCleanupWorker, DbCleanupWorkerConfig};
use crate::handlers::{DrainController, EnvironmentHandlerState};
use crate::heartbeat_monitor::{HeartbeatMonitor, HeartbeatMonitorConfig};
use crate::image_cleanup_worker::{ImageCleanupWorker, ImageCleanupWorkerConfig};
use crate::runner::Runner;
use crate::wake_scheduler::{WakeScheduler, WakeSchedulerConfig, default_wake_concurrency};

/// Idle poll interval for the wake scheduler, from
/// `RUNTARA_WAKE_POLL_INTERVAL_MS` (default 5000).
///
/// This is only the wait after a poll that found nothing more to do — a poll
/// that fills its batch is followed immediately by the next one — so it bounds
/// wake *latency* for an idle system, not wake throughput.
fn wake_poll_interval_from_env() -> Duration {
    let ms = std::env::var("RUNTARA_WAKE_POLL_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(5_000);
    Duration::from_millis(ms)
}

/// Instances claimed per wake poll, from `RUNTARA_WAKE_BATCH_SIZE`
/// (default 200).
fn wake_batch_size_from_env() -> i64 {
    std::env::var("RUNTARA_WAKE_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(200)
}

/// Concurrent relaunches within a wake batch, from `RUNTARA_WAKE_CONCURRENCY`
/// (default: eight per core, see [`default_wake_concurrency`]).
fn wake_concurrency_from_env() -> usize {
    std::env::var("RUNTARA_WAKE_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(default_wake_concurrency)
}

/// Builder for creating an [`EnvironmentRuntime`].
pub struct EnvironmentRuntimeBuilder {
    pool: Option<PgPool>,
    core_persistence: Option<Arc<dyn Persistence>>,
    runner: Option<Arc<dyn Runner>>,
    core_addr: String,
    data_dir: PathBuf,
    wake_poll_interval: Duration,
    wake_batch_size: i64,
    wake_concurrency: usize,
    request_timeout: Duration,
    cleanup_poll_interval: Duration,
    cleanup_max_age: Duration,
    heartbeat_poll_interval: Duration,
    heartbeat_timeout: Duration,
    db_cleanup_config: DbCleanupWorkerConfig,
    image_cleanup_config: ImageCleanupWorkerConfig,
}

impl Default for EnvironmentRuntimeBuilder {
    fn default() -> Self {
        Self {
            pool: None,
            core_persistence: None,
            runner: None,
            core_addr: "127.0.0.1:8001".to_string(),
            data_dir: PathBuf::from(".data"),
            wake_poll_interval: wake_poll_interval_from_env(),
            wake_batch_size: wake_batch_size_from_env(),
            wake_concurrency: wake_concurrency_from_env(),
            request_timeout: Duration::from_secs(30),
            cleanup_poll_interval: Duration::from_secs(3600), // 1 hour
            cleanup_max_age: Duration::from_secs(3 * 24 * 3600), // 3 days
            heartbeat_poll_interval: Duration::from_secs(30), // 30 seconds
            heartbeat_timeout: Duration::from_secs(120),      // 2 minutes
            db_cleanup_config: DbCleanupWorkerConfig::from_env(),
            image_cleanup_config: ImageCleanupWorkerConfig::from_env(),
        }
    }
}

impl EnvironmentRuntimeBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the PostgreSQL connection pool (required).
    pub fn pool(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Set the Core persistence layer for shared database access.
    ///
    /// When set, enables the wake scheduler to query Core's `sleep_until` column
    /// for durable sleep wake-ups. Also enables the heartbeat monitor.
    pub fn core_persistence(mut self, persistence: Arc<dyn Persistence>) -> Self {
        self.core_persistence = Some(persistence);
        self
    }

    /// Set the container runner (required).
    pub fn runner(mut self, runner: Arc<dyn Runner>) -> Self {
        self.runner = Some(runner);
        self
    }

    /// Set the address of runtara-core (passed to instances).
    ///
    /// Default: `127.0.0.1:8001`
    pub fn core_addr(mut self, addr: impl Into<String>) -> Self {
        self.core_addr = addr.into();
        self
    }

    /// Set the data directory for images and instance I/O.
    ///
    /// Default: `.data`
    pub fn data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_dir = path.into();
        self
    }

    /// Set the wake scheduler **idle** poll interval.
    ///
    /// Only applies when a poll did not fill its batch; a full batch is
    /// followed immediately by the next one.
    ///
    /// Default: 5 seconds, or `RUNTARA_WAKE_POLL_INTERVAL_MS`.
    pub fn wake_poll_interval(mut self, interval: Duration) -> Self {
        self.wake_poll_interval = interval;
        self
    }

    /// Set the wake scheduler batch size.
    ///
    /// Default: 200, or `RUNTARA_WAKE_BATCH_SIZE`.
    pub fn wake_batch_size(mut self, size: i64) -> Self {
        self.wake_batch_size = size;
        self
    }

    /// Set how many instances a wake batch relaunches concurrently.
    ///
    /// Default: eight per core, or `RUNTARA_WAKE_CONCURRENCY`.
    pub fn wake_concurrency(mut self, concurrency: usize) -> Self {
        self.wake_concurrency = concurrency;
        self
    }

    /// Set the request timeout for database operations.
    ///
    /// Default: 30 seconds
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set the cleanup worker poll interval.
    ///
    /// Default: 1 hour
    pub fn cleanup_poll_interval(mut self, interval: Duration) -> Self {
        self.cleanup_poll_interval = interval;
        self
    }

    /// Set the maximum age for run directories before cleanup.
    ///
    /// Default: 24 hours
    pub fn cleanup_max_age(mut self, max_age: Duration) -> Self {
        self.cleanup_max_age = max_age;
        self
    }

    /// Set the heartbeat monitor poll interval.
    ///
    /// Default: 30 seconds
    pub fn heartbeat_poll_interval(mut self, interval: Duration) -> Self {
        self.heartbeat_poll_interval = interval;
        self
    }

    /// Set the heartbeat timeout (time without heartbeat before marking as failed).
    ///
    /// Default: 2 minutes
    pub fn heartbeat_timeout(mut self, timeout: Duration) -> Self {
        self.heartbeat_timeout = timeout;
        self
    }

    /// Set the database cleanup worker configuration.
    ///
    /// Default: Loaded from environment variables via [`DbCleanupWorkerConfig::from_env()`].
    pub fn db_cleanup_config(mut self, config: DbCleanupWorkerConfig) -> Self {
        self.db_cleanup_config = config;
        self
    }

    /// Set the image cleanup worker configuration.
    ///
    /// Default: Loaded from environment variables via [`ImageCleanupWorkerConfig::from_env()`].
    pub fn image_cleanup_config(mut self, config: ImageCleanupWorkerConfig) -> Self {
        self.image_cleanup_config = config;
        self
    }

    /// Build the runtime configuration.
    ///
    /// Returns an error if required fields are missing.
    pub fn build(self) -> Result<EnvironmentRuntimeConfig> {
        let pool = self
            .pool
            .ok_or_else(|| anyhow::anyhow!("pool is required"))?;
        let runner = self
            .runner
            .ok_or_else(|| anyhow::anyhow!("runner is required"))?;
        let persistence = self
            .core_persistence
            .ok_or_else(|| anyhow::anyhow!("core_persistence is required"))?;
        Ok(EnvironmentRuntimeConfig {
            pool,
            persistence,
            runner,
            core_addr: self.core_addr,
            data_dir: self.data_dir,
            wake_poll_interval: self.wake_poll_interval,
            wake_batch_size: self.wake_batch_size,
            wake_concurrency: self.wake_concurrency,
            request_timeout: self.request_timeout,
            cleanup_poll_interval: self.cleanup_poll_interval,
            cleanup_max_age: self.cleanup_max_age,
            heartbeat_poll_interval: self.heartbeat_poll_interval,
            heartbeat_timeout: self.heartbeat_timeout,
            db_cleanup_config: self.db_cleanup_config,
            image_cleanup_config: self.image_cleanup_config,
        })
    }
}

/// Configuration for an [`EnvironmentRuntime`].
pub struct EnvironmentRuntimeConfig {
    pool: PgPool,
    persistence: Arc<dyn Persistence>,
    runner: Arc<dyn Runner>,
    core_addr: String,
    data_dir: PathBuf,
    wake_poll_interval: Duration,
    wake_batch_size: i64,
    wake_concurrency: usize,
    request_timeout: Duration,
    cleanup_poll_interval: Duration,
    cleanup_max_age: Duration,
    heartbeat_poll_interval: Duration,
    heartbeat_timeout: Duration,
    db_cleanup_config: DbCleanupWorkerConfig,
    image_cleanup_config: ImageCleanupWorkerConfig,
}

impl EnvironmentRuntimeConfig {
    /// Start the runtime, spawning the HTTP server and wake scheduler tasks.
    pub async fn start(self) -> Result<EnvironmentRuntime> {
        // Create shared drain controller so workers and the container monitor
        // all observe the same state.
        let drain = DrainController::new();

        // Create handler state
        let state = Arc::new(
            EnvironmentHandlerState::new(
                self.pool.clone(),
                self.persistence.clone(),
                self.runner.clone(),
                self.core_addr.clone(),
                self.data_dir.clone(),
            )
            .with_request_timeout(self.request_timeout)
            .with_drain(drain.clone()),
        );

        // Recover orphaned containers from previous Environment run
        // This handles containers that were running when Environment restarted
        if let Err(e) = recover_orphaned_containers(&self.pool, self.persistence.as_ref()).await {
            warn!(error = %e, "Failed to recover orphaned containers");
        }

        // Create wake scheduler
        let wake_config = WakeSchedulerConfig {
            poll_interval: self.wake_poll_interval,
            batch_size: self.wake_batch_size,
            concurrency: self.wake_concurrency,
            core_addr: self.core_addr.clone(),
            data_dir: self.data_dir.clone(),
        };

        let wake_scheduler = WakeScheduler::new(
            self.pool.clone(),
            self.persistence.clone(),
            self.runner.clone(),
            wake_config,
        )
        .with_drain(drain.clone());

        let wake_shutdown = wake_scheduler.shutdown_handle();

        let wake_handle = tokio::spawn(async move {
            wake_scheduler.run().await;
        });

        // Create cleanup worker. Config loads from env (so operators can tune
        // RUNTARA_RUN_DIR_CLEANUP_* at runtime) but the builder-supplied
        // data_dir and (non-default) poll/max-age overrides win.
        let mut cleanup_config = CleanupWorkerConfig::from_env();
        cleanup_config.data_dir = self.data_dir.clone();
        cleanup_config.poll_interval = self.cleanup_poll_interval;
        cleanup_config.max_age = self.cleanup_max_age;
        let cleanup_worker = CleanupWorker::new(cleanup_config);
        let cleanup_shutdown = cleanup_worker.shutdown_handle();

        // Start cleanup worker task
        let cleanup_handle = tokio::spawn(async move {
            cleanup_worker.run().await;
        });

        // Create heartbeat monitor
        let heartbeat_config = HeartbeatMonitorConfig {
            poll_interval: self.heartbeat_poll_interval,
            heartbeat_timeout: self.heartbeat_timeout,
        };
        let heartbeat_monitor = HeartbeatMonitor::new(
            self.pool.clone(),
            self.persistence.clone(),
            self.runner.clone(),
            heartbeat_config,
        )
        .with_drain(drain.clone());
        let heartbeat_shutdown = heartbeat_monitor.shutdown_handle();

        let heartbeat_handle = tokio::spawn(async move {
            heartbeat_monitor.run().await;
        });

        // Create database cleanup worker
        let db_cleanup_worker = DbCleanupWorker::new(
            self.pool.clone(),
            self.persistence.clone(),
            self.db_cleanup_config,
        );
        let db_cleanup_shutdown = db_cleanup_worker.shutdown_handle();

        let db_cleanup_handle = tokio::spawn(async move {
            db_cleanup_worker.run().await;
        });

        // Create image cleanup worker
        let mut image_cleanup_config = self.image_cleanup_config;
        image_cleanup_config.data_dir = self.data_dir.clone();
        let image_cleanup_worker = ImageCleanupWorker::new(self.pool.clone(), image_cleanup_config);
        let image_cleanup_shutdown = image_cleanup_worker.shutdown_handle();

        let image_cleanup_handle = tokio::spawn(async move {
            image_cleanup_worker.run().await;
        });

        info!(
            core_addr = %self.core_addr,
            "EnvironmentRuntime started"
        );

        Ok(EnvironmentRuntime {
            wake_handle,
            cleanup_handle,
            heartbeat_handle,
            db_cleanup_handle,
            wake_shutdown,
            cleanup_shutdown,
            heartbeat_shutdown,
            db_cleanup_shutdown,
            image_cleanup_handle,
            image_cleanup_shutdown,
            state,
            drain,
        })
    }
}

/// A running runtara-environment instance that can be embedded in an application.
///
/// The runtime manages:
/// - HTTP server for management SDK connections (images, instances, signals)
/// - Wake scheduler for durable sleep wake-ups
/// - Cleanup worker for removing old run directories
/// - Database cleanup worker for removing old database records
/// - Image cleanup worker for removing unused images
/// - Heartbeat monitor for detecting and failing stale instances
///
/// Call [`shutdown`](Self::shutdown) for graceful termination.
pub struct EnvironmentRuntime {
    wake_handle: JoinHandle<()>,
    cleanup_handle: JoinHandle<()>,
    heartbeat_handle: JoinHandle<()>,
    db_cleanup_handle: JoinHandle<()>,
    wake_shutdown: Arc<Notify>,
    cleanup_shutdown: Arc<Notify>,
    heartbeat_shutdown: Arc<Notify>,
    db_cleanup_shutdown: Arc<Notify>,
    image_cleanup_handle: JoinHandle<()>,
    image_cleanup_shutdown: Arc<Notify>,
    state: Arc<EnvironmentHandlerState>,
    drain: DrainController,
}

impl EnvironmentRuntime {
    /// Create a new builder for configuring the runtime.
    pub fn builder() -> EnvironmentRuntimeBuilder {
        EnvironmentRuntimeBuilder::new()
    }

    /// Get a reference to the shared handler state.
    pub fn state(&self) -> &Arc<EnvironmentHandlerState> {
        &self.state
    }

    /// Handle to the drain controller so external coordinators (e.g. the
    /// server's shutdown coordinator) can flip it.
    pub fn drain_handle(&self) -> DrainController {
        self.drain.clone()
    }

    /// Graceful drain of active runners.
    ///
    /// Flips the drain flag (pausing heartbeat scans and steering the
    /// container monitor's crash branch), writes a `"shutdown"` signal to
    /// every active instance so its SDK suspends at the next checkpoint,
    /// then polls up to `grace` for each one to reach a terminal status.
    /// Any stragglers are force-stopped via `Runner::stop()` and persisted
    /// as `suspended + shutdown_requested` with `sleep_until = now` — the
    /// wake scheduler then relaunches them after restart (replaying from
    /// their last checkpoint, or from the start when none exists).
    ///
    /// Returns when all tracked instances are terminal or the grace period
    /// expires, whichever comes first. Safe to call multiple times.
    pub async fn drain(&self, grace: Duration) -> Result<()> {
        self.drain.set();
        info!(grace_secs = grace.as_secs(), "EnvironmentRuntime draining");

        let container_registry = ContainerRegistry::new(self.state.pool.clone());
        let active = match container_registry.list_all_registered().await {
            Ok(list) => list,
            Err(e) => {
                warn!(error = %e, "Failed to list active containers; aborting drain");
                return Ok(());
            }
        };

        if active.is_empty() {
            info!("No active instances to drain");
            return Ok(());
        }

        info!(active_count = active.len(), "Signalling active instances");

        // Signal every active instance; the guest picks this up from core.
        for info in &active {
            if let Err(e) = self
                .state
                .persistence
                .insert_signal(&info.instance_id, "shutdown", &[])
                .await
            {
                warn!(
                    instance_id = %info.instance_id,
                    error = %e,
                    "Failed to insert shutdown signal"
                );
            }
        }

        // Poll for terminal states.
        let deadline = tokio::time::Instant::now() + grace;
        let poll_interval = Duration::from_millis(500);
        let mut remaining = active.clone();
        while !remaining.is_empty() && tokio::time::Instant::now() < deadline {
            remaining = self
                .filter_non_terminal(&self.state.persistence, remaining)
                .await;
            if remaining.is_empty() {
                break;
            }
            tokio::time::sleep(poll_interval).await;
        }

        if remaining.is_empty() {
            info!("All instances drained gracefully");
            return Ok(());
        }

        warn!(
            stragglers = remaining.len(),
            "Grace period expired; force-stopping remaining instances"
        );

        for info in remaining {
            let handle = crate::runner::RunnerHandle {
                handle_id: info.container_id.clone(),
                instance_id: info.instance_id.clone(),
                tenant_id: info.tenant_id.clone(),
                started_at: info.started_at,
                metrics: None,
            };
            if let Err(e) = self.state.runner.stop(&handle).await {
                warn!(
                    instance_id = %info.instance_id,
                    error = %e,
                    "Runner::stop() failed during drain"
                );
            }
            match self
                .state
                .persistence
                .complete_instance(
                    CompleteInstanceParams::new(&info.instance_id, "suspended")
                        .if_running()
                        .with_termination("shutdown_requested", None)
                        .with_error("Force-stopped after grace period expired during shutdown"),
                )
                .await
            {
                Ok(applied) => {
                    if applied {
                        // Mark as immediately due for wake so the wake scheduler
                        // relaunches the instance after restart. Without this a
                        // force-stopped instance (e.g. blocked in a non-durable
                        // agent call) stays suspended forever: it has no
                        // checkpoint and nothing else picks it up.
                        if let Err(e) = self
                            .state
                            .persistence
                            .set_instance_sleep(&info.instance_id, chrono::Utc::now())
                            .await
                        {
                            warn!(
                                instance_id = %info.instance_id,
                                error = %e,
                                "Failed to schedule post-restart wake"
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        instance_id = %info.instance_id,
                        error = %e,
                        "Failed to persist suspended-on-shutdown state"
                    );
                }
            }
        }

        Ok(())
    }

    async fn filter_non_terminal(
        &self,
        persistence: &Arc<dyn Persistence>,
        candidates: Vec<crate::container_registry::ContainerInfo>,
    ) -> Vec<crate::container_registry::ContainerInfo> {
        let mut still_active = Vec::with_capacity(candidates.len());
        for info in candidates {
            match persistence.get_instance(&info.instance_id).await {
                Ok(Some(inst))
                    if matches!(
                        inst.status.as_str(),
                        "suspended" | "completed" | "failed" | "cancelled"
                    ) =>
                {
                    debug!(
                        instance_id = %info.instance_id,
                        status = %inst.status,
                        "Instance drained"
                    );
                }
                Ok(Some(_)) => still_active.push(info),
                Ok(None) => {
                    // Instance row is gone — treat as drained.
                    debug!(instance_id = %info.instance_id, "Instance row missing; treating as drained");
                }
                Err(e) => {
                    warn!(
                        instance_id = %info.instance_id,
                        error = %e,
                        "Failed to read instance status during drain; keeping in queue"
                    );
                    still_active.push(info);
                }
            }
        }
        still_active
    }

    /// Gracefully shut down the runtime.
    ///
    /// This signals the wake scheduler, cleanup worker, database cleanup
    /// worker and heartbeat monitor to stop, then waits for them to complete.
    ///
    /// The management HTTP listener is not among them: it belongs to whichever
    /// host serves it, and that host stops it around this call.
    pub async fn shutdown(self) -> Result<()> {
        info!("EnvironmentRuntime shutting down...");

        // Signal wake scheduler shutdown
        self.wake_shutdown.notify_one();

        // Signal cleanup worker shutdown
        self.cleanup_shutdown.notify_one();

        // Signal heartbeat monitor shutdown
        self.heartbeat_shutdown.notify_one();

        // Signal database cleanup worker shutdown
        self.db_cleanup_shutdown.notify_one();

        // Signal image cleanup worker shutdown
        self.image_cleanup_shutdown.notify_one();

        // Wait for wake scheduler
        if let Err(e) = self.wake_handle.await {
            error!("Wake scheduler task panicked: {}", e);
        }

        // Wait for cleanup worker
        if let Err(e) = self.cleanup_handle.await {
            error!("Cleanup worker task panicked: {}", e);
        }

        // Wait for heartbeat monitor
        if let Err(e) = self.heartbeat_handle.await {
            error!("Heartbeat monitor task panicked: {}", e);
        }

        // Wait for database cleanup worker
        if let Err(e) = self.db_cleanup_handle.await {
            error!("Database cleanup worker task panicked: {}", e);
        }

        // Wait for image cleanup worker
        if let Err(e) = self.image_cleanup_handle.await {
            error!("Image cleanup worker task panicked: {}", e);
        }

        info!("EnvironmentRuntime shutdown complete");
        Ok(())
    }

    /// Check if the runtime is still running.
    pub fn is_running(&self) -> bool {
        !self.wake_handle.is_finished()
            && !self.cleanup_handle.is_finished()
            && !self.heartbeat_handle.is_finished()
            && !self.db_cleanup_handle.is_finished()
            && !self.image_cleanup_handle.is_finished()
    }
}

/// Recover orphaned containers on startup.
///
/// When the Environment restarts, there may be containers in the registry
/// that were running before the restart. This function checks each one:
///
/// Workflow guests run in-process, so an Environment restart necessarily killed
/// every one of them. Each registry entry is therefore stale by definition:
///
/// - Core shows terminal status → clean up registry
/// - Core still shows "running" → mark as crashed and clean up
///
/// This prevents "zombie" entries in the registry and ensures crashed instances
/// are properly marked.
async fn recover_orphaned_containers(pool: &PgPool, persistence: &dyn Persistence) -> Result<()> {
    let registry = ContainerRegistry::new(pool.clone());
    let containers = registry.list_all_registered().await?;

    if containers.is_empty() {
        debug!("No containers in registry to recover");
        return Ok(());
    }

    info!(
        count = containers.len(),
        "Checking registered containers for recovery"
    );

    // Summary counters for an operator-facing startup line.
    let mut recovered = 0usize;
    let mut failed = 0usize;

    for container in containers {
        let instance_id = &container.instance_id;

        // The guest died with the previous process — check Core status.
        match persistence.get_instance(instance_id).await {
            Ok(Some(inst)) => {
                let status = inst.status.as_str();
                if matches!(status, "completed" | "failed" | "cancelled" | "suspended") {
                    // Already terminal - just clean up registry
                    info!(
                        instance_id = %instance_id,
                        status = %status,
                        "Cleaning up terminated container from registry"
                    );
                    let _ = registry.cleanup(instance_id).await;
                } else {
                    // Process is gone but Core still shows the instance
                    // running: it was killed by this Environment restart. Route
                    // it into the suspend → wake → relaunch recovery path
                    // (replay-from-start with the checkpoint cache) instead of
                    // dead-ending at `failed`. A crash-loop cap bounds instances
                    // that never make progress. Per-workflow opt-out is wired in
                    // a later phase; default is to recover.
                    warn!(
                        instance_id = %instance_id,
                        status = %status,
                        "Found orphaned container (process gone, Core shows running) - recovering after Environment restart"
                    );

                    let outcome = crate::recovery::recover_or_fail(
                        persistence,
                        instance_id,
                        crate::recovery::auto_recover_enabled(),
                    )
                    .await;
                    match outcome {
                        crate::recovery::RecoveryOutcome::Recovered => recovered += 1,
                        crate::recovery::RecoveryOutcome::Failed => failed += 1,
                    }

                    // Drop the stale registry entry either way; the wake
                    // scheduler registers a fresh one on relaunch.
                    let _ = registry.cleanup(instance_id).await;

                    info!(
                        instance_id = %instance_id,
                        outcome = ?outcome,
                        "Orphaned instance recovery decision"
                    );
                }
            }
            Ok(None) => {
                // Instance not in Core - just clean up registry
                warn!(
                    instance_id = %instance_id,
                    "Container in registry but not in Core - cleaning up"
                );
                let _ = registry.cleanup(instance_id).await;
            }
            Err(e) => {
                error!(
                    instance_id = %instance_id,
                    error = %e,
                    "Failed to check instance status during recovery"
                );
            }
        }
    }

    if recovered > 0 || failed > 0 {
        info!(
            recovered,
            failed,
            "Environment-restart recovery: relaunching instances killed by the previous restart \
             (failed = exceeded RUNTARA_MAX_AUTO_RESTARTS or auto-recovery disabled)"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_default_values() {
        let builder = EnvironmentRuntimeBuilder::default();

        assert!(builder.pool.is_none());
        assert!(builder.core_persistence.is_none());
        assert!(builder.runner.is_none());
        assert_eq!(builder.core_addr, "127.0.0.1:8001");
        assert_eq!(builder.data_dir, PathBuf::from(".data"));
        assert_eq!(builder.wake_poll_interval, Duration::from_secs(5));
        // Batch of 10 could not feed a concurrent waker; the interval is now
        // only the idle wait, so a larger batch costs nothing when idle.
        assert_eq!(builder.wake_batch_size, 200);
        assert!(builder.wake_concurrency >= 1);
    }

    #[test]
    fn wake_concurrency_default_is_bounded() {
        // Eight per core, but never zero (which would deadlock the semaphore)
        // and never an unbounded surge on a very large host.
        let n = crate::wake_scheduler::default_wake_concurrency();
        assert!((1..=512).contains(&n), "concurrency out of bounds: {n}");
    }

    #[test]
    fn wake_concurrency_setter_overrides_default() {
        let builder = EnvironmentRuntimeBuilder::new().wake_concurrency(3);
        assert_eq!(builder.wake_concurrency, 3);
    }

    #[test]
    fn test_builder_new_equals_default() {
        let builder_new = EnvironmentRuntimeBuilder::new();
        let builder_default = EnvironmentRuntimeBuilder::default();

        assert_eq!(builder_new.core_addr, builder_default.core_addr);
        assert_eq!(builder_new.data_dir, builder_default.data_dir);
        assert_eq!(
            builder_new.wake_poll_interval,
            builder_default.wake_poll_interval
        );
        assert_eq!(builder_new.wake_batch_size, builder_default.wake_batch_size);
    }

    #[test]
    fn test_builder_core_addr() {
        let builder = EnvironmentRuntimeBuilder::new().core_addr("10.0.0.1:8001");

        assert_eq!(builder.core_addr, "10.0.0.1:8001");
    }

    #[test]
    fn test_builder_core_addr_from_string() {
        let addr = String::from("custom-host:8001");
        let builder = EnvironmentRuntimeBuilder::new().core_addr(addr);

        assert_eq!(builder.core_addr, "custom-host:8001");
    }

    #[test]
    fn test_builder_data_dir() {
        let builder = EnvironmentRuntimeBuilder::new().data_dir("/var/lib/runtara");

        assert_eq!(builder.data_dir, PathBuf::from("/var/lib/runtara"));
    }

    #[test]
    fn test_builder_data_dir_from_pathbuf() {
        let path = PathBuf::from("/custom/path");
        let builder = EnvironmentRuntimeBuilder::new().data_dir(path);

        assert_eq!(builder.data_dir, PathBuf::from("/custom/path"));
    }

    #[test]
    fn test_builder_wake_poll_interval() {
        let builder = EnvironmentRuntimeBuilder::new().wake_poll_interval(Duration::from_secs(30));

        assert_eq!(builder.wake_poll_interval, Duration::from_secs(30));
    }

    #[test]
    fn test_builder_wake_batch_size() {
        let builder = EnvironmentRuntimeBuilder::new().wake_batch_size(50);

        assert_eq!(builder.wake_batch_size, 50);
    }

    #[test]
    fn test_builder_chaining() {
        let builder = EnvironmentRuntimeBuilder::new()
            .core_addr("core.local:8001")
            .data_dir("/data")
            .wake_poll_interval(Duration::from_secs(10))
            .wake_batch_size(25);

        assert_eq!(builder.core_addr, "core.local:8001");
        assert_eq!(builder.data_dir, PathBuf::from("/data"));
        assert_eq!(builder.wake_poll_interval, Duration::from_secs(10));
        assert_eq!(builder.wake_batch_size, 25);
    }

    #[test]
    fn test_builder_build_fails_without_pool() {
        let builder = EnvironmentRuntimeBuilder::new();
        let result = builder.build();

        assert!(result.is_err());
        if let Err(err) = result {
            assert!(err.to_string().contains("pool is required"));
        }
    }

    #[test]
    fn test_environment_runtime_builder_static_method() {
        // Test that EnvironmentRuntime::builder() returns a builder
        let builder = EnvironmentRuntime::builder();

        // Should have default values
        assert_eq!(builder.core_addr, "127.0.0.1:8001");
    }

    #[test]
    fn test_builder_wake_poll_interval_subsecond() {
        let builder =
            EnvironmentRuntimeBuilder::new().wake_poll_interval(Duration::from_millis(500));

        assert_eq!(builder.wake_poll_interval, Duration::from_millis(500));
    }

    #[test]
    fn test_builder_wake_poll_interval_long() {
        let builder =
            EnvironmentRuntimeBuilder::new().wake_poll_interval(Duration::from_secs(3600));

        assert_eq!(builder.wake_poll_interval, Duration::from_secs(3600));
    }

    #[test]
    fn test_builder_wake_batch_size_one() {
        let builder = EnvironmentRuntimeBuilder::new().wake_batch_size(1);

        assert_eq!(builder.wake_batch_size, 1);
    }

    #[test]
    fn test_builder_wake_batch_size_large() {
        let builder = EnvironmentRuntimeBuilder::new().wake_batch_size(1000);

        assert_eq!(builder.wake_batch_size, 1000);
    }

    #[test]
    fn test_builder_core_addr_overwrite() {
        let builder = EnvironmentRuntimeBuilder::new()
            .core_addr("host1:8001")
            .core_addr("host2:8001");

        assert_eq!(builder.core_addr, "host2:8001");
    }
}
