use super::*;
use runtara_core::{
    domain::InstanceStatus,
    persistence::{
        Persistence,
        inputs::{InputAuthority, InputClosure, InputRequestSpec},
        memory::InMemoryPersistence,
    },
};
use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
use std::sync::Mutex;

#[derive(Default)]
struct RecordingChannel(Mutex<Vec<String>>);
impl Channel for RecordingChannel {
    fn send_text(
        &self,
        _conversation: &str,
        text: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        self.0.lock().unwrap().push(text.into());
        Box::pin(async { Ok(()) })
    }
}
fn message(text: &str) -> InboundMessage {
    InboundMessage {
        text: text.into(),
        sender_id: "sender".into(),
        conv_id: "conversation".into(),
        channel: "test".into(),
        attachments: vec![],
        original_message: json!({}),
        target: None,
        activity_id: None,
    }
}
async fn fixture() -> (Arc<InMemoryPersistence>, ManagedChannelInputs, String) {
    let persistence = Arc::new(InMemoryPersistence::new());
    let instance = Uuid::new_v4().to_string();
    persistence
        .register_instance(&instance, "tenant")
        .await
        .unwrap();
    persistence
        .update_instance_status(&instance, InstanceStatus::Running, None)
        .await
        .unwrap();
    let client = Arc::new(RuntimeClient::new(
        Arc::new(EnvironmentHandlerState::new(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgresql://localhost:1/unused")
                .unwrap(),
            persistence.clone(),
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )),
        crate::runtime_client::RuntimeClientConfig::new(Default::default()),
    ));
    let config = crate::valkey::ValkeyConfig::from_env().expect("isolated Valkey required");
    let conn = ConnectionManager::new(redis::Client::open(config.connection_url()).unwrap())
        .await
        .unwrap();
    let context = ManagedChannelInputs {
        client,
        conn,
        scope: QueueScope::new("tenant", &Uuid::new_v4().to_string()).unwrap(),
        prompted: Default::default(),
    };
    (persistence, context, instance)
}
async fn register(persistence: &InMemoryPersistence, instance: &str, signal: &str) -> String {
    persistence
        .input_requests()
        .unwrap()
        .register_input(
            &InputAuthority::Root {
                tenant_id: "tenant".into(),
                instance_id: instance.into(),
            },
            &InputRequestSpec {
                signal_id: signal.into(),
                deadline: None,
                response_schema: Some(json!({"name":{"type":"string","required":true}})),
                metadata: json!({"message":"Current prompt"}),
            },
        )
        .await
        .unwrap()
        .request_id
}
fn historical_event() -> Value {
    json!({"signal_id":"wait", "message":"Obsolete prompt", "response_schema":{"wrong":{"type":"boolean","required":true}}})
}

#[tokio::test]
async fn closed_terminal_and_ambiguous_history_never_prompts_or_consumes_a_reply() {
    for scenario in [
        "closed",
        "terminal",
        "failed",
        "cancelled",
        "ambiguous",
        "other-wait",
    ] {
        let (persistence, mut context, instance) = fixture().await;
        let request = register(&persistence, &instance, "wait").await;
        match scenario {
            "terminal" | "failed" | "cancelled" => persistence
                .update_instance_status(
                    &instance,
                    match scenario {
                        "failed" => InstanceStatus::Failed,
                        "cancelled" => InstanceStatus::Cancelled,
                        _ => InstanceStatus::Completed,
                    },
                    None,
                )
                .await
                .unwrap(),
            "ambiguous" => {
                register(&persistence, &instance, "other").await;
            }
            _ => {
                persistence
                    .input_requests()
                    .unwrap()
                    .close_input(
                        &InputAuthority::Root {
                            tenant_id: "tenant".into(),
                            instance_id: instance.clone(),
                        },
                        &request,
                        InputClosure::Abandoned,
                    )
                    .await
                    .unwrap();
                if scenario == "other-wait" {
                    register(&persistence, &instance, "other").await;
                }
            }
        }
        let recorded = Arc::new(RecordingChannel::default());
        let channel: Arc<dyn Channel> = recorded.clone();
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(message("must stay unread")).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            dispatch_event(
                Some("external_input_requested"),
                &historical_event(),
                &channel,
                "conversation",
            ),
        )
        .await
        .unwrap();
        assert!(recorded.0.lock().unwrap().is_empty(), "{scenario}");
        if scenario != "other-wait" {
            session_queue::push_event(
                &mut context.conn,
                context.scope.tenant_id(),
                context.scope.session_id(),
                Some(&instance),
                &json!({"message":"retain this reply"}),
            )
            .await
            .unwrap();
            assert_eq!(
                context
                    .poll(&instance, &channel, "conversation", &mut rx)
                    .await
                    .unwrap(),
                if scenario == "ambiguous" {
                    InputProgress::Ambiguous
                } else {
                    InputProgress::NoInput
                },
                "{scenario}"
            );
            let source = session_queue::peek_event(
                &mut context.conn,
                context.scope.tenant_id(),
                context.scope.session_id(),
            )
            .await
            .unwrap()
            .expect("discovery must preserve the queued reply");
            assert!(source.event.target.is_none(), "{scenario}");
            assert!(recorded.0.lock().unwrap().is_empty(), "{scenario}");
        }
        assert_eq!(rx.try_recv().unwrap().text, "must stay unread");
        assert!(matches!(
            managed::claim(&mut context.conn, &context.scope, 1000)
                .await
                .unwrap(),
            managed::ClaimOutcome::Empty
        ));
    }
}

