// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Event building utilities.

use runtara_protocol::instance_proto::{InstanceEvent, InstanceEventType};

/// Build an instance event with the current timestamp.
pub(crate) fn build_event(
    instance_id: &str,
    event_type: InstanceEventType,
    checkpoint_id: Option<String>,
    payload: Vec<u8>,
) -> InstanceEvent {
    InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: event_type.into(),
        checkpoint_id,
        payload,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    }
}

/// Build a heartbeat event (simple "I'm alive" signal).
pub(crate) fn build_heartbeat_event(instance_id: &str) -> InstanceEvent {
    build_event(instance_id, InstanceEventType::EventHeartbeat, None, vec![])
}

/// Build a completed event with output.
pub(crate) fn build_completed_event(instance_id: &str, output: Vec<u8>) -> InstanceEvent {
    build_event(instance_id, InstanceEventType::EventCompleted, None, output)
}

/// Build a failed event with error message.
pub(crate) fn build_failed_event(instance_id: &str, error: &str) -> InstanceEvent {
    build_event(
        instance_id,
        InstanceEventType::EventFailed,
        None,
        error.as_bytes().to_vec(),
    )
}

/// Build a suspended event.
pub(crate) fn build_suspended_event(instance_id: &str) -> InstanceEvent {
    build_event(instance_id, InstanceEventType::EventSuspended, None, vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_heartbeat_event() {
        let event = build_heartbeat_event("test-instance");
        assert_eq!(event.instance_id, "test-instance");
        assert_eq!(event.event_type, InstanceEventType::EventHeartbeat as i32);
        assert!(event.checkpoint_id.is_none());
    }

    #[test]
    fn test_build_completed_event() {
        let event = build_completed_event("test-instance", b"result".to_vec());
        assert_eq!(event.event_type, InstanceEventType::EventCompleted as i32);
        assert_eq!(event.payload, b"result".to_vec());
    }

    #[test]
    fn test_build_failed_event() {
        let event = build_failed_event("test-instance", "something went wrong");
        assert_eq!(event.event_type, InstanceEventType::EventFailed as i32);
        assert_eq!(event.payload, b"something went wrong".to_vec());
    }
}
