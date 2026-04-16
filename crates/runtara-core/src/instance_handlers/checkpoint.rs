// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Checkpoint-related handlers: save/resume, read-only lookup, and durable
//! sleep.

use std::time::Duration;

use anyhow::Result;
use tracing::{debug, instrument};

use super::state::InstanceHandlerState;
use super::types::{
    CheckpointRequest, CheckpointResponse, CustomSignal, GetCheckpointRequest,
    GetCheckpointResponse, Signal, SignalType, SleepRequest, SleepResponse,
};
use crate::error::CoreError;
use crate::persistence::Persistence;

/// Checkpoint handler - combines save and load semantics.
///
/// - If checkpoint with this ID exists, returns the existing state (for resume)
/// - If checkpoint doesn't exist, saves the state and returns empty (fresh execution)
///
/// Also serves as heartbeat - updates instance's last activity timestamp.
/// Includes pending signal information so instance can react to cancel/pause.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_checkpoint(
    state: &InstanceHandlerState,
    request: CheckpointRequest,
) -> Result<CheckpointResponse> {
    debug!(
        state_size = request.state.len(),
        "Processing checkpoint request"
    );

    // 1. Validate instance exists and is running
    let instance = state.persistence.get_instance(&request.instance_id).await?;
    match instance {
        Some(inst) => {
            if inst.status != "running" {
                return Err(CoreError::InvalidInstanceState {
                    instance_id: request.instance_id.clone(),
                    expected: "running".to_string(),
                    actual: inst.status,
                }
                .into());
            }
        }
        None => {
            return Err(CoreError::InstanceNotFound {
                instance_id: request.instance_id.clone(),
            }
            .into());
        }
    }

    // 2. Check if checkpoint already exists
    if let Some(existing) = state
        .persistence
        .load_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?
    {
        debug!(
            checkpoint_id = %request.checkpoint_id,
            state_size = existing.state.len(),
            "Found existing checkpoint - returning for resume"
        );

        // Check for pending signal even when returning existing checkpoint
        let pending_signal =
            get_pending_signal(state.persistence.as_ref(), &request.instance_id).await;
        let custom_signal = state
            .persistence
            .take_pending_custom_signal(&request.instance_id, &request.checkpoint_id)
            .await?
            .map(|sig| CustomSignal {
                checkpoint_id: request.checkpoint_id.clone(),
                payload: sig.payload.unwrap_or_default(),
            });

        return Ok(CheckpointResponse {
            found: true,
            state: existing.state,
            pending_signal,
            custom_signal,
            last_error: None, // TODO: Fetch last error from error_history when available
        });
    }

    // 3. Checkpoint doesn't exist
    // Only save if state is non-empty. The SDK's get_checkpoint() calls this
    // endpoint with empty state as a read-only probe. If we saved empty state,
    // subsequent save attempts with real state would find the empty checkpoint
    // and return it instead of overwriting — corrupting the checkpoint permanently.
    if request.state.is_empty() {
        return Ok(CheckpointResponse {
            found: false,
            state: vec![],
            pending_signal: get_pending_signal(state.persistence.as_ref(), &request.instance_id)
                .await,
            custom_signal: None,
            last_error: None,
        });
    }

    state
        .persistence
        .save_checkpoint(&request.instance_id, &request.checkpoint_id, &request.state)
        .await?;

    // 4. Update instance's current checkpoint_id
    state
        .persistence
        .update_instance_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?;

    // 5. Check for pending signals to include in response
    let pending_signal = get_pending_signal(state.persistence.as_ref(), &request.instance_id).await;
    let custom_signal = state
        .persistence
        .take_pending_custom_signal(&request.instance_id, &request.checkpoint_id)
        .await?
        .map(|sig| CustomSignal {
            checkpoint_id: request.checkpoint_id.clone(),
            payload: sig.payload.unwrap_or_default(),
        });

    if pending_signal.is_some() || custom_signal.is_some() {
        debug!(
            ?pending_signal,
            has_custom = custom_signal.is_some(),
            "Checkpoint saved with pending signal"
        );
    } else {
        debug!("New checkpoint saved successfully");
    }

    Ok(CheckpointResponse {
        found: false,
        state: Vec::new(),
        pending_signal,
        custom_signal,
        last_error: None,
    })
}