#[tokio::test]
async fn collected_response_uses_authoritative_schema_and_retains_exact_target() {
    let (persistence, mut context, instance) = fixture().await;
    let request = register(&persistence, &instance, "wait").await;
    let recorded = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = recorded.clone();
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(message("Ada")).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        dispatch_event(
            Some("external_input_requested"),
            &historical_event(),
            &channel,
            "conversation",
        ),
    )
    .await
    .unwrap();
    assert!(
        recorded.0.lock().unwrap().is_empty(),
        "history cannot start collection"
    );
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Retained
    );
    let prompts = recorded.0.lock().unwrap().clone();
    assert_eq!(prompts[0], "Current prompt");
    assert!(
        !prompts
            .iter()
            .any(|text| text.contains("Obsolete") || text.contains("wrong"))
    );
    let managed::ClaimOutcome::Claimed(lease) =
        managed::claim(&mut context.conn, &context.scope, 30_000)
            .await
            .unwrap()
    else {
        panic!("retained reply")
    };
    assert_eq!(lease.payload().unwrap(), json!({"name":"Ada"}));
    assert_eq!(lease.target.as_ref().unwrap().instance_id, instance);
    assert_eq!(lease.target.as_ref().unwrap().request_id, request);
    let outcome = managed::deliver_claimed(
        &mut context.conn,
        &context.scope,
        &context.client,
        lease,
        None,
    )
    .await
    .unwrap();
    assert!(matches!(outcome, managed::DeliveryOutcome::Accepted(_)));
    assert_eq!(
        context
            .client
            .list_input_requests("tenant", &[instance], 0, 10)
            .await
            .unwrap()
            .total_count,
        0
    );
}

#[tokio::test]
async fn cancelled_collection_does_not_enqueue_an_empty_response() {
    let (persistence, mut context, instance) = fixture().await;
    register(&persistence, &instance, "wait").await;
    let channel: Arc<dyn Channel> = Arc::new(RecordingChannel::default());
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(message("/cancel")).await.unwrap();
    dispatch_event(
        Some("external_input_requested"),
        &historical_event(),
        &channel,
        "conversation",
    )
    .await;
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Waiting
    );
    assert!(matches!(
        managed::claim(&mut context.conn, &context.scope, 1000)
            .await
            .unwrap(),
        managed::ClaimOutcome::Empty
    ));
    assert_eq!(
        context
            .client
            .list_input_requests("tenant", &[instance], 0, 10)
            .await
            .unwrap()
            .total_count,
        1
    );
}

