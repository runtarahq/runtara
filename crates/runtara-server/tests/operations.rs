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
