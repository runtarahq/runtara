// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Execution values shared by handlers and persistence implementations.
//!
//! These types describe execution semantics. Storage and transport adapters
//! own their encodings.

/// The lifecycle state of a stored instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    /// Queued, before execution starts.
    Pending,
    /// Executing.
    Running,
    /// Paused or waiting to wake.
    Suspended,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled by a signal.
    Cancelled,
}

impl InstanceStatus {
    /// Whether execution has ended permanently.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// An instance-wide control signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalType {
    /// Cancel execution.
    Cancel,
    /// Pause execution.
    Pause,
    /// Resume execution.
    Resume,
    /// Suspend for a server restart.
    Shutdown,
}

/// An event in an instance's persisted timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    /// The instance registered and started executing.
    Started,
    /// A legacy progress event, retained for historical timelines.
    Progress,
    /// Activity while the instance is executing.
    Heartbeat,
    /// Successful completion.
    Completed,
    /// Failed execution.
    Failed,
    /// Execution suspended.
    Suspended,
    /// A producer-defined event with an optional opaque subtype.
    Custom,
}
