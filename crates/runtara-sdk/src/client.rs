// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Main SDK client for instance communication with runtara-core.

use std::time::Duration;
use std::time::Instant;

use tracing::{debug, info, instrument};

use crate::backend::SdkBackend;
use crate::error::{Result, SdkError};
use crate::types::{CheckpointResult, Signal, SignalType, StatusResponse};

/// High-level SDK client for instance communication with runtara-core.
///
/// This client wraps a backend (HTTP or embedded) and provides ergonomic methods
/// for all instance lifecycle operations.
///
/// # Example (HTTP mode)
///
/// ```ignore
/// use runtara_sdk::RuntaraSdk;
///
/// let mut sdk = RuntaraSdk::from_env()?;
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
    /// Backend implementation (HTTP or embedded) - Arc for sharing with heartbeat task
    backend: std::sync::Arc<dyn SdkBackend>,
    /// Registration state
    registered: bool,
    /// Last signal poll time (for rate limiting)
    last_signal_poll: Instant,
    /// Cached pending signal (if any)
    pending_signal: Option<Signal>,
    /// Signal poll interval (ms)
    signal_poll_interval_ms: u64,
    /// Background heartbeat interval (ms). 0 = disabled.
    heartbeat_interval_ms: u64,
}

impl RuntaraSdk {
    // ========== HTTP Construction ==========

    /// Create an SDK instance using the HTTP backend.
    ///
    /// This connects to runtara-core's HTTP instance API.
    #[cfg(feature = "http")]
    pub fn new(config: crate::backend::http::HttpSdkConfig) -> Result<Self> {
        use crate::backend::http::HttpBackend;

        let signal_poll_interval_ms = config.signal_poll_interval_ms;
        let heartbeat_interval_ms = config.heartbeat_interval_ms;
        let backend = HttpBackend::new(&config)?;

        Ok(Self {
            backend: std::sync::Arc::new(backend),
            registered: false,
            last_signal_poll: Instant::now() - Duration::from_secs(60),
            pending_signal: None,
            signal_poll_interval_ms,
            heartbeat_interval_ms,
        })
    }

    /// Create an HTTP SDK instance from environment variables.
    ///
    /// Required: `RUNTARA_INSTANCE_ID`, `RUNTARA_TENANT_ID`
    /// Optional: `RUNTARA_HTTP_URL` (default: `http://127.0.0.1:8003`)
    #[cfg(feature = "http")]
    pub fn from_env() -> Result<Self> {
        let config = crate::backend::http::HttpSdkConfig::from_env()?;
        Self::new(config)
    }

    // ========== Embedded Construction ==========