/// Helper to get the pending instance-wide signal for an instance.
async fn get_pending_signal(persistence: &dyn Persistence, instance_id: &str) -> Option<Signal> {
    match persistence.get_pending_signal(instance_id).await {
        Ok(Some(signal)) => {
            let signal_type = match signal.signal_type.as_str() {
                "cancel" => SignalType::SignalCancel,
                "pause" => SignalType::SignalPause,
                "resume" => SignalType::SignalResume,
                "shutdown" => SignalType::SignalShutdown,
                _ => return None,
            };
            Some(Signal {
                instance_id: instance_id.to_string(),
                signal_type: signal_type.into(),
                payload: signal.payload.unwrap_or_default(),
            })
        }
        _ => None,
    }
}

/// Get checkpoint handler - read-only lookup without saving.
///
/// Returns the checkpoint state if found, or empty if not found.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_get_checkpoint(
    state: &InstanceHandlerState,
    request: GetCheckpointRequest,
) -> Result<GetCheckpointResponse> {
    debug!("Looking up checkpoint (read-only)");

    // 1. Validate instance exists
    let instance = state.persistence.get_instance(&request.instance_id).await?;
    if instance.is_none() {
        return Err(CoreError::InstanceNotFound {
            instance_id: request.instance_id.clone(),
        }
        .into());
    }

    // 2. Look up checkpoint
    if let Some(checkpoint) = state
        .persistence
        .load_checkpoint(&request.instance_id, &request.checkpoint_id)
        .await?
    {
        debug!(
            checkpoint_id = %request.checkpoint_id,
            state_size = checkpoint.state.len(),
            "Checkpoint found"
        );
        return Ok(GetCheckpointResponse {
            found: true,
            state: checkpoint.state,
        });
    }

    debug!(checkpoint_id = %request.checkpoint_id, "Checkpoint not found");
    Ok(GetCheckpointResponse {
        found: false,
        state: Vec::new(),
    })
}

/// Handle durable sleep request.
///
/// Saves the checkpoint state before sleeping, then sleeps in-process.
/// This ensures the state is durable and can be restored if the process
/// is killed during the sleep.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_sleep(
    state: &InstanceHandlerState,
    request: SleepRequest,
) -> Result<SleepResponse> {
    debug!(
        duration_ms = request.duration_ms,
        state_size = request.state.len(),
        "Processing sleep request"
    );

    // 1. Save checkpoint before sleeping (for durability)
    if !request.checkpoint_id.is_empty() {
        state
            .persistence
            .save_checkpoint(&request.instance_id, &request.checkpoint_id, &request.state)
            .await?;

        // Update instance's current checkpoint_id
        state
            .persistence
            .update_instance_checkpoint(&request.instance_id, &request.checkpoint_id)
            .await?;

        debug!(checkpoint_id = %request.checkpoint_id, "Sleep checkpoint saved");
    }

    // 2. Sleep in-process; environment may hibernate managed instances separately.
    tokio::time::sleep(Duration::from_millis(request.duration_ms)).await;
    Ok(SleepResponse {})
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;

    use super::*;
    use crate::instance_handlers::mock_persistence::{
        MockPersistence, make_checkpoint, make_instance, make_signal,
    };
    use crate::persistence::CustomSignalRecord;

    #[tokio::test]
    async fn test_checkpoint_instance_not_found() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "nonexistent".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_checkpoint_instance_not_running() {
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "tenant-1",
            "completed",
        )));
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_checkpoint_new_saves_state() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(!result.found); // New checkpoint, not found
    }

    #[tokio::test]
    async fn test_checkpoint_existing_returns_state() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_checkpoint(make_checkpoint("inst-1", "cp-1", b"existing state")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"new state".to_vec(), // This should be ignored
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.found);
        assert_eq!(result.state, b"existing state");
    }

    #[tokio::test]
    async fn test_checkpoint_returns_pending_signal() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_signal(make_signal("inst-1", "cancel")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.pending_signal.is_some());
        let signal = result.pending_signal.unwrap();
        assert_eq!(signal.signal_type, SignalType::SignalCancel as i32);
    }

    #[tokio::test]
    async fn test_checkpoint_returns_custom_signal() {
        let custom_signal = CustomSignalRecord {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            payload: Some(b"custom payload".to_vec()),
            created_at: Utc::now(),
        };
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "running"))
                .with_custom_signal(custom_signal),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = CheckpointRequest {
            instance_id: "inst-1".to_string(),
            checkpoint_id: "cp-1".to_string(),
            state: b"test state".to_vec(),
        };

        let result = handle_checkpoint(&state, request).await.unwrap();
        assert!(result.custom_signal.is_some());
        let cs = result.custom_signal.unwrap();
        assert_eq!(cs.checkpoint_id, "cp-1");
        assert_eq!(cs.payload, b"custom payload");
    }
}