#[tokio::test]
async fn collector_checks_closure_before_prompt_and_after_receiving_a_field() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for close_at in [0, 2] {
        let channel = RecordingChannel::default();
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(message("late answer")).await.unwrap();
        let checks = AtomicUsize::new(0);
        let result = collector::collect_fields(
            &json!({"name":{"type":"string","required":true}}),
            &channel,
            "conversation",
            &mut rx,
            None,
            || async {
                anyhow::ensure!(
                    checks.fetch_add(1, Ordering::SeqCst) < close_at,
                    "request closed"
                );
                Ok(())
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(channel.0.lock().unwrap().len(), usize::from(close_at > 0));
        assert_eq!(rx.try_recv().is_ok(), close_at == 0);
    }
}

#[tokio::test]
async fn collector_stops_when_request_closes_without_another_inbound_message() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let channel = RecordingChannel::default();
    let (_tx, mut rx) = mpsc::channel(1);
    let checks = AtomicUsize::new(0);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        collector::collect_fields(
            &json!({"name":{"type":"string","required":true}}),
            &channel,
            "conversation",
            &mut rx,
            None,
            || async {
                anyhow::ensure!(checks.fetch_add(1, Ordering::SeqCst) < 2, "request closed");
                Ok(())
            },
        ),
    )
    .await
    .expect("collector must observe closure without waiting for a reply");
    assert!(result.is_err());
    assert_eq!(channel.0.lock().unwrap().len(), 1);
}

async fn register_plain(persistence: &InMemoryPersistence, instance: &str, signal: &str) -> String {
    persistence
        .input_requests()
        .unwrap()
        .register_input(
            &InputAuthority::Root {
                tenant_id: "tenant".into(),
                instance_id: instance.into(),
            },
            &InputRequestSpec {
                signal_id: signal.into(),
                deadline: None,
                response_schema: Some(json!({"message":{"type":"string","required":true}})),
                metadata: json!({"message":"Please answer this wait"}),
            },
        )
        .await
        .unwrap()
        .request_id
}
async fn buffer(context: &mut ManagedChannelInputs, instance: Option<&str>, text: &str) {
    session_queue::push_event(
        &mut context.conn,
        context.scope.tenant_id(),
        context.scope.session_id(),
        instance,
        &json!({"message":text}),
    )
    .await
    .unwrap();
}
async fn head(context: &mut ManagedChannelInputs) -> session_queue::PeekedEvent {
    session_queue::peek_event(
        &mut context.conn,
        context.scope.tenant_id(),
        context.scope.session_id(),
    )
    .await
    .unwrap()
    .expect("buffered reply")
}
fn managed_key(context: &ManagedChannelInputs, suffix: &str) -> String {
    use sha2::{Digest, Sha256};
    let identity =
        serde_json::to_vec(&[context.scope.tenant_id(), context.scope.session_id()]).unwrap();
    format!(
        "runtara:session:{{{:x}}}:{suffix}",
        Sha256::digest(identity)
    )
}

#[tokio::test]
async fn plain_wait_without_events_accepts_only_a_new_reply_and_keeps_its_identity() {
    let (persistence, mut context, instance) = fixture().await;
    let request = register_plain(&persistence, &instance, "plain").await;
    let recorded = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = recorded.clone();
    let (_tx, mut rx) = mpsc::channel(1);
    // No event has been emitted, and startup input is not a response to this wait.
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Waiting
    );
    assert_eq!(
        recorded.0.lock().unwrap().as_slice(),
        ["Please answer this wait"]
    );
    assert!(matches!(
        managed::claim(&mut context.conn, &context.scope, 1000)
            .await
            .unwrap(),
        managed::ClaimOutcome::Empty
    ));
    buffer(&mut context, Some(&instance), "a new reply").await;
    let original = head(&mut context).await;
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Retained
    );
    let managed::ClaimOutcome::Claimed(lease) =
        managed::claim(&mut context.conn, &context.scope, 30_000)
            .await
            .unwrap()
    else {
        panic!("bound response")
    };
    assert_eq!(lease.target.as_ref().unwrap().request_id, request);
    assert_eq!(lease.operation_id, original.event.message_id);
    assert_eq!(lease.payload().unwrap(), original.event.payload);
    assert!(matches!(
        managed::deliver_claimed(
            &mut context.conn,
            &context.scope,
            &context.client,
            lease,
            None
        )
        .await
        .unwrap(),
        managed::DeliveryOutcome::Accepted(_)
    ));
}

