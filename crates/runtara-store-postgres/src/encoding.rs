// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! PostgreSQL enum encodings for execution domain values.
//!
//! Public for host operations that share the PostgreSQL schema. Core neither
//! knows these spellings nor accepts unvalidated strings as execution values.

use runtara_core::domain::{EventType, InstanceStatus, SignalType};

/// Encode a domain value as a PostgreSQL enum label.
pub fn status_to_str(value: InstanceStatus) -> &'static str {
    match value {
        InstanceStatus::Pending => "pending",
        InstanceStatus::Running => "running",
        InstanceStatus::Suspended => "suspended",
        InstanceStatus::Completed => "completed",
        InstanceStatus::Failed => "failed",
        InstanceStatus::Cancelled => "cancelled",
    }
}

/// Decode a PostgreSQL enum label, rejecting unrecognized stored values.
pub fn status_from_str(value: &str) -> Result<InstanceStatus, sqlx::Error> {
    match value {
        "pending" => Ok(InstanceStatus::Pending),
        "running" => Ok(InstanceStatus::Running),
        "suspended" => Ok(InstanceStatus::Suspended),
        "completed" => Ok(InstanceStatus::Completed),
        "failed" => Ok(InstanceStatus::Failed),
        "cancelled" => Ok(InstanceStatus::Cancelled),
        _ => Err(sqlx::Error::Decode("unrecognized stored status".into())),
    }
}

/// Encode a domain value as a PostgreSQL enum label.
pub fn signal_type_to_str(value: SignalType) -> &'static str {
    match value {
        SignalType::Cancel => "cancel",
        SignalType::Pause => "pause",
        SignalType::Resume => "resume",
        SignalType::Shutdown => "shutdown",
    }
}

/// Decode a PostgreSQL enum label, rejecting unrecognized stored values.
pub fn signal_type_from_str(value: &str) -> Result<SignalType, sqlx::Error> {
    match value {
        "cancel" => Ok(SignalType::Cancel),
        "pause" => Ok(SignalType::Pause),
        "resume" => Ok(SignalType::Resume),
        "shutdown" => Ok(SignalType::Shutdown),
        _ => Err(sqlx::Error::Decode(
            "unrecognized stored signal_type".into(),
        )),
    }
}

/// Encode a domain value as a PostgreSQL enum label.
pub fn event_type_to_str(value: EventType) -> &'static str {
    match value {
        EventType::Started => "started",
        EventType::Progress => "progress",
        EventType::Heartbeat => "heartbeat",
        EventType::Completed => "completed",
        EventType::Failed => "failed",
        EventType::Suspended => "suspended",
        EventType::Custom => "custom",
    }
}

/// Decode a PostgreSQL enum label, rejecting unrecognized stored values.
pub fn event_type_from_str(value: &str) -> Result<EventType, sqlx::Error> {
    match value {
        "started" => Ok(EventType::Started),
        "progress" => Ok(EventType::Progress),
        "heartbeat" => Ok(EventType::Heartbeat),
        "completed" => Ok(EventType::Completed),
        "failed" => Ok(EventType::Failed),
        "suspended" => Ok(EventType::Suspended),
        "custom" => Ok(EventType::Custom),
        _ => Err(sqlx::Error::Decode("unrecognized stored event_type".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stored_label_round_trips_and_unknown_labels_fail() {
        for label in [
            "pending",
            "running",
            "suspended",
            "completed",
            "failed",
            "cancelled",
        ] {
            assert_eq!(status_to_str(status_from_str(label).unwrap()), label);
        }
        for invalid in ["", "unknown", "INVALID"] {
            assert!(status_from_str(invalid).is_err());
        }
        for label in ["cancel", "pause", "resume", "shutdown"] {
            assert_eq!(
                signal_type_to_str(signal_type_from_str(label).unwrap()),
                label
            );
        }
        for invalid in ["", "unknown", "INVALID"] {
            assert!(signal_type_from_str(invalid).is_err());
        }
        for label in [
            "started",
            "progress",
            "heartbeat",
            "completed",
            "failed",
            "suspended",
            "custom",
        ] {
            assert_eq!(
                event_type_to_str(event_type_from_str(label).unwrap()),
                label
            );
        }
        for invalid in ["", "unknown", "INVALID"] {
            assert!(event_type_from_str(invalid).is_err());
        }
    }
}
