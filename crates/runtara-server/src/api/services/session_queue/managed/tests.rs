use super::*;
use redis::AsyncCommands;
use serde_json::json;

async fn fixture() -> (ConnectionManager, QueueScope) {
    let config = crate::valkey::ValkeyConfig::from_env().expect("isolated Valkey required");
    let client = redis::Client::open(config.connection_url()).unwrap();
    let conn = ConnectionManager::new(client).await.unwrap();
    (
        conn,
        QueueScope::new("queue-test", &Uuid::new_v4().to_string()).unwrap(),
    )
}
async fn leased(conn: &mut ConnectionManager, scope: &QueueScope) -> Envelope {
    match claim(conn, scope, 30_000).await.unwrap() {
        ClaimOutcome::Claimed(e) => e,
        other => panic!("expected claim, got {other:?}"),
    }
}
async fn expire(conn: &mut ConnectionManager, scope: &QueueScope, e: &Envelope) {
    let mut stored = serde_json::to_value(e).unwrap();
    stored["lease_deadline_ms"] = json!(0);
    conn.hset::<_, _, _, ()>(&scope.keys()[1], &e.message_id, stored.to_string())
        .await
        .unwrap();
}
fn receipt(e: &Envelope) -> InputReceipt {
    InputReceipt {
        receipt_id: "receipt".into(),
        operation_id: e.operation_id.clone(),
        request_id: e.target.as_ref().unwrap().request_id.clone(),
        accepted_at: chrono::Utc::now(),
        payload: canonical_payload(&e.payload().unwrap()),
        acceptance_context: None,
    }
}
fn target(id: &str) -> InputTarget {
    InputTarget {
        instance_id: "instance".into(),
        request_id: id.into(),
    }
}

#[tokio::test]
async fn unresolved_responses_keep_their_existing_session_route() {
    let (mut conn, scope) = fixture().await;
    let route = SessionRoute {
        workflow_id: "workflow".into(),
        instance_id: "original".into(),
    };
    configure_route(&mut conn, &scope, &route).await.unwrap();
    enqueue(
        &mut conn,
        &scope,
        "message",
        "operation",
        &json!({"answer":true}),
    )
    .await
    .unwrap();
    configure_route(&mut conn, &scope, &route).await.unwrap();
    let replacement = SessionRoute {
        instance_id: "replacement".into(),
        ..route.clone()
    };
    assert!(matches!(
        configure_route(&mut conn, &scope, &replacement).await,
        Err(QueueError::Conflict)
    ));
    assert_eq!(
        session_route(&mut conn, &scope).await.unwrap().instance_id,
        route.instance_id
    );
    let lease = leased(&mut conn, &scope).await;
    block(&mut conn, &scope, &lease, DeliveryReason::StaleTarget)
        .await
        .unwrap();
    assert!(matches!(
        configure_route(&mut conn, &scope, &replacement).await,
        Err(QueueError::Conflict)
    ));
    fail_blocked(&mut conn, &scope, "message").await.unwrap();
    configure_route(&mut conn, &scope, &replacement)
        .await
        .unwrap();
    assert_eq!(
        get(&mut conn, &scope, "message").await.unwrap().state,
        DeliveryState::Failed
    );
}

#[tokio::test]
async fn enqueue_is_atomic_idempotent_and_preserves_large_json_numbers() {
    let (mut conn, scope) = fixture().await;
    let payload = json!({"large":u64::MAX,"array":[2,1],"nested":{"b":false,"a":null}});
    let first = enqueue(&mut conn, &scope, "message", "operation", &payload)
        .await
        .unwrap();
    let duplicate = enqueue(
        &mut conn,
        &scope,
        "message",
        "operation",
        &json!({"array":[2,1],"nested":{"a":null,"b":false},"large":u64::MAX}),
    )
    .await
    .unwrap();
    assert_eq!(first.enqueued_at_ms, duplicate.enqueued_at_ms);
    assert_eq!(duplicate.payload().unwrap(), payload);
    assert!(matches!(
        enqueue(&mut conn, &scope, "message", "operation", &json!({})).await,
        Err(QueueError::Conflict)
    ));
    assert!(matches!(
        enqueue(&mut conn, &scope, "different", "operation", &payload).await,
        Err(QueueError::Conflict)
    ));
    assert_eq!(conn.llen::<_, u64>(&scope.keys()[0]).await.unwrap(), 1);
    for key in scope.keys().into_iter().take(3) {
        assert_eq!(conn.ttl::<_, i64>(key).await.unwrap(), -1);
    }
}

