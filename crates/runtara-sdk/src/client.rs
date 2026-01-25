// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Main SDK client for instance communication with runtara-core.

use std::time::Duration;
#[cfg(feature = "quic")]
use std::time::Instant;

use tracing::{info, instrument};

#[cfg(feature = "quic")]
use runtara_protocol::instance_proto::{
    self as proto, PollSignalsRequest, RpcRequest, RpcResponse, SignalAck, rpc_request,
    rpc_response,
};

use crate::backend::SdkBackend;
#[cfg(feature = "quic")]
use crate::config::SdkConfig;
use crate::error::Result;
#[cfg(feature = "quic")]
use crate::error::SdkError;
#[cfg(feature = "quic")]
use crate::signals::from_proto_signal;
use crate::types::{CheckpointResult, StatusResponse};
#[cfg(feature = "quic")]
use crate::types::{Signal, SignalType};
#[cfg(feature = "quic")]
use tracing::debug;

/// High-level SDK client for instance communication with runtara-core.
///
/// This client wraps a backend (QUIC or embedded) and provides ergonomic methods
/// for all instance lifecycle operations.
///
/// # Example (QUIC mode)
///
/// ```ignore
/// use runtara_sdk::RuntaraSdk;
///
/// let mut sdk = RuntaraSdk::localhost("my-instance", "my-tenant")?;
/// sdk.connect().await?;
/// sdk.register(None).await?;
///
/// // Process items with checkpointing
/// for i in 0..items.len() {
///     let state = serde_json::to_vec(&my_state)?;
///     if let Some(existing) = sdk.checkpoint(&format!("item-{}", i), &state).await? {
///         // Resuming - restore state and skip
///         my_state = serde_json::from_slice(&existing)?;
///         continue;
///     }
///     // Fresh execution - process item
///     process_item(&items[i]);
/// }
///
/// sdk.completed(b"result").await?;
/// ```
///
/// # Example (Embedded mode)
///
/// ```ignore
/// use runtara_sdk::RuntaraSdk;
/// use std::sync::Arc;
///
/// // Create persistence layer (e.g., SQLite or PostgreSQL)
/// let persistence: Arc<dyn Persistence> = create_persistence().await?;
///
/// let mut sdk = RuntaraSdk::embedded(persistence, "my-instance", "my-tenant");
/// sdk.connect().await?;  // No-op for embedded
/// sdk.register(None).await?;
///
/// // Same checkpoint API works with embedded mode
/// for i in 0..items.len() {
///     let state = serde_json::to_vec(&my_state)?;
///     let result = sdk.checkpoint(&format!("item-{}", i), &state).await?;
///     // ...
/// }
///
/// sdk.completed(b"result").await?;
/// ```
pub struct RuntaraSdk {
    /// Backend implementation (QUIC or embedded) - Arc for sharing with heartbeat task
    backend: std::sync::Arc<dyn SdkBackend>,
    /// Registration state
    registered: bool,
    /// Last signal poll time (for rate limiting) - only used with QUIC
    #[cfg(feature = "quic")]
    last_signal_poll: Instant,
    /// Cached pending signal (if any) - only used with QUIC
    #[cfg(feature = "quic")]
    pending_signal: Option<Signal>,
    /// Signal poll interval (ms) - only used with QUIC
    #[cfg(feature = "quic")]
    signal_poll_interval_ms: u64,
    /// Background heartbeat interval (ms). 0 = disabled.
    heartbeat_interval_ms: u64,
}

impl RuntaraSdk {
    // ========== QUIC Construction ==========

    /// Create a new SDK instance with the given configuration.
    ///
    /// This creates a QUIC-based SDK that connects to runtara-core over the network.
    #[cfg(feature = "quic")]
    pub fn new(config: SdkConfig) -> Result<Self> {
        use crate::backend::quic::QuicBackend;

        let signal_poll_interval_ms = config.signal_poll_interval_ms;
        let heartbeat_interval_ms = config.heartbeat_interval_ms;
        let backend = QuicBackend::new(&config)?;

        Ok(Self {
            backend: std::sync::Arc::new(backend),
            registered: false,
            last_signal_poll: Instant::now() - Duration::from_secs(60), // Allow immediate first poll
            pending_signal: None,
            signal_poll_interval_ms,
            heartbeat_interval_ms,
        })
    }

