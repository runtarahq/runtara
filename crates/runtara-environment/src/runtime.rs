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
//! use runtara_environment::runner::oci::OciRunner;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let pool = sqlx::PgPool::connect("postgres://...").await?;
//!     let runner = Arc::new(OciRunner::from_env());
//!
//!     let runtime = EnvironmentRuntime::builder()
//!         .pool(pool)
//!         .runner(runner)
//!         .core_addr("127.0.0.1:8001")  // External Core server
//!         .bind_addr("0.0.0.0:8002".parse()?)
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
//! # Embedded Core Example
//!
//! To run both Environment and Core in a single process, provide `core_persistence`
//! and `core_bind_addr`:
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use runtara_core::persistence::PostgresPersistence;
//! use runtara_environment::runtime::EnvironmentRuntime;
//! use runtara_environment::runner::oci::OciRunner;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let pool = sqlx::PgPool::connect("postgres://...").await?;
//!     let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
//!     let runner = Arc::new(OciRunner::from_env());
//!
//!     let runtime = EnvironmentRuntime::builder()
//!         .pool(pool)
//!         .runner(runner)
//!         .core_persistence(persistence)               // Share persistence with Core
//!         .core_bind_addr("0.0.0.0:8001".parse()?)     // Start embedded Core on this address
//!         .core_addr("127.0.0.1:8001")                 // Address instances use to connect
//!         .bind_addr("0.0.0.0:8002".parse()?)
//!         .build()?
//!         .start()
//!         .await?;
//!
//!     // Both Environment QUIC server (8002) and Core QUIC server (8001) are now running
//!     // in this single process, sharing the same persistence layer.
//!
//!     runtime.shutdown().await?;  // Shuts down both Environment and embedded Core
//!     Ok(())
//! }
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use runtara_core::persistence::Persistence;
use runtara_core::runtime::CoreRuntime;
use sqlx::PgPool;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::cleanup_worker::{CleanupWorker, CleanupWorkerConfig};
use crate::container_registry::ContainerRegistry;
use crate::db_cleanup_worker::{DbCleanupWorker, DbCleanupWorkerConfig};
use crate::handlers::EnvironmentHandlerState;
use crate::heartbeat_monitor::{HeartbeatMonitor, HeartbeatMonitorConfig};
use crate::runner::Runner;
use crate::wake_scheduler::{WakeScheduler, WakeSchedulerConfig};

/// Builder for creating an [`EnvironmentRuntime`].
pub struct EnvironmentRuntimeBuilder {
    pool: Option<PgPool>,
    core_persistence: Option<Arc<dyn Persistence>>,
    runner: Option<Arc<dyn Runner>>,
    bind_addr: SocketAddr,
    core_addr: String,
    core_bind_addr: Option<SocketAddr>,
    data_dir: PathBuf,
    wake_poll_interval: Duration,
    wake_batch_size: i64,
    request_timeout: Duration,
    cleanup_poll_interval: Duration,
    cleanup_max_age: Duration,
    heartbeat_poll_interval: Duration,
    heartbeat_timeout: Duration,
    db_cleanup_config: DbCleanupWorkerConfig,
}

