// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plain Rust request/response types, enums, and error constants for the
//! instance protocol.

/// Signal type for instance-wide signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalType {
    /// Cancel execution.
    SignalCancel = 0,
    /// Pause execution (checkpoint and wait).
    SignalPause = 1,
    /// Resume paused execution.
    SignalResume = 2,
    /// Server draining: suspend at next checkpoint so the instance can be
    /// resumed after restart. Behaves like a cancel from the SDK's perspective
    /// but transitions the instance to `suspended + termination_reason="shutdown_requested"`
    /// rather than `cancelled`.
    SignalShutdown = 3,
}

impl From<SignalType> for i32 {
    fn from(s: SignalType) -> i32 {
        s as i32
    }
}

/// Instance event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceEventType {
    /// Simple "I'm alive" signal.
    EventHeartbeat = 0,
    /// Instance finished successfully, payload = output.
    EventCompleted = 2,
    /// Instance failed, payload = error details.
    EventFailed = 3,
    /// Instance suspended (waiting for wake).
    EventSuspended = 4,
    /// Generic custom event with arbitrary subtype.
    EventCustom = 5,
}

impl From<InstanceEventType> for i32 {
    fn from(e: InstanceEventType) -> i32 {
        e as i32
    }
}

/// Instance status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    /// Unknown status.
    StatusUnknown = 0,
    /// Queued, not yet started.
    StatusPending = 1,
    /// Currently executing.
    StatusRunning = 2,
    /// Sleeping / waiting for wake.
    StatusSuspended = 3,
    /// Finished successfully.
    StatusCompleted = 4,
    /// Finished with error.
    StatusFailed = 5,
    /// Cancelled by signal.
    StatusCancelled = 6,
}

impl From<InstanceStatus> for i32 {
    fn from(s: InstanceStatus) -> i32 {
        s as i32
    }
}

impl InstanceStatus {
    /// Try to convert from an i32 value.
    pub fn try_from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::StatusUnknown),
            1 => Some(Self::StatusPending),
            2 => Some(Self::StatusRunning),
            3 => Some(Self::StatusSuspended),
            4 => Some(Self::StatusCompleted),
            5 => Some(Self::StatusFailed),
            6 => Some(Self::StatusCancelled),
            _ => None,
        }
    }
}

/// Register instance request.
pub struct RegisterInstanceRequest {
    /// Instance identifier.
    pub instance_id: String,
    /// Tenant identifier.
    pub tenant_id: String,
    /// Optional checkpoint ID to resume from.
    pub checkpoint_id: Option<String>,
}

/// Register instance response.
pub struct RegisterInstanceResponse {
    /// Whether registration succeeded.
    pub success: bool,
    /// Error message if registration failed.
    pub error: String,
}

/// Checkpoint request.
pub struct CheckpointRequest {
    /// Instance identifier.
    pub instance_id: String,
    /// Checkpoint identifier (unique per durable function call).
    pub checkpoint_id: String,
    /// Serialized workflow state.
    pub state: Vec<u8>,
}

/// Signal forwarded from core to instance.
#[derive(Debug, Clone)]
pub struct Signal {
    /// Instance identifier.
    pub instance_id: String,
    /// Signal type as integer (see `SignalType` enum values).
    pub signal_type: i32,
    /// Signal payload bytes.
    pub payload: Vec<u8>,
}

/// Custom signal targeted at a specific checkpoint_id.
#[derive(Debug, Clone)]
pub struct CustomSignal {
    /// Checkpoint ID this signal targets.
    pub checkpoint_id: String,
    /// Signal payload bytes.
    pub payload: Vec<u8>,
}

/// Error info returned with checkpoint responses.
#[derive(Debug, Clone)]
pub struct CheckpointErrorInfo {
    /// Machine-readable error code.
    pub code: String,
    /// Human-readable error message.
    pub message: String,
}

/// Checkpoint response.
pub struct CheckpointResponse {
    /// True if checkpoint already existed (resume case).
    pub found: bool,
    /// Existing checkpoint state if found, empty if new.
    pub state: Vec<u8>,
    /// Pending instance-wide signal (cancel/pause/resume).
    pub pending_signal: Option<Signal>,
    /// Pending checkpoint-scoped custom signal.
    pub custom_signal: Option<CustomSignal>,
    /// Last error from a previous checkpoint attempt.
    pub last_error: Option<CheckpointErrorInfo>,
}

/// Get checkpoint request (read-only lookup).
pub struct GetCheckpointRequest {
    /// Instance identifier.
    pub instance_id: String,
    /// Checkpoint ID to look up.
    pub checkpoint_id: String,
}

/// Get checkpoint response.
pub struct GetCheckpointResponse {
    /// True if checkpoint exists.
    pub found: bool,
    /// Checkpoint state if found.
    pub state: Vec<u8>,
}

/// Sleep request.
pub struct SleepRequest {
    /// Instance identifier.
    pub instance_id: String,
    /// Sleep duration in milliseconds.
    pub duration_ms: u64,
    /// Checkpoint ID for resume after wake.
    pub checkpoint_id: String,
    /// State to restore on wake.
    pub state: Vec<u8>,
}

/// Sleep response (empty - sleep completes in-process).
pub struct SleepResponse {}