#[tokio::test]
async fn structured_wait_consumes_early_buffered_fields_without_debug_events() {
    let (persistence, mut context, instance) = fixture().await;
    let request = register(&persistence, &instance, "structured").await;
    buffer(&mut context, Some(&instance), "Ada").await;
    let channel: Arc<dyn Channel> = Arc::new(RecordingChannel::default());
    let (_tx, mut rx) = mpsc::channel(1);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(2),
            context.poll(&instance, &channel, "conversation", &mut rx)
        )
        .await
        .unwrap()
        .unwrap(),
        InputProgress::Retained
    );
    let managed::ClaimOutcome::Claimed(lease) =
        managed::claim(&mut context.conn, &context.scope, 1000)
            .await
            .unwrap()
    else {
        panic!("collected reply")
    };
    assert_eq!(lease.target.as_ref().unwrap().request_id, request);
    assert_eq!(lease.payload().unwrap(), json!({"name":"Ada"}));
    assert!(
        !session_queue::has_events(
            &mut context.conn,
            context.scope.tenant_id(),
            context.scope.session_id()
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn ambiguity_and_discovery_failure_leave_the_source_unconsumed() {
    use redis::AsyncCommands;
    let (persistence, mut context, instance) = fixture().await;
    register_plain(&persistence, &instance, "one").await;
    register_plain(&persistence, &instance, "two").await;
    buffer(&mut context, Some(&instance), "retain this").await;
    let original = head(&mut context).await;
    let channel: Arc<dyn Channel> = Arc::new(RecordingChannel::default());
    let (_tx, mut rx) = mpsc::channel(1);
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Ambiguous
    );
    assert!(head(&mut context).await.event.target.is_none());
    // A closed SQL pool injects a real storage error without external DB access.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgresql://localhost:1/unused")
        .unwrap();
    pool.close().await;
    context.client = Arc::new(RuntimeClient::new(
        Arc::new(EnvironmentHandlerState::new(
            pool.clone(),
            Arc::new(runtara_store_postgres::PostgresPersistence::new(pool)),
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )),
        crate::runtime_client::RuntimeClientConfig::new(Default::default()),
    ));
    assert!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .is_err()
    );
    let retained = head(&mut context).await;
    assert_eq!(retained.event.message_id, original.event.message_id);
    assert_eq!(retained.event.payload, original.event.payload);
    assert!(retained.event.target.is_none());
    let ttl: i64 = context
        .conn
        .ttl(format!(
            "queue:{}:{}",
            context.scope.tenant_id(),
            context.scope.session_id()
        ))
        .await
        .unwrap();
    assert_eq!(ttl, -1, "unresolved buffered replies must not expire");
}

#[tokio::test]
async fn failed_handoff_preserves_binding_and_cannot_retarget_after_wait_closure() {
    use redis::AsyncCommands;
    let (persistence, mut context, instance) = fixture().await;
    let request = register_plain(&persistence, &instance, "original").await;
    buffer(&mut context, Some(&instance), "retained reply").await;
    let key = managed_key(&context, "envelopes");
    context
        .conn
        .set::<_, _, ()>(&key, "invalid envelope store")
        .await
        .unwrap();
    let channel: Arc<dyn Channel> = Arc::new(RecordingChannel::default());
    let (_tx, mut rx) = mpsc::channel(1);
    assert!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .is_err()
    );
    let original = head(&mut context).await;
    assert_eq!(original.event.target.as_ref().unwrap().request_id, request);
    persistence
        .input_requests()
        .unwrap()
        .close_input(
            &InputAuthority::Root {
                tenant_id: "tenant".into(),
                instance_id: instance.clone(),
            },
            &request,
            InputClosure::Abandoned,
        )
        .await
        .unwrap();
    let next = register_plain(&persistence, &instance, "next").await;
    context.conn.del::<_, ()>(&key).await.unwrap();
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Retained
    );
    let managed::ClaimOutcome::Claimed(lease) =
        managed::claim(&mut context.conn, &context.scope, 30_000)
            .await
            .unwrap()
    else {
        panic!("original binding")
    };
    assert_eq!(lease.operation_id, original.event.message_id);
    assert_eq!(lease.target.as_ref().unwrap().request_id, request);
    assert!(matches!(
        managed::deliver_claimed(
            &mut context.conn,
            &context.scope,
            &context.client,
            lease,
            None
        )
        .await
        .unwrap(),
        managed::DeliveryOutcome::Blocked(_)
    ));
    assert_eq!(
        context
            .client
            .list_input_requests("tenant", &[instance], 0, 10)
            .await
            .unwrap()
            .requests[0]
            .request_id,
        next
    );
}

