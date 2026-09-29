//! Operations contracts over isolated server and runtime databases.
use std::sync::Arc;

use runtara_core::{
    domain::InstanceStatus,
    persistence::{Persistence, run_state::StatePatch},
};
use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
use runtara_server::{
    api::repositories::workflows::WorkflowRepository,
    product_events::ProductEventSink,
    runtime_client::{RuntimeClient, RuntimeClientConfig},
    workers::execution_engine::ExecutionEngine,
};
use runtara_store_postgres::PostgresPersistence;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    server: PgPool,
    runtime: PgPool,
    persistence: Arc<PostgresPersistence>,
    client: Arc<RuntimeClient>,
    engine: ExecutionEngine,
    tenant: String,
    workflow: String,
    image: String,
}

impl Fixture {
    async fn new() -> Self {
        static CONFIG: std::sync::Once = std::sync::Once::new();
        CONFIG.call_once(|| {
            // All tests enter this guard before starting runtime work.
            unsafe {
                std::env::set_var("TENANT_ID", "operations-tests");
                std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
                std::env::set_var("OBJECT_MODEL_DATABASE_URL", "postgres://unused/unused");
            }
            runtara_server::config::init(
                runtara_server::config::Config::from_env().expect("test configuration"),
            );
        });
        let server = PgPool::connect(
            &std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL").expect("isolated server database"),
        )
        .await
        .expect("server database connection");
        sqlx::migrate!("./migrations").run(&server).await.unwrap();
        let runtime = PgPool::connect(
            &std::env::var("TEST_RUNTARA_DATABASE_URL").expect("isolated runtime database"),
        )
        .await
        .expect("runtime database connection");
        runtara_environment::migrations::run(&runtime)
            .await
            .unwrap();
        let persistence = Arc::new(PostgresPersistence::new(runtime.clone()));
        let client = Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                runtime.clone(),
                persistence.clone(),
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        ));
        let (events, _receiver) = tokio::sync::mpsc::channel(16);
        let engine = ExecutionEngine::new(
            server.clone(),
            Arc::new(WorkflowRepository::new(server.clone())),
            Some(client.clone()),
            None,
            ProductEventSink::new(events),
        );
        let tenant = Uuid::new_v4().to_string();
        let workflow = Uuid::new_v4().to_string();
        let image = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO workflows (tenant_id, workflow_id, version_count, latest_version) VALUES ($1,$2,2,2)")
            .bind(&tenant).bind(&workflow).execute(&server).await.unwrap();
        let definition = json!({"name":"Approval","steps":{},"executionPlan":[],"entryPoint":null});
        for version in [1, 2] {
            sqlx::query("INSERT INTO workflow_definitions (tenant_id,workflow_id,version,definition,file_size) VALUES ($1,$2,$3,$4,$5)")
                .bind(&tenant).bind(&workflow).bind(version).bind(&definition)
                .bind(serde_json::to_vec(&definition).unwrap().len() as i32)
                .execute(&server).await.unwrap();
        }
        sqlx::query("INSERT INTO images (image_id,tenant_id,name,binary_path) VALUES ($1,$2,$3,'/test-only')")
            .bind(&image).bind(&tenant).bind(format!("{workflow}:1"))
            .execute(&runtime).await.unwrap();
        Self {
            server,
            runtime,
            persistence,
            client,
            engine,
            tenant,
            workflow,
            image,
        }
    }

    async fn run(&self, label: Option<&str>) -> String {
        let id = Uuid::new_v4().to_string();
        let input =
            serde_json::to_vec(&json!({"data":{"order":"ORDER-123"},"variables":{}})).unwrap();
        self.persistence
            .try_register_instance_with_label(&id, &self.tenant, Some(&input), label)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO instance_images (instance_id,image_id,tenant_id) VALUES ($1,$2,$3)",
        )
        .bind(&id)
        .bind(&self.image)
        .bind(&self.tenant)
        .execute(&self.runtime)
        .await
        .unwrap();
        self.persistence
            .update_instance_status(&id, InstanceStatus::Running, None)
            .await
            .unwrap();
        id
    }
}

