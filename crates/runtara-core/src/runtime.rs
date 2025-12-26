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

impl std::fmt::Debug for CoreRuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreRuntimeConfig")
            .field("persistence", &"...")
            .field("bind_addr", &self.bind_addr)
            .finish()
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreError;
    use crate::persistence::{
        CheckpointRecord, CustomSignalRecord, EventRecord, InstanceRecord, ListEventsFilter,
        Persistence, SignalRecord,
    };
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};

    /// Mock persistence for testing the runtime builder without database.
    struct MockPersistence;

    #[async_trait]
    impl Persistence for MockPersistence {
        async fn register_instance(
            &self,
            _instance_id: &str,
            _tenant_id: &str,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn get_instance(
            &self,
            _instance_id: &str,
        ) -> Result<Option<InstanceRecord>, CoreError> {
            Ok(None)
        }

        async fn update_instance_status(
            &self,
            _instance_id: &str,
            _status: &str,
            _started_at: Option<DateTime<Utc>>,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn update_instance_checkpoint(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn complete_instance(
            &self,
            _instance_id: &str,
            _output: Option<&[u8]>,
            _error: Option<&str>,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn save_checkpoint(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
            _state: &[u8],
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn load_checkpoint(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
        ) -> Result<Option<CheckpointRecord>, CoreError> {
            Ok(None)
        }

        async fn list_checkpoints(
            &self,
            _instance_id: &str,
            _checkpoint_id: Option<&str>,
            _limit: i64,
            _offset: i64,
            _created_after: Option<DateTime<Utc>>,
            _created_before: Option<DateTime<Utc>>,
        ) -> Result<Vec<CheckpointRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn count_checkpoints(
            &self,
            _instance_id: &str,
            _checkpoint_id: Option<&str>,
            _created_after: Option<DateTime<Utc>>,
            _created_before: Option<DateTime<Utc>>,
        ) -> Result<i64, CoreError> {
            Ok(0)
        }

        async fn insert_event(&self, _event: &EventRecord) -> Result<(), CoreError> {
            Ok(())
        }

        async fn insert_signal(
            &self,
            _instance_id: &str,
            _signal_type: &str,
            _payload: &[u8],
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn get_pending_signal(
            &self,
            _instance_id: &str,
        ) -> Result<Option<SignalRecord>, CoreError> {
            Ok(None)
        }

        async fn acknowledge_signal(&self, _instance_id: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn insert_custom_signal(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
            _payload: &[u8],
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn take_pending_custom_signal(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
        ) -> Result<Option<CustomSignalRecord>, CoreError> {
            Ok(None)
        }

        async fn save_retry_attempt(
            &self,
            _instance_id: &str,
            _checkpoint_id: &str,
            _attempt: i32,
            _error_message: Option<&str>,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn list_instances(
            &self,
            _tenant_id: Option<&str>,
            _status: Option<&str>,
            _limit: i64,
            _offset: i64,
        ) -> Result<Vec<InstanceRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn health_check_db(&self) -> Result<bool, CoreError> {
            Ok(true)
        }

        async fn count_active_instances(&self) -> Result<i64, CoreError> {
            Ok(0)
        }

        async fn set_instance_sleep(
            &self,
            _instance_id: &str,
            _sleep_until: DateTime<Utc>,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn clear_instance_sleep(&self, _instance_id: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn get_sleeping_instances_due(
            &self,
            _limit: i64,
        ) -> Result<Vec<InstanceRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn list_events(
            &self,
            _instance_id: &str,
            _filter: &ListEventsFilter,
            _limit: i64,
            _offset: i64,
        ) -> Result<Vec<EventRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn count_events(
            &self,
            _instance_id: &str,
            _filter: &ListEventsFilter,
        ) -> Result<i64, CoreError> {
            Ok(0)
        }
    }

    #[test]
    fn test_builder_default() {
        let builder = CoreRuntimeBuilder::default();
        assert!(builder.persistence.is_none());
        assert_eq!(builder.bind_addr.port(), 8001);
    }

    #[test]
    fn test_builder_new() {
        let builder = CoreRuntimeBuilder::new();
        assert!(builder.persistence.is_none());
        assert_eq!(builder.bind_addr.port(), 8001);
    }

    #[test]
    fn test_builder_persistence() {
        let persistence = Arc::new(MockPersistence);
        let builder = CoreRuntimeBuilder::new().persistence(persistence);
        assert!(builder.persistence.is_some());
    }

    #[test]
    fn test_builder_bind_addr() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let builder = CoreRuntimeBuilder::new().bind_addr(addr);
        assert_eq!(builder.bind_addr.port(), 9000);
    }

    #[test]
    fn test_builder_chaining() {
        let persistence = Arc::new(MockPersistence);
        let addr: SocketAddr = "127.0.0.1:9001".parse().unwrap();
        let builder = CoreRuntimeBuilder::new()
            .persistence(persistence)
            .bind_addr(addr);
        assert!(builder.persistence.is_some());
        assert_eq!(builder.bind_addr.port(), 9001);
    }

    #[test]
    fn test_builder_debug() {
        let builder = CoreRuntimeBuilder::new();
        let debug_str = format!("{:?}", builder);
        assert!(debug_str.contains("CoreRuntimeBuilder"));
        assert!(debug_str.contains("bind_addr"));
    }

    #[test]
    fn test_builder_debug_with_persistence() {
        let persistence = Arc::new(MockPersistence);
        let builder = CoreRuntimeBuilder::new().persistence(persistence);
        let debug_str = format!("{:?}", builder);
        assert!(debug_str.contains("CoreRuntimeBuilder"));
        // persistence is shown as "..." to avoid leaking details
        assert!(debug_str.contains("..."));
    }

    #[test]
    fn test_builder_build_missing_persistence() {
        let result = CoreRuntimeBuilder::new().build();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("persistence is required"));
    }

    #[test]
    fn test_builder_build_success() {
        let persistence = Arc::new(MockPersistence);
        let result = CoreRuntimeBuilder::new().persistence(persistence).build();
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.bind_addr.port(), 8001);
    }

    #[test]
    fn test_builder_build_with_custom_addr() {
        let persistence = Arc::new(MockPersistence);
        let addr: SocketAddr = "0.0.0.0:9002".parse().unwrap();
        let result = CoreRuntimeBuilder::new()
            .persistence(persistence)
            .bind_addr(addr)
            .build();
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.bind_addr.port(), 9002);
    }

    #[test]
    fn test_core_runtime_builder_static_method() {
        let builder = CoreRuntime::builder();
        assert!(builder.persistence.is_none());
    }

    #[tokio::test]
    async fn test_runtime_start_and_shutdown() {
        let persistence = Arc::new(MockPersistence);
        // Use port 0 to let OS assign an available port
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let config = CoreRuntimeBuilder::new()
            .persistence(persistence)
            .bind_addr(addr)
            .build()
            .unwrap();

        let runtime = config.start().await;
        // Start may fail in CI environments without network access
        if let Ok(runtime) = runtime {
            assert!(runtime.is_running());

            // bind_addr() returns the configured addr (port 0 if OS-assigned)
            // Just verify we can call it
            let _actual_addr = runtime.bind_addr();

            // Get persistence reference
            let _persistence = runtime.persistence();
            let _state = runtime.state();

            // Shutdown
            let result = runtime.shutdown().await;
            assert!(result.is_ok());
        }
    }
}
