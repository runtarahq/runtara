// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance registration handler.

use anyhow::Result;
use chrono::Utc;
use tracing::{debug, info, instrument, warn};

use super::state::InstanceHandlerState;
use super::types::{
    ERROR_MAX_CONCURRENT_INSTANCES, ERROR_SERVER_DRAINING, RegisterInstanceRequest,
    RegisterInstanceResponse,
};
use crate::persistence::EventRecord;

/// Handle instance registration request.
///
/// Registers an instance with Core, optionally resuming from a checkpoint.
/// If the instance doesn't exist, it's created (self-registration).
///
/// # Errors
///
/// Returns an error response if:
/// - `instance_id` or `tenant_id` is empty
/// - A specified `checkpoint_id` doesn't exist
#[instrument(skip(state, request), fields(
    instance_id = %request.instance_id,
    tenant_id = %request.tenant_id,
    checkpoint_id = ?request.checkpoint_id,
))]
pub async fn handle_register_instance(
    state: &InstanceHandlerState,
    request: RegisterInstanceRequest,
) -> Result<RegisterInstanceResponse> {
    info!(
        tenant_id = %request.tenant_id,
        resuming_from = ?request.checkpoint_id,
        "Instance registering"
    );

    // 1. Validate instance_id is not empty
    if request.instance_id.is_empty() {
        return Ok(RegisterInstanceResponse {
            success: false,
            error: "instance_id is required".to_string(),
        });
    }

    // 2. Validate tenant_id is not empty
    if request.tenant_id.is_empty() {
        return Ok(RegisterInstanceResponse {
            success: false,
            error: "tenant_id is required".to_string(),
        });
    }

    // 3. Refuse new registrations when the core is draining. Existing instances
    //    (which already have a row in persistence) can still resume.
    let instance_exists = state
        .persistence
        .get_instance(&request.instance_id)
        .await
        .map(|opt| opt.is_some())
        .unwrap_or(false);

    if !instance_exists && state.is_draining() {
        info!("Refusing registration: server draining");
        return Ok(RegisterInstanceResponse {
            success: false,
            error: ERROR_SERVER_DRAINING.to_string(),
        });
    }

    // 4. If checkpoint_id provided, verify it exists
    if let Some(ref cp_id) = request.checkpoint_id {
        let checkpoint = state
            .persistence
            .load_checkpoint(&request.instance_id, cp_id)
            .await;
        match checkpoint {
            Ok(Some(_)) => {
                debug!(checkpoint_id = %cp_id, "Checkpoint found for resume");
            }
            Ok(None) => {
                return Ok(RegisterInstanceResponse {
                    success: false,
                    error: format!("Checkpoint '{}' not found", cp_id),
                });
            }
            Err(e) => {
                return Ok(RegisterInstanceResponse {
                    success: false,
                    error: format!("Failed to verify checkpoint: {}", e),
                });
            }
        }
    }

    // 5. Enforce RUNTARA_MAX_CONCURRENT_INSTANCES for fresh registrations.
    //    Resumes are allowed past the cap (they already consumed a slot).
    if !instance_exists && state.max_concurrent_instances > 0 {
        match state.persistence.count_active_instances().await {
            Ok(active) if active >= state.max_concurrent_instances as i64 => {
                warn!(
                    active,
                    limit = state.max_concurrent_instances,
                    "Refusing registration: max concurrent instances reached"
                );
                return Ok(RegisterInstanceResponse {
                    success: false,
                    error: ERROR_MAX_CONCURRENT_INSTANCES.to_string(),
                });
            }
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, "Failed to count active instances; allowing registration");
            }
        }
    }

    if !instance_exists {
        // Self-registration: create instance record
        info!("Instance not found, creating self-registered instance");
        if let Err(e) = state
            .persistence
            .register_instance(&request.instance_id, &request.tenant_id)
            .await
        {
            return Ok(RegisterInstanceResponse {
                success: false,
                error: format!("Failed to create instance: {}", e),
            });
        }
    }

    // 5. Update instance status to RUNNING
    let started_at = Utc::now();
    if let Err(e) = state
        .persistence
        .update_instance_status(&request.instance_id, "running", Some(started_at))
        .await
    {
        return Ok(RegisterInstanceResponse {
            success: false,
            error: format!("Failed to update instance status: {}", e),
        });
    }

    // 6. Insert started event
    let event = EventRecord {
        id: None,
        instance_id: request.instance_id.clone(),
        event_type: "started".to_string(),
        checkpoint_id: request.checkpoint_id.clone(),
        payload: None,
        created_at: started_at,
        subtype: None,
    };
    if let Err(e) = state.persistence.insert_event(&event).await {
        warn!("Failed to insert started event: {}", e);
        // Don't fail registration just because event logging failed
    }

    info!("Instance registered successfully");

    Ok(RegisterInstanceResponse {
        success: true,
        error: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::instance_handlers::mock_persistence::{
        MockPersistence, make_checkpoint, make_instance,
    };

    #[tokio::test]
    async fn test_register_empty_instance_id() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(!result.success);
        assert!(result.error.contains("instance_id is required"));
    }

    #[tokio::test]
    async fn test_register_empty_tenant_id() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(!result.success);
        assert!(result.error.contains("tenant_id is required"));
    }

    #[tokio::test]
    async fn test_register_self_registration() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-new".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(result.success);
        assert!(result.error.is_empty());
    }

    #[tokio::test]
    async fn test_register_existing_instance() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "pending")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_register_with_valid_checkpoint() {
        let persistence = Arc::new(
            MockPersistence::new()
                .with_instance(make_instance("inst-1", "tenant-1", "pending"))
                .with_checkpoint(make_checkpoint("inst-1", "cp-1", b"state")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: Some("cp-1".to_string()),
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_register_with_invalid_checkpoint() {
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "pending")),
        );
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: Some("nonexistent".to_string()),
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(!result.success);
        assert!(result.error.contains("not found"));
    }

    #[tokio::test]
    async fn test_register_creates_started_event() {
        let mock = MockPersistence::new();
        let persistence = Arc::new(mock);
        let state = InstanceHandlerState::new(persistence.clone());

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let result = handle_register_instance(&state, request).await.unwrap();
        assert!(result.success);

        // Check that started event was created
        let events = persistence.get_events();
        assert!(!events.is_empty());
        assert_eq!(events[0].event_type, "started");
        assert_eq!(events[0].instance_id, "inst-1");
    }

    #[tokio::test]
    async fn test_register_rejected_when_draining() {
        let persistence = Arc::new(MockPersistence::new());
        let state = InstanceHandlerState::new(persistence);
        state.draining.store(true, Ordering::SeqCst);

        let request = RegisterInstanceRequest {
            instance_id: "new-inst".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let resp = handle_register_instance(&state, request).await.unwrap();
        assert!(!resp.success);
        assert_eq!(resp.error, ERROR_SERVER_DRAINING);
    }

    #[tokio::test]
    async fn test_register_existing_instance_allowed_during_drain() {
        // Existing (resuming) instances must still be able to register — we only
        // want to keep out fresh work.
        let persistence = Arc::new(
            MockPersistence::new().with_instance(make_instance("inst-1", "tenant-1", "running")),
        );
        let state = InstanceHandlerState::new(persistence);
        state.draining.store(true, Ordering::SeqCst);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let resp = handle_register_instance(&state, request).await.unwrap();
        assert!(resp.success, "drain should not block resuming instances");
    }

    #[tokio::test]
    async fn test_register_rejected_when_max_concurrent_reached() {
        let persistence = Arc::new(MockPersistence::new().with_active_count(32));
        let state = InstanceHandlerState::with_limits(persistence, 32);

        let request = RegisterInstanceRequest {
            instance_id: "new-inst".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let resp = handle_register_instance(&state, request).await.unwrap();
        assert!(!resp.success);
        assert_eq!(resp.error, ERROR_MAX_CONCURRENT_INSTANCES);
    }

    #[tokio::test]
    async fn test_register_under_cap_allowed() {
        let persistence = Arc::new(MockPersistence::new().with_active_count(5));
        let state = InstanceHandlerState::with_limits(persistence, 32);

        let request = RegisterInstanceRequest {
            instance_id: "new-inst".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let resp = handle_register_instance(&state, request).await.unwrap();
        assert!(resp.success);
    }
}