impl Default for EnvironmentRuntimeBuilder {
    fn default() -> Self {
        Self {
            pool: None,
            core_persistence: None,
            runner: None,
            bind_addr: "0.0.0.0:8002".parse().unwrap(),
            core_addr: "127.0.0.1:8001".to_string(),
            core_bind_addr: None,
            data_dir: PathBuf::from(".data"),
            wake_poll_interval: Duration::from_secs(5),
            wake_batch_size: 10,
            request_timeout: Duration::from_secs(30),
            cleanup_poll_interval: Duration::from_secs(3600), // 1 hour
            cleanup_max_age: Duration::from_secs(24 * 3600),  // 24 hours
            heartbeat_poll_interval: Duration::from_secs(30), // 30 seconds
            heartbeat_timeout: Duration::from_secs(120),      // 2 minutes
            db_cleanup_config: DbCleanupWorkerConfig::from_env(),
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

    /// Set the bind address for the QUIC server.
    ///
    /// Default: `0.0.0.0:8002`
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
        self
    }

    /// Set the address of runtara-core (passed to instances).
    ///
    /// Default: `127.0.0.1:8001`
    pub fn core_addr(mut self, addr: impl Into<String>) -> Self {
        self.core_addr = addr.into();
        self
    }

    /// Set the bind address for the embedded runtara-core QUIC server.
    ///
    /// When set along with [`core_persistence`](Self::core_persistence), an embedded
    /// `CoreRuntime` will be started that listens on this address for instance connections.
    /// This allows running both Environment and Core in a single process.
    ///
    /// If not set, no embedded Core is started and instances must connect to an external
    /// runtara-core server at `core_addr`.
    ///
    /// Default: `None` (no embedded Core)
    pub fn core_bind_addr(mut self, addr: SocketAddr) -> Self {
        self.core_bind_addr = Some(addr);
        self
    }

    /// Set the data directory for images, bundles, and instance I/O.
    ///
    /// Default: `.data`
    pub fn data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_dir = path.into();
        self
    }

    /// Set the wake scheduler poll interval.
    ///
    /// Default: 5 seconds
    pub fn wake_poll_interval(mut self, interval: Duration) -> Self {
        self.wake_poll_interval = interval;
        self
    }

    /// Set the wake scheduler batch size.
    ///
    /// Default: 10
    pub fn wake_batch_size(mut self, size: i64) -> Self {
        self.wake_batch_size = size;
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
            bind_addr: self.bind_addr,
            core_addr: self.core_addr,
            core_bind_addr: self.core_bind_addr,
            data_dir: self.data_dir,
            wake_poll_interval: self.wake_poll_interval,
            wake_batch_size: self.wake_batch_size,
            request_timeout: self.request_timeout,
            cleanup_poll_interval: self.cleanup_poll_interval,
            cleanup_max_age: self.cleanup_max_age,
            heartbeat_poll_interval: self.heartbeat_poll_interval,
            heartbeat_timeout: self.heartbeat_timeout,
            db_cleanup_config: self.db_cleanup_config,
        })
    }
}

/// Configuration for an [`EnvironmentRuntime`].
pub struct EnvironmentRuntimeConfig {
    pool: PgPool,
    persistence: Arc<dyn Persistence>,
    runner: Arc<dyn Runner>,
    bind_addr: SocketAddr,
    core_addr: String,
    core_bind_addr: Option<SocketAddr>,
    data_dir: PathBuf,
    wake_poll_interval: Duration,
    wake_batch_size: i64,
    request_timeout: Duration,
    cleanup_poll_interval: Duration,
    cleanup_max_age: Duration,
    heartbeat_poll_interval: Duration,
    heartbeat_timeout: Duration,
    db_cleanup_config: DbCleanupWorkerConfig,
}

impl EnvironmentRuntimeConfig {
    /// Start the runtime, spawning the QUIC server and wake scheduler tasks.
    pub async fn start(self) -> Result<EnvironmentRuntime> {
        // Start embedded CoreRuntime if core_bind_addr is configured
        let core_runtime = if let Some(core_bind_addr) = self.core_bind_addr {
            info!(
                bind_addr = %core_bind_addr,
                "Starting embedded CoreRuntime"
            );

            let core = CoreRuntime::builder()
                .persistence(self.persistence.clone())
                .bind_addr(core_bind_addr)
                .build()?
                .start()
                .await?;

            Some(core)
        } else {
            debug!(
                core_addr = %self.core_addr,
                "core_bind_addr not configured; instances will connect to external Core"
            );
            None
        };

        // Create handler state
        let state = Arc::new(
            EnvironmentHandlerState::new(
                self.pool.clone(),
                self.persistence.clone(),
                self.runner.clone(),
                self.core_addr.clone(),
                self.data_dir.clone(),
            )
            .with_request_timeout(self.request_timeout),
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
            core_addr: self.core_addr.clone(),
            data_dir: self.data_dir.clone(),
        };

        let wake_scheduler = WakeScheduler::new(
            self.pool.clone(),
            self.persistence.clone(),
            self.runner.clone(),
            wake_config,
        );

        let wake_shutdown = wake_scheduler.shutdown_handle();

        let wake_handle = tokio::spawn(async move {
            wake_scheduler.run().await;
        });

        // Create cleanup worker
        let cleanup_config = CleanupWorkerConfig {
            data_dir: self.data_dir.clone(),
            poll_interval: self.cleanup_poll_interval,
            max_age: self.cleanup_max_age,
        };
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
        );
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