#[tokio::test]
async fn lost_buffer_ack_recovers_original_receipt_even_after_queue_retention_cleanup() {
    let (persistence, mut context, instance) = fixture().await;
    let request = register_plain(&persistence, &instance, "original").await;
    buffer(&mut context, Some(&instance), "original reply").await;
    let original = head(&mut context).await;
    let bound = session_queue::bind_event(
        &mut context.conn,
        context.scope.tenant_id(),
        context.scope.session_id(),
        &original,
        &InputTarget {
            instance_id: instance.clone(),
            request_id: request.clone(),
        },
    )
    .await
    .unwrap();
    managed::enqueue_targeted(
        &mut context.conn,
        &context.scope,
        &bound.event.message_id,
        &bound.event.message_id,
        &bound.event.payload,
        bound.event.target.as_ref().unwrap(),
    )
    .await
    .unwrap();
    let managed::DeliveryOutcome::Accepted(accepted) =
        managed::deliver_to_instance(&mut context.conn, &context.scope, &context.client, None)
            .await
            .unwrap()
    else {
        panic!("accepted before source ack loss")
    };
    let next = register_plain(&persistence, &instance, "next").await;
    // Model seven-day destination cleanup while the source still awaits its ack.
    let completed = managed_key(&context, "completed");
    redis::cmd("ZADD")
        .arg(completed)
        .arg(0)
        .arg(&bound.event.message_id)
        .query_async::<()>(&mut context.conn)
        .await
        .unwrap();
    assert_eq!(
        managed::prune_completed(&mut context.conn, &context.scope, 10)
            .await
            .unwrap(),
        1
    );
    let channel: Arc<dyn Channel> = Arc::new(RecordingChannel::default());
    let (_tx, mut rx) = mpsc::channel(1);
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Retained
    );
    let managed::DeliveryOutcome::Accepted(replayed) =
        managed::deliver_to_instance(&mut context.conn, &context.scope, &context.client, None)
            .await
            .unwrap()
    else {
        panic!("receipt replay after cleanup")
    };
    assert_eq!(replayed.receipt_id, accepted.receipt_id);
    assert_eq!(replayed.target.as_ref().unwrap().request_id, request);
    assert_eq!(
        context
            .client
            .list_input_requests("tenant", &[instance], 0, 10)
            .await
            .unwrap()
            .requests[0]
            .request_id,
        next
    );
}

#[tokio::test]
async fn buffer_ack_is_conditional_and_terminal_replies_cannot_become_startup_input() {
    let (persistence, mut context, instance) = fixture().await;
    managed::configure_route(
        &mut context.conn,
        &context.scope,
        &managed::SessionRoute {
            workflow_id: "workflow".into(),
            instance_id: instance.clone(),
        },
    )
    .await
    .unwrap();
    buffer(&mut context, Some(&instance), "identical reply").await;
    buffer(&mut context, Some(&instance), "identical reply").await;
    let first = head(&mut context).await;
    assert!(matches!(
        session_queue::take_startup_event(
            &mut context.conn,
            context.scope.tenant_id(),
            context.scope.session_id()
        )
        .await,
        Err(managed::QueueError::Conflict)
    ));
    session_queue::acknowledge_event(
        &mut context.conn,
        context.scope.tenant_id(),
        context.scope.session_id(),
        &first,
    )
    .await
    .unwrap();
    assert!(matches!(
        session_queue::acknowledge_event(
            &mut context.conn,
            context.scope.tenant_id(),
            context.scope.session_id(),
            &first
        )
        .await,
        Err(managed::QueueError::Conflict)
    ));
    let second = head(&mut context).await;
    assert_ne!(second.event.message_id, first.event.message_id);
    persistence
        .update_instance_status(&instance, InstanceStatus::Completed, None)
        .await
        .unwrap();
    assert!(!context.finish_instance(&instance).await.unwrap());
    assert!(context.finish_instance(&instance).await.unwrap());
    assert!(
        managed::has_unresolved(&mut context.conn, &context.scope)
            .await
            .unwrap()
    );
    let retained = managed::get(&mut context.conn, &context.scope, &second.event.message_id)
        .await
        .unwrap();
    assert_eq!(
        retained.payload().unwrap(),
        json!({"message":"identical reply"})
    );
    assert!(matches!(
        managed::configure_route(
            &mut context.conn,
            &context.scope,
            &managed::SessionRoute {
                workflow_id: "workflow".into(),
                instance_id: "replacement".into()
            }
        )
        .await,
        Err(managed::QueueError::Conflict)
    ));
}
