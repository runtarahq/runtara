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
use crate::persistence::{CompleteInstanceParams, EventRecord};

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
#[instrument(skip(state, event), fields(
    instance_id = %event.instance_id,
    checkpoint_id = ?event.checkpoint_id,
    event_type = ?event.event_type(),
))]
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
            // Guard with `if_running()` to prevent race condition with PID monitor:
            // if the process crashed and the PID monitor already set status to
            // "failed", we should not overwrite it with "completed" from a queued
            // SDK event.
            let mut params =
                CompleteInstanceParams::new(&event.instance_id, "completed").if_running();
            if let Some(o) = output {
                params = params.with_output(o);
            }
            let applied = state.persistence.complete_instance(params).await?;
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
            // Guard with `if_running()` to prevent race condition with PID monitor:
            // if the PID monitor already set status to "failed", don't overwrite
            // with the SDK event.
            let applied = state
                .persistence
                .complete_instance(
                    CompleteInstanceParams::new(&event.instance_id, "failed")
                        .if_running()
                        .with_error(error),
                )
                .await?;
            if applied {
                warn!(error = %error, "Instance failed");
            } else {
                warn!(error = %error, "Instance failure event skipped (already in terminal state)");
            }
        }
        InstanceEventType::EventSuspended => {
            // A suspend event carries no payload. This arm once sniffed a
            // `{wake_at_ms, state}` sleep payload out of it, but that producer
            // disappeared with the move to HTTP-only transport: durable sleep
            // now goes through the dedicated sleep endpoint, which calls
            // `set_instance_sleep` directly. Warn if a payload ever turns up so
            // a stale out-of-tree client is loud rather than silently parking
            // with no wake armed.
            if !event.payload.is_empty() {
                warn!(
                    payload_len = event.payload.len(),
                    checkpoint_id = ?event.checkpoint_id,
                    "Suspend event carried a payload; ignoring it, no wake armed"
                );
            }

            // Guard with `if_running()` to prevent race condition with the PID
            // monitor.
            let applied = state
                .persistence
                .complete_instance(
                    CompleteInstanceParams::new(&event.instance_id, "suspended").if_running(),
                )
                .await?;
            if applied {
                info!("Instance suspended");
            } else {
                warn!("Instance suspend event skipped (already in terminal state)");
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
    async fn test_handle_event_suspended_with_payload_arms_no_sleep() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence.clone());

        // The shape a stale out-of-tree client would still send. Nothing in the
        // repo produces it, and nothing consumes it any more.
        let payload = serde_json::json!({
            "wake_at_ms": (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp_millis(),
            "state": "dGVzdCBjaGVja3BvaW50IHN0YXRl",
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

        // Plain suspend: no wake armed, no "sleeping" termination, no
        // checkpoint written out of the payload.
        let inst = persistence.get_instance("inst-1").await.unwrap().unwrap();
        assert_eq!(inst.status, "suspended");
        assert!(inst.sleep_until.is_none());
        assert_ne!(inst.termination_reason.as_deref(), Some("sleeping"));
        assert!(
            persistence
                .load_checkpoint("inst-1", "sleep-cp-1")
                .await
                .unwrap()
                .is_none()
        );
    }
}
