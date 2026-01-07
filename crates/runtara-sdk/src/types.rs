// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! High-level types for the SDK.

#[cfg(feature = "quic")]
use runtara_protocol::instance_proto as proto;

/// Instance status as returned by status queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    /// Status is unknown
    Unknown,
    /// Instance is queued but not yet started
    Pending,
    /// Instance is currently executing
    Running,
    /// Instance is sleeping/waiting for wake
    Suspended,
    /// Instance finished successfully
    Completed,
    /// Instance finished with an error
    Failed,
    /// Instance was cancelled by signal
    Cancelled,
}

#[cfg(feature = "quic")]
impl From<proto::InstanceStatus> for InstanceStatus {
    fn from(status: proto::InstanceStatus) -> Self {
        match status {
            proto::InstanceStatus::StatusUnknown => InstanceStatus::Unknown,
            proto::InstanceStatus::StatusPending => InstanceStatus::Pending,
            proto::InstanceStatus::StatusRunning => InstanceStatus::Running,
            proto::InstanceStatus::StatusSuspended => InstanceStatus::Suspended,
            proto::InstanceStatus::StatusCompleted => InstanceStatus::Completed,
            proto::InstanceStatus::StatusFailed => InstanceStatus::Failed,
            proto::InstanceStatus::StatusCancelled => InstanceStatus::Cancelled,
        }
    }
}

#[cfg(feature = "quic")]
impl From<i32> for InstanceStatus {
    fn from(value: i32) -> Self {
        proto::InstanceStatus::try_from(value)
            .map(InstanceStatus::from)
            .unwrap_or(InstanceStatus::Unknown)
    }
}

/// Signal types that can be received from runtara-core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalType {
    /// Cancel execution
    Cancel,
    /// Pause execution (checkpoint and wait)
    Pause,
    /// Resume paused execution
    Resume,
}

#[cfg(feature = "quic")]
impl From<proto::SignalType> for SignalType {
    fn from(signal: proto::SignalType) -> Self {
        match signal {
            proto::SignalType::SignalCancel => SignalType::Cancel,
            proto::SignalType::SignalPause => SignalType::Pause,
            proto::SignalType::SignalResume => SignalType::Resume,
        }
    }
}

#[cfg(feature = "quic")]
impl From<i32> for SignalType {
    fn from(value: i32) -> Self {
        proto::SignalType::try_from(value)
            .map(SignalType::from)
            .unwrap_or(SignalType::Cancel)
    }
}

#[cfg(feature = "quic")]
impl From<SignalType> for proto::SignalType {
    fn from(signal: SignalType) -> Self {
        match signal {
            SignalType::Cancel => proto::SignalType::SignalCancel,
            SignalType::Pause => proto::SignalType::SignalPause,
            SignalType::Resume => proto::SignalType::SignalResume,
        }
    }
}

/// A signal received from runtara-core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    /// The type of signal
    pub signal_type: SignalType,
    /// Signal-specific payload data
    pub payload: Vec<u8>,
    /// Optional checkpoint_id when representing a custom signal
    pub checkpoint_id: Option<String>,
}

/// Checkpoint response with signal information.
///
/// The checkpoint API now returns pending signal information along with the
/// checkpoint state. This allows instances to efficiently check for cancel/pause
/// signals during checkpoint operations without additional RPC calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointResult {
    /// If true, an existing checkpoint was found and state is returned.
    /// If false, a new checkpoint was saved.
    pub found: bool,
    /// Checkpoint state (existing state if found, empty if saved).
    pub state: Vec<u8>,
    /// Pending instance-wide signal if any (cancel, pause, resume).
    /// Instance should handle this signal after processing the checkpoint.
    pub pending_signal: Option<Signal>,
    /// Pending checkpoint-scoped custom signal (if waiting on a specific checkpoint_id).
    pub custom_signal: Option<CustomSignal>,
}