        // Start QUIC server task
        let (server_shutdown_tx, server_shutdown_rx) = watch::channel(false);
        let bind_addr = self.bind_addr;
        let server_state = state.clone();

        let server_handle = tokio::spawn(run_environment_server_with_shutdown(
            bind_addr,
            server_state,
            server_shutdown_rx,
        ));

        info!(
            bind_addr = %bind_addr,
            core_addr = %self.core_addr,
            embedded_core = core_runtime.is_some(),
            "EnvironmentRuntime started"
        );

        Ok(EnvironmentRuntime {
            server_handle,
            wake_handle,
            cleanup_handle,
            heartbeat_handle,
            db_cleanup_handle,
            core_runtime,
            server_shutdown_tx,
            wake_shutdown,
            cleanup_shutdown,
            heartbeat_shutdown,
            db_cleanup_shutdown,
            state,
            bind_addr,
        })
    }
}

/// A running runtara-environment instance that can be embedded in an application.
///
/// The runtime manages:
/// - QUIC server for management SDK connections (images, instances, signals)
/// - Wake scheduler for durable sleep wake-ups
/// - Cleanup worker for removing old run directories
/// - Database cleanup worker for removing old database records
/// - Heartbeat monitor for detecting and failing stale instances
/// - Embedded runtara-core (optional, when `core_bind_addr` is configured)
///
/// Call [`shutdown`](Self::shutdown) for graceful termination.
pub struct EnvironmentRuntime {
    server_handle: JoinHandle<Result<()>>,
    wake_handle: JoinHandle<()>,
    cleanup_handle: JoinHandle<()>,
    heartbeat_handle: JoinHandle<()>,
    db_cleanup_handle: JoinHandle<()>,
    core_runtime: Option<CoreRuntime>,
    server_shutdown_tx: watch::Sender<bool>,
    wake_shutdown: Arc<Notify>,
    cleanup_shutdown: Arc<Notify>,
    heartbeat_shutdown: Arc<Notify>,
    db_cleanup_shutdown: Arc<Notify>,
    state: Arc<EnvironmentHandlerState>,
    bind_addr: SocketAddr,
}

impl EnvironmentRuntime {
    /// Create a new builder for configuring the runtime.
    pub fn builder() -> EnvironmentRuntimeBuilder {
        EnvironmentRuntimeBuilder::new()
    }

    /// Get the bind address of the QUIC server.
    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    /// Get a reference to the shared handler state.
    pub fn state(&self) -> &Arc<EnvironmentHandlerState> {
        &self.state
    }