#[tokio::test]
async fn two_workers_preserve_fifo_and_expired_tokens_cannot_bind_renew_or_ack() {
    let (mut conn, scope) = fixture().await;
    for id in ["one", "two"] {
        enqueue(&mut conn, &scope, id, id, &json!({"answer":true}))
            .await
            .unwrap();
    }
    let mut other = conn.clone();
    let (a, b) = tokio::join!(
        claim(&mut conn, &scope, 30_000),
        claim(&mut other, &scope, 30_000)
    );
    let first = match (a.unwrap(), b.unwrap()) {
        (ClaimOutcome::Claimed(e), ClaimOutcome::Busy)
        | (ClaimOutcome::Busy, ClaimOutcome::Claimed(e)) => e,
        result => panic!("competing claims: {result:?}"),
    };
    assert_eq!(first.message_id, "one");
    let bound = bind(&mut conn, &scope, &first, &target("wait"))
        .await
        .unwrap();
    expire(&mut conn, &scope, &bound).await;
    let recovered = leased(&mut other, &scope).await;
    assert_eq!(recovered.target, bound.target);
    assert_ne!(recovered.lease_token, bound.lease_token);
    assert!(matches!(
        bind(&mut conn, &scope, &bound, &target("other")).await,
        Err(QueueError::LeaseLost)
    ));
    assert!(matches!(
        renew(&mut conn, &scope, &bound, 30_000).await,
        Err(QueueError::LeaseLost)
    ));
    assert!(matches!(
        acknowledge(&mut conn, &scope, &bound, &receipt(&bound)).await,
        Err(QueueError::LeaseLost)
    ));
    assert!(matches!(
        bind(&mut conn, &scope, &recovered, &target("other")).await,
        Err(QueueError::Conflict)
    ));
    let accepted = acknowledge(&mut conn, &scope, &recovered, &receipt(&recovered))
        .await
        .unwrap();
    assert_eq!(accepted.state, DeliveryState::Accepted);
    assert_eq!(leased(&mut conn, &scope).await.message_id, "two");
    assert_eq!(
        enqueue(&mut conn, &scope, "one", "one", &json!({"answer":true}))
            .await
            .unwrap()
            .state,
        DeliveryState::Accepted
    );
}

#[tokio::test]
async fn blocking_head_requires_explicit_resolution_and_never_changes_a_bound_target() {
    let (mut conn, scope) = fixture().await;
    for id in ["one", "two"] {
        enqueue(&mut conn, &scope, id, id, &json!({}))
            .await
            .unwrap();
    }
    let first = leased(&mut conn, &scope).await;
    block(&mut conn, &scope, &first, DeliveryReason::AmbiguousTarget)
        .await
        .unwrap();
    assert!(matches!(
        claim(&mut conn, &scope, 30_000).await.unwrap(),
        ClaimOutcome::Blocked(_)
    ));
    resolve(&mut conn, &scope, "one", &target("chosen"))
        .await
        .unwrap();
    let bound = leased(&mut conn, &scope).await;
    assert_eq!(bound.target, Some(target("chosen")));
    block(&mut conn, &scope, &bound, DeliveryReason::StaleTarget)
        .await
        .unwrap();
    assert!(matches!(
        resolve(&mut conn, &scope, "one", &target("replacement")).await,
        Err(QueueError::Conflict)
    ));
    let failed = fail_blocked(&mut conn, &scope, "one").await.unwrap();
    assert_eq!(failed.state, DeliveryState::Failed);
    assert_eq!(leased(&mut conn, &scope).await.message_id, "two");
    assert_eq!(
        get(&mut conn, &scope, "one").await.unwrap().reason,
        Some(DeliveryReason::ExplicitFailure)
    );
}

#[tokio::test]
async fn retry_delays_use_backend_time_and_keep_original_target() {
    let (mut conn, scope) = fixture().await;
    enqueue(&mut conn, &scope, "one", "one", &json!({}))
        .await
        .unwrap();
    let first = leased(&mut conn, &scope).await;
    let first = bind(&mut conn, &scope, &first, &target("wait"))
        .await
        .unwrap();
    let waiting = retry(
        &mut conn,
        &scope,
        &first,
        DeliveryReason::BackendUnavailable,
        30_000,
    )
    .await
    .unwrap();
    assert!(waiting.retry_at_ms.unwrap() > first.enqueued_at_ms);
    assert!(matches!(
        claim(&mut conn, &scope, 30_000).await.unwrap(),
        ClaimOutcome::Deferred(_)
    ));
    let mut stored = serde_json::to_value(waiting).unwrap();
    stored["retry_at_ms"] = json!(0);
    conn.hset::<_, _, _, ()>(&scope.keys()[1], "one", stored.to_string())
        .await
        .unwrap();
    let next = leased(&mut conn, &scope).await;
    assert_eq!(next.target, first.target);
    assert_eq!(next.operation_id, first.operation_id);
    assert_eq!(next.attempts, 2);
}

