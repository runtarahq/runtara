// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conversion helpers between protocol enums and their database representations.

use super::types::{InstanceEventType, InstanceStatus, SignalType};

/// Map proto event type to database enum string.
pub fn map_event_type(event_type: InstanceEventType) -> &'static str {
    match event_type {
        InstanceEventType::EventHeartbeat => "heartbeat",
        InstanceEventType::EventCompleted => "completed",
        InstanceEventType::EventFailed => "failed",
        InstanceEventType::EventSuspended => "suspended",
        InstanceEventType::EventCustom => "custom",
    }
}

/// Map database status string to InstanceStatus enum.
pub fn map_status(status: &str) -> InstanceStatus {
    match status {
        "pending" => InstanceStatus::StatusPending,
        "running" => InstanceStatus::StatusRunning,
        "suspended" => InstanceStatus::StatusSuspended,
        "completed" => InstanceStatus::StatusCompleted,
        "failed" => InstanceStatus::StatusFailed,
        "cancelled" => InstanceStatus::StatusCancelled,
        _ => InstanceStatus::StatusUnknown,
    }
}

/// Map SignalType enum to database enum string.
pub fn map_signal_type(signal_type: SignalType) -> &'static str {
    match signal_type {
        SignalType::SignalCancel => "cancel",
        SignalType::SignalPause => "pause",
        SignalType::SignalResume => "resume",
        SignalType::SignalShutdown => "shutdown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_mapping() {
        assert_eq!(
            map_event_type(InstanceEventType::EventHeartbeat),
            "heartbeat"
        );
        assert_eq!(
            map_event_type(InstanceEventType::EventCompleted),
            "completed"
        );
        assert_eq!(map_event_type(InstanceEventType::EventFailed), "failed");
        assert_eq!(
            map_event_type(InstanceEventType::EventSuspended),
            "suspended"
        );
        assert_eq!(map_event_type(InstanceEventType::EventCustom), "custom");
    }

    #[test]
    fn test_status_mapping_all_variants() {
        assert_eq!(map_status("pending"), InstanceStatus::StatusPending);
        assert_eq!(map_status("running"), InstanceStatus::StatusRunning);
        assert_eq!(map_status("suspended"), InstanceStatus::StatusSuspended);
        assert_eq!(map_status("completed"), InstanceStatus::StatusCompleted);
        assert_eq!(map_status("failed"), InstanceStatus::StatusFailed);
        assert_eq!(map_status("cancelled"), InstanceStatus::StatusCancelled);
        assert_eq!(map_status("invalid"), InstanceStatus::StatusUnknown);
        assert_eq!(map_status(""), InstanceStatus::StatusUnknown);
    }

    #[test]
    fn test_signal_type_mapping() {
        assert_eq!(map_signal_type(SignalType::SignalCancel), "cancel");
        assert_eq!(map_signal_type(SignalType::SignalPause), "pause");
        assert_eq!(map_signal_type(SignalType::SignalResume), "resume");
        assert_eq!(map_signal_type(SignalType::SignalShutdown), "shutdown");
    }
}
