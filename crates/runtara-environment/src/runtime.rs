// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embeddable runtime for runtara-environment.
//!
//! This module provides [`EnvironmentRuntime`] which allows embedding runtara-environment
//! into an existing tokio application instead of running it as a standalone server.
//!
//! # Example
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
//!         .core_addr("127.0.0.1:8001")
//!         .bind_addr("0.0.0.0:8002".parse()?)
//!         .build()?
//!         .start()
//!         .await?;
//!
//!     // ... run your application ...
//!
//!     // Graceful shutdown
//!     runtime.shutdown().await?;
//!     Ok(())
//! }
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use runtara_core::persistence::Persistence;
use sqlx::PgPool;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::handlers::EnvironmentHandlerState;
use crate::runner::Runner;
use crate::wake_scheduler::{WakeScheduler, WakeSchedulerConfig};

/// Builder for creating an [`EnvironmentRuntime`].
pub struct EnvironmentRuntimeBuilder {
    pool: Option<PgPool>,
    core_persistence: Option<Arc<dyn Persistence>>,
    runner: Option<Arc<dyn Runner>>,
    bind_addr: SocketAddr,
    core_addr: String,
    data_dir: PathBuf,
    wake_poll_interval: Duration,
    wake_batch_size: i64,
}

impl Default for EnvironmentRuntimeBuilder {
    fn default() -> Self {
        Self {
            pool: None,
            core_persistence: None,
            runner: None,
            bind_addr: "0.0.0.0:8002".parse().unwrap(),
            core_addr: "127.0.0.1:8001".to_string(),
            data_dir: PathBuf::from(".data"),
            wake_poll_interval: Duration::from_secs(5),
            wake_batch_size: 10,
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
    /// When set, the wake scheduler will query Core's `sleep_until` column
    /// directly instead of using the legacy `wake_queue` table.
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

        Ok(EnvironmentRuntimeConfig {
            pool,
            core_persistence: self.core_persistence,
            runner,
            bind_addr: self.bind_addr,
            core_addr: self.core_addr,
            data_dir: self.data_dir,
            wake_poll_interval: self.wake_poll_interval,
            wake_batch_size: self.wake_batch_size,
        })
    }
}

/// Configuration for an [`EnvironmentRuntime`].
pub struct EnvironmentRuntimeConfig {
    pool: PgPool,
    core_persistence: Option<Arc<dyn Persistence>>,
    runner: Arc<dyn Runner>,
    bind_addr: SocketAddr,
    core_addr: String,
    data_dir: PathBuf,
    wake_poll_interval: Duration,
    wake_batch_size: i64,
}

impl EnvironmentRuntimeConfig {
    /// Start the runtime, spawning the QUIC server and wake scheduler tasks.
    pub async fn start(self) -> Result<EnvironmentRuntime> {
        // Create handler state
        let state = if let Some(ref persistence) = self.core_persistence {
            Arc::new(EnvironmentHandlerState::with_core_persistence(
                self.pool.clone(),
                persistence.clone(),
                self.runner.clone(),
                self.core_addr.clone(),
                self.data_dir.clone(),
            ))
        } else {
            Arc::new(EnvironmentHandlerState::new(
                self.pool.clone(),
                self.runner.clone(),
                self.core_addr.clone(),
                self.data_dir.clone(),
            ))
        };

        // Create wake scheduler
        let wake_config = WakeSchedulerConfig {
            poll_interval: self.wake_poll_interval,
            batch_size: self.wake_batch_size,
            core_addr: self.core_addr.clone(),
            data_dir: self.data_dir.clone(),
        };

        let wake_scheduler = if let Some(ref persistence) = self.core_persistence {
            WakeScheduler::with_core_persistence(
                self.pool.clone(),
                persistence.clone(),
                self.runner.clone(),
                wake_config,
            )
        } else {
            WakeScheduler::new(self.pool.clone(), self.runner.clone(), wake_config)
        };

        let wake_shutdown = wake_scheduler.shutdown_handle();

        // Start wake scheduler task
        let wake_handle = tokio::spawn(async move {
            wake_scheduler.run().await;
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
            "EnvironmentRuntime started"
        );

        Ok(EnvironmentRuntime {
            server_handle,
            wake_handle,
            server_shutdown_tx,
            wake_shutdown,
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
///
/// Call [`shutdown`](Self::shutdown) for graceful termination.
pub struct EnvironmentRuntime {
    server_handle: JoinHandle<Result<()>>,
    wake_handle: JoinHandle<()>,
    server_shutdown_tx: watch::Sender<bool>,
    wake_shutdown: Arc<Notify>,
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
    /// This signals both the QUIC server and wake scheduler to stop,
    /// then waits for them to complete.
    pub async fn shutdown(self) -> Result<()> {
        info!("EnvironmentRuntime shutting down...");

        // Signal server shutdown
        let _ = self.server_shutdown_tx.send(true);

        // Signal wake scheduler shutdown
        self.wake_shutdown.notify_one();

        // Wait for wake scheduler
        if let Err(e) = self.wake_handle.await {
            error!("Wake scheduler task panicked: {}", e);
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
        !self.server_handle.is_finished() && !self.wake_handle.is_finished()
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