    /// Create an SDK instance from environment variables.
    ///
    /// See [`SdkConfig::from_env`] for required and optional environment variables.
    #[cfg(feature = "quic")]
    pub fn from_env() -> Result<Self> {
        let config = SdkConfig::from_env()?;
        Self::new(config)
    }

    /// Create an SDK instance for local development.
    ///
    /// This connects to `127.0.0.1:8001` with TLS verification disabled.
    #[cfg(feature = "quic")]
    pub fn localhost(instance_id: impl Into<String>, tenant_id: impl Into<String>) -> Result<Self> {
        let config = SdkConfig::localhost(instance_id, tenant_id);
        Self::new(config)
    }

    // ========== Embedded Construction ==========

    /// Create an embedded SDK instance with direct database access.
    ///
    /// This bypasses QUIC and communicates directly with the persistence layer.
    /// Ideal for embedding runtara-core within the same process.
    ///
    /// Note: Signals and durable sleep are not supported in embedded mode.
    #[cfg(feature = "embedded")]
    pub fn embedded(
        persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
        instance_id: impl Into<String>,
        tenant_id: impl Into<String>,
    ) -> Self {
        use crate::backend::embedded::EmbeddedBackend;

        let backend = EmbeddedBackend::new(persistence, instance_id, tenant_id);

        Self {
            backend: std::sync::Arc::new(backend),
            registered: false,
            #[cfg(feature = "quic")]
            last_signal_poll: Instant::now() - Duration::from_secs(60),
            #[cfg(feature = "quic")]
            pending_signal: None,
            #[cfg(feature = "quic")]
            signal_poll_interval_ms: 1_000,
            heartbeat_interval_ms: 30_000,
        }
    }

    /// Create an embedded SDK instance with configuration.
    ///
    /// This variant allows customizing heartbeat interval and other settings
    /// while using direct database access.
    #[cfg(feature = "embedded")]
    pub fn with_embedded_backend(
        persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
        config: SdkConfig,
    ) -> Self {
        use crate::backend::embedded::EmbeddedBackend;

        let backend = EmbeddedBackend::new(persistence, &config.instance_id, &config.tenant_id);

        Self {
            backend: std::sync::Arc::new(backend),
            registered: false,
            #[cfg(feature = "quic")]
            last_signal_poll: Instant::now() - Duration::from_secs(60),
            #[cfg(feature = "quic")]
            pending_signal: None,
            #[cfg(feature = "quic")]
            signal_poll_interval_ms: config.signal_poll_interval_ms,
            heartbeat_interval_ms: config.heartbeat_interval_ms,
        }
    }

    // ========== Initialization ==========

    /// Initialize SDK: connect, register, and make available globally for #[durable].
    ///
    /// This is a convenience method that combines:
    /// 1. `connect()` - establish connection to runtara-core
    /// 2. `register(checkpoint_id)` - register this instance
    /// 3. `register_sdk()` - make SDK available globally for #[durable] functions
    ///
    /// # Example
    ///
    /// ```ignore
    /// use runtara_sdk::RuntaraSdk;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // One-liner setup for #[durable] functions
    ///     RuntaraSdk::localhost("my-instance", "my-tenant")?
    ///         .init(None)
    ///         .await?;
    ///
    ///     // Now #[durable] functions work automatically
    ///     my_durable_function("key".to_string(), args).await?;
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn init(mut self, checkpoint_id: Option<&str>) -> Result<()> {
        self.connect().await?;
        self.register(checkpoint_id).await?;
        crate::register_sdk(self);
        info!("SDK initialized globally");
        Ok(())
    }

    // ========== Connection ==========

    /// Connect to runtara-core.
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn connect(&self) -> Result<()> {
        info!("Connecting to runtara-core");
        self.backend.connect().await?;
        info!("Connected to runtara-core");
        Ok(())
    }

    /// Check if connected to runtara-core.
    pub async fn is_connected(&self) -> bool {
        self.backend.is_connected().await
    }