/// Custom signal targeted to a specific checkpoint/wait key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomSignal {
    /// Target checkpoint/wait key
    pub checkpoint_id: String,
    /// Signal payload
    pub payload: Vec<u8>,
}

impl CheckpointResult {
    /// Returns Some(state) if an existing checkpoint was found (for resume).
    /// Returns None if a new checkpoint was saved.
    pub fn existing_state(&self) -> Option<&[u8]> {
        if self.found { Some(&self.state) } else { None }
    }

    /// Check if the instance should pause.
    pub fn should_pause(&self) -> bool {
        matches!(
            self.pending_signal.as_ref().map(|s| s.signal_type),
            Some(SignalType::Pause)
        )
    }

    /// Check if the instance should cancel.
    pub fn should_cancel(&self) -> bool {
        matches!(
            self.pending_signal.as_ref().map(|s| s.signal_type),
            Some(SignalType::Cancel)
        )
    }

    /// Check if the instance should exit due to a signal.
    pub fn should_exit(&self) -> bool {
        matches!(
            self.pending_signal.as_ref().map(|s| s.signal_type),
            Some(SignalType::Pause) | Some(SignalType::Cancel)
        )
    }
}

/// Instance status response with full details.
#[derive(Debug, Clone)]
pub struct StatusResponse {
    /// Whether the instance was found
    pub found: bool,
    /// Current status
    pub status: InstanceStatus,
    /// Last known checkpoint ID
    pub checkpoint_id: Option<String>,
    /// Output data if completed
    pub output: Option<Vec<u8>>,
    /// Error message if failed
    pub error: Option<String>,
}

#[cfg(feature = "quic")]
impl From<proto::GetInstanceStatusResponse> for StatusResponse {
    fn from(resp: proto::GetInstanceStatusResponse) -> Self {
        Self {
            found: true,
            status: resp.status.into(),
            checkpoint_id: resp.checkpoint_id,
            output: resp.output,
            error: resp.error,
        }
    }
}

// ============================================================================
// Retry Configuration
// ============================================================================

/// Retry strategy for durable functions.
///
/// Determines how delay between retry attempts is calculated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetryStrategy {
    /// Exponential backoff: delay * 2^(attempt-1)
    ///
    /// First retry: delay * 1
    /// Second retry: delay * 2
    /// Third retry: delay * 4
    /// ...
    #[default]
    ExponentialBackoff,
}

/// Configuration for retry behavior in durable functions.
///
/// Used by the `#[durable]` macro to control retry logic:
/// ```ignore
/// #[durable(max_retries = 3, strategy = ExponentialBackoff, delay = 1000)]
/// pub async fn my_function(...) -> Result<T, E> { ... }
/// ```
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries, just one attempt).
    pub max_retries: u32,
    /// Base delay between retries in milliseconds.
    pub delay_ms: u64,
    /// Retry strategy for calculating delays.
    pub strategy: RetryStrategy,
}

impl RetryConfig {
    /// Create a new retry configuration.
    pub fn new(max_retries: u32, delay_ms: u64, strategy: RetryStrategy) -> Self {
        Self {
            max_retries,
            delay_ms,
            strategy,
        }
    }