#[tokio::test]
async fn answers_record_the_original_actor_and_keep_receipt_replay_compatible() {
    use runtara_core::persistence::inputs::{InputAuthority, InputRequestSpec, InputState};
    use runtara_server::{
        api::services::workflow_runtime::submit_authenticated_input,
        auth::{AuthContext, AuthMethod},
    };
    let fx = Fixture::new().await;
    let id = fx.run(Some("ORDER-123")).await;
    let request = fx
        .persistence
        .input_requests()
        .unwrap()
        .register_input(
            &InputAuthority::Root {
                tenant_id: fx.tenant.clone(),
                instance_id: id.clone(),
            },
            &InputRequestSpec {
                signal_id: "approval".into(),
                response_schema: Some(
                    json!({"decision":{"type":"string","enum":["approve","reject"]}}),
                ),
                metadata: json!({"action_key":"approval"}),
                deadline: None,
            },
        )
        .await
        .unwrap();
    let payload = json!({"decision":"approve"});
    let alice = AuthContext::new(fx.tenant.clone(), "alice".into(), AuthMethod::Jwt);
    let bob = AuthContext::new(fx.tenant.clone(), "bob".into(), AuthMethod::ApiKey);
    let first = submit_authenticated_input(
        &fx.client,
        &fx.server,
        &alice,
        &id,
        &request.request_id,
        "answer",
        &payload,
    )
    .await
    .unwrap();
    let replay = submit_authenticated_input(
        &fx.client,
        &fx.server,
        &bob,
        &id,
        &request.request_id,
        "answer",
        &payload,
    )
    .await
    .unwrap();
    assert_eq!(first, replay);
    let legacy = fx
        .client
        .submit_input_response(&fx.tenant, &id, &request.request_id, "answer", &payload)
        .await
        .unwrap();
    assert_eq!(legacy.receipt_id, first.receipt_id);
    let stored = fx
        .persistence
        .input_requests()
        .unwrap()
        .get_input(&fx.tenant, &id, &request.request_id)
        .await
        .unwrap();
    let InputState::Accepted { receipt } = stored.state else {
        panic!("accepted")
    };
    let attribution: Value =
        serde_json::from_slice(receipt.acceptance_context.as_ref().unwrap()).unwrap();
    assert_eq!(attribution["source"], "user");
    assert_eq!(attribution["principal"], "alice");
    #[cfg(feature = "valkey-integration-tests")]
    {
        use runtara_server::api::services::session_queue::managed::*;
        let config = runtara_server::valkey::ValkeyConfig::from_env().unwrap();
        let mut conn = redis::aio::ConnectionManager::new(
            redis::Client::open(config.connection_url()).unwrap(),
        )
        .await
        .unwrap();
        let scope = QueueScope::new(&fx.tenant, &Uuid::new_v4().to_string()).unwrap();
        let target = InputTarget {
            instance_id: id.clone(),
            request_id: request.request_id.clone(),
        };
        let retained = enqueue_authenticated(
            &mut conn,
            &scope,
            "session-replay",
            "answer",
            &payload,
            &target,
            &bob,
        )
        .await
        .unwrap();
        assert_eq!(retained.actor_id.as_deref(), Some("bob"));
        let DeliveryOutcome::Accepted(accepted) =
            deliver_to_instance(&mut conn, &scope, &fx.client)
                .await
                .unwrap()
        else {
            panic!("session receipt replay")
        };
        assert_eq!(
            accepted.receipt_id.as_deref(),
            Some(first.receipt_id.as_str())
        );
        // The queued retry did not answer originally: retain the receipt's actor.
        assert_eq!(accepted.actor_id.as_deref(), Some("alice"));
    }
    let events: Vec<(Option<String>, Value)> = sqlx::query_as("SELECT actor_user_id, payload FROM audit_events WHERE tenant_id=$1 AND event_type='input.answer'")
        .bind(&fx.tenant).fetch_all(&fx.server).await.unwrap();
    assert_eq!(events.len(), 2);
    for (actor, payload) in events {
        assert_eq!(actor.as_deref(), Some("alice"));
        assert_eq!(payload["receiptId"], first.receipt_id);
        assert!(payload.get("decision").is_none());
    }
    assert!(
        submit_authenticated_input(
            &fx.client,
            &fx.server,
            &bob,
            &id,
            &request.request_id,
            "other-answer",
            &payload
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn both_detail_paths_return_state_and_reject_foreign_owners() {
    let fx = Fixture::new().await;
    let id = fx.run(Some("ORDER-123")).await;
    let state = json!({"order":"ORDER-123","amount":48200,"stage":"approval"});
    fx.persistence
        .run_state()
        .unwrap()
        .apply_state(
            &fx.tenant,
            &id,
            &"a".repeat(64),
            &StatePatch::from_object(state.as_object().unwrap()),
        )
        .await
        .unwrap();
    let direct = fx.engine.get_execution(&fx.tenant, &id).await.unwrap();
    let scoped = fx
        .engine
        .get_execution_with_metadata(&fx.workflow, &id, &fx.tenant)
        .await
        .unwrap();
    assert_eq!(direct.state, Some(state));
    assert_eq!(scoped.instance.state, direct.state);
    assert_eq!(scoped.instance.state_updated_at, direct.state_updated_at);
    assert!(direct.state_updated_at.is_some());
    assert!(fx.engine.get_execution("foreign", &id).await.is_err());
    assert!(
        fx.engine
            .get_execution_with_metadata(&fx.workflow, &id, "foreign")
            .await
            .is_err()
    );
    assert!(
        fx.engine
            .get_execution_with_metadata("other-workflow", &id, &fx.tenant)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn replay_preserves_label_inputs_and_provenance_with_a_new_run_and_latest_version() {
    let fx = Fixture::new().await;
    for label in [Some("ORDER-123"), None] {
        let id = fx.run(label).await;
        fx.persistence
            .update_instance_status(&id, InstanceStatus::Failed, None)
            .await
            .unwrap();
        let replay = fx.engine.replay(&fx.tenant, &id).await.unwrap();
        assert_ne!(replay.instance_id.to_string(), id);
        let event: Value = sqlx::query_scalar(
            "SELECT trigger_event FROM execution_requests WHERE tenant_id=$1 AND instance_id=$2",
        )
        .bind(&fx.tenant)
        .bind(replay.instance_id.to_string())
        .fetch_one(&fx.server)
        .await
        .unwrap();
        assert_eq!(event.get("runLabel").and_then(Value::as_str), label);
        assert_eq!(event["trigger"]["original_instance_id"], id);
        assert_eq!(event["version"], 2);
        assert_eq!(
            event["inputs"],
            json!({"data":{"order":"ORDER-123"},"variables":{}})
        );
        assert!(fx.engine.replay("foreign", &id).await.is_err());
    }
}

#[tokio::test]
async fn queues_count_requests_filter_active_work_and_project_typed_state() {
    use axum::{Json, extract::State, http::StatusCode};
    use runtara_core::persistence::inputs::{InputAuthority, InputRequestSpec};
    use runtara_server::api::{
        dto::{
            executions::{QueryExecutionsRequest, StateSortDto},
            operations::QueryOperationRequests,
        },
        handlers::{executions::query_executions_handler, operations::query_operation_requests},
        repositories::operations::OperationsRepository,
        services::operations::discover_queues,
    };
    use runtara_server::middleware::tenant_auth::OrgId;
    let fx = Fixture::new().await;
    let small = fx.run(Some("SMALL")).await;
    let large = fx.run(Some("LARGE")).await;
    let empty = fx.run(Some("EMPTY")).await;
    for (id, amount) in [(&small, 2), (&large, 10)] {
        fx.persistence
            .run_state()
            .unwrap()
            .apply_state(
                &fx.tenant,
                id,
                &"1".repeat(64),
                &StatePatch::from_object(
                    json!({"amount":amount,"secret":"not selected"})
                        .as_object()
                        .unwrap(),
                ),
            )
            .await
            .unwrap();
    }
    let store = fx.persistence.input_requests().unwrap();
    let mut requests = Vec::new();
    for (id, signal, deadline) in [
        (&small, "first", None),
        (&small, "second", None),
        (&large, "third", None),
        (&empty, "fourth", None),
        (
            &small,
            "expired",
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        ),
        (&small, "inactive", None),
    ] {
        let request = store
            .register_input(
                &InputAuthority::Root {
                    tenant_id: fx.tenant.clone(),
                    instance_id: id.clone(),
                },
                &InputRequestSpec {
                    signal_id: signal.into(),
                    response_schema: Some(json!({"decision":{"type":"string","enum":[signal]}})),
                    metadata: json!({"action_key":"old_key","context":{"signal":signal}}),
                    deadline,
                },
            )
            .await
            .unwrap();
        if signal == "inactive" {
            sqlx::query("UPDATE instance_input_requests SET invocation_path='missing/invocation' WHERE instance_id=$1 AND request_id=$2").bind(id).bind(&request.request_id).execute(&fx.runtime).await.unwrap();
        }
        requests.push(request);
    }
    let terminal = fx.run(None).await;
    store
        .register_input(
            &InputAuthority::Root {
                tenant_id: fx.tenant.clone(),
                instance_id: terminal.clone(),
            },
            &InputRequestSpec {
                signal_id: "terminal".into(),
                response_schema: None,
                metadata: json!({"action_key":"old_key"}),
                deadline: None,
            },
        )
        .await
        .unwrap();
    fx.persistence
        .update_instance_status(&terminal, InstanceStatus::Completed, None)
        .await
        .unwrap();
    let queues = discover_queues(
        &OperationsRepository::new(fx.server.clone()),
        &fx.client,
        &fx.tenant,
    )
    .await
    .unwrap();
    assert_eq!(queues.len(), 1);
    assert_eq!(queues[0].action_key, "old_key"); // Absent in the latest graph.
    assert_eq!(queues[0].count, 4);
    assert!(
        discover_queues(
            &OperationsRepository::new(fx.server.clone()),
            &fx.client,
            "foreign"
        )
        .await
        .unwrap()
        .is_empty()
    );
    let engine = Arc::new(fx.engine);
    for (page, expected) in [(0, &small), (1, &large)] {
        let (status, Json(body)) = query_operation_requests(
            OrgId(fx.tenant.clone()),
            State(engine.clone()),
            State(Some(fx.client.clone())),
            Json(QueryOperationRequests {
                workflow_id: fx.workflow.clone(),
                action_key: "old_key".into(),
                query: QueryExecutionsRequest {
                    page: Some(page),
                    size: Some(2),
                    state_fields: vec!["amount".into(), "missing".into()],
                    state_sort: Some(StateSortDto {
                        field: "amount".into(),
                        descending: false,
                    }),
                    ..Default::default()
                },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["totalElements"], 4);
        assert_eq!(body["data"]["content"][0]["instanceId"], *expected);
        if page == 1 {
            assert_eq!(body["data"]["content"][1]["instanceId"], empty);
        }
        if page == 0 {
            assert_eq!(body["data"]["content"].as_array().unwrap().len(), 2);
            assert_eq!(body["data"]["content"][0]["instanceId"], *expected);
            assert_eq!(body["data"]["content"][1]["instanceId"], small);
            assert_ne!(
                body["data"]["content"][0]["requestId"],
                body["data"]["content"][1]["requestId"]
            );
        }
        for row in body["data"]["content"].as_array().unwrap() {
            assert!(row["state"].get("secret").is_none());
            assert!(row["state"].get("missing").is_none());
            assert_eq!(
                row["inputSchema"]["decision"]["enum"][0],
                row["context"]["signal"]
            );
        }
    }
    let (_, Json(filtered)) = query_operation_requests(
        OrgId(fx.tenant.clone()),
        State(engine.clone()),
        State(Some(fx.client.clone())),
        Json(QueryOperationRequests {
            workflow_id: fx.workflow.clone(),
            action_key: "old_key".into(),
            query: serde_json::from_value(
                json!({"state":[{"field":"amount","op":"gte","value":10}]}),
            )
            .unwrap(),
        }),
    )
    .await;
    assert_eq!(filtered["data"]["totalElements"], 1);
    assert_eq!(filtered["data"]["content"][0]["instanceId"], large);
    assert!(filtered["data"]["content"][0].get("state").is_none());

    let query = QueryExecutionsRequest {
        workflow_id: Some(fx.workflow.clone()),
        state_fields: vec!["amount".into()],
        state_sort: Some(StateSortDto {
            field: "amount".into(),
            descending: true,
        }),
        ..Default::default()
    };
    let (status, Json(body)) =
        query_executions_handler(OrgId(fx.tenant.clone()), State(engine.clone()), Json(query))
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["content"][0]["id"], large);
    assert_eq!(body["data"]["content"][1]["id"], small);
    assert_eq!(body["data"]["content"][0]["state"], json!({"amount":10}));
    let (_, Json(body)) = query_executions_handler(
        OrgId(fx.tenant.clone()),
        State(engine.clone()),
        Json(QueryExecutionsRequest::default()),
    )
    .await;
    assert!(
        body["data"]["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("state").is_none())
    );
    let (status, Json(body)) = query_operation_requests(
        OrgId("foreign".into()),
        State(engine.clone()),
        State(Some(fx.client.clone())),
        Json(QueryOperationRequests {
            workflow_id: fx.workflow.clone(),
            action_key: "old_key".into(),
            query: QueryExecutionsRequest::default(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["totalElements"], 0);
}

#[tokio::test]
async fn shared_views_validate_scope_and_reject_lost_updates() {
    use axum::{
        Extension, Json,
        extract::{Path, State},
        http::StatusCode,
    };
    use runtara_server::{
        api::{
            dto::operations::SaveOperationView,
            handlers::operations::{create_operation_view, update_operation_view},
            repositories::operations::OperationsRepository,
        },
        auth::{AuthContext, AuthMethod},
        middleware::tenant_auth::OrgId,
    };
    let fx = Fixture::new().await;
    let actor = AuthContext::new(fx.tenant.clone(), "alice".into(), AuthMethod::Jwt);
    let config = json!({"name":"Approvals","workflow":fx.workflow,"columns":["amount"],"where":{"openRequest":"approve","state":[{"field":"dueAt","op":"lt","value":{"relative":"now","offsetSeconds":0}}]},"formats":{"amount":{"kind":"number","decimals":2,"prefix":"$"}}});
    let request = SaveOperationView {
        configuration: serde_json::from_value(config).unwrap(),
        revision: None,
    };
    let (status, Json(body)) = create_operation_view(
        OrgId(fx.tenant.clone()),
        State(fx.server.clone()),
        Extension(actor.clone()),
        Json(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["data"]["id"].as_str().unwrap().to_owned();
    let repo = OperationsRepository::new(fx.server.clone());
    assert!(repo.get_view("foreign", &id).await.unwrap().is_none());
    assert_eq!(repo.list_views(&fx.tenant).await.unwrap().len(), 1);
    let mut edit = request.clone();
    edit.revision = Some(1);
    edit.configuration.name = "Shared edit".into();
    let bob = AuthContext::new(fx.tenant.clone(), "bob".into(), AuthMethod::Jwt);
    let (status, Json(body)) = update_operation_view(
        OrgId(fx.tenant.clone()),
        State(fx.server.clone()),
        Extension(bob),
        Path(id.clone()),
        Json(edit.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revision"], 2);
    let (status, _) = update_operation_view(
        OrgId(fx.tenant.clone()),
        State(fx.server.clone()),
        Extension(actor.clone()),
        Path(id.clone()),
        Json(edit),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let mut bad = request.clone();
    bad.configuration.workflow = "foreign-workflow".into();
    let (status, _) = create_operation_view(
        OrgId(fx.tenant.clone()),
        State(fx.server.clone()),
        Extension(actor.clone()),
        Json(bad),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut bad = request;
    bad.configuration.columns = vec!["bad\0field".into()];
    let (status, _) = create_operation_view(
        OrgId(fx.tenant.clone()),
        State(fx.server.clone()),
        Extension(actor),
        Json(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!repo.delete_view("foreign", &id, 2).await.unwrap());
    assert!(!repo.delete_view(&fx.tenant, &id, 1).await.unwrap());
    assert!(repo.delete_view(&fx.tenant, &id, 2).await.unwrap());
}

#[tokio::test]
async fn failed_run_lists_include_step_errors_and_host_fallback() {
    use runtara_server::api::dto::executions::ExecutionFilters;
    let fx = Fixture::new().await;
    let id = fx.run(None).await;
    let detail = json!({"step_id":"call","error":{"code":"HTTP_TIMEOUT","category":"transient","message":"Request timed out","retryable":true}});
    sqlx::query("INSERT INTO instance_events(instance_id,event_type,subtype,payload) VALUES ($1,'custom','step_debug_end',$2)").bind(&id).bind(serde_json::to_vec(&detail).unwrap()).execute(&fx.runtime).await.unwrap();
    fx.persistence
        .update_instance_status(&id, InstanceStatus::Failed, Some(chrono::Utc::now()))
        .await
        .unwrap();
    let page = fx
        .engine
        .list_all_executions(&fx.tenant, None, None, ExecutionFilters::default())
        .await
        .unwrap();
    assert_eq!(page.content.len(), 1);
    let summary = page.content[0].error_summary.as_ref().unwrap();
    assert_eq!(summary.code.as_deref(), Some("HTTP_TIMEOUT"));
    assert_eq!(summary.category.as_deref(), Some("transient"));
    assert_eq!(summary.message, "Request timed out");
    sqlx::query("DELETE FROM instance_events WHERE instance_id=$1")
        .bind(&id)
        .execute(&fx.runtime)
        .await
        .unwrap();
    let terminal = json!({"code":"RETRYABLE", "category":"transient", "message":"Error step terminated", "severity":"warning"});
    sqlx::query("UPDATE instances SET error=$2 WHERE instance_id=$1")
        .bind(&id)
        .bind(terminal.to_string())
        .execute(&fx.runtime)
        .await
        .unwrap();
    let page = fx
        .engine
        .list_all_executions(&fx.tenant, None, None, ExecutionFilters::default())
        .await
        .unwrap();
    let summary = page.content[0].error_summary.as_ref().unwrap();
    assert_eq!(summary.code.as_deref(), Some("RETRYABLE"));
    assert_eq!(summary.severity.as_deref(), Some("warning"));
    sqlx::query("UPDATE instances SET error='Runner exited' WHERE instance_id=$1")
        .bind(&id)
        .execute(&fx.runtime)
        .await
        .unwrap();
    let page = fx
        .engine
        .list_all_executions(&fx.tenant, None, None, ExecutionFilters::default())
        .await
        .unwrap();
    assert_eq!(page.content[0].error.as_deref(), Some("Runner exited"));
    assert!(page.content[0].error_summary.is_none());
    assert!(
        fx.client
            .operation_failures("foreign", &[id])
            .await
            .unwrap()
            .is_empty()
    );
}
