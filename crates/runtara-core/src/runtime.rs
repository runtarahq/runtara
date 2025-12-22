// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embeddable runtime for runtara-core.
//!
//! This module provides [`CoreRuntime`] which allows embedding runtara-core
//! into an existing tokio application instead of running it as a standalone server.
//!
//! # Example
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use runtara_core::runtime::CoreRuntime;
//! use runtara_core::persistence::PostgresPersistence;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let pool = sqlx::PgPool::connect("postgres://...").await?;
//!     let persistence = Arc::new(PostgresPersistence::new(pool));
//!
//!     let runtime = CoreRuntime::builder()
//!         .persistence(persistence)
//!         .bind_addr("0.0.0.0:8001".parse()?)
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
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::instance_handlers::InstanceHandlerState;
use crate::persistence::Persistence;
use crate::server::InstanceServerState;

/// Builder for creating a [`CoreRuntime`].
pub struct CoreRuntimeBuilder {
    persistence: Option<Arc<dyn Persistence>>,
    bind_addr: SocketAddr,
}

impl std::fmt::Debug for CoreRuntimeBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreRuntimeBuilder")
            .field("persistence", &self.persistence.as_ref().map(|_| "..."))
            .field("bind_addr", &self.bind_addr)
            .finish()
    }
}

impl Default for CoreRuntimeBuilder {
    fn default() -> Self {
        Self {
            persistence: None,
            bind_addr: "0.0.0.0:8001".parse().unwrap(),
        }
    }
}

impl CoreRuntimeBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the persistence layer (required).
    pub fn persistence(mut self, persistence: Arc<dyn Persistence>) -> Self {
        self.persistence = Some(persistence);
        self
    }

    /// Set the bind address for the QUIC server.
    ///
    /// Default: `0.0.0.0:8001`
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
        self
    }

    /// Build the runtime configuration.
    ///
    /// Returns an error if required fields are missing.
    pub fn build(self) -> Result<CoreRuntimeConfig> {
        let persistence = self
            .persistence
            .ok_or_else(|| anyhow::anyhow!("persistence is required"))?;

        Ok(CoreRuntimeConfig {
            persistence,
            bind_addr: self.bind_addr,
        })
    }
}

/// Configuration for a [`CoreRuntime`].
pub struct CoreRuntimeConfig {
    persistence: Arc<dyn Persistence>,
    bind_addr: SocketAddr,
}

impl CoreRuntimeConfig {
    /// Start the runtime, spawning the QUIC server task.
    pub async fn start(self) -> Result<CoreRuntime> {
        let state = Arc::new(InstanceHandlerState::new(self.persistence));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let bind_addr = self.bind_addr;
        let server_handle = tokio::spawn(run_instance_server_with_shutdown(
            bind_addr,
            state.clone(),
            shutdown_rx,
        ));

        info!(addr = %bind_addr, "CoreRuntime started");

        Ok(CoreRuntime {
            server_handle,
            shutdown_tx,
            state,
            bind_addr,
        })
    }
}

/// A running runtara-core instance that can be embedded in an application.
///
/// The runtime manages:
/// - QUIC server for instance connections (checkpoints, signals, events)
///
/// Call [`shutdown`](Self::shutdown) for graceful termination.
pub struct CoreRuntime {
    server_handle: JoinHandle<Result<()>>,
    shutdown_tx: watch::Sender<bool>,
    state: Arc<InstanceServerState>,
    bind_addr: SocketAddr,
}

impl CoreRuntime {
    /// Create a new builder for configuring the runtime.
    pub fn builder() -> CoreRuntimeBuilder {
        CoreRuntimeBuilder::new()
    }

    /// Get the bind address of the QUIC server.
    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    /// Get a reference to the shared instance handler state.
    ///
    /// This can be used for direct access to persistence or other shared resources.
    pub fn state(&self) -> &Arc<InstanceServerState> {
        &self.state
    }

    /// Get a reference to the persistence layer.
    pub fn persistence(&self) -> &Arc<dyn Persistence> {
        &self.state.persistence
    }

    /// Gracefully shut down the runtime.
    ///
    /// This signals the QUIC server to stop accepting new connections and
    /// waits for it to complete.
    pub async fn shutdown(self) -> Result<()> {
        info!("CoreRuntime shutting down...");

        // Signal shutdown
        let _ = self.shutdown_tx.send(true);

        // Wait for server task to complete
        match self.server_handle.await {
            Ok(Ok(())) => {
                info!("CoreRuntime shutdown complete");
                Ok(())
            }
            Ok(Err(e)) => {
                error!("CoreRuntime server error during shutdown: {}", e);
                Err(e)
            }
            Err(e) => {
                error!("CoreRuntime server task panicked: {}", e);
                Err(anyhow::anyhow!("server task panicked: {}", e))
            }
        }
    }

    /// Check if the runtime is still running.
    pub fn is_running(&self) -> bool {
        !self.server_handle.is_finished()
    }
}

/// Run the instance QUIC server with shutdown support.
async fn run_instance_server_with_shutdown(
    bind_addr: SocketAddr,
    state: Arc<InstanceServerState>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    use runtara_protocol::server::RuntaraServer;
    use tracing::debug;

    let server = RuntaraServer::localhost(bind_addr)?;

    info!(addr = %bind_addr, "Instance QUIC server starting");

    loop {
        tokio::select! {
            biased;

            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Instance QUIC server received shutdown signal");
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
                                    crate::server::instance_server::handle_connection(conn_handler, state).await;
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

    info!("Instance QUIC server stopped");
    Ok(())
}