    /// Calculate delay for a given attempt (1-indexed).
    ///
    /// Returns the duration to wait before the given retry attempt.
    /// Attempt 1 is the first retry (after the initial failure).
    pub fn delay_for_attempt(&self, attempt: u32) -> std::time::Duration {
        let multiplier = match self.strategy {
            RetryStrategy::ExponentialBackoff => 2u64.saturating_pow(attempt.saturating_sub(1)),
        };
        std::time::Duration::from_millis(self.delay_ms.saturating_mul(multiplier))
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 0,
            delay_ms: 1000,
            strategy: RetryStrategy::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // InstanceStatus Tests
    // ============================================================================

    #[cfg(feature = "quic")]
    #[test]
    fn test_instance_status_conversion() {
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusRunning),
            InstanceStatus::Running
        );
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusCompleted),
            InstanceStatus::Completed
        );
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_instance_status_all_variants() {
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusUnknown),
            InstanceStatus::Unknown
        );
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusPending),
            InstanceStatus::Pending
        );
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusRunning),
            InstanceStatus::Running
        );
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusSuspended),
            InstanceStatus::Suspended
        );
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusCompleted),
            InstanceStatus::Completed
        );
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusFailed),
            InstanceStatus::Failed
        );
        assert_eq!(
            InstanceStatus::from(proto::InstanceStatus::StatusCancelled),
            InstanceStatus::Cancelled
        );
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_instance_status_from_i32_valid() {
        // Valid proto values
        assert_eq!(InstanceStatus::from(0i32), InstanceStatus::Unknown);
        assert_eq!(InstanceStatus::from(1i32), InstanceStatus::Pending);
        assert_eq!(InstanceStatus::from(2i32), InstanceStatus::Running);
        assert_eq!(InstanceStatus::from(3i32), InstanceStatus::Suspended);
        assert_eq!(InstanceStatus::from(4i32), InstanceStatus::Completed);
        assert_eq!(InstanceStatus::from(5i32), InstanceStatus::Failed);
        assert_eq!(InstanceStatus::from(6i32), InstanceStatus::Cancelled);
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_instance_status_from_i32_invalid() {
        // Invalid values should return Unknown
        assert_eq!(InstanceStatus::from(100i32), InstanceStatus::Unknown);
        assert_eq!(InstanceStatus::from(-1i32), InstanceStatus::Unknown);
        assert_eq!(InstanceStatus::from(i32::MAX), InstanceStatus::Unknown);
    }

    #[test]
    fn test_instance_status_clone_eq() {
        let status = InstanceStatus::Running;
        let cloned = status;
        assert_eq!(status, cloned);
    }

    #[test]
    fn test_instance_status_debug() {
        let status = InstanceStatus::Completed;
        let debug_str = format!("{:?}", status);
        assert!(debug_str.contains("Completed"));
    }

    // ============================================================================
    // SignalType Tests
    // ============================================================================

    #[cfg(feature = "quic")]
    #[test]
    fn test_signal_type_conversion() {
        assert_eq!(
            SignalType::from(proto::SignalType::SignalCancel),
            SignalType::Cancel
        );
        assert_eq!(
            proto::SignalType::from(SignalType::Pause),
            proto::SignalType::SignalPause
        );
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_signal_type_all_variants() {
        assert_eq!(
            SignalType::from(proto::SignalType::SignalCancel),
            SignalType::Cancel
        );
        assert_eq!(
            SignalType::from(proto::SignalType::SignalPause),
            SignalType::Pause
        );
        assert_eq!(
            SignalType::from(proto::SignalType::SignalResume),
            SignalType::Resume
        );
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_signal_type_to_proto() {
        assert_eq!(
            proto::SignalType::from(SignalType::Cancel),
            proto::SignalType::SignalCancel
        );
        assert_eq!(
            proto::SignalType::from(SignalType::Pause),
            proto::SignalType::SignalPause
        );
        assert_eq!(
            proto::SignalType::from(SignalType::Resume),
            proto::SignalType::SignalResume
        );
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_signal_type_from_i32_valid() {
        assert_eq!(SignalType::from(0i32), SignalType::Cancel);
        assert_eq!(SignalType::from(1i32), SignalType::Pause);
        assert_eq!(SignalType::from(2i32), SignalType::Resume);
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_signal_type_from_i32_invalid() {
        // Invalid values should default to Cancel
        assert_eq!(SignalType::from(100i32), SignalType::Cancel);
        assert_eq!(SignalType::from(-1i32), SignalType::Cancel);
    }

    #[test]
    fn test_signal_type_clone_eq() {
        let signal = SignalType::Pause;
        let cloned = signal;
        assert_eq!(signal, cloned);
    }

    // ============================================================================
    // Signal Tests
    // ============================================================================

    #[test]
    fn test_signal_creation() {
        let signal = Signal {
            signal_type: SignalType::Cancel,
            payload: vec![1, 2, 3],
            checkpoint_id: Some("checkpoint-1".to_string()),
        };

        assert_eq!(signal.signal_type, SignalType::Cancel);
        assert_eq!(signal.payload, vec![1, 2, 3]);
        assert_eq!(signal.checkpoint_id, Some("checkpoint-1".to_string()));
    }

    #[test]
    fn test_signal_without_checkpoint() {
        let signal = Signal {
            signal_type: SignalType::Pause,
            payload: vec![],
            checkpoint_id: None,
        };

        assert_eq!(signal.signal_type, SignalType::Pause);
        assert!(signal.payload.is_empty());
        assert!(signal.checkpoint_id.is_none());
    }

    #[test]
    fn test_signal_clone() {
        let signal = Signal {
            signal_type: SignalType::Resume,
            payload: vec![42],
            checkpoint_id: Some("cp".to_string()),
        };

        let cloned = signal.clone();
        assert_eq!(signal, cloned);
    }

    #[test]
    fn test_signal_debug() {
        let signal = Signal {
            signal_type: SignalType::Cancel,
            payload: vec![1],
            checkpoint_id: None,
        };
        let debug_str = format!("{:?}", signal);
        assert!(debug_str.contains("Cancel"));
    }

    // ============================================================================
    // CheckpointResult Tests
    // ============================================================================

    #[test]
    fn test_checkpoint_result_existing_state_found() {
        let result = CheckpointResult {
            found: true,
            state: vec![1, 2, 3],
            pending_signal: None,
            custom_signal: None,
        };

        assert!(result.found);
        assert_eq!(result.existing_state(), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn test_checkpoint_result_existing_state_not_found() {
        let result = CheckpointResult {
            found: false,
            state: vec![1, 2, 3], // State might be present but found=false means new checkpoint
            pending_signal: None,
            custom_signal: None,
        };

        assert!(!result.found);
        assert_eq!(result.existing_state(), None);
    }

    #[test]
    fn test_checkpoint_result_should_pause() {
        let result = CheckpointResult {
            found: false,
            state: vec![],
            pending_signal: Some(Signal {
                signal_type: SignalType::Pause,
                payload: vec![],
                checkpoint_id: None,
            }),
            custom_signal: None,
        };

        assert!(result.should_pause());
        assert!(!result.should_cancel());
        assert!(result.should_exit()); // Pause means exit
    }

    #[test]
    fn test_checkpoint_result_should_cancel() {
        let result = CheckpointResult {
            found: false,
            state: vec![],
            pending_signal: Some(Signal {
                signal_type: SignalType::Cancel,
                payload: vec![],
                checkpoint_id: None,
            }),
            custom_signal: None,
        };

        assert!(result.should_cancel());
        assert!(!result.should_pause());
        assert!(result.should_exit()); // Cancel means exit
    }

    #[test]
    fn test_checkpoint_result_should_not_exit_on_resume() {
        let result = CheckpointResult {
            found: false,
            state: vec![],
            pending_signal: Some(Signal {
                signal_type: SignalType::Resume,
                payload: vec![],
                checkpoint_id: None,
            }),
            custom_signal: None,
        };

        assert!(!result.should_pause());
        assert!(!result.should_cancel());
        assert!(!result.should_exit()); // Resume doesn't mean exit
    }

    #[test]
    fn test_checkpoint_result_no_signal() {
        let result = CheckpointResult {
            found: true,
            state: vec![42],
            pending_signal: None,
            custom_signal: None,
        };

        assert!(!result.should_pause());
        assert!(!result.should_cancel());
        assert!(!result.should_exit());
    }

    #[test]
    fn test_checkpoint_result_with_custom_signal() {
        let result = CheckpointResult {
            found: false,
            state: vec![],
            pending_signal: None,
            custom_signal: Some(CustomSignal {
                checkpoint_id: "wait-key".to_string(),
                payload: vec![10, 20, 30],
            }),
        };

        // Custom signal doesn't affect should_pause/cancel/exit
        assert!(!result.should_pause());
        assert!(!result.should_cancel());
        assert!(!result.should_exit());

        // But we can access it
        let custom = result.custom_signal.as_ref().unwrap();
        assert_eq!(custom.checkpoint_id, "wait-key");
        assert_eq!(custom.payload, vec![10, 20, 30]);
    }

    #[test]
    fn test_checkpoint_result_clone() {
        let result = CheckpointResult {
            found: true,
            state: vec![1, 2, 3],
            pending_signal: Some(Signal {
                signal_type: SignalType::Pause,
                payload: vec![4],
                checkpoint_id: Some("cp".to_string()),
            }),
            custom_signal: Some(CustomSignal {
                checkpoint_id: "key".to_string(),
                payload: vec![5],
            }),
        };

        let cloned = result.clone();
        assert_eq!(result, cloned);
    }

    #[test]
    fn test_checkpoint_result_empty_state() {
        let result = CheckpointResult {
            found: true,
            state: vec![],
            pending_signal: None,
            custom_signal: None,
        };

        // Empty state is still "found"
        assert_eq!(result.existing_state(), Some(&[][..]));
    }

    // ============================================================================
    // CustomSignal Tests
    // ============================================================================

    #[test]
    fn test_custom_signal_creation() {
        let signal = CustomSignal {
            checkpoint_id: "my-wait-key".to_string(),
            payload: vec![1, 2, 3, 4],
        };

        assert_eq!(signal.checkpoint_id, "my-wait-key");
        assert_eq!(signal.payload, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_custom_signal_empty_payload() {
        let signal = CustomSignal {
            checkpoint_id: "empty-payload".to_string(),
            payload: vec![],
        };

        assert!(signal.payload.is_empty());
    }

    #[test]
    fn test_custom_signal_clone_eq() {
        let signal = CustomSignal {
            checkpoint_id: "test".to_string(),
            payload: vec![42],
        };

        let cloned = signal.clone();
        assert_eq!(signal, cloned);
    }

    // ============================================================================
    // StatusResponse Tests
    // ============================================================================

    #[test]
    fn test_status_response_not_found() {
        let response = StatusResponse {
            found: false,
            status: InstanceStatus::Unknown,
            checkpoint_id: None,
            output: None,
            error: None,
        };

        assert!(!response.found);
        assert_eq!(response.status, InstanceStatus::Unknown);
    }

    #[test]
    fn test_status_response_completed() {
        let response = StatusResponse {
            found: true,
            status: InstanceStatus::Completed,
            checkpoint_id: Some("final".to_string()),
            output: Some(vec![1, 2, 3]),
            error: None,
        };

        assert!(response.found);
        assert_eq!(response.status, InstanceStatus::Completed);
        assert_eq!(response.output, Some(vec![1, 2, 3]));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_status_response_failed() {
        let response = StatusResponse {
            found: true,
            status: InstanceStatus::Failed,
            checkpoint_id: Some("step-3".to_string()),
            output: None,
            error: Some("something went wrong".to_string()),
        };

        assert!(response.found);
        assert_eq!(response.status, InstanceStatus::Failed);
        assert!(response.output.is_none());
        assert_eq!(response.error, Some("something went wrong".to_string()));
    }

    #[test]
    fn test_status_response_clone() {
        let response = StatusResponse {
            found: true,
            status: InstanceStatus::Running,
            checkpoint_id: Some("cp".to_string()),
            output: Some(vec![42]),
            error: None,
        };

        let cloned = response.clone();
        assert_eq!(cloned.found, response.found);
        assert_eq!(cloned.status, response.status);
        assert_eq!(cloned.checkpoint_id, response.checkpoint_id);
        assert_eq!(cloned.output, response.output);
    }

    #[cfg(feature = "quic")]
    #[test]
    fn test_status_response_from_proto() {
        let proto_response = proto::GetInstanceStatusResponse {
            instance_id: "test-instance".to_string(),
            status: proto::InstanceStatus::StatusCompleted.into(),
            checkpoint_id: Some("last-cp".to_string()),
            output: Some(vec![99]),
            error: None,
            started_at_ms: 1000,
            finished_at_ms: Some(2000),
        };

        let response = StatusResponse::from(proto_response);
        assert!(response.found);
        assert_eq!(response.status, InstanceStatus::Completed);
        assert_eq!(response.checkpoint_id, Some("last-cp".to_string()));
        assert_eq!(response.output, Some(vec![99]));
    }

    // ============================================================================
    // RetryConfig Tests
    // ============================================================================

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 0);
        assert_eq!(config.delay_ms, 1000);
        assert_eq!(config.strategy, RetryStrategy::ExponentialBackoff);
    }

    #[test]
    fn test_retry_config_delay_calculation() {
        let config = RetryConfig::new(3, 100, RetryStrategy::ExponentialBackoff);

        // Attempt 1 (first retry): 100ms * 2^0 = 100ms
        assert_eq!(
            config.delay_for_attempt(1),
            std::time::Duration::from_millis(100)
        );

        // Attempt 2 (second retry): 100ms * 2^1 = 200ms
        assert_eq!(
            config.delay_for_attempt(2),
            std::time::Duration::from_millis(200)
        );

        // Attempt 3 (third retry): 100ms * 2^2 = 400ms
        assert_eq!(
            config.delay_for_attempt(3),
            std::time::Duration::from_millis(400)
        );
    }

    #[test]
    fn test_retry_strategy_default() {
        assert_eq!(RetryStrategy::default(), RetryStrategy::ExponentialBackoff);
    }

    #[test]
    fn test_retry_config_new() {
        let config = RetryConfig::new(5, 500, RetryStrategy::ExponentialBackoff);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.delay_ms, 500);
        assert_eq!(config.strategy, RetryStrategy::ExponentialBackoff);
    }

    #[test]
    fn test_retry_config_delay_attempt_zero() {
        let config = RetryConfig::new(3, 100, RetryStrategy::ExponentialBackoff);
        // Attempt 0 uses saturating_sub, so 2^(0-1) = 2^u32::MAX = saturates
        // Actually: 0.saturating_sub(1) = 0, so 2^0 = 1
        assert_eq!(
            config.delay_for_attempt(0),
            std::time::Duration::from_millis(100)
        );
    }

    #[test]
    fn test_retry_config_delay_large_attempt() {
        let config = RetryConfig::new(10, 100, RetryStrategy::ExponentialBackoff);
        // Attempt 10: 100ms * 2^9 = 100 * 512 = 51200ms
        assert_eq!(
            config.delay_for_attempt(10),
            std::time::Duration::from_millis(51200)
        );
    }

    #[test]
    fn test_retry_config_delay_overflow_protection() {
        // Test that we don't overflow with very large values
        let config = RetryConfig::new(100, u64::MAX, RetryStrategy::ExponentialBackoff);
        // This should use saturating_mul and not panic
        let _delay = config.delay_for_attempt(64); // 2^63 would overflow
        // Just verify it doesn't panic
    }

    #[test]
    fn test_retry_config_clone() {
        let config = RetryConfig::new(3, 200, RetryStrategy::ExponentialBackoff);
        let cloned = config.clone();
        assert_eq!(config.max_retries, cloned.max_retries);
        assert_eq!(config.delay_ms, cloned.delay_ms);
        assert_eq!(config.strategy, cloned.strategy);
    }

    #[test]
    fn test_retry_config_debug() {
        let config = RetryConfig::new(2, 1000, RetryStrategy::ExponentialBackoff);
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("max_retries"));
        assert!(debug_str.contains("delay_ms"));
        assert!(debug_str.contains("strategy"));
    }
}