#[tokio::test]
async fn corruption_is_visible_and_does_not_pop_the_message() {
    let (mut conn, scope) = fixture().await;
    enqueue(&mut conn, &scope, "one", "one", &json!({}))
        .await
        .unwrap();
    conn.hset::<_, _, _, ()>(&scope.keys()[1], "one", "broken-json")
        .await
        .unwrap();
    assert!(matches!(
        claim(&mut conn, &scope, 30_000).await,
        Err(QueueError::Corrupt)
    ));
    assert_eq!(conn.llen::<_, u64>(&scope.keys()[0]).await.unwrap(), 1);
    assert!(matches!(
        get(&mut conn, &scope, "one").await,
        Err(QueueError::Corrupt)
    ));
}

#[tokio::test]
async fn retention_prunes_only_completed_identities_in_bounded_batches() {
    let (mut conn, scope) = fixture().await;
    for id in ["one", "two", "unresolved"] {
        enqueue(&mut conn, &scope, id, id, &json!({}))
            .await
            .unwrap();
    }
    for id in ["one", "two"] {
        let e = leased(&mut conn, &scope).await;
        block(&mut conn, &scope, &e, DeliveryReason::NoTarget)
            .await
            .unwrap();
        fail_blocked(&mut conn, &scope, id).await.unwrap();
    }
    assert_eq!(prune_completed(&mut conn, &scope, 100).await.unwrap(), 0);
    for id in ["one", "two"] {
        conn.zadd::<_, _, _, ()>(&scope.keys()[3], id, 0)
            .await
            .unwrap();
    }
    assert_eq!(prune_completed(&mut conn, &scope, 1).await.unwrap(), 1);
    assert_eq!(prune_completed(&mut conn, &scope, 1).await.unwrap(), 1);
    assert!(matches!(
        get(&mut conn, &scope, "one").await,
        Err(QueueError::NotFound)
    ));
    assert_eq!(
        get(&mut conn, &scope, "unresolved").await.unwrap().state,
        DeliveryState::Queued
    );
    enqueue(&mut conn, &scope, "one", "one", &json!({}))
        .await
        .unwrap();
}

#[tokio::test]
async fn tenant_and_session_identities_cannot_inject_a_hash_slot_or_collide() {
    let (mut conn, _) = fixture().await;
    let unique = Uuid::new_v4().to_string();
    let scopes = [
        QueueScope::new(&format!("{unique}:a"), "b:{evil}").unwrap(),
        QueueScope::new(&unique, "a:b:{evil}").unwrap(),
    ];
    assert_ne!(scopes[0].prefix, scopes[1].prefix);
    for scope in &scopes {
        assert!(!scope.prefix.contains("evil"));
        assert_eq!(scope.prefix.matches('{').count(), 1);
    }
    enqueue(&mut conn, &scopes[0], "one", "one", &json!({}))
        .await
        .unwrap();
    assert!(matches!(
        claim(&mut conn, &scopes[1], 30_000).await.unwrap(),
        ClaimOutcome::Empty
    ));
}

