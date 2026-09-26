//! Managed response delivery to existing sessions, including receipt recovery.
//! Requires isolated server and runtime PostgreSQL databases plus Valkey.
use std::{sync::Arc, time::Duration};

use redis::aio::ConnectionManager;
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
        repositories::workflows::WorkflowRepository,
        services::session_queue::{delivery::deliver_session, managed::*},
    },
    product_events::ProductEventSink,
    runtime_client::{RuntimeClient, RuntimeClientConfig},
    workers::execution_engine::{ExecutionEngine, QueueRequest, TriggerSource},
};
use runtara_store_postgres::PostgresPersistence;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    server: PgPool,
    runtime: PgPool,
    persistence: Arc<PostgresPersistence>,
    client: Arc<RuntimeClient>,
    conn: ConnectionManager,
    scope: QueueScope,
    route: SessionRoute,
    image: String,
}

impl Fixture {
    async fn new() -> Self {
        init_config();
        let server_url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
            .expect("isolated server database required");
        let runtime_url = std::env::var("TEST_ENVIRONMENT_DATABASE_URL")
            .or_else(|_| std::env::var("TEST_RUNTARA_DATABASE_URL"))
            .expect("isolated runtime database required");
        let server = PgPool::connect(&server_url)
            .await
            .expect("server database connection");
        sqlx::migrate!("./migrations").run(&server).await.unwrap();
        let runtime = PgPool::connect(&runtime_url)
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
        let config = runtara_server::valkey::ValkeyConfig::from_env().unwrap();
        let conn = ConnectionManager::new(redis::Client::open(config.connection_url()).unwrap())
            .await
            .unwrap();
        let scope =
            QueueScope::new(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string()).unwrap();
        let workflow = Uuid::new_v4().to_string();
        let repo = WorkflowRepository::new(server.clone());
        repo.create(scope.tenant_id(), &workflow, None, &workflow, "")
            .await
            .unwrap();
        repo.create_initial_version(
            scope.tenant_id(),
            &workflow,
            "Session fixture",
            "",
            Default::default(),
            false,
        )
        .await
        .unwrap();
        let route = SessionRoute {
            workflow_id: workflow.clone(),
            instance_id: Uuid::new_v4().to_string(),
        };
        let image = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO images (image_id,tenant_id,name,binary_path) VALUES ($1,$2,$3,'/session-test-only')")
            .bind(&image).bind(scope.tenant_id()).bind(format!("{workflow}:1")).execute(&runtime).await.unwrap();
        let mut fixture = Self {
            server,
            runtime,
            persistence,
            client,
            conn,
            scope,
            route,
            image,
        };
        fixture
            .register_instance(&fixture.route.instance_id, InstanceStatus::Running)
            .await;
        configure_route(&mut fixture.conn, &fixture.scope, &fixture.route)
            .await
            .unwrap();
        enqueue(
            &mut fixture.conn,
            &fixture.scope,
            "message",
            "operation",
            &json!({"answer":true}),
        )
        .await
        .unwrap();
        fixture
    }

    fn engine(&self) -> ExecutionEngine {
        let (events, _receiver) = tokio::sync::mpsc::channel(1);
        ExecutionEngine::new(
            self.server.clone(),
            Arc::new(WorkflowRepository::new(self.server.clone())),
            Some(self.client.clone()),
            None,
            ProductEventSink::new(events),
        )
    }