/// Instance event.
pub struct InstanceEvent {
    /// Instance identifier.
    pub instance_id: String,
    /// Event type as integer (see `InstanceEventType` enum values).
    pub event_type: i32,
    /// Current checkpoint position.
    pub checkpoint_id: Option<String>,
    /// Event-specific data (output, error, etc.).
    pub payload: Vec<u8>,
    /// Event timestamp in milliseconds since epoch.
    pub timestamp_ms: i64,
    /// Arbitrary subtype for custom events.
    pub subtype: Option<String>,
}

impl InstanceEvent {
    /// Get the event type as an enum.
    pub fn event_type(&self) -> InstanceEventType {
        match self.event_type {
            0 => InstanceEventType::EventHeartbeat,
            2 => InstanceEventType::EventCompleted,
            3 => InstanceEventType::EventFailed,
            4 => InstanceEventType::EventSuspended,
            5 => InstanceEventType::EventCustom,
            _ => InstanceEventType::EventCustom,
        }
    }
}

/// Instance event response.
pub struct InstanceEventResponse {
    /// Whether the event was persisted successfully.
    pub success: bool,
    /// Error message if persistence failed.
    pub error: Option<String>,
}

/// Get instance status request.
pub struct GetInstanceStatusRequest {
    /// Instance identifier.
    pub instance_id: String,
}

/// Get instance status response.
pub struct GetInstanceStatusResponse {
    /// Instance identifier.
    pub instance_id: String,
    /// Instance status as integer (see `InstanceStatus` enum values).
    pub status: i32,
    /// Last known checkpoint ID.
    pub checkpoint_id: Option<String>,
    /// Instance start timestamp in milliseconds since epoch.
    pub started_at_ms: i64,
    /// Instance finish timestamp in milliseconds since epoch.
    pub finished_at_ms: Option<i64>,
    /// Output data if completed.
    pub output: Option<Vec<u8>>,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Poll signals request.
pub struct PollSignalsRequest {
    /// Instance identifier.
    pub instance_id: String,
    /// Optional checkpoint ID for custom signal polling.
    pub checkpoint_id: Option<String>,
}

/// Poll signals response.
pub struct PollSignalsResponse {
    /// Pending instance-wide signal if any.
    pub signal: Option<Signal>,
    /// Pending checkpoint-scoped custom signal if any.
    pub custom_signal: Option<CustomSignal>,
}

/// Signal acknowledgement.
pub struct SignalAck {
    /// Instance identifier.
    pub instance_id: String,
    /// Signal type as integer (see `SignalType` enum values).
    pub signal_type: i32,
    /// Whether the signal was acknowledged.
    pub acknowledged: bool,
}

impl SignalAck {
    /// Get the signal type as an enum.
    pub fn signal_type(&self) -> SignalType {
        match self.signal_type {
            0 => SignalType::SignalCancel,
            1 => SignalType::SignalPause,
            2 => SignalType::SignalResume,
            3 => SignalType::SignalShutdown,
            _ => SignalType::SignalCancel,
        }
    }
}

/// Error metadata for retry decisions.
pub struct ErrorMetadata {
    /// Error category (0=unknown, 1=transient, 2=permanent, 3=business).
    pub category: i32,
    /// Error severity (0=info, 1=warning, 2=error, 3=critical).
    pub severity: i32,
    /// Retry hint (0=unknown, 1=immediately, 2=backoff, 3=after, 4=do_not_retry).
    pub retry_hint: i32,
    /// Retry delay in milliseconds for `retry_after` hint.
    pub retry_after_ms: Option<u64>,
    /// Machine-readable error code.
    pub error_code: Option<String>,
}

impl ErrorMetadata {
    /// Get the error category name.
    pub fn category(&self) -> &'static str {
        match self.category {
            1 => "transient",
            2 => "permanent",
            3 => "business",
            _ => "unknown",
        }
    }

    /// Get the error severity name.
    pub fn severity(&self) -> &'static str {
        match self.severity {
            0 => "info",
            1 => "warning",
            2 => "error",
            3 => "critical",
            _ => "unknown",
        }
    }

    /// Get the retry hint name.
    pub fn retry_hint(&self) -> &'static str {
        match self.retry_hint {
            1 => "retry_immediately",
            2 => "retry_with_backoff",
            3 => "retry_after",
            4 => "do_not_retry",
            _ => "unknown",
        }
    }
}

/// Retry attempt event.
pub struct RetryAttemptEvent {
    /// Instance identifier.
    pub instance_id: String,
    /// Durable function cache key.
    pub checkpoint_id: String,
    /// 1-indexed retry attempt number.
    pub attempt_number: u32,
    /// Timestamp in milliseconds since epoch.
    pub timestamp_ms: i64,
    /// Error from previous attempt.
    pub error_message: Option<String>,
    /// Structured error metadata for retry decisions.
    pub error_metadata: Option<ErrorMetadata>,
}

/// Error string returned by `handle_register_instance` when the core is
/// draining. The HTTP layer maps this to `503 Service Unavailable`.
pub const ERROR_SERVER_DRAINING: &str = "server draining";

/// Error string returned by `handle_register_instance` when the active-instance
/// count has reached `RUNTARA_MAX_CONCURRENT_INSTANCES`. The HTTP layer maps
/// this to `429 Too Many Requests`.
pub const ERROR_MAX_CONCURRENT_INSTANCES: &str = "max concurrent instances reached";
