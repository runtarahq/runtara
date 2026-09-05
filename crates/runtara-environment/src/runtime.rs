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
//!     let runner = build_runner(persistence.clone(), None)?;
//!
//!     let runtime = EnvironmentRuntime::builder()
//!         .pool(pool)
//!         .runner(runner)
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
use crate::execution_timeout::ExecutionTimeoutPolicy;
use crate::handlers::{DrainController, EnvironmentHandlerState};
use crate::heartbeat_monitor::{HeartbeatMonitor, HeartbeatMonitorConfig};
use crate::image_cleanup_worker::{ImageCleanupWorker, ImageCleanupWorkerConfig};
use crate::launch_dispatcher::{LaunchDispatcher, LaunchLifecycleObservers};
use crate::runner::Runner;
use crate::wake_scheduler::{WakeScheduler, WakeSchedulerConfig, default_wake_concurrency};

/// Idle poll interval for the wake scheduler, from
/// `RUNTARA_WAKE_POLL_INTERVAL_MS` (default 5000).
///
/// This is only the wait after a poll that found nothing more to do — a poll
/// that fills its batch is followed immediately by the next one — so it bounds
/// wake *latency* for an idle system, not wake throughput.
fn wake_poll_interval_from_env() -> Duration {
    wake_poll_interval_from_raw(
        std::env::var("RUNTARA_WAKE_POLL_INTERVAL_MS")
            .ok()
            .as_deref(),
    )
}

/// Instances claimed per wake poll, from `RUNTARA_WAKE_BATCH_SIZE`
/// (default 200).
fn wake_batch_size_from_env() -> i64 {
    wake_batch_size_from_raw(std::env::var("RUNTARA_WAKE_BATCH_SIZE").ok().as_deref())
}

