// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance event handlers: generic event ingestion and retry-attempt logging.

use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::{debug, info, instrument, warn};

use super::mappers::map_event_type;
use super::state::InstanceHandlerState;
use super::types::{InstanceEvent, InstanceEventResponse, InstanceEventType, RetryAttemptEvent};
use crate::error::CoreError;
use crate::persistence::EventRecord;

/// Handle instance event.
///
/// Processes events from instances:
/// - **Heartbeat**: Update activity timestamp
/// - **Completed**: Mark instance as completed, store output
/// - **Failed**: Mark instance as failed, store error
/// - **Suspended**: Mark instance as suspended
/// - **Custom**: Store custom event for telemetry (debug events, etc.)
///
/// All events return `InstanceEventResponse` to acknowledge persistence.
/// This ensures no events are lost due to race conditions when the process exits.
#[instrument(skip(state, event), fields(instance_id = %event.instance_id, event_type = ?event.event_type()))]
pub async fn handle_instance_event(
    state: &InstanceHandlerState,
    event: InstanceEvent,
) -> Result<InstanceEventResponse> {
    debug!(
        event_type = ?event.event_type,
        checkpoint_id = ?event.checkpoint_id,
        payload_size = event.payload.len(),
        timestamp_ms = event.timestamp_ms,
        "Received instance event"
    );

    // 1. Map proto event type to DB enum
    let event_type = map_event_type(event.event_type());

    // 2. Validate instance_id is not empty
    if event.instance_id.is_empty() {
        return Err(CoreError::ValidationError {
            field: "instance_id".to_string(),
            message: "instance_id is required".to_string(),
        }
        .into());
    }

    // 3. Determine timestamp
    let created_at = DateTime::from_timestamp_millis(event.timestamp_ms).unwrap_or_else(Utc::now);

    // 4. Insert event record
    let event_record = EventRecord {
        id: None,
        instance_id: event.instance_id.clone(),
        event_type: event_type.to_string(),
        checkpoint_id: event.checkpoint_id.clone(),
        payload: if event.payload.is_empty() {
            None
        } else {
            Some(event.payload.clone())
        },
        created_at,
        subtype: event.subtype.clone(),
    };
    state.persistence.insert_event(&event_record).await?;

    // 5. Update instance status based on event type
    // All events return a response to acknowledge persistence
    match event.event_type() {
        InstanceEventType::EventHeartbeat => {
            // Heartbeat is just an "I'm alive" signal - no state changes needed
            // The event was already logged above
            debug!("Heartbeat received");
        }
        InstanceEventType::EventCompleted => {
            let output = if event.payload.is_empty() {
                None
            } else {
                Some(event.payload.as_slice())
            };
            // Use _if_running to prevent race condition with PID monitor:
            // If process crashed and PID monitor already set status to "failed",
            // we should not overwrite it with "completed" from queued SDK event.
            let applied = state
                .persistence
                .complete_instance_if_running(
                    &event.instance_id,
                    "completed",
                    output,
                    None,
                    None,
                    None,
                )
                .await?;
            if applied {
                info!("Instance completed successfully");
            } else {
                warn!("Instance completion skipped (already in terminal state)");
            }
        }
        InstanceEventType::EventFailed => {
            let error = if event.payload.is_empty() {
                "Unknown error"
            } else {
                std::str::from_utf8(&event.payload).unwrap_or("Unknown error (binary payload)")
            };
            // Use _if_running to prevent race condition with PID monitor:
            // If PID monitor already set status to "failed", don't overwrite with SDK event.
            let applied = state
                .persistence
                .complete_instance_if_running(
                    &event.instance_id,
                    "failed",
                    None,
                    Some(error),
                    None,
                    None,
                )
                .await?;
            if applied {
                warn!(error = %error, "Instance failed");
            } else {
                warn!(error = %error, "Instance failure event skipped (already in terminal state)");
            }
        }
        InstanceEventType::EventSuspended => {
            // Check if this is a suspended-with-sleep event (has sleep data in payload)
            if let (false, Some(checkpoint_id)) = (event.payload.is_empty(), &event.checkpoint_id) {
                if let Some(sleep_data) = parse_sleep_payload(&event.payload) {
                    // Save checkpoint with state from payload
                    state
                        .persistence
                        .save_checkpoint(&event.instance_id, checkpoint_id, &sleep_data.state)
                        .await?;

                    // Update instance checkpoint reference
                    state
                        .persistence
                        .update_instance_checkpoint(&event.instance_id, checkpoint_id)
                        .await?;

                    // Set sleep_until for wake scheduler
                    if let Some(wake_at) = sleep_data.wake_at {
                        state
                            .persistence
                            .set_instance_sleep(&event.instance_id, wake_at)
                            .await?;
                    }

                    // Mark as suspended with termination_reason "sleeping"
                    // Use _if_running to prevent race condition with PID monitor.
                    let applied = state
                        .persistence
                        .complete_instance_with_termination_if_running(
                            &event.instance_id,
                            "suspended",
                            Some("sleeping"),
                            None, // exit_code
                            None, // output
                            None, // error
                            None, // stderr
                            Some(checkpoint_id),
                        )
                        .await?;

                    if applied {
                        info!(
                            checkpoint_id = %checkpoint_id,
                            wake_at = ?sleep_data.wake_at,
                            "Instance sleeping until scheduled wake"
                        );
                    } else {
                        warn!(
                            checkpoint_id = %checkpoint_id,
                            "Instance sleep event skipped (already in terminal state)"
                        );
                    }
                } else {
                    // Payload present but not valid sleep data - just suspend
                    // Use _if_running to prevent race condition with PID monitor.
                    let applied = state
                        .persistence
                        .complete_instance_if_running(
                            &event.instance_id,
                            "suspended",
                            None,
                            None,
                            None,
                            None,
                        )
                        .await?;
                    if applied {
                        info!("Instance suspended (with payload but no valid sleep data)");
                    } else {
                        warn!("Instance suspend event skipped (already in terminal state)");
                    }
                }
            } else {
                // No payload or no checkpoint_id - simple suspend
                // Use _if_running to prevent race condition with PID monitor.
                let applied = state
                    .persistence
                    .complete_instance_if_running(
                        &event.instance_id,
                        "suspended",
                        None,
                        None,
                        None,
                        None,
                    )
                    .await?;
                if applied {
                    info!("Instance suspended");
                } else {
                    warn!("Instance suspend event skipped (already in terminal state)");
                }
            }
        }
        InstanceEventType::EventCustom => {
            // Custom events are just stored for telemetry - no state changes needed
            // The event was already logged above with its subtype
            debug!(subtype = ?event.subtype, "Custom event received");
        }
    }

    Ok(InstanceEventResponse {
        success: true,
        error: None,
    })
}

