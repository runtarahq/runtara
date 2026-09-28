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
        intake_id: None,
        workflow: None,
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
        intake: None,
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
        if scenario == "ambiguous" {
            // Nothing binds a channel reply while several inputs are open.
            assert_eq!(
                context.reply_target(&instance).await.unwrap(),
                ReplyTarget::Ambiguous
            );
            assert_eq!(
                context
                    .poll(&instance, &channel, "conversation", &mut rx)
                    .await
                    .unwrap(),
                InputProgress::Ambiguous
            );
        } else if scenario != "other-wait" {
            // A reply received for this request while it was open is dropped
            // once it closes, never retained for a later request.
            buffer(&mut context, &instance, Some(&request), "late reply").await;
            assert_eq!(
                context
                    .poll(&instance, &channel, "conversation", &mut rx)
                    .await
                    .unwrap(),
                InputProgress::Undelivered,
                "{scenario}"
            );
            assert!(peek(&mut context, &instance).await.is_none(), "{scenario}");
        }
        assert!(recorded.0.lock().unwrap().is_empty(), "{scenario}");
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
    let outcome =
        managed::deliver_claimed(&mut context.conn, &context.scope, &context.client, lease)
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
/// Buffer a reply as the actor does on arrival, bound to the request it answers.
async fn buffer(
    context: &mut ManagedChannelInputs,
    instance: &str,
    for_request: Option<&str>,
    text: &str,
) {
    session_queue::push_event(
        &mut context.conn,
        context.scope.tenant_id(),
        context.scope.session_id(),
        Some(instance),
        for_request,
        None,
        &json!({"message":text}),
    )
    .await
    .unwrap();
}
async fn peek(
    context: &mut ManagedChannelInputs,
    instance: &str,
) -> Option<session_queue::PeekedEvent> {
    session_queue::peek_event(
        &mut context.conn,
        context.scope.tenant_id(),
        context.scope.session_id(),
        Some(instance),
    )
    .await
    .unwrap()
}
async fn head(context: &mut ManagedChannelInputs, instance: &str) -> session_queue::PeekedEvent {
    peek(context, instance).await.expect("buffered reply")
}
fn close(
    persistence: &InMemoryPersistence,
    instance: &str,
    request: &str,
) -> impl std::future::Future<Output = ()> {
    let inputs = persistence.input_requests().unwrap();
    let authority = InputAuthority::Root {
        tenant_id: "tenant".into(),
        instance_id: instance.into(),
    };
    let request = request.to_owned();
    async move {
        inputs
            // Expired only applies once a deadline passes; closing without a
            // deadline models the same unanswered end of the wait.
            .close_input(&authority, &request, InputClosure::Abandoned)
            .await
            .unwrap();
    }
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
    assert_eq!(
        context.reply_target(&instance).await.unwrap(),
        ReplyTarget::Request(request.clone())
    );
    buffer(&mut context, &instance, Some(&request), "a new reply").await;
    let original = head(&mut context, &instance).await;
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
        managed::deliver_claimed(&mut context.conn, &context.scope, &context.client, lease)
            .await
            .unwrap(),
        managed::DeliveryOutcome::Accepted(_)
    ));
}

#[tokio::test]
async fn structured_wait_consumes_early_buffered_fields_without_debug_events() {
    let (persistence, mut context, instance) = fixture().await;
    let request = register(&persistence, &instance, "structured").await;
    buffer(&mut context, &instance, Some(&request), "Ada").await;
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
async fn a_reply_bound_while_one_input_was_open_survives_ambiguity_and_discovery_failure() {
    use redis::AsyncCommands;
    let (persistence, mut context, instance) = fixture().await;
    let one = register_plain(&persistence, &instance, "one").await;
    buffer(&mut context, &instance, Some(&one), "retain this").await;
    let original = head(&mut context, &instance).await;
    register_plain(&persistence, &instance, "two").await;
    let channel: Arc<dyn Channel> = Arc::new(RecordingChannel::default());
    let (_tx, mut rx) = mpsc::channel(1);
    // A closed SQL pool injects a real storage error without external DB access.
    let working = context.client.clone();
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
    let retained = head(&mut context, &instance).await;
    assert_eq!(retained.event.message_id, original.event.message_id);
    assert_eq!(retained.event.payload, original.event.payload);
    assert!(retained.event.target.is_none());
    let ttl: i64 = context
        .conn
        .ttl(format!(
            "queue:{}:{}:{}",
            context.scope.tenant_id(),
            context.scope.session_id(),
            instance
        ))
        .await
        .unwrap();
    assert!(ttl > 0, "orphaned reply buffers need a cleanup backstop");
    // A second open request does not make the earlier reply ambiguous: it was
    // bound to `one` when it arrived and is delivered there only.
    context.client = working;
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
        panic!("bound reply")
    };
    assert_eq!(lease.target.unwrap().request_id, one);
}

