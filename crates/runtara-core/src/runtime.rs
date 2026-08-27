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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::instance_handlers::InstanceHandlerState;
use crate::persistence::Persistence;
use crate::server::InstanceServerState;

/// How long [`CoreRuntime::shutdown`] waits for in-flight requests to finish
/// before it stops waiting and aborts the server task.
///
/// Deliberately short. This wait is a backstop for the instance-protocol
/// requests still on the wire — checkpoints, events, signal acks — which take
/// milliseconds. In an embedded host it is a *second* phase, appended after
/// that host's own execution drain, so a generous value here eats into the
/// process-level shutdown budget (`stop_grace_period` in `docker-compose.yml`)
/// for no benefit.
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Builder for creating a [`CoreRuntime`].
pub struct CoreRuntimeBuilder {
    persistence: Option<Arc<dyn Persistence>>,
    bind_addr: SocketAddr,
    max_concurrent_instances: u32,
    shutdown_grace: Duration,
}

impl std::fmt::Debug for CoreRuntimeBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreRuntimeBuilder")
            .field("persistence", &self.persistence.as_ref().map(|_| "..."))
            .field("bind_addr", &self.bind_addr)
            .field("max_concurrent_instances", &self.max_concurrent_instances)
            .field("shutdown_grace", &self.shutdown_grace)
            .finish()
    }
}

impl Default for CoreRuntimeBuilder {
    fn default() -> Self {
        Self {
            persistence: None,
            bind_addr: "0.0.0.0:8001".parse().unwrap(),
            max_concurrent_instances: 0,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
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

    /// Set the bind address for the HTTP server.
    ///
    /// Default: `0.0.0.0:8001`
    pub fn bind_addr(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
        self
    }

    /// Enforce a ceiling on how many instances may be `running` at once.
    /// New-registration requests beyond the cap are rejected with
    /// `429 Too Many Requests`. Default (`0`) disables the check.
    ///
    /// Only `running` counts. A resume is exempt, and a `suspended` instance —
    /// parked in a durable sleep or waiting on a signal, possibly for days —
    /// holds no slot. What the cap bounds is concurrent execution, not the
    /// number of workflows in flight.
    pub fn max_concurrent_instances(mut self, limit: u32) -> Self {
        self.max_concurrent_instances = limit;
        self
    }

    /// How long [`CoreRuntime::shutdown`] waits for in-flight requests to
    /// finish before aborting the server task.
    ///
    /// Default: [`DEFAULT_SHUTDOWN_GRACE`]. Set this above the slowest request
    /// the deployment expects to serve; the wait ends as soon as the last
    /// request finishes, so a generous value costs nothing when nothing is in
    /// flight.
    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
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
            max_concurrent_instances: self.max_concurrent_instances,
            shutdown_grace: self.shutdown_grace,
        })
    }
}

/// Configuration for a [`CoreRuntime`].
pub struct CoreRuntimeConfig {
    persistence: Arc<dyn Persistence>,
    bind_addr: SocketAddr,
    max_concurrent_instances: u32,
    shutdown_grace: Duration,
}

impl std::fmt::Debug for CoreRuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreRuntimeConfig")
            .field("persistence", &"...")
            .field("bind_addr", &self.bind_addr)
            .field("max_concurrent_instances", &self.max_concurrent_instances)
            .field("shutdown_grace", &self.shutdown_grace)
            .finish()
    }
}

impl CoreRuntimeConfig {
    /// Start the runtime, spawning the HTTP server task.
    pub async fn start(self) -> Result<CoreRuntime> {
        let state = Arc::new(InstanceHandlerState::with_limits(
            self.persistence,
            self.max_concurrent_instances,
        ));
        let draining = state.draining_handle();

        let bind_addr = self.bind_addr;
        let server_state = state.clone();
        let shutdown_signal = Arc::new(Notify::new());
        let server_shutdown = Arc::clone(&shutdown_signal);
        let server_handle = tokio::spawn(async move {
            crate::server::http_server::run_http_server_with_shutdown(
                bind_addr,
                server_state,
                async move { server_shutdown.notified().await },
            )
            .await
        });

        info!(addr = %bind_addr, "CoreRuntime started");

        Ok(CoreRuntime {
            server_handle,
            state,
            bind_addr,
            draining,
            shutdown_signal,
            shutdown_grace: self.shutdown_grace,
        })
    }
}