/// Handle retry attempt event (fire-and-forget).
///
/// Records a retry attempt for audit trail. Retry attempts are stored
/// in the checkpoints table with `is_retry_attempt=true`.
///
/// This is sent by the SDK when a durable function fails and is about
/// to be retried (before the backoff delay).
#[instrument(skip(state, event), fields(
    instance_id = %event.instance_id,
    checkpoint_id = %event.checkpoint_id,
    attempt = event.attempt_number,
    error_message = ?event.error_message,
))]
pub async fn handle_retry_attempt(
    state: &InstanceHandlerState,
    event: RetryAttemptEvent,
) -> Result<()> {
    debug!(timestamp_ms = event.timestamp_ms, "Recording retry attempt");

    // Save retry attempt record for audit trail
    state
        .persistence
        .save_retry_attempt(
            &event.instance_id,
            &event.checkpoint_id,
            event.attempt_number as i32,
            event.error_message.as_deref(),
        )
        .await?;

    if let Some(ref meta) = event.error_metadata {
        info!(
            error_category = ?meta.category(),
            error_severity = ?meta.severity(),
            retry_hint = ?meta.retry_hint(),
            error_code = ?meta.error_code,
            retry_after_ms = ?meta.retry_after_ms,
            "Retry attempt with error metadata"
        );
    }

    debug!("Retry attempt recorded");

    Ok(())
}

/// Parsed sleep data from a suspended event payload.
struct SleepPayload {
    wake_at: Option<DateTime<Utc>>,
    state: Vec<u8>,
}

