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
        subtype: None,
    }
}

/// Build a custom event with an arbitrary subtype.
///
/// Custom events are used for extensibility - the subtype can be any string
/// and the payload is opaque bytes. runtara-core stores them without interpretation.
pub(crate) fn build_custom_event(
    instance_id: &str,
    subtype: &str,
    payload: Vec<u8>,
) -> InstanceEvent {
    InstanceEvent {
        instance_id: instance_id.to_string(),
        event_type: InstanceEventType::EventCustom.into(),
        checkpoint_id: None,
        payload,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: Some(subtype.to_string()),
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

/// Build a suspended event with sleep data for durable sleep.
///
/// The payload contains JSON with wake_at_ms and base64-encoded state.
/// Core will parse this and set sleep_until for the wake scheduler.
pub(crate) fn build_suspended_with_sleep_event(
    instance_id: &str,
    checkpoint_id: &str,
    wake_at: chrono::DateTime<chrono::Utc>,
    state: &[u8],
) -> InstanceEvent {
    use base64::Engine;

    // Encode payload as JSON with wake time and state
    let payload = serde_json::json!({
        "wake_at_ms": wake_at.timestamp_millis(),
        "state": base64::engine::general_purpose::STANDARD.encode(state),
    });

    build_event(
        instance_id,
        InstanceEventType::EventSuspended,
        Some(checkpoint_id.to_string()),
        payload.to_string().into_bytes(),
    )
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

    #[test]
    fn test_build_custom_event() {
        let event = build_custom_event("test-instance", "step_debug_start", b"payload".to_vec());
        assert_eq!(event.instance_id, "test-instance");
        assert_eq!(event.event_type, InstanceEventType::EventCustom as i32);
        assert_eq!(event.subtype, Some("step_debug_start".to_string()));
        assert_eq!(event.payload, b"payload".to_vec());
    }

    #[test]
    fn test_build_suspended_with_sleep_event() {
        use chrono::TimeZone;

        let wake_at = chrono::Utc.with_ymd_and_hms(2025, 6, 15, 12, 0, 0).unwrap();
        let state = b"test state data";

        let event =
            build_suspended_with_sleep_event("test-instance", "checkpoint-1", wake_at, state);

        assert_eq!(event.instance_id, "test-instance");
        assert_eq!(event.event_type, InstanceEventType::EventSuspended as i32);
        assert_eq!(event.checkpoint_id, Some("checkpoint-1".to_string()));

        // Verify payload is valid JSON with expected fields
        let payload_str = String::from_utf8(event.payload).unwrap();
        let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap();
        assert_eq!(payload["wake_at_ms"], wake_at.timestamp_millis());
        assert!(payload["state"].as_str().is_some()); // base64 encoded
    }
}