/// A running runtara-core instance that can be embedded in an application.
///
/// The runtime manages:
/// - HTTP server for instance connections (checkpoints, signals, events)
///
/// Call [`shutdown`](Self::shutdown) for graceful termination.
pub struct CoreRuntime {
    server_handle: JoinHandle<anyhow::Result<()>>,
    state: Arc<InstanceServerState>,
    bind_addr: SocketAddr,
    draining: Arc<AtomicBool>,
    shutdown_signal: Arc<Notify>,
    shutdown_grace: Duration,
}

impl CoreRuntime {
    /// Create a new builder for configuring the runtime.
    pub fn builder() -> CoreRuntimeBuilder {
        CoreRuntimeBuilder::new()
    }

    /// Get the bind address of the HTTP server.
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

    /// Mark the runtime as draining.
    ///
    /// New-registration requests will be refused with `503 Service Unavailable`
    /// after this is set. In-flight operations (checkpoint, event, signal ack)
    /// keep working so running instances can reach a checkpoint and suspend.
    ///
    /// This does NOT stop the HTTP server; call [`shutdown`](Self::shutdown)
    /// after the drain grace period to do that.
    pub fn set_draining(&self) {
        self.draining.store(true, Ordering::SeqCst);
        info!("CoreRuntime draining: refusing new registrations");
    }