/// Parse sleep data from a suspended event payload.
///
/// The SDK sends JSON with:
/// - `wake_at_ms`: Unix timestamp in milliseconds for when to wake
/// - `state`: Base64-encoded checkpoint state
fn parse_sleep_payload(payload: &[u8]) -> Option<SleepPayload> {
    use base64::Engine;

    // Try to parse as JSON
    let json: serde_json::Value = serde_json::from_slice(payload).ok()?;

    // Extract wake_at_ms
    let wake_at_ms = json.get("wake_at_ms")?.as_i64()?;
    let wake_at = DateTime::from_timestamp_millis(wake_at_ms);

    // Extract and decode state
    let state_b64 = json.get("state")?.as_str()?;
    let state = base64::engine::general_purpose::STANDARD
        .decode(state_b64)
        .ok()?;

    Some(SleepPayload { wake_at, state })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::instance_handlers::mock_persistence::{MockPersistence, make_instance};
    use crate::persistence::Persistence;

    #[tokio::test]
    async fn test_handle_event_heartbeat() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventHeartbeat as i32,
            checkpoint_id: None,
            payload: Vec::new(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify event was inserted
        let events = persistence.get_events();
        assert!(!events.is_empty());
        assert_eq!(events[0].event_type, "heartbeat");
    }

    #[tokio::test]
    async fn test_handle_event_completed() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventCompleted as i32,
            checkpoint_id: None,
            payload: b"result".to_vec(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify instance was completed
        let inst = persistence.get_instance("inst-1").await.unwrap().unwrap();
        assert_eq!(inst.status, "completed");
    }

    #[tokio::test]
    async fn test_handle_event_failed() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventFailed as i32,
            checkpoint_id: None,
            payload: b"error message".to_vec(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify instance was failed
        let inst = persistence.get_instance("inst-1").await.unwrap().unwrap();
        assert_eq!(inst.status, "failed");
    }

    #[tokio::test]
    async fn test_handle_event_suspended() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventSuspended as i32,
            checkpoint_id: None,
            payload: Vec::new(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify instance was suspended
        let inst = persistence.get_instance("inst-1").await.unwrap().unwrap();
        assert_eq!(inst.status, "suspended");
    }

    #[tokio::test]
    async fn test_handle_event_custom() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventCustom as i32,
            checkpoint_id: None,
            payload: b"custom data".to_vec(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: Some("my_custom_type".to_string()),
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify event was inserted with subtype
        let events = persistence.get_events();
        assert!(!events.is_empty());
        assert_eq!(events[0].event_type, "custom");
        assert_eq!(events[0].subtype.as_deref(), Some("my_custom_type"));
    }

    #[tokio::test]
    async fn test_handle_event_suspended_with_sleep() {
        use base64::Engine;

        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        // Create sleep payload like SDK does
        let wake_at = chrono::Utc::now() + chrono::Duration::hours(1);
        let checkpoint_state = b"test checkpoint state";
        let payload = serde_json::json!({
            "wake_at_ms": wake_at.timestamp_millis(),
            "state": base64::engine::general_purpose::STANDARD.encode(checkpoint_state),
        });

        let event = InstanceEvent {
            instance_id: "inst-1".to_string(),
            event_type: InstanceEventType::EventSuspended as i32,
            checkpoint_id: Some("sleep-cp-1".to_string()),
            payload: payload.to_string().into_bytes(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            subtype: None,
        };

        let result = handle_instance_event(&state, event).await.unwrap();
        assert!(result.success);

        // Verify instance was suspended with sleep data
        let inst = persistence.get_instance("inst-1").await.unwrap().unwrap();
        assert_eq!(inst.status, "suspended");
        assert_eq!(inst.termination_reason.as_deref(), Some("sleeping"));
        assert_eq!(inst.checkpoint_id.as_deref(), Some("sleep-cp-1"));
        assert!(inst.sleep_until.is_some());

        // Verify checkpoint was saved
        let cp = persistence
            .load_checkpoint("inst-1", "sleep-cp-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cp.state, checkpoint_state);
    }

    #[test]
    fn test_parse_sleep_payload_valid() {
        use base64::Engine;

        let state = b"test state";
        let wake_at_ms = 1750000000000i64; // Some future timestamp
        let payload = serde_json::json!({
            "wake_at_ms": wake_at_ms,
            "state": base64::engine::general_purpose::STANDARD.encode(state),
        });

        let result = parse_sleep_payload(&payload.to_string().into_bytes());
        assert!(result.is_some());

        let sleep_data = result.unwrap();
        assert_eq!(sleep_data.state, state);
        assert!(sleep_data.wake_at.is_some());
        assert_eq!(sleep_data.wake_at.unwrap().timestamp_millis(), wake_at_ms);
    }

    #[test]
    fn test_parse_sleep_payload_invalid_json() {
        let result = parse_sleep_payload(b"not json");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sleep_payload_missing_fields() {
        let payload = serde_json::json!({
            "wake_at_ms": 1750000000000i64,
            // missing "state"
        });
        let result = parse_sleep_payload(&payload.to_string().into_bytes());
        assert!(result.is_none());
    }
}