fn runtime(
    persistence: std::sync::Arc<dyn runtara_core::persistence::Persistence>,
) -> crate::runtime_client::RuntimeClient {
    use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
    use std::sync::Arc;
    crate::runtime_client::RuntimeClient::new(
        Arc::new(EnvironmentHandlerState::new(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgresql://localhost:1/unused")
                .unwrap(),
            persistence,
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )),
        crate::runtime_client::RuntimeClientConfig::new(Default::default()),
    )
}
async fn register(
    persistence: &dyn runtara_core::persistence::Persistence,
    signal: &str,
) -> String {
    use runtara_core::persistence::inputs::{InputAuthority, InputRequestSpec};
    persistence
        .input_requests()
        .unwrap()
        .register_input(
            &InputAuthority::Root {
                tenant_id: "queue-test".into(),
                instance_id: "instance".into(),
            },
            &InputRequestSpec {
                signal_id: signal.into(),
                response_schema: Some(json!({"answer":{"type":"boolean","required":true}})),
                metadata: json!({}),
                deadline: None,
            },
        )
        .await
        .unwrap()
        .request_id
}
#[tokio::test]
async fn delivery_never_guesses_a_target_and_blocks_schema_failure_without_consuming() {
    use runtara_core::{
        domain::InstanceStatus,
        persistence::{Persistence, memory::InMemoryPersistence},
    };
    let (mut conn, scope) = fixture().await;
    let persistence = std::sync::Arc::new(InMemoryPersistence::new());
    persistence
        .register_instance("instance", "queue-test")
        .await
        .unwrap();
    persistence
        .update_instance_status("instance", InstanceStatus::Running, None)
        .await
        .unwrap();
    // Even a single open request is not a safe guess for an unbound message:
    // the sender may have been answering a request that has since closed.
    let first = register(persistence.as_ref(), "first").await;
    let client = runtime(persistence);
    enqueue(
        &mut conn,
        &scope,
        "message",
        "operation",
        &json!({"answer":"invalid"}),
    )
    .await
    .unwrap();
    let outcome = deliver_to_instance(&mut conn, &scope, &client)
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        DeliveryOutcome::Blocked(Envelope {
            reason: Some(DeliveryReason::NoTarget),
            target: None,
            ..
        })
    ));
    resolve(&mut conn, &scope, "message", &target(&first))
        .await
        .unwrap();
    let outcome = deliver_to_instance(&mut conn, &scope, &client)
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        DeliveryOutcome::Blocked(Envelope {
            reason: Some(DeliveryReason::InvalidPayload),
            ..
        })
    ));
    assert_eq!(conn.llen::<_, u64>(&scope.keys()[0]).await.unwrap(), 1);
    assert_eq!(
        get(&mut conn, &scope, "message").await.unwrap().target,
        Some(target(&first))
    );
}

#[tokio::test]
async fn delivery_after_lost_ack_replays_original_receipt_without_retargeting() {
    use runtara_core::{
        domain::InstanceStatus,
        persistence::{Persistence, memory::InMemoryPersistence},
    };
    let (mut conn, scope) = fixture().await;
    let persistence = std::sync::Arc::new(InMemoryPersistence::new());
    persistence
        .register_instance("instance", "queue-test")
        .await
        .unwrap();
    persistence
        .update_instance_status("instance", InstanceStatus::Running, None)
        .await
        .unwrap();
    let first = register(persistence.as_ref(), "first").await;
    let second = register(persistence.as_ref(), "second").await;
    let client = runtime(persistence.clone());
    enqueue(
        &mut conn,
        &scope,
        "message",
        "operation",
        &json!({"answer":true}),
    )
    .await
    .unwrap();
    let lease = leased(&mut conn, &scope).await;
    let lease = bind(&mut conn, &scope, &lease, &target(&first))
        .await
        .unwrap();
    let accepted = client
        .submit_input_response(
            "queue-test",
            "instance",
            &first,
            "operation",
            &json!({"answer":true}),
        )
        .await
        .unwrap();
    // Simulate process death after the durable receipt but before queue ack.
    expire(&mut conn, &scope, &lease).await;
    let result = deliver_to_instance(&mut conn, &scope, &client)
        .await
        .unwrap();
    let DeliveryOutcome::Accepted(envelope) = result else {
        panic!("{result:?}");
    };
    assert_eq!(envelope.receipt_id, Some(accepted.receipt_id));
    assert_eq!(envelope.target, Some(target(&first)));
    let page = client
        .list_input_requests("queue-test", &["instance".into()], 0, 10)
        .await
        .unwrap();
    assert_eq!(page.requests.len(), 1);
    assert_eq!(page.requests[0].request_id, second);
    assert!(matches!(
        claim(&mut conn, &scope, 30_000).await.unwrap(),
        ClaimOutcome::Empty
    ));
}

#[tokio::test]
async fn session_metadata_does_not_expire_independently_of_unresolved_messages() {
    let (mut conn, scope) = fixture().await;
    super::super::set_session_meta(
        &mut conn,
        scope.tenant_id(),
        scope.session_id(),
        "instance",
        "workflow",
    )
    .await
    .unwrap();
    let key = scope.keys()[4].clone();
    // An older metadata TTL must also be removed by the atomic route update.
    conn.expire::<_, ()>(&key, 3600).await.unwrap();
    super::super::set_session_meta(
        &mut conn,
        scope.tenant_id(),
        scope.session_id(),
        "replacement",
        "workflow",
    )
    .await
    .unwrap();
    assert_eq!(conn.ttl::<_, i64>(&key).await.unwrap(), -1);
    assert_eq!(
        super::super::get_session_meta(&mut conn, scope.tenant_id(), scope.session_id())
            .await
            .unwrap()
            .unwrap()
            .instance_id,
        "replacement"
    );
}