    async fn register_instance(&self, id: &str, status: InstanceStatus) {
        self.persistence
            .register_instance(id, self.scope.tenant_id())
            .await
            .unwrap();
        self.persistence
            .update_instance_status(id, status, None)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO instance_images(instance_id,image_id,tenant_id) VALUES ($1,$2,$3)",
        )
        .bind(id)
        .bind(&self.image)
        .bind(self.scope.tenant_id())
        .execute(&self.runtime)
        .await
        .unwrap();
    }

    async fn request_count(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_requests WHERE tenant_id=$1")
            .bind(self.scope.tenant_id())
            .fetch_one(&self.server)
            .await
            .unwrap()
    }

    async fn expire_lease(&mut self, lease: &Envelope) {
        let renewed = renew(&mut self.conn, &self.scope, lease, 1).await.unwrap();
        wait_until_backend_time(&mut self.conn, renewed.lease_deadline_ms.unwrap()).await;
    }

    async fn deliver(&mut self) -> DeliveryOutcome {
        deliver_session(&mut self.conn, &self.scope, &self.client)
            .await
            .unwrap()
    }

    async fn register_wait(&self, id: &str, signal: &str) -> String {
        self.persistence
            .input_requests()
            .unwrap()
            .register_input(
                &InputAuthority::Root {
                    tenant_id: self.scope.tenant_id().into(),
                    instance_id: id.into(),
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

    async fn cleanup(mut self) {
        // CI shares the service databases across integration targets. Leave no
        // pending source rows for another target's global outbox relay to claim.
        for table in [
            "execution_requests",
            "execution_admission_tenants",
            "workflows",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id=$1"))
                .bind(self.scope.tenant_id())
                .execute(&self.server)
                .await
                .unwrap();
        }
        for table in ["instance_images", "instances", "images"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id=$1"))
                .bind(self.scope.tenant_id())
                .execute(&self.runtime)
                .await
                .unwrap();
        }
        use sha2::{Digest, Sha256};
        let identity =
            serde_json::to_vec(&[self.scope.tenant_id(), self.scope.session_id()]).unwrap();
        let digest = Sha256::digest(identity);
        let prefix = format!("runtara:session:{{{digest:x}}}");
        let mut delete = redis::cmd("DEL");
        for suffix in ["pending", "envelopes", "operations", "completed", "owner"] {
            delete.arg(format!("{prefix}:{suffix}"));
        }
        delete.query_async::<()>(&mut self.conn).await.unwrap();
    }
}

fn init_config() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // Admission reads process configuration. Construct an isolated fixture
        // explicitly rather than mutating environment variables after Tokio starts.
        runtara_server::config::init(runtara_server::config::Config {
            tenant_id: "session-test".into(),
            max_concurrent_executions: 100,
            execution_timeout_policy: Default::default(),
            checkpoint_ttl_hours: 48,
            adaptive_rate_limiting_enabled: false,
            auto_retry_on_429_enabled: false,
            max_429_retries: 0,
            max_retry_delay_ms: 0,
            object_model_database_url: "postgresql://unused/unused".into(),
            object_model_max_connections: 1,
            object_model_soft_delete: true,
            object_model_bulk_request_limit: 100,
            runtime_pool: Default::default(),
            shutdown_grace: Default::default(),
            raw_sql_guardrails: runtara_object_store::SqlGuardrails {
                statement_timeout_ms: 1000,
                max_rows: 100,
                max_response_bytes: 4096,
            },
            object_model_pool: Default::default(),
            object_model_pool_cache_max: 1,
            object_model_pool_cache_ttl_secs: 60,
            agent_components_dir: None,
            direct_wasm_components_dir: None,
            isolation_policy: None,
            mcp_allowed_hosts: vec!["localhost".into()],
            mcp_session_store: runtara_server::config::McpSessionStore::Local,
            mcp_session_ttl_seconds: 60,
            dev_mode: true,
            entitlement_snapshot:
                runtara_server::entitlements::EntitlementSnapshot::parse_entitlements(
                    "session-test",
                    None,
                    None,
                    None,
                    &Default::default(),
                )
                .unwrap(),
        });
    });
}