    /// Create an embedded SDK instance with direct database access.
    ///
    /// This communicates directly with the persistence layer.
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
            last_signal_poll: Instant::now() - Duration::from_secs(60),
            pending_signal: None,
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
        instance_id: impl Into<String>,
        tenant_id: impl Into<String>,
        signal_poll_interval_ms: u64,
        heartbeat_interval_ms: u64,
    ) -> Self {
        use crate::backend::embedded::EmbeddedBackend;

        let backend = EmbeddedBackend::new(persistence, instance_id, tenant_id);

        Self {
            backend: std::sync::Arc::new(backend),
            registered: false,
            last_signal_poll: Instant::now() - Duration::from_secs(60),
            pending_signal: None,
            signal_poll_interval_ms,
            heartbeat_interval_ms,
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
    ///     RuntaraSdk::from_env()?
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
    #[instrument(skip(self, state), fields(instance_id = %self.backend.instance_id(), checkpoint_id = %checkpoint_id, state_size = state.len()))]
    pub async fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult> {
        self.backend.checkpoint(checkpoint_id, state).await
    }

    /// Get a checkpoint by ID without saving (read-only lookup).
    ///
    /// Returns the checkpoint state if found, or None if not found.
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
    #[instrument(skip(self, state), fields(instance_id = %self.backend.instance_id(), duration_ms = duration.as_millis() as u64))]
    pub async fn sleep(&self, duration: Duration, checkpoint_id: &str, state: &[u8]) -> Result<()> {
        self.backend
            .durable_sleep(duration, checkpoint_id, state)
            .await
    }

    // ========== Events ==========

    /// Send a heartbeat event (simple "I'm alive" signal).
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

    /// Send a suspended event (for pause signals).
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn suspended(&self) -> Result<()> {
        self.backend.suspended().await
    }

    /// Suspend with durable sleep - saves checkpoint and schedules wake.
    #[instrument(skip(self, state), fields(instance_id = %self.backend.instance_id(), checkpoint_id = %checkpoint_id))]
    pub async fn sleep_until(
        &self,
        checkpoint_id: &str,
        wake_at: chrono::DateTime<chrono::Utc>,
        state: &[u8],
    ) -> Result<()> {
        self.backend
            .sleep_until(checkpoint_id, wake_at, state)
            .await
    }

    /// Send a custom event with arbitrary subtype and payload.
    #[instrument(skip(self, payload), fields(instance_id = %self.backend.instance_id(), subtype = %subtype))]
    pub async fn custom_event(&self, subtype: &str, payload: Vec<u8>) -> Result<()> {
        self.backend.send_custom_event(subtype, payload).await
    }

    // ========== Signals ==========

    /// Poll for pending signals.
    ///
    /// Rate-limited to avoid hammering the server.
    /// Returns `Some(Signal)` if a signal is pending, `None` otherwise.
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
    pub async fn poll_signal_now(&mut self) -> Result<Option<Signal>> {
        self.last_signal_poll = Instant::now();

        let (signal, custom) = self.backend.poll_signals(None).await?;

        if let Some(sig) = signal {
            debug!(signal_type = ?sig.signal_type, "Signal received");
            return Ok(Some(sig));
        }

        if let Some(custom) = custom {
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

    /// Poll for a custom signal scoped to a specific checkpoint/signal ID.
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id(), signal_id = %signal_id))]
    pub async fn poll_custom_signal(&mut self, signal_id: &str) -> Result<Option<Vec<u8>>> {
        let (_signal, custom) = self.backend.poll_signals(Some(signal_id)).await?;

        if let Some(custom) = custom {
            debug!(signal_id = %signal_id, "Custom signal received");
            return Ok(Some(custom.payload));
        }
        Ok(None)
    }

    /// Acknowledge a received signal.
    #[instrument(skip(self), fields(instance_id = %self.backend.instance_id()))]
    pub async fn acknowledge_signal(
        &self,
        signal_type: SignalType,
    ) -> Result<()> {
        self.backend.acknowledge_signal(signal_type).await?;
        debug!("Signal acknowledged");
        Ok(())
    }

    /// Check for cancellation and return error if cancelled.
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
    pub async fn get_instance_status(&self, instance_id: &str) -> Result<StatusResponse> {
        self.backend.get_instance_status(instance_id).await
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
    pub(crate) fn backend_arc(&self) -> std::sync::Arc<dyn SdkBackend> {
        self.backend.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "http")]
    #[test]
    fn test_sdk_creation_http() {
        let config = crate::backend::http::HttpSdkConfig {
            instance_id: "test-instance".to_string(),
            tenant_id: "test-tenant".to_string(),
            base_url: "http://127.0.0.1:8003".to_string(),
            request_timeout_ms: 30_000,
            signal_poll_interval_ms: 1_000,
            heartbeat_interval_ms: 30_000,
        };

        let sdk = RuntaraSdk::new(config).unwrap();
        assert_eq!(sdk.instance_id(), "test-instance");
        assert_eq!(sdk.tenant_id(), "test-tenant");
        assert!(!sdk.is_registered());
    }

    #[cfg(feature = "http")]
    #[test]
    fn test_sdk_initial_state() {
        let config = crate::backend::http::HttpSdkConfig {
            instance_id: "test".to_string(),
            tenant_id: "test".to_string(),
            base_url: "http://127.0.0.1:8003".to_string(),
            request_timeout_ms: 30_000,
            signal_poll_interval_ms: 1_000,
            heartbeat_interval_ms: 30_000,
        };

        let sdk = RuntaraSdk::new(config).unwrap();
        assert!(!sdk.is_registered());
        assert!(sdk.pending_signal.is_none());
    }
}