#[tokio::test]
async fn retained_queue_can_be_discovered_after_worker_restart() {
    let (mut conn, scope) = fixture().await;
    enqueue(&mut conn, &scope, "message", "operation", &json!({}))
        .await
        .unwrap();
    let config = crate::valkey::ValkeyConfig::from_env().unwrap();
    let mut recovered =
        ConnectionManager::new(redis::Client::open(config.connection_url()).unwrap())
            .await
            .unwrap();
    let mut cursor = 0;
    let mut found = false;
    loop {
        let scan = scan_scopes(&mut recovered, cursor, 1).await.unwrap();
        found |= scan.scopes.iter().any(|candidate| {
            candidate.prefix == scope.prefix
                && candidate.tenant_id() == scope.tenant_id()
                && candidate.session_id() == scope.session_id()
        });
        cursor = scan.cursor;
        if cursor == 0 {
            break;
        }
    }
    assert!(
        found,
        "retained queue must be discoverable without in-memory session state"
    );
    assert_eq!(leased(&mut recovered, &scope).await.message_id, "message");
}

#[tokio::test]
async fn corrupt_owner_is_reported_without_blocking_discovery_of_other_queues() {
    let (mut conn, scope) = fixture().await;
    enqueue(&mut conn, &scope, "message", "operation", &json!({}))
        .await
        .unwrap();
    let broken = QueueScope::new("broken", &Uuid::new_v4().to_string()).unwrap();
    conn.set::<_, _, ()>(&broken.keys()[4], "invalid-owner-type")
        .await
        .unwrap();
    let mut cursor = 0;
    let mut found = false;
    let mut reported = false;
    loop {
        let scan = scan_scopes(&mut conn, cursor, 1).await.unwrap();
        found |= scan
            .scopes
            .iter()
            .any(|candidate| candidate.prefix == scope.prefix);
        reported |= scan.corrupt_keys.contains(&broken.keys()[4]);
        cursor = scan.cursor;
        if cursor == 0 {
            break;
        }
    }
    assert!(found);
    assert!(reported);
}

#[tokio::test]
async fn selected_response_is_bound_before_claim_and_replay_cannot_change_its_target() {
    let (mut conn, scope) = fixture().await;
    let selected = target("chosen");
    let payload = json!({"answer":true});
    let retained = enqueue_targeted(
        &mut conn,
        &scope,
        "selected",
        "operation",
        &payload,
        &selected,
    )
    .await
    .unwrap();
    assert_eq!(retained.target, Some(selected.clone()));
    let lease = leased(&mut conn, &scope).await;
    assert_eq!(lease.target, Some(selected.clone()));
    // Even a claimant with another candidate cannot change explicit selection.
    assert!(matches!(
        bind(&mut conn, &scope, &lease, &target("later")).await,
        Err(QueueError::Conflict)
    ));
    acknowledge(&mut conn, &scope, &lease, &receipt(&lease))
        .await
        .unwrap();
    let replayed = enqueue_targeted(
        &mut conn,
        &scope,
        "selected",
        "operation",
        &payload,
        &selected,
    )
    .await
    .unwrap();
    assert_eq!(replayed.state, DeliveryState::Accepted);
    assert_eq!(replayed.receipt_id.as_deref(), Some("receipt"));
    for changed in [
        target("other"),
        InputTarget {
            instance_id: "other-instance".into(),
            ..selected.clone()
        },
    ] {
        assert!(matches!(
            enqueue_targeted(
                &mut conn,
                &scope,
                "selected",
                "operation",
                &payload,
                &changed
            )
            .await,
            Err(QueueError::Conflict)
        ));
    }
}

#[tokio::test]
async fn selected_enqueue_cannot_repurpose_an_existing_unbound_operation() {
    let (mut conn, scope) = fixture().await;
    let payload = json!({"message":"original"});
    enqueue(&mut conn, &scope, "message", "operation", &payload)
        .await
        .unwrap();
    assert!(matches!(
        enqueue_targeted(
            &mut conn,
            &scope,
            "message",
            "operation",
            &payload,
            &target("new")
        )
        .await,
        Err(QueueError::Conflict)
    ));
    assert!(
        get(&mut conn, &scope, "message")
            .await
            .unwrap()
            .target
            .is_none()
    );
}
