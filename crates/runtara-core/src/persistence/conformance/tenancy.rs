//! Shared-store tenant isolation requirements, reusable by every backend.
use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::TenantId;
use crate::domain::{EventType, InstanceStatus, SignalType, WakeReason};
use crate::error::CoreError;
use crate::persistence::invocations::*;
use crate::persistence::*;

fn missing<T>(result: Result<T, CoreError>) {
    assert!(matches!(result, Err(CoreError::InstanceNotFound { .. })));
}

fn rejected<T>(result: FenceResult<T>) {
    assert!(matches!(
        result,
        Err(InvocationFenceError::Rejected(FenceRejection::UnknownRoot))
    ));
}

fn vocabulary() -> EventVocabulary {
    EventVocabulary::new(EventVocabularySpec {
        start_subtype: "open",
        end_subtype: "close",
        correlation_key: "id",
        kind_key: "kind",
        label_key: "label",
        inputs_key: "in",
        outputs_key: "out",
        error_key: "error",
        error_flag_key: "failed",
        launched_at_key: "start",
        settled_at_key: "end",
    })
    .unwrap()
}

/// Every targeted operation hides foreign parents and leaves their data intact.
pub async fn targeted_operations(p: &dyn Persistence) {
    let a = TenantId::new(format!("a-{}", Uuid::new_v4())).unwrap();
    let b = TenantId::new(format!("b-{}", Uuid::new_v4())).unwrap();
    let id = Uuid::new_v4().to_string();
    let absent = Uuid::new_v4().to_string();
    assert!(
        p.try_register_instance(&b, &id, Some(b"private input"))
            .await
            .unwrap()
    );
    assert!(
        !p.try_register_instance(&b, &id, Some(b"replacement"))
            .await
            .unwrap()
    );
    assert!(matches!(
        p.try_register_instance(&a, &id, None).await,
        Err(CoreError::InstanceAlreadyExists { .. })
    ));
    assert!(matches!(
        p.register_instance(&a, &id).await,
        Err(CoreError::InstanceAlreadyExists { .. })
    ));
    p.update_instance_status(&b, &id, InstanceStatus::Running, Some(Utc::now()))
        .await
        .unwrap();
    p.save_checkpoint(&b, &id, "cp", b"private checkpoint")
        .await
        .unwrap();
    p.update_instance_checkpoint(&b, &id, "cp").await.unwrap();
    p.insert_signal(&b, &id, SignalType::Pause, b"private signal")
        .await
        .unwrap();
    let command = p.get_pending_signal(&b, &id).await.unwrap().unwrap();
    let custom = p
        .put_custom_signal(&b, &id, "cp", b"private custom signal")
        .await
        .unwrap();
    let event = EventRecord {
        id: None,
        instance_id: id.clone(),
        event_type: EventType::Custom,
        checkpoint_id: None,
        payload: Some(b"private event".to_vec()),
        created_at: Utc::now(),
        subtype: Some("open".into()),
    };
    p.insert_event(&b, &event).await.unwrap();
    let vocab = vocabulary();
    for target in [&id, &absent] {
        assert!(p.get_instance(&a, target).await.unwrap().is_none());
        assert!(p.get_instance_meta(&a, target).await.unwrap().is_none());
        missing(
            p.update_instance_status(&a, target, InstanceStatus::Failed, None)
                .await,
        );
        missing(p.update_instance_checkpoint(&a, target, "stolen").await);
        missing(p.store_instance_input(&a, target, b"stolen").await);
        missing(p.mark_instance_running(&a, target, Utc::now()).await);
        assert!(
            !p.mark_instance_started(&a, target, Utc::now())
                .await
                .unwrap()
        );
        missing(
            p.complete_instance(
                &a,
                CompleteInstanceParams::new(target, InstanceStatus::Completed),
            )
            .await,
        );
        assert!(
            !p.complete_instance(
                &a,
                CompleteInstanceParams::new(target, InstanceStatus::Completed).if_running()
            )
            .await
            .unwrap()
        );
        missing(p.save_checkpoint(&a, target, "cp", b"stolen").await);
        missing(p.load_checkpoint(&a, target, "cp").await);
        missing(
            p.list_checkpoints(&a, target, None, 10, 0, None, None)
                .await,
        );
        missing(p.count_checkpoints(&a, target, None, None, None).await);
        missing(
            p.insert_event(
                &a,
                &EventRecord {
                    instance_id: target.clone(),
                    ..event.clone()
                },
            )
            .await,
        );
        missing(
            p.list_events(&a, target, &ListEventsFilter::default(), 10, 0)
                .await,
        );
        missing(
            p.count_events(&a, target, &ListEventsFilter::default())
                .await,
        );
        missing(
            p.list_paired_records(
                &a,
                target,
                &vocab,
                &ListPairedRecordsFilter::default(),
                10,
                0,
            )
            .await,
        );
        missing(
            p.count_paired_records(&a, target, &vocab, &ListPairedRecordsFilter::default())
                .await,
        );
        missing(
            p.insert_signal(&a, target, SignalType::Cancel, b"stolen")
                .await,
        );
        missing(p.get_pending_signal(&a, target).await);
        missing(
            p.acknowledge_signal(&a, target, &command.command_id, SignalType::Pause)
                .await,
        );
        missing(
            p.apply_lifecycle_command(&a, target, &command.command_id, SignalType::Pause)
                .await,
        );
        missing(
            p.park_instance(
                &a,
                target,
                crate::lifecycle::ParkRequest {
                    reason: crate::lifecycle::ParkReason::Timer,
                    deadline: Some(Utc::now()),
                },
            )
            .await,
        );
        missing(p.put_custom_signal(&a, target, "cp", b"stolen").await);
        missing(p.get_custom_signal(&a, target, "cp").await);
        missing(
            p.save_retry_attempt(&a, target, "cp", 1, Some("stolen"))
                .await,
        );
        missing(p.set_instance_sleep(&a, target, Utc::now()).await);
        missing(
            p.schedule_wake(&a, target, Utc::now(), WakeReason::Timer)
                .await,
        );
        missing(p.clear_instance_sleep(&a, target).await);
        assert!(!p.claim_sleeping_instance(&a, target).await.unwrap());
        assert!(
            p.cancel_suspended_instances(&a, Some(target), 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            p.delete_instances_batch(&a, std::slice::from_ref(target))
                .await
                .unwrap(),
            0
        );
    }
    let record = p.get_instance(&b, &id).await.unwrap().unwrap();
    assert_eq!(record.status, InstanceStatus::Running);
    assert_eq!(record.input.as_deref(), Some(b"private input".as_slice()));
    assert_eq!(record.checkpoint_id.as_deref(), Some("cp"));
    assert_eq!(
        p.load_checkpoint(&b, &id, "cp")
            .await
            .unwrap()
            .unwrap()
            .state,
        b"private checkpoint"
    );
    assert_eq!(
        p.get_custom_signal(&b, &id, "cp")
            .await
            .unwrap()
            .unwrap()
            .signal_id,
        custom
    );
    assert_eq!(
        p.get_pending_signal(&b, &id)
            .await
            .unwrap()
            .unwrap()
            .command_id,
        command.command_id
    );
    assert_eq!(
        p.count_events(&b, &id, &ListEventsFilter::default())
            .await
            .unwrap(),
        1
    );
    assert!(
        p.load_checkpoint(&b, &id, "absent")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        p.get_custom_signal(&b, &id, "absent")
            .await
            .unwrap()
            .is_none()
    );
}

/// Lists, counters, wake claims, cancellation and retention filter before limiting.
pub async fn batches(p: &dyn Persistence) {
    let a = TenantId::new(format!("a-{}", Uuid::new_v4())).unwrap();
    let b = TenantId::new(format!("b-{}", Uuid::new_v4())).unwrap();
    let a_id = Uuid::new_v4().to_string();
    let b_id = Uuid::new_v4().to_string();
    for (tenant, id) in [(&b, &b_id), (&a, &a_id)] {
        p.register_instance(tenant, id).await.unwrap();
        p.update_instance_status(tenant, id, InstanceStatus::Running, None)
            .await
            .unwrap();
    }
    assert_eq!(p.count_active_instances(&a).await.unwrap(), 1);
    assert_eq!(p.count_active_instances(&b).await.unwrap(), 1);
    let page = p.list_instances(&a, None, 1, 0).await.unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].instance_id, a_id);
    assert!(p.list_instances(&a, None, 1, 1).await.unwrap().is_empty());
    for (tenant, id) in [(&b, &b_id), (&a, &a_id)] {
        p.update_instance_status(tenant, id, InstanceStatus::Suspended, None)
            .await
            .unwrap();
        p.set_instance_sleep(tenant, id, Utc::now() - Duration::seconds(10))
            .await
            .unwrap();
    }
    assert_eq!(
        p.get_sleeping_instances_due(&a, 1).await.unwrap()[0].instance_id,
        a_id
    );
    let retry = Utc::now() + Duration::hours(1);
    let (first, second) = tokio::join!(
        p.claim_sleeping_instances_due(&a, 1, retry),
        p.claim_sleeping_instances_due(&a, 1, retry)
    );
    let claimed: Vec<_> = first.unwrap().into_iter().chain(second.unwrap()).collect();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].instance_id, a_id);
    assert_eq!(
        p.get_sleeping_instances_due(&b, 1).await.unwrap()[0].instance_id,
        b_id
    );
    for (tenant, id) in [(&b, &b_id), (&a, &a_id)] {
        p.insert_signal(tenant, id, SignalType::Cancel, b"")
            .await
            .unwrap();
    }
    let cancelled = p.cancel_suspended_instances(&a, None, 1).await.unwrap();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0].instance_id, a_id);
    assert_eq!(
        p.get_instance(&b, &b_id).await.unwrap().unwrap().status,
        InstanceStatus::Suspended
    );
    p.complete_instance(
        &b,
        CompleteInstanceParams::new(&b_id, InstanceStatus::Completed),
    )
    .await
    .unwrap();
    let expired = p
        .get_terminal_instances_older_than(&a, Utc::now() + Duration::seconds(1), 1)
        .await
        .unwrap();
    assert_eq!(expired, vec![a_id.clone()]);
    let vocab = vocabulary();
    for (tenant, id) in [(&b, &b_id), (&a, &a_id)] {
        p.insert_event(
            tenant,
            &EventRecord {
                id: None,
                instance_id: id.clone(),
                event_type: EventType::Custom,
                checkpoint_id: None,
                payload: None,
                subtype: Some("open".into()),
                created_at: Utc::now() - Duration::days(2),
            },
        )
        .await
        .unwrap();
    }
    assert_eq!(
        p.delete_paired_events_older_than(&a, &vocab, Utc::now(), 1)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        p.count_events(&b, &b_id, &ListEventsFilter::default())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        p.delete_instances_batch(&a, &[b_id.clone(), a_id.clone(), "absent".into()])
            .await
            .unwrap(),
        1
    );
    assert!(p.get_instance(&a, &a_id).await.unwrap().is_none());
    assert!(p.get_instance(&b, &b_id).await.unwrap().is_some());
}