#[tokio::test]
async fn failed_handoff_preserves_binding_and_cannot_retarget_after_wait_closure() {
    use redis::AsyncCommands;
    let (persistence, mut context, instance) = fixture().await;
    let request = register_plain(&persistence, &instance, "original").await;
    buffer(&mut context, &instance, Some(&request), "retained reply").await;
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
    let original = head(&mut context, &instance).await;
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
        managed::deliver_claimed(&mut context.conn, &context.scope, &context.client, lease)
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
    buffer(&mut context, &instance, Some(&request), "original reply").await;
    let original = head(&mut context, &instance).await;
    let bound = session_queue::bind_event(
        &mut context.conn,
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
        managed::deliver_to_instance(&mut context.conn, &context.scope, &context.client)
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
        managed::deliver_to_instance(&mut context.conn, &context.scope, &context.client)
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
async fn buffer_ack_is_conditional_and_terminal_replies_are_reported_not_rerouted() {
    let (persistence, mut context, instance) = fixture().await;
    let request = register_plain(&persistence, &instance, "wait").await;
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
    buffer(&mut context, &instance, Some(&request), "identical reply").await;
    buffer(&mut context, &instance, Some(&request), "identical reply").await;
    let first = head(&mut context, &instance).await;
    // Replies live apart from startup input and can never start a run.
    assert!(
        session_queue::take_startup_event(
            &mut context.conn,
            context.scope.tenant_id(),
            context.scope.session_id()
        )
        .await
        .unwrap()
        .is_none()
    );
    session_queue::acknowledge_event(&mut context.conn, &first)
        .await
        .unwrap();
    assert!(matches!(
        session_queue::acknowledge_event(&mut context.conn, &first).await,
        Err(managed::QueueError::Conflict)
    ));
    let second = head(&mut context, &instance).await;
    assert_ne!(second.event.message_id, first.event.message_id);
    persistence
        .update_instance_status(&instance, InstanceStatus::Completed, None)
        .await
        .unwrap();
    let recorded = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = recorded.clone();
    assert!(
        !context
            .finish_instance(&instance, &channel, "conversation")
            .await
            .unwrap()
    );
    assert!(
        context
            .finish_instance(&instance, &channel, "conversation")
            .await
            .unwrap()
    );
    // The unbound reply is reported undeliverable, not left for a later run.
    assert_eq!(recorded.0.lock().unwrap().as_slice(), [UNDELIVERED_NOTICE]);
    assert!(
        !managed::has_unresolved(&mut context.conn, &context.scope)
            .await
            .unwrap()
    );
    managed::configure_route(
        &mut context.conn,
        &context.scope,
        &managed::SessionRoute {
            workflow_id: "workflow".into(),
            instance_id: "replacement".into(),
        },
    )
    .await
    .unwrap();
}

/// The audited bug: a late reply to a timed-out request must not be accepted
/// as the answer to the next request, whose prompt the user never saw.
#[tokio::test]
async fn late_reply_to_a_closed_request_is_never_applied_to_the_next_request() {
    let (persistence, mut context, instance) = fixture().await;
    let recorded = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = recorded.clone();
    let (_tx, mut rx) = mpsc::channel(1);
    let approve = register_plain(&persistence, &instance, "approve-deploy").await;
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Waiting
    );
    // "yes" arrives while R0 is open, then R0 times out before delivery.
    let ReplyTarget::Request(answered) = context.reply_target(&instance).await.unwrap() else {
        panic!("the prompted request is the reply's target")
    };
    assert_eq!(answered, approve);
    buffer(&mut context, &instance, Some(&answered), "yes").await;
    close(&persistence, &instance, &approve).await;
    let delete = register_plain(&persistence, &instance, "delete-old-data").await;
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Undelivered
    );
    assert!(matches!(
        managed::claim(&mut context.conn, &context.scope, 1000)
            .await
            .unwrap(),
        managed::ClaimOutcome::Empty
    ));
    // R1 is still open and is now prompted, instead of silently answered.
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Waiting
    );
    let page = context
        .client
        .list_input_requests("tenant", std::slice::from_ref(&instance), 0, 10)
        .await
        .unwrap();
    assert_eq!(page.requests.len(), 1);
    assert_eq!(page.requests[0].request_id, delete);
    assert_eq!(
        recorded.0.lock().unwrap().len(),
        2,
        "both prompts were sent"
    );
}