async fn wait_until_backend_time(conn: &mut ConnectionManager, deadline_ms: u64) {
    tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            let (seconds, micros): (u64, u64) = redis::cmd("TIME").query_async(conn).await.unwrap();
            if seconds * 1000 + micros / 1000 >= deadline_ms {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("backend deadline");
}

/// A message retained before any wait existed was not an answer to a prompt
/// the sender saw. It is blocked for resolution, and neither starts another
/// execution nor answers the wait that opens afterwards.
#[tokio::test]
async fn an_unbound_message_is_never_bound_to_a_wait_that_opens_later() {
    let mut f = Fixture::new().await;
    let ClaimOutcome::Claimed(lease) = claim(&mut f.conn, &f.scope, 30_000).await.unwrap() else {
        panic!("claim")
    };
    block(&mut f.conn, &f.scope, &lease, DeliveryReason::StaleTarget)
        .await
        .unwrap();
    fail_blocked(&mut f.conn, &f.scope, "message")
        .await
        .unwrap();
    let instance = Uuid::new_v4();
    f.engine()
        .queue(QueueRequest {
            tenant_id: f.scope.tenant_id(),
            workflow_id: &f.route.workflow_id,
            version: Some(1),
            inputs: json!({"data":{"sessionId":f.scope.session_id()},"variables":{}}),
            instance_id: Some(instance),
            idempotency_key: None,
            trigger_source: TriggerSource::Session,
            run_label: None,
            debug: false,
            correlation_id: None,
        })
        .await
        .unwrap();
    f.route.instance_id = instance.to_string();
    configure_route(&mut f.conn, &f.scope, &f.route)
        .await
        .unwrap();
    enqueue(
        &mut f.conn,
        &f.scope,
        "initial-message",
        "initial-operation",
        &json!({"answer":true}),
    )
    .await
    .unwrap();
    assert!(matches!(
        f.client.get_instance_info(&f.route.instance_id).await,
        Err(runtara_server::runtime_client::RuntimeError::InstanceNotFound(_))
    ));
    let DeliveryOutcome::Blocked(blocked) = f.deliver().await else {
        panic!("an unbound message requires explicit resolution")
    };
    assert_eq!(blocked.reason, Some(DeliveryReason::NoTarget));
    assert_eq!(f.request_count().await, 1);
    f.register_instance(&f.route.instance_id, InstanceStatus::Running)
        .await;
    let request = f.register_wait(&f.route.instance_id, "initial").await;
    assert!(matches!(f.deliver().await, DeliveryOutcome::Blocked(_)));
    let page = f
        .client
        .list_input_requests(f.scope.tenant_id(), &[f.route.instance_id.clone()], 0, 10)
        .await
        .unwrap();
    assert_eq!(page.requests.len(), 1);
    assert_eq!(page.requests[0].request_id, request);
    assert_eq!(f.request_count().await, 1);
    f.cleanup().await;
}

#[tokio::test]
async fn terminal_session_blocks_response_without_launching_a_replacement() {
    let mut f = Fixture::new().await;
    f.register_wait(&f.route.instance_id, "old-wait").await;
    f.persistence
        .update_instance_status(&f.route.instance_id, InstanceStatus::Completed, None)
        .await
        .unwrap();
    let DeliveryOutcome::Blocked(blocked) = f.deliver().await else {
        panic!("a terminal session must require explicit resolution")
    };
    assert_eq!(blocked.reason, Some(DeliveryReason::NoTarget));
    assert_eq!(blocked.payload().unwrap(), json!({"answer":true}));
    assert!(blocked.target.is_none());
    assert_eq!(f.request_count().await, 0);
    assert_eq!(
        session_route(&mut f.conn, &f.scope)
            .await
            .unwrap()
            .instance_id,
        f.route.instance_id
    );
    assert!(matches!(f.deliver().await, DeliveryOutcome::Blocked(_)));
    f.cleanup().await;
}

#[tokio::test]
async fn lost_ack_replays_the_bound_receipt_without_answering_the_next_wait() {
    for complete in [false, true] {
        let mut f = Fixture::new().await;
        let request = f.register_wait(&f.route.instance_id, "first").await;
        let ClaimOutcome::Claimed(lease) = claim(&mut f.conn, &f.scope, 30_000).await.unwrap()
        else {
            panic!("claim")
        };
        let bound = bind(
            &mut f.conn,
            &f.scope,
            &lease,
            &InputTarget {
                instance_id: f.route.instance_id.clone(),
                request_id: request.clone(),
            },
        )
        .await
        .unwrap();
        let receipt = f
            .client
            .submit_input_response(
                f.scope.tenant_id(),
                &f.route.instance_id,
                &request,
                &bound.operation_id,
                &bound.payload().unwrap(),
            )
            .await
            .unwrap();
        // Lose acknowledgement after persistence accepted the response. A fresh
        // delivery pass must replay the retained operation, not discover a new wait.
        let next = f.register_wait(&f.route.instance_id, "next").await;
        if complete {
            f.persistence
                .update_instance_status(&f.route.instance_id, InstanceStatus::Completed, None)
                .await
                .unwrap();
        }
        f.expire_lease(&bound).await;
        let DeliveryOutcome::Accepted(accepted) = f.deliver().await else {
            panic!("receipt replay, even when the root completed")
        };
        assert_eq!(
            accepted.receipt_id.as_deref(),
            Some(receipt.receipt_id.as_str())
        );
        assert_eq!(accepted.target.as_ref().unwrap().request_id, request);
        assert_eq!(accepted.operation_id, bound.operation_id);
        assert_eq!(f.request_count().await, 0);
        assert!(matches!(f.deliver().await, DeliveryOutcome::Idle));
        let page = f
            .client
            .list_input_requests(f.scope.tenant_id(), &[f.route.instance_id.clone()], 0, 10)
            .await
            .unwrap();
        if complete {
            assert_eq!(page.total_count, 0);
        } else {
            assert_eq!(page.total_count, 1);
            assert_eq!(page.requests[0].request_id, next);
        }
        f.cleanup().await;
    }
}

#[tokio::test]
async fn open_waits_retain_an_unbound_response_without_choosing_one() {
    let mut f = Fixture::new().await;
    f.register_wait(&f.route.instance_id, "first").await;
    f.register_wait(&f.route.instance_id, "second").await;
    let DeliveryOutcome::Blocked(blocked) = f.deliver().await else {
        panic!("ambiguous response requires an explicit target")
    };
    assert_eq!(blocked.reason, Some(DeliveryReason::NoTarget));
    assert!(blocked.target.is_none());
    assert_eq!(blocked.payload().unwrap(), json!({"answer":true}));
    assert_eq!(
        f.client
            .list_input_requests(f.scope.tenant_id(), &[f.route.instance_id.clone()], 0, 10)
            .await
            .unwrap()
            .total_count,
        2
    );
    assert_eq!(f.request_count().await, 0);
    f.cleanup().await;
}
