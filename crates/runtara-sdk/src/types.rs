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
#[derive(Debug, Clone)]
pub struct Signal {
    /// The type of signal
    pub signal_type: SignalType,
    /// Signal-specific payload data
    pub payload: Vec<u8>,
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
#[derive(Debug, Clone)]
pub struct CheckpointResult {
    /// If true, an existing checkpoint was found and state is returned.
    /// If false, a new checkpoint was saved.
    pub found: bool,
    /// Checkpoint state (existing state if found, empty if saved).
    pub state: Vec<u8>,
    /// Pending signal type if any (cancel, pause).
    /// Instance should handle this signal after processing the checkpoint.
    pub pending_signal: Option<SignalType>,
}

impl CheckpointResult {
    /// Returns Some(state) if an existing checkpoint was found (for resume).
    /// Returns None if a new checkpoint was saved.
    pub fn existing_state(&self) -> Option<&[u8]> {
        if self.found { Some(&self.state) } else { None }
    }

    /// Check if the instance should pause.
    pub fn should_pause(&self) -> bool {
        self.pending_signal == Some(SignalType::Pause)
    }

    /// Check if the instance should cancel.
    pub fn should_cancel(&self) -> bool {
        self.pending_signal == Some(SignalType::Cancel)
    }

    /// Check if the instance should exit due to a signal.
    pub fn should_exit(&self) -> bool {
        matches!(
            self.pending_signal,
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
}
