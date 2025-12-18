// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! High-level types for the SDK.

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

impl From<proto::SignalType> for SignalType {
    fn from(signal: proto::SignalType) -> Self {
        match signal {
            proto::SignalType::SignalCancel => SignalType::Cancel,
            proto::SignalType::SignalPause => SignalType::Pause,
            proto::SignalType::SignalResume => SignalType::Resume,
        }
    }
}

impl From<i32> for SignalType {
    fn from(value: i32) -> Self {
        proto::SignalType::try_from(value)
            .map(SignalType::from)
            .unwrap_or(SignalType::Cancel)
    }
}

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

/// Sleep response indicating whether sleep was deferred.
#[derive(Debug, Clone, Copy)]
pub struct SleepResult {
    /// If true, instance should exit - core will wake it later.
    /// If false, sleep completed in-process, continue execution.
    pub deferred: bool,
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
    /// Instance ID
    pub instance_id: String,
    /// Current status
    pub status: InstanceStatus,
    /// Last known checkpoint ID
    pub checkpoint_id: Option<String>,
    /// When the instance started (milliseconds since epoch)
    pub started_at_ms: i64,
    /// When the instance finished (milliseconds since epoch)
    pub finished_at_ms: Option<i64>,
    /// Output data if completed
    pub output: Option<Vec<u8>>,
    /// Error message if failed
    pub error: Option<String>,
}

impl From<proto::GetInstanceStatusResponse> for StatusResponse {
    fn from(resp: proto::GetInstanceStatusResponse) -> Self {
        Self {
            instance_id: resp.instance_id,
            status: resp.status.into(),
            checkpoint_id: resp.checkpoint_id,
            started_at_ms: resp.started_at_ms,
            finished_at_ms: resp.finished_at_ms,
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
}
