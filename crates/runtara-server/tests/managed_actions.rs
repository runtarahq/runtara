//! Public action handlers over real request persistence and workflow associations.
use axum::{
    Json,
    extract::{FromRequest, Path, State},
    http::StatusCode,
};
use runtara_core::{
    domain::InstanceStatus,
    persistence::{
        Persistence,
        inputs::{InputAuthority, InputRequestSpec},
    },
};
use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
use runtara_server::{
    api::{
        handlers::step_events,
        repositories::workflows::WorkflowRepository,
        services::workflow_runtime::{
            SubmitWorkflowActionRequest, list_instance_actions, list_workflow_actions,
        },
    },
    middleware::tenant_auth::OrgId,
    product_events::ProductEventSink,
    runtime_client::{RuntimeClient, RuntimeClientConfig},
    workers::execution_engine::ExecutionEngine,
};
use runtara_store_postgres::PostgresPersistence;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

fn mcp_server(
    pool: sqlx::PgPool,
    engine: Arc<ExecutionEngine>,
    client: Option<Arc<RuntimeClient>>,
    tenant: &str,
) -> runtara_server::mcp::server::SmoMcpServer {
    let discovery_client = client.clone();
    let router = axum::Router::new()
        .route(
            "/api/runtime/workflows/{workflow}/instances/{instance}/pending-input",
            axum::routing::get(move |owner: OrgId, path: Path<(String, String)>| {
                let engine = engine.clone();
                let client = discovery_client.clone();
                async move {
                    step_events::get_pending_input(owner, path, State(engine), State(client)).await
                }
            }),
        )
        .route(
            "/api/runtime/signals/{instance}",
            axum::routing::post(step_events::submit_signal).with_state(client.clone()),
        );
    runtara_server::mcp::server::SmoMcpServer::new(
        pool,
        Arc::new(
            runtara_server::api::repositories::object_model::ObjectStoreManager::new(String::new()),
        ),
        client,
        tenant.into(),
        router,
    )
}