    /// Gracefully shut down the runtime.
    ///
    /// This signals the QUIC server, wake scheduler, cleanup worker, database
    /// cleanup worker, heartbeat monitor, and embedded CoreRuntime (if present)
    /// to stop, then waits for them to complete.
    pub async fn shutdown(self) -> Result<()> {
        info!("EnvironmentRuntime shutting down...");

        // Signal server shutdown
        let _ = self.server_shutdown_tx.send(true);

        // Signal wake scheduler shutdown
        self.wake_shutdown.notify_one();

        // Signal cleanup worker shutdown
        self.cleanup_shutdown.notify_one();

        // Signal heartbeat monitor shutdown
        self.heartbeat_shutdown.notify_one();

        // Signal database cleanup worker shutdown
        self.db_cleanup_shutdown.notify_one();

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

        // Shutdown embedded CoreRuntime (if running)
        if let Some(core) = self.core_runtime
            && let Err(e) = core.shutdown().await
        {
            error!("Embedded CoreRuntime error during shutdown: {}", e);
        }

        // Wait for server
        match self.server_handle.await {
            Ok(Ok(())) => {
                info!("EnvironmentRuntime shutdown complete");
                Ok(())
            }
            Ok(Err(e)) => {
                error!("EnvironmentRuntime server error during shutdown: {}", e);
                Err(e)
            }
            Err(e) => {
                error!("EnvironmentRuntime server task panicked: {}", e);
                Err(anyhow::anyhow!("server task panicked: {}", e))
            }
        }
    }

    /// Check if the runtime is still running.
    pub fn is_running(&self) -> bool {
        let core_running = self.core_runtime.as_ref().is_none_or(|c| c.is_running());

        !self.server_handle.is_finished()
            && !self.wake_handle.is_finished()
            && !self.cleanup_handle.is_finished()
            && !self.heartbeat_handle.is_finished()
            && !self.db_cleanup_handle.is_finished()
            && core_running
    }

    /// Get a reference to the embedded CoreRuntime, if running.
    pub fn core_runtime(&self) -> Option<&CoreRuntime> {
        self.core_runtime.as_ref()
    }
}

/// Run the environment QUIC server with shutdown support.
async fn run_environment_server_with_shutdown(
    bind_addr: SocketAddr,
    state: Arc<EnvironmentHandlerState>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    use runtara_protocol::server::RuntaraServer;

    let server = RuntaraServer::localhost(bind_addr)?;

    info!(addr = %bind_addr, "Environment QUIC server starting");

    loop {
        tokio::select! {
            biased;

            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Environment QUIC server received shutdown signal");
                    server.close();
                    break;
                }
            }

            incoming = server.accept() => {
                match incoming {
                    Some(incoming) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            match incoming.await {
                                Ok(connection) => {
                                    let remote_addr = connection.remote_address();
                                    debug!(%remote_addr, "accepted connection");

                                    let conn_handler = runtara_protocol::server::ConnectionHandler::new(connection);
                                    crate::server::handle_connection(conn_handler, state).await;
                                }
                                Err(e) => {
                                    debug!("failed to accept connection: {}", e);
                                }
                            }
                        });
                    }
                    None => {
                        // Endpoint closed
                        break;
                    }
                }
            }
        }
    }

    info!("Environment QUIC server stopped");
    Ok(())
}

/// Check if a process is alive by checking /proc/<pid> existence.
fn is_process_alive(pid: i32) -> bool {
    std::path::Path::new(&format!("/proc/{}", pid)).exists()
}