    /// Handle to the draining flag so external coordinators can flip it.
    pub fn draining_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.draining)
    }

    /// Gracefully shut down the runtime.
    ///
    /// Stops accepting new connections and waits for the requests already in
    /// flight to finish, so an instance mid-checkpoint gets to finish writing
    /// instead of being severed. The wait is bounded by the grace period from
    /// [`CoreRuntimeBuilder::shutdown_grace`].
    ///
    /// When that grace expires the server task is aborted, which ends *this
    /// wait* — not the requests. axum serves each connection on its own
    /// spawned task, so a straggler keeps running detached; what actually
    /// severs it is the process exiting afterwards. A caller that tears down
    /// shared resources (a connection pool, telemetry) right after this
    /// returns should treat a grace expiry as "handlers may still be running".
    ///
    /// The grace only covers requests already accepted. An instance that has
    /// not yet issued its final checkpoint POST is not protected by it, so
    /// pair this with [`set_draining`](Self::set_draining): flip the drain flag
    /// first so no new instances register, give running ones time to reach a
    /// checkpoint, and only then call this.
    pub async fn shutdown(mut self) -> Result<()> {
        info!(
            grace_ms = self.shutdown_grace.as_millis() as u64,
            "CoreRuntime shutting down..."
        );

        // Ask the HTTP server to stop accepting and drain what it is serving.
        // `notify_one` rather than `notify_waiters` so the request is not lost
        // if the server task has not polled the shutdown future yet.
        self.shutdown_signal.notify_one();

        let drained = tokio::time::timeout(self.shutdown_grace, &mut self.server_handle).await;

        let outcome = match drained {
            Ok(joined) => joined,
            Err(_) => {
                warn!(
                    grace_ms = self.shutdown_grace.as_millis() as u64,
                    "CoreRuntime shutdown grace expired with requests still in flight; aborting"
                );
                self.server_handle.abort();
                (&mut self.server_handle).await
            }
        };

        match outcome {
            Ok(Ok(())) => {
                info!("CoreRuntime shutdown complete");
                Ok(())
            }
            Ok(Err(e)) => {
                error!("CoreRuntime server error during shutdown: {}", e);
                Err(e)
            }
            Err(e) if e.is_cancelled() => {
                info!("CoreRuntime shutdown complete");
                Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreError;
    use crate::persistence::{
        CheckpointRecord, CompleteInstanceParams, CustomSignalRecord, EventRecord, InstanceRecord,
        ListEventsFilter, ListStepSummariesFilter, Persistence, SignalRecord, StepSummaryRecord,
    };
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::time::Instant;

    /// Mock persistence for testing the runtime without a database.
    struct MockPersistence {
        /// How long `health_check_db` blocks, so a test can hold a request in
        /// flight across a shutdown.
        health_delay: Duration,
        /// Fired on entry to `health_check_db`, so a test can establish that the
        /// request is inside the handler before it signals shutdown. Inferring
        /// that from elapsed time instead is unreliable: the request can finish
        /// first, and the test then passes against a runtime that never drained
        /// anything.
        health_entered: Arc<Notify>,
    }

    impl MockPersistence {
        fn new() -> Self {
            Self {
                health_delay: Duration::ZERO,
                health_entered: Arc::new(Notify::new()),
            }
        }

        fn slow_health(health_delay: Duration, health_entered: Arc<Notify>) -> Self {
            Self {
                health_delay,
                health_entered,
            }
        }
    }

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
            _params: CompleteInstanceParams<'_>,
        ) -> Result<bool, CoreError> {
            Ok(true)
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
            self.health_entered.notify_one();
            if !self.health_delay.is_zero() {
                tokio::time::sleep(self.health_delay).await;
            }
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

        async fn list_step_summaries(
            &self,
            _instance_id: &str,
            _filter: &ListStepSummariesFilter,
            _limit: i64,
            _offset: i64,
        ) -> Result<Vec<StepSummaryRecord>, CoreError> {
            Ok(Vec::new())
        }

        async fn count_step_summaries(
            &self,
            _instance_id: &str,
            _filter: &ListStepSummariesFilter,
        ) -> Result<i64, CoreError> {
            Ok(0)
        }
    }

    /// Claim a port by binding and releasing it. `start()` binds asynchronously
    /// inside the spawned server task, so callers pair this with
    /// [`wait_until_listening`].
    async fn free_port() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        addr
    }

    async fn wait_until_listening(addr: SocketAddr) {
        for _ in 0..100 {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("server never started listening on {addr}");
    }

    /// `GET /health`, read until the server closes the connection. Returns
    /// whatever arrived — empty or partial if the connection was severed.
    async fn get_health(addr: SocketAddr) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[tokio::test]
    async fn shutdown_lets_an_in_flight_request_finish() {
        let entered = Arc::new(Notify::new());
        let addr = free_port().await;
        let runtime = CoreRuntime::builder()
            .persistence(Arc::new(MockPersistence::slow_health(
                Duration::from_millis(800),
                Arc::clone(&entered),
            )))
            .bind_addr(addr)
            .shutdown_grace(Duration::from_secs(10))
            .build()
            .unwrap()
            .start()
            .await
            .unwrap();

        wait_until_listening(addr).await;
        let client = tokio::spawn(get_health(addr));
        entered.notified().await;

        let started = Instant::now();
        runtime.shutdown().await.unwrap();
        let waited = started.elapsed();

        let response = client.await.unwrap();
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "in-flight request was severed by shutdown: {response:?}"
        );
        assert!(
            waited >= Duration::from_millis(500),
            "shutdown returned before the in-flight request finished: {waited:?}"
        );
    }

    #[tokio::test]
    async fn shutdown_stops_waiting_once_the_grace_expires() {
        let entered = Arc::new(Notify::new());
        let addr = free_port().await;
        let runtime = CoreRuntime::builder()
            .persistence(Arc::new(MockPersistence::slow_health(
                Duration::from_secs(3),
                Arc::clone(&entered),
            )))
            .bind_addr(addr)
            .shutdown_grace(Duration::from_millis(300))
            .build()
            .unwrap()
            .start()
            .await
            .unwrap();

        wait_until_listening(addr).await;
        let client = tokio::spawn(get_health(addr));
        entered.notified().await;

        let started = Instant::now();
        runtime.shutdown().await.unwrap();
        let waited = started.elapsed();

        assert!(
            waited >= Duration::from_millis(300),
            "shutdown gave up before its grace elapsed: {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(2),
            "shutdown waited past its grace for a request that needed 3s: {waited:?}"
        );
        // axum spawns a task per connection (`handle_connection` in its `serve`
        // module), so aborting the serve task ends the *wait*, not the request
        // itself — what severs the straggler is the process exiting afterwards.
        // The property under test is that shutdown is bounded, so assert the
        // request really was still running when it returned.
        assert!(
            !client.is_finished(),
            "the request finished on its own; the grace never actually expired"
        );
        client.abort();
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
        let persistence = Arc::new(MockPersistence::new());
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
        let persistence = Arc::new(MockPersistence::new());
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
        let persistence = Arc::new(MockPersistence::new());
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
        let persistence = Arc::new(MockPersistence::new());
        let result = CoreRuntimeBuilder::new().persistence(persistence).build();
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.bind_addr.port(), 8001);
    }

    #[test]
    fn test_builder_build_with_custom_addr() {
        let persistence = Arc::new(MockPersistence::new());
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
        let persistence = Arc::new(MockPersistence::new());
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