fn mcp_json(result: rmcp::model::CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[cfg(feature = "valkey-integration-tests")]
async fn delivery_api_contract(
    tenant: &str,
    session: &str,
    instance: &str,
    request: &str,
    mut conn: redis::aio::ConnectionManager,
    engine: Arc<ExecutionEngine>,
    client: Arc<RuntimeClient>,
) {
    use axum::extract::Query;
    use runtara_server::api::{handlers::sessions::*, services::session_queue::managed::*};
    let scope = QueueScope::new(tenant, session).unwrap();
    let enqueue = |owner: &str, operation: &str| {
        submit_event(
            OrgId(owner.into()),
            State(Some(conn.clone())),
            Path(session.into()),
            Ok(Json(SubmitEventRequest {
                message_id: "api-message".into(),
                operation_id: operation.into(),
                message: None,
                payload: Some(json!({"answer":true})),
            })),
        )
    };
    assert_eq!(
        enqueue("foreign", "api-operation").await.0,
        StatusCode::NOT_FOUND
    );
    let (status, Json(first)) = enqueue(tenant, "api-operation").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["data"]["state"], "queued");
    assert_eq!(enqueue(tenant, "api-operation").await.1.0, first);
    assert_eq!(enqueue(tenant, "different").await.0, StatusCode::CONFLICT);
    assert!(serde_json::from_value::<SubmitEventRequest>(json!({"message":"legacy"})).is_err());
    let page = |owner: &str, limit| {
        list_session_deliveries(
            OrgId(owner.into()),
            State(Some(conn.clone())),
            Path(session.into()),
            Ok(Query(DeliveryQuery {
                cursor: "0".into(),
                limit,
            })),
        )
    };
    assert_eq!(page("foreign", 50).await.0, StatusCode::NOT_FOUND);
    assert_eq!(page(tenant, 0).await.0, StatusCode::BAD_REQUEST);
    let (status, Json(body)) = page(tenant, 50).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["deliveries"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["nextCursor"], "0");
    for field in ["payload", "payload_json", "lease_token", "launch_json"] {
        assert!(body["data"]["deliveries"][0].get(field).is_none());
    }
    let resolve = |owner: &str, body| {
        resolve_session_delivery(
            OrgId(owner.into()),
            State(Some(conn.clone())),
            State(engine.clone()),
            State(Some(client.clone())),
            Path((session.into(), "api-message".into())),
            Ok(Json(body)),
        )
    };
    let selection = || ResolveDeliveryRequest::Select {
        instance_id: instance.into(),
        request_id: request.into(),
    };
    assert_eq!(resolve(tenant, selection()).await.0, StatusCode::CONFLICT);
    // Two eligible waits block the message; no latest-event selection is allowed.
    assert!(matches!(
        runtara_server::api::services::session_queue::delivery::deliver_session(
            &mut conn.clone(),
            &scope,
            &client,
            &engine
        )
        .await
        .unwrap(),
        DeliveryOutcome::Blocked(_)
    ));
    assert_eq!(
        resolve("foreign", ResolveDeliveryRequest::Fail).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        resolve(
            tenant,
            ResolveDeliveryRequest::Select {
                instance_id: instance.into(),
                request_id: "missing".into()
            }
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (a, b) = tokio::join!(resolve(tenant, selection()), resolve(tenant, selection()));
    assert!(matches!(
        (a.0, b.0),
        (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
    ));
    let ClaimOutcome::Claimed(lease) = claim(&mut conn.clone(), &scope, 30_000).await.unwrap()
    else {
        panic!("resolved queue claim")
    };
    assert_eq!(
        resolve(tenant, ResolveDeliveryRequest::Fail).await.0,
        StatusCode::CONFLICT
    );
    block(
        &mut conn.clone(),
        &scope,
        &lease,
        DeliveryReason::StaleTarget,
    )
    .await
    .unwrap();
    let (status, Json(failed)) = resolve(tenant, ResolveDeliveryRequest::Fail).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(failed["data"]["state"], "failed");
    let (status, Json(found)) = get_session_delivery(
        OrgId(tenant.into()),
        State(Some(conn.clone())),
        Path((session.into(), "api-message".into())),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found, failed);
    assert!(matches!(
        claim(&mut conn, &scope, 30_000).await.unwrap(),
        ClaimOutcome::Empty
    ));
}

#[tokio::test]
async fn managed_actions_page_requests_and_replay_receipts_after_completion() {
    let url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
        .or_else(|_| std::env::var("TEST_RUNTARA_DATABASE_URL"))
        .expect("isolated runtime database required");
    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("test database connection");
    runtara_environment::migrations::run(&pool).await.unwrap();
    let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
    let client = Arc::new(RuntimeClient::new(
        Arc::new(EnvironmentHandlerState::new(
            pool.clone(),
            persistence.clone(),
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )),
        RuntimeClientConfig::new(Default::default()),
    ));
    let (events, _receiver) = tokio::sync::mpsc::channel(1);
    let engine = Arc::new(ExecutionEngine::new(
        pool.clone(),
        Arc::new(WorkflowRepository::new(pool.clone())),
        Some(client.clone()),
        None,
        ProductEventSink::new(events),
    ));
    let tenant = Uuid::new_v4().to_string();
    let workflow = format!("managed_%_{}", Uuid::new_v4());
    let image = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1,$2,$3,'/test-only')")
        .bind(&image).bind(&tenant).bind(format!("{workflow}:1")).execute(&pool).await.unwrap();
    let inputs = persistence.input_requests().unwrap();
    for _ in 0..105 {
        let instance = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance, &tenant)
            .await
            .unwrap();
        persistence
            .update_instance_status(&instance, InstanceStatus::Running, None)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO instance_images (instance_id,image_id,tenant_id) VALUES ($1,$2,$3)",
        )
        .bind(&instance)
        .bind(&image)
        .bind(&tenant)
        .execute(&pool)
        .await
        .unwrap();
        for wait in ["one", "two"] {
            inputs
                .register_input(
                    &InputAuthority::Root {
                        tenant_id: tenant.clone(),
                        instance_id: instance.clone(),
                    },
                    &InputRequestSpec {
                        signal_id: format!("{instance}/{wait}"),
                        response_schema: Some(json!({"answer":{"type":"boolean","required":true}})),
                        metadata: json!({"step_name":"Approve","correlation":{"order":"order-1"}}),
                        deadline: None,
                    },
                )
                .await
                .unwrap();
        }
        persistence
            .update_instance_status(&instance, InstanceStatus::Suspended, None)
            .await
            .unwrap();
    }
    // Workflow IDs containing LIKE metacharacters remain literal filters.
    let unrelated_image = Uuid::new_v4().to_string();
    let unrelated_instance = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO images (image_id,tenant_id,name,binary_path) VALUES ($1,$2,$3,'/test-only')",
    )
    .bind(&unrelated_image)
    .bind(&tenant)
    .bind(format!(
        "{}:1",
        workflow.replace('_', "x").replace('%', "wild")
    ))
    .execute(&pool)
    .await
    .unwrap();
    persistence
        .register_instance(&unrelated_instance, &tenant)
        .await
        .unwrap();
    persistence
        .update_instance_status(&unrelated_instance, InstanceStatus::Running, None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO instance_images (instance_id,image_id,tenant_id) VALUES ($1,$2,$3)")
        .bind(&unrelated_instance)
        .bind(&unrelated_image)
        .bind(&tenant)
        .execute(&pool)
        .await
        .unwrap();
    inputs
        .register_input(
            &InputAuthority::Root {
                tenant_id: tenant.clone(),
                instance_id: unrelated_instance,
            },
            &InputRequestSpec {
                signal_id: "unrelated".into(),
                response_schema: None,
                metadata: json!({}),
                deadline: None,
            },
        )
        .await
        .unwrap();
    let page = list_workflow_actions(&engine, &client, &tenant, &workflow, Some(4), Some(25))
        .await
        .unwrap();
    assert_eq!(page.page.total_count, 210);
    assert_eq!(page.page.offset, 100);
    assert_eq!(page.actions.len(), 25);
    assert!(page.page.has_next_page);
    let last = list_workflow_actions(&engine, &client, &tenant, &workflow, Some(8), Some(25))
        .await
        .unwrap();
    assert_eq!(last.actions.len(), 10);
    assert_eq!(last.page.total_count, 210);
    assert!(!last.page.has_next_page);
    assert!(
        list_workflow_actions(&engine, &client, "foreign", &workflow, Some(0), Some(25))
            .await
            .unwrap()
            .actions
            .is_empty()
    );
    let action = &page.actions[0];
    assert_eq!(action.id, action.request_id);
    assert_ne!(action.request_id, action.signal_id);
    assert_eq!(
        action.input_schema.as_ref().unwrap()["answer"]["type"],
        "boolean"
    );

    let pending = |owner: String| {
        step_events::get_pending_input(
            OrgId(owner),
            Path((workflow.clone(), action.instance_id.clone())),
            State(engine.clone()),
            State(Some(client.clone())),
        )
    };
    let (status, Json(discovered)) = pending(tenant.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(discovered["data"]["count"], 2);
    assert!(
        discovered["data"]["pendingInputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|input| input["requestId"] == action.request_id)
    );
    let (status, Json(error)) = pending("foreign".into()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "INPUT_NOT_FOUND");
    use runtara_server::mcp::tools::signals as mcp;
    let mcp = mcp_server(pool.clone(), engine.clone(), Some(client.clone()), &tenant);
    let mcp_foreign = mcp_server(
        pool.clone(),
        engine.clone(),
        Some(client.clone()),
        "foreign",
    );
    let mcp_unavailable = mcp_server(pool.clone(), engine.clone(), None, &tenant);
    let mcp_list = || mcp::ListPendingSignalsParams {
        workflow_id: workflow.clone(),
        instance_id: action.instance_id.clone(),
    };
    assert_eq!(
        mcp_json(mcp::list_pending_signals(&mcp, mcp_list()).await.unwrap()),
        discovered
    );
    assert!(
        mcp::list_pending_signals(&mcp_foreign, mcp_list())
            .await
            .is_err()
    );
    let mcp_schema = || mcp::GetSignalSchemaParams {
        workflow_id: workflow.clone(),
        instance_id: action.instance_id.clone(),
        request_id: action.request_id.clone(),
    };
    assert_eq!(
        mcp_json(mcp::get_signal_schema(&mcp, mcp_schema()).await.unwrap())["data"]["responseSchema"]
            ["answer"]["type"],
        "boolean"
    );
    // Unknown discovery must not be flattened into an absent request/schema.
    assert!(
        mcp::list_pending_signals(&mcp_unavailable, mcp_list())
            .await
            .is_err()
    );
    assert!(
        mcp::get_signal_schema(&mcp_unavailable, mcp_schema())
            .await
            .is_err()
    );
    #[cfg(feature = "valkey-integration-tests")]
    let (session_id, valkey) = {
        use runtara_server::{
            api::{handlers::sessions, services::session_queue},
            valkey::ValkeyConfig,
        };
        let config = ValkeyConfig::from_env().expect("isolated Valkey required");
        let redis = redis::Client::open(config.connection_url()).unwrap();
        let mut connection = redis::aio::ConnectionManager::new(redis).await.unwrap();
        let session = Uuid::new_v4().to_string();
        session_queue::set_session_meta(
            &mut connection,
            &tenant,
            &session,
            &action.instance_id,
            &workflow,
        )
        .await
        .unwrap();
        let (status, Json(session_page)) = sessions::session_pending_input(
            OrgId(tenant.clone()),
            State(Some(client.clone())),
            State(Some(connection.clone())),
            Path(session.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(session_page["data"], discovered["data"]);
        let (status, Json(error)) = sessions::session_pending_input(
            OrgId("foreign".into()),
            State(Some(client.clone())),
            State(Some(connection.clone())),
            Path(session.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["code"], "INPUT_NOT_FOUND");
        (session, connection)
    };
    #[cfg(feature = "valkey-integration-tests")]
    delivery_api_contract(
        &tenant,
        &session_id,
        &action.instance_id,
        &action.request_id,
        valkey.clone(),
        engine.clone(),
        client.clone(),
    )
    .await;
    let malformed = axum::http::Request::builder()
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{malformed"))
        .unwrap();
    let rejection = Json::<SubmitWorkflowActionRequest>::from_request(malformed, &())
        .await
        .unwrap_err();
    let (status, Json(error)) = step_events::submit_signal(
        OrgId(tenant.clone()),
        Path(action.instance_id.clone()),
        State(Some(client.clone())),
        Err(rejection),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "INPUT_INVALID_REQUEST");
    let signal = |owner: String, operation: &str, payload: Value| {
        step_events::submit_signal(
            OrgId(owner),
            Path(action.instance_id.clone()),
            State(Some(client.clone())),
            Ok(Json(SubmitWorkflowActionRequest {
                request_id: action.request_id.clone(),
                operation_id: operation.into(),
                payload,
            })),
        )
    };
    let (status, Json(error)) = signal("foreign".into(), "signal", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "INPUT_NOT_FOUND");
    let (status, Json(error)) = signal(tenant.clone(), "", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "INPUT_INVALID_REQUEST");
    let (status, Json(error)) = signal(tenant.clone(), "signal", json!({"answer":"wrong"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "INPUT_INVALID_PAYLOAD");
    // No default/legacy identity may silently turn a retry into a new submission.
    assert!(
        serde_json::from_value::<step_events::SubmitSignalRequest>(
            json!({"requestId": action.request_id, "payload": {}})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<step_events::SubmitSignalRequest>(
            json!({"signalId": action.signal_id, "payload": {}})
        )
        .is_err()
    );

    #[cfg(feature = "valkey-integration-tests")]
    let queued = {
        use runtara_server::api::services::session_queue::managed::*;
        let scope = QueueScope::new(&tenant, &session_id).unwrap();
        let mut connection = valkey.clone();
        enqueue(
            &mut connection,
            &scope,
            "lost-ack-message",
            "reply",
            &json!({"answer":true}),
        )
        .await
        .unwrap();
        let ClaimOutcome::Claimed(lease) = claim(&mut connection, &scope, 30_000).await.unwrap()
        else {
            panic!("queue claim");
        };
        let lease = bind(
            &mut connection,
            &scope,
            &lease,
            &InputTarget {
                instance_id: action.instance_id.clone(),
                request_id: action.request_id.clone(),
            },
        )
        .await
        .unwrap();
        (scope, lease)
    };

    let submit = |owner: String, operation: &str, payload: Value| {
        step_events::submit_workflow_action(
            OrgId(owner),
            State(engine.clone()),
            State(Some(client.clone())),
            Path((
                workflow.clone(),
                action.instance_id.clone(),
                action.request_id.clone(),
            )),
            Ok(Json(SubmitWorkflowActionRequest {
                request_id: action.request_id.clone(),
                operation_id: operation.into(),
                payload,
            })),
        )
    };
    let (status, Json(error)) = submit("foreign".into(), "reply", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["code"], "INPUT_NOT_FOUND");
    let (status, Json(error)) = submit(tenant.clone(), "reply", json!({"answer":"invalid"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "INPUT_INVALID_PAYLOAD");
    let (status, Json(accepted)) = submit(tenant.clone(), "reply", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["data"]["requestId"], action.request_id);
    assert!(accepted["data"].get("payload").is_none());
    let (status, Json(error)) = submit(tenant.clone(), "competing", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "INPUT_ALREADY_ANSWERED");
    persistence
        .update_instance_status(&action.instance_id, InstanceStatus::Completed, None)
        .await
        .unwrap();
    assert!(
        list_instance_actions(&client, &tenant, &workflow, &action.instance_id)
            .await
            .unwrap()
            .is_empty()
    );
    let (status, Json(replayed)) = submit(tenant.clone(), "reply", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed, accepted);
    let (status, Json(signal_replayed)) =
        signal(tenant.clone(), "reply", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(signal_replayed["data"], accepted["data"]);
    assert_eq!(
        mcp_json(mcp::list_pending_signals(&mcp, mcp_list()).await.unwrap())["data"]["count"],
        0
    );
    let mcp_reply = || mcp::SubmitSignalResponseParams {
        instance_id: action.instance_id.clone(),
        request_id: action.request_id.clone(),
        operation_id: "reply".into(),
        payload: json!({"answer":true}),
    };
    assert_eq!(
        mcp_json(
            mcp::submit_signal_response(&mcp, mcp_reply())
                .await
                .unwrap()
        )["data"],
        accepted["data"]
    );
    assert!(
        mcp::submit_signal_response(&mcp_foreign, mcp_reply())
            .await
            .is_err()
    );
    #[cfg(feature = "valkey-integration-tests")]
    {
        use runtara_server::api::services::session_queue::managed::*;
        let mut connection = valkey.clone();
        // PostgreSQL acceptance committed, but the delivering worker never acked
        // Valkey. Expire only its lease, then recover with a fresh connection.
        renew(&mut connection, &queued.0, &queued.1, 1)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let config = runtara_server::valkey::ValkeyConfig::from_env().unwrap();
        let mut recovered = redis::aio::ConnectionManager::new(
            redis::Client::open(config.connection_url()).unwrap(),
        )
        .await
        .unwrap();
        // Start the production worker with no attached session/SSE handler.
        // It discovers retained ownership and replays the completed root's receipt.
        let shutdown = runtara_server::shutdown::ShutdownSignal::new();
        let worker = tokio::spawn(runtara_server::workers::session_delivery_worker::run(
            recovered.clone(),
            client.clone(),
            engine.clone(),
            shutdown.clone(),
        ));
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let envelope = get(&mut recovered, &queued.0, "lost-ack-message")
                    .await
                    .unwrap();
                if envelope.state == DeliveryState::Accepted {
                    break envelope;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;
        shutdown.trigger();
        tokio::time::timeout(std::time::Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        let envelope = outcome.expect("worker must recover the retained queue without SSE");
        assert_eq!(
            envelope.receipt_id.as_deref(),
            accepted["data"]["receiptId"].as_str()
        );
        assert_eq!(envelope.target.unwrap().instance_id, action.instance_id);
        assert!(matches!(
            claim(&mut recovered, &queued.0, 30_000).await.unwrap(),
            ClaimOutcome::Empty
        ));
    }

    let (status, Json(discovered)) = pending(tenant.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(discovered["data"]["pendingInputs"], json!([]));

    let (status, Json(error)) = submit(tenant.clone(), "reply", json!({"answer":false})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "INPUT_OPERATION_CONFLICT");
    let (status, Json(error)) =
        submit(tenant.clone(), "new-operation", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Only a matching receipt bypasses current root liveness.
    assert_eq!(error["code"], "INPUT_INACTIVE");
    #[cfg(feature = "valkey-integration-tests")]
    {
        let (status, Json(session_page)) =
            runtara_server::api::handlers::sessions::session_pending_input(
                OrgId(tenant.clone()),
                State(Some(client.clone())),
                State(Some(valkey)),
                Path(session_id),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(session_page["data"]["pendingInputs"], json!([]));
    }
    pool.close().await;
    let (status, Json(error)) = submit(tenant, "reply", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["code"], "INPUT_UNAVAILABLE");
    assert!(!error["message"].as_str().unwrap().contains("pool"));
}
