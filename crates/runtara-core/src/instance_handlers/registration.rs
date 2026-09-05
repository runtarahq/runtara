// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Instance registration handler.

use crate::domain::InstanceStatus as CoreInstanceStatus;

use anyhow::Result;
use chrono::Utc;
use tracing::{debug, info, instrument, warn};

use super::state::InstanceHandlerState;
use super::types::{
    ERROR_MAX_CONCURRENT_INSTANCES, ERROR_SERVER_DRAINING, RegisterInstanceRequest,
    RegisterInstanceResponse,
};
use crate::error::CoreError;
use crate::persistence::EventRecord;

/// Handle instance registration request.
///
/// Registers an instance with Core, optionally resuming from a checkpoint.
/// If the instance doesn't exist, it's created (self-registration).
///
/// # Errors
///
/// Two distinct channels, because the caller has to tell them apart:
///
/// A `success: false` response means the caller cannot register right now and
/// the fault is theirs or the cluster's — empty `instance_id`/`tenant_id`, the
/// core draining, the concurrency cap. The HTTP layer renders these as 4xx (or
/// the 503/429 the drain and cap sentinels ask for).
///
/// An `Err` carries a [`CoreError`] and lets the HTTP layer pick the status from
/// it: `CheckpointNotFound` for a resume naming a checkpoint that isn't there
/// (404, matching what the checkpoint route answers for the same fact), and the
/// persistence errors for a read or write that failed (503, because that is the
/// server's problem and is worth retrying). Reporting a failed write as
/// `success: false` would instead tell the caller its request was wrong.
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
        .get_instance_meta(&request.instance_id)
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
        // A read that fails is the server's problem, propagated for the same
        // reason as the writes below: reporting it as a refusal would tell a
        // resuming caller its request was wrong and stop it retrying a blip.
        // A checkpoint that is genuinely absent is the caller's problem, and
        // carries the error the checkpoint route already uses for it so both
        // routes answer the same status for the same fact.
        match state
            .persistence
            .load_checkpoint(&request.instance_id, cp_id)
            .await?
        {
            Some(_) => {
                debug!(checkpoint_id = %cp_id, "Checkpoint found for resume");
            }
            None => {
                return Err(CoreError::CheckpointNotFound {
                    instance_id: request.instance_id.clone(),
                    checkpoint_id: Some(cp_id.clone()),
                }
                .into());
            }
        }
    }

    // 5. Enforce RUNTARA_MAX_CONCURRENT_INSTANCES for fresh registrations.
    //    Resumes are allowed past the cap, and the count behind it covers
    //    `running` only — a suspended instance holds no slot while parked.
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
        // Propagate rather than reporting `success: false`. The response's error
        // string is the caller's-fault channel — drain, cap, bad input — and the
        // HTTP layer answers it with a 4xx. A persistence failure is the server's
        // fault and must reach the caller as a 5xx it can retry, which only the
        // `CoreError` carries.
        state
            .persistence
            .register_instance(&request.instance_id, &request.tenant_id)
            .await?;
    }

    // 5. Update instance status to RUNNING
    let started_at = Utc::now();
    // Propagated for the same reason as the registration write above.
    state
        .persistence
        .update_instance_status(
            &request.instance_id,
            CoreInstanceStatus::Running,
            Some(started_at),
        )
        .await?;

    // 6. Insert started event
    let event = EventRecord {
        id: None,
        instance_id: request.instance_id.clone(),
        event_type: crate::domain::EventType::Started,
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
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "tenant-1",
            CoreInstanceStatus::Pending,
        )));
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
                .with_instance(make_instance(
                    "inst-1",
                    "tenant-1",
                    CoreInstanceStatus::Pending,
                ))
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
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "tenant-1",
            CoreInstanceStatus::Pending,
        )));
        let state = InstanceHandlerState::new(persistence);

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: Some("nonexistent".to_string()),
        };

        let err = match handle_register_instance(&state, request).await {
            Ok(_) => panic!("a missing checkpoint must not report success: false"),
            Err(e) => e,
        };
        // Carries the same error the checkpoint route raises, so both answer 404.
        assert!(matches!(
            err.downcast_ref::<CoreError>(),
            Some(CoreError::CheckpointNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn a_failed_registration_write_is_an_error_not_a_refusal() {
        // `success: false` means "the caller cannot register" — drain, cap, bad
        // input — and the HTTP layer answers those with a 4xx. A write that
        // failed is the server's problem, so it has to surface as an error the
        // caller is told to retry, not as a refusal it is told to accept.
        let persistence = MockPersistence::new();
        persistence.set_fail_register();
        let state = InstanceHandlerState::new(Arc::new(persistence));

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let err = match handle_register_instance(&state, request).await {
            Ok(_) => panic!("a failed persistence write must not report success: false"),
            Err(e) => e,
        };
        assert!(matches!(
            err.downcast_ref::<CoreError>(),
            Some(CoreError::DatabaseError { .. })
        ));
    }

    #[tokio::test]
    async fn a_failed_status_update_is_an_error_not_a_refusal() {
        let persistence = MockPersistence::new();
        persistence.set_fail_status_update();
        let state = InstanceHandlerState::new(Arc::new(persistence));

        let request = RegisterInstanceRequest {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            checkpoint_id: None,
        };

        let err = match handle_register_instance(&state, request).await {
            Ok(_) => panic!("a failed status update must not report success: false"),
            Err(e) => e,
        };
        assert!(matches!(
            err.downcast_ref::<CoreError>(),
            Some(CoreError::DatabaseError { .. })
        ));
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
        assert_eq!(events[0].event_type, crate::domain::EventType::Started);
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
        let persistence = Arc::new(MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "tenant-1",
            CoreInstanceStatus::Running,
        )));
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