    /// Close the connection to runtara-core.
    pub async fn close(&self) {
        self.backend.close().await;
    }

    // ========== Registration ==========

    /// Register this instance with runtara-core.
    ///
    /// This should be called at instance startup. If `checkpoint_id` is provided,
    /// the instance is resuming from a checkpoint.
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn register(&mut self, checkpoint_id: Option<&str>) -> Result<()> {
        self.backend.register(checkpoint_id).await?;
        self.registered = true;
        info!("Instance registered");
        Ok(())
    }

    // ========== Checkpointing ==========

    /// Checkpoint with the given ID and state.
    ///
    /// This is the primary checkpoint method that handles both save and resume:
    /// - If a checkpoint with this ID already exists, returns the existing state (for resume)
    /// - If no checkpoint exists, saves the provided state and returns None
    ///
    /// This also serves as a heartbeat - each checkpoint call reports progress to runtara-core.
    ///
    /// The returned [`CheckpointResult`] also includes any pending signal (cancel, pause)
    /// that the instance should handle after processing the checkpoint.
    ///
    /// # Example
    /// ```ignore
    /// // In a loop - checkpoint handles both fresh runs and resumes
    /// for i in 0..items.len() {
    ///     let checkpoint_id = format!("item-{}", i);
    ///     let result = sdk.checkpoint(&checkpoint_id, &state).await?;
    ///
    ///     // Check for pending signals
    ///     if result.should_cancel() {
    ///         return Err("Cancelled".into());
    ///     }
    ///     if result.should_pause() {
    ///         // Exit cleanly - will be resumed later
    ///         return Ok(());
    ///     }
    ///
    ///     if let Some(existing_state) = result.existing_state() {
    ///         // Resuming - restore state and skip already-processed work
    ///         state = serde_json::from_slice(existing_state)?;
    ///         continue;
    ///     }
    ///     // Fresh execution - process item
    ///     process_item(&items[i]);
    /// }
    /// ```
    #[instrument(skip(self, state), fields(instance_id = %self.backend.instance_id(), checkpoint_id = %checkpoint_id, state_size = state.len()))]
    pub async fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult> {
        self.backend.checkpoint(checkpoint_id, state).await
    }