/// A valid lease for another tenant cannot authorize any invocation operation.
pub async fn invocation_scope(p: &dyn Persistence) {
    let a = TenantId::new(format!("a-{}", Uuid::new_v4())).unwrap();
    let b = TenantId::new(format!("b-{}", Uuid::new_v4())).unwrap();
    let id = Uuid::new_v4().to_string();
    p.register_instance(&b, &id).await.unwrap();
    p.update_instance_status(&b, &id, InstanceStatus::Running, None)
        .await
        .unwrap();
    let f = p.invocation_fences().expect("this suite requires fencing");
    let lease = f
        .claim_invocation_lease(&b, &id, "owner", None)
        .await
        .unwrap();
    let attempt = f
        .begin_invocation_attempt(&b, &lease, "child", "start")
        .await
        .unwrap();
    let checkpoint = InvocationCheckpoint {
        checkpoint_id: "cp".into(),
        state: b"stolen".to_vec(),
    };
    rejected(f.get_invocation_lease(&a, &id).await);
    rejected(f.claim_invocation_lease(&a, &id, "owner", None).await);
    rejected(f.revoke_invocation_lease(&a, &lease).await);
    rejected(
        f.begin_invocation_attempt(&a, &lease, "child", "start")
            .await,
    );
    rejected(f.cancel_invocation_attempt(&a, &attempt.fence).await);
    rejected(
        f.settle_invocation_attempt(&a, &attempt.fence, Some(&checkpoint))
            .await,
    );
    rejected(
        f.invocation_checkpoint(&a, &attempt.fence, &checkpoint)
            .await,
    );
    rejected(
        f.invocation_sleep_checkpoint(&a, &attempt.fence, &checkpoint)
            .await,
    );
    rejected(
        f.invocation_retry(
            &a,
            &attempt.fence,
            &InvocationRetry {
                checkpoint_id: "cp".into(),
                attempt_number: 1,
                error_message: None,
            },
        )
        .await,
    );
    rejected(
        f.invocation_event(
            &a,
            &attempt.fence,
            &InvocationEvent {
                kind: InvocationEventKind::Heartbeat,
                payload: vec![],
                created_at: Utc::now(),
            },
        )
        .await,
    );
    assert!(
        f.get_invocation_lease(&b, &id)
            .await
            .unwrap()
            .unwrap()
            .active
    );
    assert!(p.load_checkpoint(&b, &id, "cp").await.unwrap().is_none());
    assert_eq!(
        p.count_events(&b, &id, &ListEventsFilter::default())
            .await
            .unwrap(),
        0
    );
    assert!(
        f.invocation_checkpoint(&b, &attempt.fence, &checkpoint)
            .await
            .is_ok()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::memory::InMemoryPersistence;

    #[tokio::test]
    async fn targeted_tenant_isolation() {
        targeted_operations(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn tenant_batches_and_concurrent_claims() {
        batches(&InMemoryPersistence::new()).await;
    }
    #[tokio::test]
    async fn invocation_tenant_isolation() {
        invocation_scope(&InMemoryPersistence::new()).await;
    }
}
