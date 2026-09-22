use super::mock_persistence::MockPersistence;
use super::*;
use crate::{
    TenantId,
    error::CoreError,
    persistence::{Persistence, memory::InMemoryPersistence},
};
use chrono::Utc;
use std::sync::{Arc, Mutex};

fn missing<T>(result: anyhow::Result<T>) {
    let Err(error) = result else {
        panic!("foreign or absent parent must fail");
    };
    assert!(matches!(
        error.downcast_ref::<CoreError>(),
        Some(CoreError::InstanceNotFound { .. })
    ));
}

fn event(id: &str) -> InstanceEvent {
    InstanceEvent {
        instance_id: id.into(),
        event_type: InstanceEventType::EventCustom as i32,
        checkpoint_id: None,
        payload: b"event".to_vec(),
        timestamp_ms: Utc::now().timestamp_millis(),
        subtype: Some("test".into()),
    }
}

#[derive(Default)]
struct Observer(Mutex<Vec<TenantId>>);
impl InstanceEventObserver for Observer {
    fn on_event_persisted(&self, tenant_id: &TenantId, _subtype: Option<&str>) {
        self.0.lock().unwrap().push(tenant_id.clone());
    }
}

#[tokio::test]
async fn handlers_cannot_read_or_mutate_another_tenant() {
    let a = TenantId::new("a").unwrap();
    let b = TenantId::new("b").unwrap();
    let persistence = Arc::new(InMemoryPersistence::new());
    let observer = Arc::new(Observer::default());
    let state =
        InstanceHandlerState::new(persistence.clone()).with_event_observer(observer.clone());
    persistence.register_instance(&b, "foreign").await.unwrap();
    persistence
        .update_instance_status(&b, "foreign", crate::domain::InstanceStatus::Running, None)
        .await
        .unwrap();
    persistence
        .save_checkpoint(&b, "foreign", "cp", b"private")
        .await
        .unwrap();
    persistence
        .insert_signal(&b, "foreign", crate::domain::SignalType::Pause, b"private")
        .await
        .unwrap();
    let command = persistence
        .get_pending_signal(&b, "foreign")
        .await
        .unwrap()
        .unwrap();
    for id in ["foreign", "absent"] {
        missing(
            handle_checkpoint(
                &state,
                &a,
                CheckpointRequest {
                    instance_id: id.into(),
                    checkpoint_id: "cp".into(),
                    state: b"stolen".to_vec(),
                },
            )
            .await,
        );
        missing(
            handle_get_checkpoint(
                &state,
                &a,
                GetCheckpointRequest {
                    instance_id: id.into(),
                    checkpoint_id: "cp".into(),
                },
            )
            .await,
        );
        missing(
            handle_sleep(
                &state,
                &a,
                SleepRequest {
                    instance_id: id.into(),
                    checkpoint_id: "cp".into(),
                    state: b"stolen".to_vec(),
                    duration_ms: 0,
                },
            )
            .await,
        );
        missing(handle_instance_event(&state, &a, event(id)).await);
        missing(handle_instance_event_with_run_label(&state, &a, event(id), None).await);
        missing(
            handle_retry_attempt(
                &state,
                &a,
                RetryAttemptEvent {
                    instance_id: id.into(),
                    checkpoint_id: "cp".into(),
                    attempt_number: 1,
                    timestamp_ms: Utc::now().timestamp_millis(),
                    error_message: None,
                    error_metadata: None,
                },
            )
            .await,
        );
        missing(
            handle_poll_signals(
                &state,
                &a,
                PollSignalsRequest {
                    instance_id: id.into(),
                    checkpoint_id: Some("cp".into()),
                },
            )
            .await,
        );
        for boolean_api in [true, false] {
            let ack = SignalAck {
                instance_id: id.into(),
                command_id: command.command_id.clone(),
                signal_type: SignalType::SignalPause as i32,
                acknowledged: true,
            };
            if boolean_api {
                missing(handle_signal_ack(&state, &a, ack).await);
            } else {
                missing(handle_signal_ack_decision(&state, &a, ack).await);
            }
        }
        let status = handle_get_instance_status(
            &state,
            &a,
            GetInstanceStatusRequest {
                instance_id: id.into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(status.status, InstanceStatus::StatusUnknown as i32);
        assert!(status.output.is_none() && status.checkpoint_id.is_none());
    }
    assert!(observer.0.lock().unwrap().is_empty());
    assert_eq!(
        persistence
            .load_checkpoint(&b, "foreign", "cp")
            .await
            .unwrap()
            .unwrap()
            .state,
        b"private"
    );
    handle_instance_event(&state, &b, event("foreign"))
        .await
        .unwrap();
    assert_eq!(*observer.0.lock().unwrap(), vec![b]);
}

#[tokio::test]
async fn registration_requires_matching_host_identity_and_tenant_local_admission() {
    let a = TenantId::new("a").unwrap();
    let b = TenantId::new("b").unwrap();
    let persistence = Arc::new(InMemoryPersistence::new());
    let state = InstanceHandlerState::with_limits(persistence.clone(), 1);
    let request = |id: &str, tenant: &TenantId| RegisterInstanceRequest {
        instance_id: id.into(),
        tenant_id: tenant.to_string(),
        checkpoint_id: None,
    };
    assert!(
        handle_register_instance(&state, &b, request("b1", &b))
            .await
            .unwrap()
            .success
    );
    // B's occupied slot must not consume A's allowance on the same handler state.
    assert!(
        handle_register_instance(&state, &a, request("a1", &a))
            .await
            .unwrap()
            .success
    );
    assert!(
        !handle_register_instance(&state, &a, request("a2", &a))
            .await
            .unwrap()
            .success
    );
    let Err(error) = handle_register_instance(&state, &a, request("new", &b)).await else {
        panic!("payload tenant must not select authority");
    };
    assert!(
        matches!(error.downcast_ref::<CoreError>(), Some(CoreError::ValidationError { field, .. }) if field == "tenant_id")
    );
    // Disable admission to exercise the global ID collision, rather than hitting the cap.
    let state = InstanceHandlerState::new(persistence.clone());
    let Err(error) = handle_register_instance(&state, &a, request("b1", &a)).await else {
        panic!("foreign ID must not be adopted");
    };
    assert!(matches!(
        error.downcast_ref::<CoreError>(),
        Some(CoreError::InstanceAlreadyExists { .. })
    ));
    assert!(persistence.get_instance(&a, "new").await.unwrap().is_none());
    assert!(persistence.get_instance(&b, "new").await.unwrap().is_none());
}

#[tokio::test]
async fn registration_does_not_hide_loss_of_parent_during_telemetry() {
    let tenant = TenantId::new("a").unwrap();
    let persistence = Arc::new(MockPersistence::new());
    persistence.set_remove_parent_before_event();
    let state = InstanceHandlerState::new(persistence.clone());
    let error = handle_register_instance(
        &state,
        &tenant,
        RegisterInstanceRequest {
            instance_id: "lost-parent".into(),
            tenant_id: tenant.to_string(),
            checkpoint_id: None,
        },
    )
    .await
    .err()
    .expect("scope loss must fail registration");
    assert!(matches!(
        error.downcast_ref::<CoreError>(),
        Some(CoreError::InstanceNotFound { .. })
    ));
    assert!(persistence.get_events().is_empty());
}

#[tokio::test]
async fn registration_and_status_preserve_storage_errors() {
    let tenant = TenantId::new("a").unwrap();
    for metadata_failure in [true, false] {
        let persistence = Arc::new(MockPersistence::new());
        if metadata_failure {
            persistence.set_fail_instance_read();
        } else {
            persistence.set_fail_count();
        }
        let state = InstanceHandlerState::with_limits(persistence.clone(), 1);
        let Err(error) = handle_register_instance(
            &state,
            &tenant,
            RegisterInstanceRequest {
                instance_id: "new".into(),
                tenant_id: tenant.to_string(),
                checkpoint_id: None,
            },
        )
        .await
        else {
            panic!("storage failure must deny registration");
        };
        assert!(matches!(
            error.downcast_ref::<CoreError>(),
            Some(CoreError::PersistenceError { .. })
        ));
        assert!(persistence.get_events().is_empty());
        if metadata_failure {
            let Err(error) = handle_get_instance_status(
                &state,
                &tenant,
                GetInstanceStatusRequest {
                    instance_id: "new".into(),
                },
            )
            .await
            else {
                panic!("storage failure must not become unknown status");
            };
            assert!(matches!(
                error.downcast_ref::<CoreError>(),
                Some(CoreError::PersistenceError { .. })
            ));
        } else {
            assert!(
                persistence
                    .get_instance(&tenant, "new")
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }
}