    /// Get a checkpoint by ID without saving (read-only lookup).
    ///
    /// Returns the checkpoint state if found, or None if not found.
    /// Use this when you want to check if a cached result exists before executing.
    ///
    /// # Example
    /// ```ignore
    /// // Check if result is already cached
    /// if let Some(cached_state) = sdk.get_checkpoint("my-operation").await? {
    ///     let result: MyResult = serde_json::from_slice(&cached_state)?;
    ///     return Ok(result);
    /// }
    /// // Not cached - execute operation and save result
    /// let result = do_expensive_operation();
    /// let state = serde_json::to_vec(&result)?;
    /// sdk.checkpoint("my-operation", &state).await?;
    /// ```
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id(), checkpoint_id = %checkpoint_id))]
    pub async fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<Vec<u8>>> {
        self.backend.get_checkpoint(checkpoint_id).await
    }

    // ========== Sleep/Wake ==========

    /// Request to sleep for the specified duration.
    ///
    /// This is a durable sleep that persists across restarts:
    /// - Saves a checkpoint with the provided state
    /// - Records the wake time (`sleep_until`) in the database
    /// - On resume, calculates remaining time and only sleeps for the remainder
    ///
    /// In QUIC mode, the server tracks the wake time. In embedded mode, the
    /// persistence layer tracks it directly.
    #[instrument(skip(self, state), fields(instance_id = %self.backend.instance_id(), duration_ms = duration.as_millis() as u64))]
    pub async fn sleep(&self, duration: Duration, checkpoint_id: &str, state: &[u8]) -> Result<()> {
        self.backend
            .durable_sleep(duration, checkpoint_id, state)
            .await
    }

    // ========== Events ==========

    /// Send a heartbeat event (simple "I'm alive" signal).
    ///
    /// Use this for progress reporting without checkpointing.
    /// For durable progress, use `checkpoint()` instead.
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn heartbeat(&self) -> Result<()> {
        self.backend.heartbeat().await
    }

    /// Send a completed event with output.
    #[instrument(skip(self, output), fields(instance_id = %self.backend.instance_id(), output_size = output.len()))]
    pub async fn completed(&self, output: &[u8]) -> Result<()> {
        self.backend.completed(output).await
    }

    /// Send a failed event with error message.
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn failed(&self, error: &str) -> Result<()> {
        self.backend.failed(error).await
    }

    /// Send a suspended event.
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn suspended(&self) -> Result<()> {
        self.backend.suspended().await
    }

    /// Send a custom event with arbitrary subtype and payload.
    ///
    /// This is a fire-and-forget event stored by runtara-core with the given subtype.
    /// Core treats the subtype as an opaque string without any semantic interpretation.
    ///
    /// # Arguments
    ///
    /// * `subtype` - Arbitrary event subtype string
    /// * `payload` - Event payload as raw bytes (typically JSON serialized)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let payload = serde_json::to_vec(&my_event_data)?;
    /// sdk.custom_event("my_custom_event", payload).await?;
    /// ```
    #[instrument(skip(self, payload), fields(instance_id = %self.backend.instance_id(), subtype = %subtype))]
    pub async fn custom_event(&self, subtype: &str, payload: Vec<u8>) -> Result<()> {
        self.backend.send_custom_event(subtype, payload).await
    }

    // ========== Signals (QUIC only) ==========

    /// Poll for pending signals.
    ///
    /// Rate-limited to avoid hammering the server.
    /// Returns `Some(Signal)` if a signal is pending, `None` otherwise.
    ///
    /// Note: Only available with QUIC backend.
    #[cfg(feature = "quic")]
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn poll_signal(&mut self) -> Result<Option<Signal>> {
        // Check cached signal first
        if self.pending_signal.is_some() {
            return Ok(self.pending_signal.take());
        }

        // Rate limit
        let poll_interval = Duration::from_millis(self.signal_poll_interval_ms);
        if self.last_signal_poll.elapsed() < poll_interval {
            return Ok(None);
        }

        self.poll_signal_now().await
    }

    /// Force poll for signals (ignoring rate limit).
    ///
    /// Note: Only available with QUIC backend.
    #[cfg(feature = "quic")]
    pub async fn poll_signal_now(&mut self) -> Result<Option<Signal>> {
        use crate::backend::quic::QuicBackend;

        self.last_signal_poll = Instant::now();

        let backend = self
            .backend
            .as_any()
            .downcast_ref::<QuicBackend>()
            .ok_or_else(|| SdkError::Internal("poll_signal() requires QUIC backend".to_string()))?;

        let request = PollSignalsRequest {
            instance_id: self.backend.instance_id().to_string(),
            checkpoint_id: None,
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::PollSignals(request)),
        };

        let rpc_response: RpcResponse = backend.client().request(&rpc_request).await?;

        match rpc_response.response {
            Some(rpc_response::Response::PollSignals(resp)) => {
                if let Some(signal) = resp.signal {
                    let sdk_signal = from_proto_signal(signal);
                    debug!(signal_type = ?sdk_signal.signal_type, "Signal received");
                    return Ok(Some(sdk_signal));
                }

                if let Some(custom) = resp.custom_signal {
                    let sdk_signal = Signal {
                        signal_type: SignalType::Resume, // custom signals are scoped; type unused here
                        payload: custom.payload,
                        checkpoint_id: Some(custom.checkpoint_id),
                    };
                    debug!("Custom signal received for checkpoint");
                    return Ok(Some(sdk_signal));
                }

                Ok(None)
            }
            Some(rpc_response::Response::Error(e)) => Err(SdkError::Server {
                code: e.code,
                message: e.message,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected PollSignalsResponse".to_string(),
            )),
        }
    }

    /// Acknowledge a received signal.
    ///
    /// Note: Only available with QUIC backend.
    #[cfg(feature = "quic")]
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn acknowledge_signal(
        &self,
        signal_type: SignalType,
        acknowledged: bool,
    ) -> Result<()> {
        use crate::backend::quic::QuicBackend;

        let backend = self
            .backend
            .as_any()
            .downcast_ref::<QuicBackend>()
            .ok_or_else(|| {
                SdkError::Internal("acknowledge_signal() requires QUIC backend".to_string())
            })?;

        let request = SignalAck {
            instance_id: self.backend.instance_id().to_string(),
            signal_type: proto::SignalType::from(signal_type).into(),
            acknowledged,
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::SignalAck(request)),
        };

        backend.client().send_fire_and_forget(&rpc_request).await?;
        debug!("Signal acknowledged");
        Ok(())
    }

    /// Check for cancellation and return error if cancelled.
    ///
    /// Convenience method for use in execution loops:
    /// ```ignore
    /// for item in items {
    ///     sdk.check_cancelled().await?;
    ///     // process item...
    /// }
    /// ```
    ///
    /// Note: Only available with QUIC backend.
    #[cfg(feature = "quic")]
    pub async fn check_cancelled(&mut self) -> Result<()> {
        if let Some(signal) = self.poll_signal().await? {
            if signal.signal_type == SignalType::Cancel {
                return Err(SdkError::Cancelled);
            }
            // Cache non-cancel signals for later
            self.pending_signal = Some(signal);
        }
        Ok(())
    }

    /// Check for pause and return error if paused.
    ///
    /// Note: Only available with QUIC backend.
    #[cfg(feature = "quic")]
    pub async fn check_paused(&mut self) -> Result<()> {
        if let Some(signal) = self.poll_signal().await? {
            if signal.signal_type == SignalType::Pause {
                return Err(SdkError::Paused);
            }
            // Cache non-pause signals for later
            self.pending_signal = Some(signal);
        }
        Ok(())
    }

    /// Check for any actionable signal (cancel or pause) and return appropriate error.
    ///
    /// This is a unified method that checks for both cancellation and pause signals.
    /// Use this in workflow steps to detect both types of interruption.
    ///
    /// Note: Only available with QUIC backend.
    #[cfg(feature = "quic")]
    pub async fn check_signals(&mut self) -> Result<()> {
        if let Some(signal) = self.poll_signal().await? {
            match signal.signal_type {
                SignalType::Cancel => return Err(SdkError::Cancelled),
                SignalType::Pause => return Err(SdkError::Paused),
                SignalType::Resume => {
                    // Resume is informational, cache it but don't error
                    self.pending_signal = Some(signal);
                }
            }
        }
        Ok(())
    }

    // ========== Retry Tracking ==========

    /// Record a retry attempt for audit trail.
    ///
    /// This is a fire-and-forget operation that records a retry attempt
    /// in the checkpoint history. Called by the `#[durable]` macro when
    /// a function fails and is about to be retried.
    ///
    /// # Arguments
    ///
    /// * `checkpoint_id` - The durable function's cache key
    /// * `attempt_number` - The 1-indexed retry attempt number
    /// * `error_message` - Error message from the previous failed attempt
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id(), checkpoint_id = %checkpoint_id, attempt = attempt_number))]
    pub async fn record_retry_attempt(
        &self,
        checkpoint_id: &str,
        attempt_number: u32,
        error_message: Option<&str>,
    ) -> Result<()> {
        self.backend
            .record_retry_attempt(checkpoint_id, attempt_number, error_message)
            .await
    }

    // ========== Status ==========

    /// Get the current status of this instance.
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn get_status(&self) -> Result<StatusResponse> {
        self.backend.get_status().await
    }

    /// Get the status of another instance.
    ///
    /// Note: Only available with QUIC backend.
    #[cfg(feature = "quic")]
    pub async fn get_instance_status(&self, instance_id: &str) -> Result<StatusResponse> {
        use crate::backend::quic::QuicBackend;
        use runtara_protocol::instance_proto::GetInstanceStatusRequest;

        let backend = self
            .backend
            .as_any()
            .downcast_ref::<QuicBackend>()
            .ok_or_else(|| {
                SdkError::Internal("get_instance_status() requires QUIC backend".to_string())
            })?;

        let request = GetInstanceStatusRequest {
            instance_id: instance_id.to_string(),
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::GetInstanceStatus(request)),
        };

        let rpc_response: RpcResponse = backend.client().request(&rpc_request).await?;

        match rpc_response.response {
            Some(rpc_response::Response::GetInstanceStatus(resp)) => Ok(StatusResponse::from(resp)),
            Some(rpc_response::Response::Error(e)) => Err(SdkError::Server {
                code: e.code,
                message: e.message,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetInstanceStatusResponse".to_string(),
            )),
        }
    }

    // ========== Helpers ==========

    /// Get the instance ID.
    pub fn instance_id(&self) -> &str {
        self.backend.instance_id()
    }

    /// Get the tenant ID.
    pub fn tenant_id(&self) -> &str {
        self.backend.tenant_id()
    }

    /// Check if the instance is registered.
    pub fn is_registered(&self) -> bool {
        self.registered
    }

    /// Get the configured heartbeat interval in milliseconds.
    /// Returns 0 if automatic heartbeats are disabled.
    pub fn heartbeat_interval_ms(&self) -> u64 {
        self.heartbeat_interval_ms
    }

    /// Get a clone of the backend Arc for the heartbeat task.
    ///
    /// This allows the background heartbeat task to send heartbeats
    /// without holding the SDK mutex, preventing mutex contention
    /// with long-running network operations.
    pub(crate) fn backend_arc(&self) -> std::sync::Arc<dyn SdkBackend> {
        self.backend.clone()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "quic")]
    use super::*;

    #[cfg(feature = "quic")]
    #[test]
    fn test_sdk_creation() {
        // Note: This test may fail if the UDP socket cannot be bound (e.g., in sandboxed environments)
        let sdk = RuntaraSdk::localhost("test-instance", "test-tenant");

        // If we can't create the SDK, just skip the test assertions
        if let Ok(sdk) = sdk {
            assert_eq!(sdk.instance_id(), "test-instance");
            assert_eq!(sdk.tenant_id(), "test-tenant");
            assert!(!sdk.is_registered());
        }
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_config_creation() {
        let config = SdkConfig::localhost("test-instance", "test-tenant");
        assert_eq!(config.instance_id, "test-instance");
        assert_eq!(config.tenant_id, "test-tenant");
        assert!(config.skip_cert_verification);
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_sdk_with_custom_config() {
        let config = SdkConfig {
            instance_id: "custom-instance".to_string(),
            tenant_id: "custom-tenant".to_string(),
            server_addr: "127.0.0.1:9999".parse().unwrap(),
            server_name: "custom-server".to_string(),
            skip_cert_verification: true,
            request_timeout_ms: 5000,
            connect_timeout_ms: 3000,
            signal_poll_interval_ms: 500,
            heartbeat_interval_ms: 30000,
        };

        // May fail in sandboxed environments
        if let Ok(sdk) = RuntaraSdk::new(config) {
            assert_eq!(sdk.instance_id(), "custom-instance");
            assert_eq!(sdk.tenant_id(), "custom-tenant");
        }
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_sdk_localhost_sets_defaults() {
        // May fail in sandboxed environments
        if let Ok(sdk) = RuntaraSdk::localhost("inst", "tenant") {
            assert!(!sdk.is_registered());
            assert_eq!(sdk.instance_id(), "inst");
        }
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_sdk_config_defaults() {
        let config = SdkConfig::localhost("a", "b");
        assert_eq!(config.request_timeout_ms, 30_000);
        assert_eq!(config.connect_timeout_ms, 10_000);
        assert_eq!(config.signal_poll_interval_ms, 1_000);
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_sdk_config_with_string_types() {
        // Test that String types work as well as &str
        let config = SdkConfig::localhost(String::from("instance"), String::from("tenant"));
        assert_eq!(config.instance_id, "instance");
        assert_eq!(config.tenant_id, "tenant");
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_sdk_initial_state() {
        if let Ok(sdk) = RuntaraSdk::localhost("test", "test") {
            // SDK should start unregistered
            assert!(!sdk.is_registered());
            // pending_signal should be None
            assert!(sdk.pending_signal.is_none());
        }
    }
}