#[tokio::test]
async fn a_reply_needs_a_prompted_open_request_when_it_arrives() {
    let (persistence, mut context, instance) = fixture().await;
    assert_eq!(
        context.reply_target(&instance).await.unwrap(),
        ReplyTarget::NotWaiting
    );
    // Open, but its prompt has not been sent yet: the sender cannot be answering it.
    let request = register_plain(&persistence, &instance, "wait").await;
    assert_eq!(
        context.reply_target(&instance).await.unwrap(),
        ReplyTarget::NotWaiting
    );
    let channel: Arc<dyn Channel> = Arc::new(RecordingChannel::default());
    let (_tx, mut rx) = mpsc::channel(1);
    context
        .poll(&instance, &channel, "conversation", &mut rx)
        .await
        .unwrap();
    assert_eq!(
        context.reply_target(&instance).await.unwrap(),
        ReplyTarget::Request(request)
    );
}

#[tokio::test]
async fn startup_messages_and_execution_replies_never_block_each_other() {
    let (persistence, mut context, instance) = fixture().await;
    let request = register_plain(&persistence, &instance, "wait").await;
    session_queue::push_event(
        &mut context.conn,
        context.scope.tenant_id(),
        context.scope.session_id(),
        None,
        None,
        None,
        &json!({"message":"start another run"}),
    )
    .await
    .unwrap();
    buffer(&mut context, &instance, Some(&request), "answer").await;
    let channel: Arc<dyn Channel> = Arc::new(RecordingChannel::default());
    let (_tx, mut rx) = mpsc::channel(1);
    assert_eq!(
        context
            .poll(&instance, &channel, "conversation", &mut rx)
            .await
            .unwrap(),
        InputProgress::Retained
    );
    assert_eq!(
        session_queue::take_startup_event(
            &mut context.conn,
            context.scope.tenant_id(),
            context.scope.session_id()
        )
        .await
        .unwrap(),
        Some(json!({"message":"start another run"}))
    );
}

#[tokio::test]
async fn a_retained_reply_whose_request_closed_is_failed_and_reported_while_idle() {
    let (persistence, mut context, instance) = fixture().await;
    let request = register_plain(&persistence, &instance, "wait").await;
    managed::enqueue_targeted(
        &mut context.conn,
        &context.scope,
        "late",
        "late",
        &json!({"message":"late"}),
        &InputTarget {
            instance_id: instance.clone(),
            request_id: request.clone(),
        },
    )
    .await
    .unwrap();
    close(&persistence, &instance, &request).await;
    let recorded = Arc::new(RecordingChannel::default());
    let channel: Arc<dyn Channel> = recorded.clone();
    assert!(
        !context
            .settle_queue(&channel, "conversation")
            .await
            .unwrap()
    );
    assert_eq!(recorded.0.lock().unwrap().as_slice(), [UNDELIVERED_NOTICE]);
    assert!(
        !managed::has_unresolved(&mut context.conn, &context.scope)
            .await
            .unwrap()
    );
}