/// Concurrent relaunches within a wake batch, from `RUNTARA_WAKE_CONCURRENCY`
/// (default: eight per core, see [`default_wake_concurrency`]).
/// How long a wake claim is leased for, from `RUNTARA_WAKE_CLAIM_LEASE_SECS`
/// (default: 300s). A batch claimed by a process that then dies becomes due
/// again after this long, which is the recovery path for an interrupted wake.
fn wake_claim_lease_from_env() -> Duration {
    std::env::var("RUNTARA_WAKE_CLAIM_LEASE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(300))
}

fn wake_concurrency_from_env() -> usize {
    wake_concurrency_from_raw(std::env::var("RUNTARA_WAKE_CONCURRENCY").ok().as_deref())
}

// The parsing halves are split out so they can be tested without mutating
// process-global environment state, which is shared by every test in the
// binary. Each rejects a non-positive or unparseable value rather than
// honouring it: a zero interval would busy-spin, and a zero batch or
// concurrency would stop the scheduler entirely.

fn wake_poll_interval_from_raw(raw: Option<&str>) -> Duration {
    Duration::from_millis(
        raw.and_then(|v| v.parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .unwrap_or(5_000),
    )
}

fn wake_batch_size_from_raw(raw: Option<&str>) -> i64 {
    raw.and_then(|v| v.parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(200)
}

fn wake_concurrency_from_raw(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(default_wake_concurrency)
}

/// Builder for creating an [`EnvironmentRuntime`].
pub struct EnvironmentRuntimeBuilder {
    pool: Option<PgPool>,
    core_persistence: Option<Arc<dyn Persistence>>,
    runner: Option<Arc<dyn Runner>>,
    data_dir: PathBuf,
    wake_poll_interval: Duration,
    wake_batch_size: i64,
    wake_concurrency: usize,
    wake_claim_lease: Duration,
    request_timeout: Duration,
    execution_timeout_policy: ExecutionTimeoutPolicy,
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
            data_dir: PathBuf::from(".data"),
            wake_poll_interval: wake_poll_interval_from_env(),
            wake_batch_size: wake_batch_size_from_env(),
            wake_concurrency: wake_concurrency_from_env(),
            wake_claim_lease: wake_claim_lease_from_env(),
            request_timeout: Duration::from_secs(30),
            execution_timeout_policy: ExecutionTimeoutPolicy::default(),
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

    /// Set the bounded active-execution timeout policy.
    ///
    /// The same policy must be supplied to the caller that starts instances so
    /// new starts, resumes, and scheduler wakes persist and enforce one
    /// deadline contract.
    pub fn execution_timeout_policy(mut self, policy: ExecutionTimeoutPolicy) -> Self {
        self.execution_timeout_policy = policy;
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
            data_dir: self.data_dir,
            wake_poll_interval: self.wake_poll_interval,
            wake_batch_size: self.wake_batch_size,
            wake_concurrency: self.wake_concurrency,
            wake_claim_lease: self.wake_claim_lease,
            request_timeout: self.request_timeout,
            execution_timeout_policy: self.execution_timeout_policy,
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
    data_dir: PathBuf,
    wake_poll_interval: Duration,
    wake_batch_size: i64,
    wake_concurrency: usize,
    wake_claim_lease: Duration,
    request_timeout: Duration,
    execution_timeout_policy: ExecutionTimeoutPolicy,
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
        // Scope legacy-pending recovery to rows that predate this process. New
        // starts atomically create a queue row with their instance, so this
        // can never race a valid modern request.
        let pending_start_cutoff = chrono::Utc::now();
        // Create shared drain controller so workers and the container monitor
        // all observe the same state.
        let drain = DrainController::new();
        let launch_notifier = Arc::new(Notify::new());
        let lifecycle_observers = LaunchLifecycleObservers::default();

        // Create handler state
        let state = Arc::new(
            EnvironmentHandlerState::new(
                self.pool.clone(),
                self.persistence.clone(),
                self.runner.clone(),
                self.data_dir.clone(),
            )
            .with_request_timeout(self.request_timeout)
            .with_execution_timeout_policy(self.execution_timeout_policy)
            .with_drain(drain.clone())
            .with_launch_control(launch_notifier.clone(), lifecycle_observers.clone()),
        );

        // Recover orphaned containers from previous Environment run
        // This handles containers that were running when Environment restarted
        if let Err(e) = recover_orphaned_containers(&self.pool, self.persistence.as_ref()).await {
            warn!(error = %e, "Failed to recover orphaned containers");
        }

        if let Err(e) = fail_interrupted_pending_starts(&self.pool, pending_start_cutoff).await {
            warn!(error = %e, "Failed to recover legacy pending starts without a durable launch");
        }

        // Create wake scheduler
        let wake_config = WakeSchedulerConfig {
            poll_interval: self.wake_poll_interval,
            batch_size: self.wake_batch_size,
            concurrency: self.wake_concurrency,
            claim_lease: self.wake_claim_lease,
            failed_wake_retry_delay: Duration::from_secs(5),
        };

        let wake_scheduler =
            WakeScheduler::new(self.pool.clone(), self.persistence.clone(), wake_config)
                .with_drain(drain.clone())
                .with_launch_control(launch_notifier.clone(), lifecycle_observers.clone());

        let wake_shutdown = wake_scheduler.shutdown_handle();

        let wake_handle = tokio::spawn(async move {
            wake_scheduler.run().await;
        });

        // Only this worker hands a generation to a runner. Sources commit a
        // queue row then notify it, and a periodic scan recovers notifications
        // lost across process interruption.
        let launch_dispatcher = LaunchDispatcher::new(
            self.pool.clone(),
            self.persistence.clone(),
            self.runner.clone(),
            launch_notifier,
            lifecycle_observers.clone(),
        )
        .with_execution_timeout_policy(self.execution_timeout_policy)
        .with_drain(drain.clone());
        let launch_dispatcher_shutdown = launch_dispatcher.shutdown_handle();
        let launch_dispatcher_handle = tokio::spawn(async move {
            launch_dispatcher.run().await;
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

        info!("EnvironmentRuntime started");

        Ok(EnvironmentRuntime {
            wake_handle,
            launch_dispatcher_handle,
            cleanup_handle,
            heartbeat_handle,
            db_cleanup_handle,
            wake_shutdown,
            launch_dispatcher_shutdown,
            cleanup_shutdown,
            heartbeat_shutdown,
            db_cleanup_shutdown,
            image_cleanup_handle,
            image_cleanup_shutdown,
            state,
            drain,
            lifecycle_observers,
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
    launch_dispatcher_handle: JoinHandle<()>,
    cleanup_handle: JoinHandle<()>,
    heartbeat_handle: JoinHandle<()>,
    db_cleanup_handle: JoinHandle<()>,
    wake_shutdown: Arc<Notify>,
    launch_dispatcher_shutdown: Arc<Notify>,
    cleanup_shutdown: Arc<Notify>,
    heartbeat_shutdown: Arc<Notify>,
    db_cleanup_shutdown: Arc<Notify>,
    image_cleanup_handle: JoinHandle<()>,
    image_cleanup_shutdown: Arc<Notify>,
    state: Arc<EnvironmentHandlerState>,
    drain: DrainController,
    lifecycle_observers: LaunchLifecycleObservers,
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

    /// Return the post-start-installable server admission lifecycle hook.
    ///
    /// The server starts Environment before it constructs its execution
    /// engine/outbox. It can obtain this holder afterwards and install an
    /// observer without changing that startup order.
    pub fn launch_lifecycle_observers(&self) -> LaunchLifecycleObservers {
        self.lifecycle_observers.clone()
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

        // Stop dispatch before taking the active-run snapshot. Queued rows
        // remain durable and will be reclaimed on the next runtime start.
        self.launch_dispatcher_shutdown.notify_one();

        // Stop the wake scheduler before the snapshot below, and give it a
        // moment to finish the batch it is on. The snapshot is what decides who
        // gets a shutdown signal, so a wake that registers its container after
        // it is taken is invisible to the drain: no signal, absent from the
        // straggler list, and still running into teardown. The scheduler also
        // re-checks the drain flag per launch, which covers the batch already
        // in flight here; this just stops new ones being claimed at all.
        self.wake_shutdown.notify_one();
        let quiesce_deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !self.wake_handle.is_finished() && std::time::Instant::now() < quiesce_deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !self.wake_handle.is_finished() {
            warn!("Wake scheduler did not quiesce before the drain snapshot");
        }

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
                launch_id: info.launch_id.clone(),
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
            match persistence.get_instance_meta(&info.instance_id).await {
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

        // Signal durable launch dispatcher shutdown
        self.launch_dispatcher_shutdown.notify_one();

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

        if let Err(e) = self.launch_dispatcher_handle.await {
            error!("Launch dispatcher task panicked: {}", e);
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
            && !self.launch_dispatcher_handle.is_finished()
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
        match persistence.get_instance_meta(instance_id).await {
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
                        pool,
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

/// Terminalize legacy starts left pending without a durable queue generation.
///
/// A modern accepted start has one active `instance_launches` row and is never
/// a candidate, even if it waited through a process restart. This narrow
/// startup repair exists only for rows created before the queue migration (or
/// malformed rows written outside the atomic initial-claim transaction).
///
/// The status predicate on the `UPDATE` is a final guard. It makes a row that
/// advanced concurrently a no-op rather than overwriting a live transition.
async fn fail_interrupted_pending_starts<'e, E>(
    executor: E,
    started_before: chrono::DateTime<chrono::Utc>,
) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows: Vec<(bool,)> = sqlx::query_as(
        r#"
        WITH candidates AS (
            SELECT
                i.instance_id,
                EXISTS (
                    SELECT 1
                    FROM instance_images ii
                    WHERE ii.instance_id = i.instance_id
                ) AS has_image
            FROM instances i
            WHERE i.status = 'pending'
              AND i.created_at < $1
              AND NOT EXISTS (
                    SELECT 1
                    FROM container_registry cr
                    WHERE cr.instance_id = i.instance_id
              )
              AND NOT EXISTS (
                    SELECT 1
                    FROM instance_launches launch
                    WHERE launch.instance_id = i.instance_id
                      AND launch.state IN ('queued', 'preparing', 'leased', 'starting', 'running')
              )
        )
        UPDATE instances i
        SET status = 'failed',
            finished_at = NOW(),
            sleep_until = NULL,
            termination_reason = 'environment_restart',
            error = CASE
                WHEN candidates.has_image THEN
                    'Instance start was interrupted before runner launch'
                ELSE
                    'Instance start was interrupted before an image was bound'
            END
        FROM candidates
        WHERE i.instance_id = candidates.instance_id
          AND i.status = 'pending'
        RETURNING candidates.has_image
        "#,
    )
    .bind(started_before)
    .fetch_all(executor)
    .await?;

    if rows.is_empty() {
        debug!("No interrupted pending starts to recover");
        return Ok(());
    }

    let image_bound = rows.iter().filter(|(has_image,)| *has_image).count();
    let image_less = rows.len() - image_bound;
    warn!(
        image_bound,
        image_less,
        total = rows.len(),
        "Terminalized pending starts interrupted before runner registration"
    );

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
        assert_eq!(builder.data_dir, PathBuf::from(".data"));
        assert_eq!(builder.wake_poll_interval, Duration::from_secs(5));
        // Batch of 10 could not feed a concurrent waker; the interval is now
        // only the idle wait, so a larger batch costs nothing when idle.
        assert_eq!(builder.wake_batch_size, 200);
        assert!(builder.wake_concurrency >= 1);
    }

    #[test]
    fn wake_poll_interval_parsing() {
        assert_eq!(
            wake_poll_interval_from_raw(None),
            Duration::from_secs(5),
            "unset falls back to the documented default"
        );
        assert_eq!(
            wake_poll_interval_from_raw(Some("250")),
            Duration::from_millis(250)
        );
        // A zero or malformed interval would turn the loop into a busy wait.
        assert_eq!(
            wake_poll_interval_from_raw(Some("0")),
            Duration::from_secs(5)
        );
        assert_eq!(
            wake_poll_interval_from_raw(Some("not-a-number")),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn wake_batch_size_parsing() {
        assert_eq!(wake_batch_size_from_raw(None), 200);
        assert_eq!(wake_batch_size_from_raw(Some("50")), 50);
        // Zero or negative would claim nothing, stalling the scheduler.
        assert_eq!(wake_batch_size_from_raw(Some("0")), 200);
        assert_eq!(wake_batch_size_from_raw(Some("-5")), 200);
        assert_eq!(wake_batch_size_from_raw(Some("")), 200);
    }

    #[test]
    fn wake_concurrency_parsing() {
        assert_eq!(wake_concurrency_from_raw(Some("7")), 7);
        // Zero would deadlock the semaphore; fall back to the cored default.
        assert_eq!(
            wake_concurrency_from_raw(Some("0")),
            default_wake_concurrency()
        );
        assert_eq!(
            wake_concurrency_from_raw(Some("nope")),
            default_wake_concurrency()
        );
        assert_eq!(wake_concurrency_from_raw(None), default_wake_concurrency());
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

        assert_eq!(builder_new.data_dir, builder_default.data_dir);
        assert_eq!(
            builder_new.wake_poll_interval,
            builder_default.wake_poll_interval
        );
        assert_eq!(builder_new.wake_batch_size, builder_default.wake_batch_size);
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
            .data_dir("/data")
            .wake_poll_interval(Duration::from_secs(10))
            .wake_batch_size(25);

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
        assert_eq!(builder.data_dir, PathBuf::from(".data"));
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

    /// The pending-start scan is deliberately scoped to entries that predate
    /// this runtime. This test uses an old timestamp so its transaction cannot
    /// interfere with other feature-gated unit tests that may be starting a
    /// current instance against the shared test database.
    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn startup_recovery_fails_only_old_pending_starts_without_a_runner() {
        let pool = crate::test_support::pool().await;
        let mut tx = pool.begin().await.expect("begin test transaction");

        let tenant_id = crate::test_support::unique_id("pending-start-recovery-tenant");
        let image_id = crate::test_support::unique_id("pending-start-recovery-image");
        let image_name = crate::test_support::unique_id("pending-start-recovery-name");
        let unbound = crate::test_support::unique_id("pending-start-recovery-unbound");
        let bound = crate::test_support::unique_id("pending-start-recovery-bound");
        let registered = crate::test_support::unique_id("pending-start-recovery-registered");
        let preparing = crate::test_support::unique_id("pending-start-recovery-preparing");
        let fresh = crate::test_support::unique_id("pending-start-recovery-fresh");
        let old = chrono::Utc::now() - chrono::Duration::hours(2);
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);

        sqlx::query(
            r#"
            INSERT INTO images (image_id, tenant_id, name, binary_path)
            VALUES ($1, $2, $3, '/test/pending-start-recovery.wasm')
            "#,
        )
        .bind(&image_id)
        .bind(&tenant_id)
        .bind(&image_name)
        .execute(&mut *tx)
        .await
        .expect("insert image");

        for instance_id in [&unbound, &bound, &registered, &preparing] {
            sqlx::query(
                r#"
                INSERT INTO instances (instance_id, tenant_id, status, created_at)
                VALUES ($1, $2, 'pending', $3)
                "#,
            )
            .bind(instance_id)
            .bind(&tenant_id)
            .bind(old)
            .execute(&mut *tx)
            .await
            .expect("insert old pending instance");
        }
        sqlx::query(
            r#"
            INSERT INTO instances (instance_id, tenant_id, status, created_at)
            VALUES ($1, $2, 'pending', NOW())
            "#,
        )
        .bind(&fresh)
        .bind(&tenant_id)
        .execute(&mut *tx)
        .await
        .expect("insert fresh pending instance");

        for instance_id in [&bound, &registered, &preparing] {
            sqlx::query(
                r#"
                INSERT INTO instance_images (instance_id, image_id, tenant_id, created_at)
                VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(instance_id)
            .bind(&image_id)
            .bind(&tenant_id)
            .bind(old)
            .execute(&mut *tx)
            .await
            .expect("bind image to pending instance");
        }
        sqlx::query(
            r#"
            INSERT INTO instance_launches (
                launch_id, instance_id, tenant_id, image_id, kind, state,
                available_at, deadline_at, lease_owner, lease_expires_at,
                attempt_count, created_at, updated_at
            ) VALUES (
                $1, $2, $3, $4, 'start', 'preparing',
                $5, $6, 'preparation-worker', $6, 1, $5, $5
            )
            "#,
        )
        .bind(crate::test_support::unique_id(
            "pending-start-recovery-preparing-launch",
        ))
        .bind(&preparing)
        .bind(&tenant_id)
        .bind(&image_id)
        .bind(old)
        .bind(old + chrono::Duration::hours(3))
        .execute(&mut *tx)
        .await
        .expect("insert active preparation launch");
        sqlx::query(
            r#"
            INSERT INTO container_registry (
                container_id, launch_id, instance_id, tenant_id, binary_path, started_at
            ) VALUES ($1, $2, $3, $4, '/test/pending-start-recovery.wasm', $5)
            "#,
        )
        .bind(crate::test_support::unique_id(
            "pending-start-recovery-container",
        ))
        .bind(crate::test_support::unique_id(
            "pending-start-recovery-launch",
        ))
        .bind(&registered)
        .bind(&tenant_id)
        .bind(old)
        .execute(&mut *tx)
        .await
        .expect("register live pending start");

        fail_interrupted_pending_starts(&mut *tx, cutoff)
            .await
            .expect("recover interrupted pending starts");

        let states: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
            SELECT instance_id, status::TEXT, error, termination_reason::TEXT
            FROM instances
            WHERE instance_id = ANY($1)
            "#,
        )
        .bind(vec![
            unbound.clone(),
            bound.clone(),
            registered.clone(),
            preparing.clone(),
            fresh.clone(),
        ])
        .fetch_all(&mut *tx)
        .await
        .expect("read recovered states");
        let state = |instance_id: &str| {
            states
                .iter()
                .find(|(id, _, _, _)| id == instance_id)
                .expect("seeded instance must exist")
        };

        let (_, unbound_status, unbound_error, unbound_reason) = state(&unbound);
        assert_eq!(unbound_status, "failed");
        assert_eq!(
            unbound_error.as_deref(),
            Some("Instance start was interrupted before an image was bound")
        );
        assert_eq!(unbound_reason.as_deref(), Some("environment_restart"));

        let (_, bound_status, bound_error, bound_reason) = state(&bound);
        assert_eq!(bound_status, "failed");
        assert_eq!(
            bound_error.as_deref(),
            Some("Instance start was interrupted before runner launch")
        );
        assert_eq!(bound_reason.as_deref(), Some("environment_restart"));

        let (_, registered_status, registered_error, registered_reason) = state(&registered);
        assert_eq!(registered_status, "pending");
        assert!(registered_error.is_none());
        assert!(registered_reason.is_none());

        let (_, preparing_status, preparing_error, preparing_reason) = state(&preparing);
        assert_eq!(preparing_status, "pending");
        assert!(preparing_error.is_none());
        assert!(preparing_reason.is_none());

        let (_, fresh_status, fresh_error, fresh_reason) = state(&fresh);
        assert_eq!(fresh_status, "pending");
        assert!(fresh_error.is_none());
        assert!(fresh_reason.is_none());

        tx.rollback().await.expect("roll back test transaction");
    }
}
