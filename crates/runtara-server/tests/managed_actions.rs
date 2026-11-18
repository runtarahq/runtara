//! Public action handlers over real request persistence and workflow associations.
use axum::{
    Extension, Json,
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

/// What handlers ask the engine through: this engine and runtime, in process.
fn engine_api(
    pool: &sqlx::PgPool,
    engine: &Arc<ExecutionEngine>,
    client: Option<Arc<RuntimeClient>>,
) -> Arc<dyn runtara_server::engine::Engine> {
    Arc::new(runtara_server::engine::LocalEngine::new(
        client,
        engine.clone(),
        pool.clone(),
    ))
}

fn mcp_server(
    pool: sqlx::PgPool,
    engine: Arc<ExecutionEngine>,
    client: Option<Arc<RuntimeClient>>,
    tenant: &str,
) -> runtara_server::mcp::server::SmoMcpServer {
    let discovery = engine_api(&pool, &engine, client);
    let signals = discovery.clone();
    let tools = discovery.clone();
    let router = axum::Router::new()
        .route(
            "/api/runtime/workflows/{workflow}/instances/{instance}/pending-input",
            axum::routing::get(move |owner: OrgId, path: Path<(String, String)>| {
                let engine = discovery.clone();
                async move { step_events::get_pending_input(owner, path, State(engine)).await }
            }),
        )
        .route(
            "/api/runtime/signals/{instance}",
            axum::routing::post(
                move |owner: OrgId,
                      auth: Extension<runtara_server::auth::AuthContext>,
                      path: Path<String>,
                      body| {
                    let engine = signals.clone();
                    async move {
                        step_events::submit_signal(owner, auth, path, State(engine), body).await
                    }
                },
            ),
        );
    runtara_server::mcp::server::SmoMcpServer::new(pool, tools, tenant.into(), router)
}

fn mcp_json(result: rmcp::model::CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

/// Settle the targeted submission so the legacy message reaches the queue head.
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
    let api = engine_api(&pool, &engine, Some(client.clone()));
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
            State(api.clone()),
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
    let malformed = axum::http::Request::builder()
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{malformed"))
        .unwrap();
    let rejection = Json::<SubmitWorkflowActionRequest>::from_request(malformed, &())
        .await
        .unwrap_err();
    let (status, Json(error)) = step_events::submit_signal(
        OrgId(tenant.clone()),
        Extension(runtara_server::auth::AuthContext::new(
            tenant.clone(),
            "operator".into(),
            runtara_server::auth::AuthMethod::Unauthenticated,
        )),
        Path(action.instance_id.clone()),
        State(api.clone()),
        Err(rejection),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["code"], "INPUT_INVALID_REQUEST");
    let signal = |owner: String, operation: &str, payload: Value| {
        step_events::submit_signal(
            OrgId(owner.clone()),
            Extension(runtara_server::auth::AuthContext::new(
                owner,
                "operator".into(),
                runtara_server::auth::AuthMethod::Unauthenticated,
            )),
            Path(action.instance_id.clone()),
            State(api.clone()),
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

    let submit = |owner: String, operation: &str, payload: Value| {
        step_events::submit_workflow_action(
            OrgId(owner.clone()),
            Extension(runtara_server::auth::AuthContext::new(
                owner,
                "operator".into(),
                runtara_server::auth::AuthMethod::Unauthenticated,
            )),
            State(api.clone()),
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
    pool.close().await;
    let (status, Json(error)) = submit(tenant, "reply", json!({"answer":true})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["code"], "INPUT_UNAVAILABLE");
    assert!(!error["message"].as_str().unwrap().contains("pool"));
}