/// Recover orphaned containers on startup.
///
/// When the Environment restarts, there may be containers in the registry
/// that were running before the restart. This function checks each one:
///
/// - If PID exists → the container is still running (will be handled by heartbeat monitor)
/// - If PID is gone + Core shows terminal status → clean up registry
/// - If PID is gone + Core shows "running" → mark as crashed and clean up
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

    for container in containers {
        let instance_id = &container.instance_id;

        // Check if process is still alive
        let is_alive = if let Some(pid) = container.pid {
            is_process_alive(pid)
        } else {
            // No PID recorded - can't check, assume dead
            false
        };

        if is_alive {
            // Process is still running - heartbeat monitor will handle it
            debug!(
                instance_id = %instance_id,
                pid = ?container.pid,
                "Container process still alive, leaving for heartbeat monitor"
            );
            continue;
        }

        // Process is gone - check Core status
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
                    // Still shows "running" but process is gone - crashed
                    warn!(
                        instance_id = %instance_id,
                        status = %status,
                        pid = ?container.pid,
                        "Found orphaned container (process gone, Core shows running) - marking as crashed"
                    );

                    // Mark as crashed in Core
                    if let Err(e) = persistence
                        .complete_instance_with_termination_if_running(
                            instance_id,
                            "failed",
                            Some("crashed"),
                            None,
                            None,
                            Some("Process terminated during Environment restart"),
                            None,
                            None,
                        )
                        .await
                    {
                        error!(
                            instance_id = %instance_id,
                            error = %e,
                            "Failed to mark orphaned instance as crashed"
                        );
                    }

                    // Clean up registry
                    let _ = registry.cleanup(instance_id).await;
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
        assert_eq!(
            builder.bind_addr,
            "0.0.0.0:8002".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(builder.core_addr, "127.0.0.1:8001");
        assert!(builder.core_bind_addr.is_none());
        assert_eq!(builder.data_dir, PathBuf::from(".data"));
        assert_eq!(builder.wake_poll_interval, Duration::from_secs(5));
        assert_eq!(builder.wake_batch_size, 10);
    }

    #[test]
    fn test_builder_new_equals_default() {
        let builder_new = EnvironmentRuntimeBuilder::new();
        let builder_default = EnvironmentRuntimeBuilder::default();

        assert_eq!(builder_new.bind_addr, builder_default.bind_addr);
        assert_eq!(builder_new.core_addr, builder_default.core_addr);
        assert_eq!(builder_new.core_bind_addr, builder_default.core_bind_addr);
        assert_eq!(builder_new.data_dir, builder_default.data_dir);
        assert_eq!(
            builder_new.wake_poll_interval,
            builder_default.wake_poll_interval
        );
        assert_eq!(builder_new.wake_batch_size, builder_default.wake_batch_size);
    }

    #[test]
    fn test_builder_bind_addr() {
        let builder =
            EnvironmentRuntimeBuilder::new().bind_addr("192.168.1.1:9000".parse().unwrap());

        assert_eq!(
            builder.bind_addr,
            "192.168.1.1:9000".parse::<SocketAddr>().unwrap()
        );
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
    fn test_builder_core_bind_addr() {
        let builder =
            EnvironmentRuntimeBuilder::new().core_bind_addr("0.0.0.0:8001".parse().unwrap());

        assert_eq!(
            builder.core_bind_addr,
            Some("0.0.0.0:8001".parse::<SocketAddr>().unwrap())
        );
    }

    #[test]
    fn test_builder_core_bind_addr_default_none() {
        let builder = EnvironmentRuntimeBuilder::new();

        assert!(builder.core_bind_addr.is_none());
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
            .bind_addr("0.0.0.0:9000".parse().unwrap())
            .core_addr("core.local:8001")
            .data_dir("/data")
            .wake_poll_interval(Duration::from_secs(10))
            .wake_batch_size(25);

        assert_eq!(
            builder.bind_addr,
            "0.0.0.0:9000".parse::<SocketAddr>().unwrap()
        );
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
        assert_eq!(
            builder.bind_addr,
            "0.0.0.0:8002".parse::<SocketAddr>().unwrap()
        );
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
    fn test_builder_ipv6_bind_addr() {
        let builder = EnvironmentRuntimeBuilder::new().bind_addr("[::]:8002".parse().unwrap());

        assert_eq!(
            builder.bind_addr,
            "[::]:8002".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn test_builder_overwrite_values() {
        let builder = EnvironmentRuntimeBuilder::new()
            .bind_addr("0.0.0.0:9000".parse().unwrap())
            .bind_addr("0.0.0.0:9001".parse().unwrap());

        // Last value should win
        assert_eq!(
            builder.bind_addr,
            "0.0.0.0:9001".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn test_builder_core_addr_overwrite() {
        let builder = EnvironmentRuntimeBuilder::new()
            .core_addr("host1:8001")
            .core_addr("host2:8001");

        assert_eq!(builder.core_addr, "host2:8001");
    }
}
