//! Durable channel intake at the HTTP boundary, for every provider.
//! Requires an isolated server database (`TEST_RUNTARA_SERVER_DATABASE_URL`)
//! and Valkey.
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use redis::aio::ConnectionManager;
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, crypto::noop::NoOpCipher,
    integration_compatibility::IntegrationCompatibility,
};
use runtara_core::persistence::memory::InMemoryPersistence;
use runtara_environment::{handlers::EnvironmentHandlerState, runner::MockRunner};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::session::ChannelRouter;
use super::teams_auth::tests::{TEST_RSA_E, TEST_RSA_N, TEST_RSA_PEM};
use crate::api::dto::triggers::{CreateInvocationTriggerRequest, TriggerType};
use crate::api::repositories::{triggers::TriggerRepository, workflows::WorkflowRepository};
use crate::runtime_client::{RuntimeClient, RuntimeClientConfig};
use crate::workers::execution_engine::ExecutionEngine;

const TEAMS_APP_ID: &str = "99999999-2222-3333-4444-555555555555";
const TEAMS_KID: &str = "intake-harness-kid";
const TEAMS_SERVICE_URL: &str = "https://smba.trafficmanager.net/amer/";
const SLACK_SECRET: &str = "intake-harness-signing-secret";

#[derive(Clone, Copy, Debug)]
enum Provider {
    Telegram,
    Slack,
    Teams,
    Mailgun,
}

const PROVIDERS: [Provider; 4] = [
    Provider::Telegram,
    Provider::Slack,
    Provider::Teams,
    Provider::Mailgun,
];

impl Provider {
    fn integration_id(self) -> &'static str {
        match self {
            Self::Telegram => "telegram_bot",
            Self::Slack => "slack_bot",
            Self::Teams => "teams_bot",
            Self::Mailgun => "mailgun",
        }
    }

    fn parameters(self) -> Value {
        match self {
            Self::Telegram => json!({"bot_token": "test-token"}),
            Self::Slack => json!({"bot_token": "test-token", "signing_secret": SLACK_SECRET}),
            Self::Teams => {
                json!({"app_id": TEAMS_APP_ID, "app_password": "unused", "app_type": "multi_tenant"})
            }
            Self::Mailgun => json!({"api_key": "test-key", "domain": "mail.example.test"}),
        }
    }

    /// One provider delivery. The same `delivery` value yields the same
    /// provider identity, as a provider redelivery would.
    fn request(self, connection_id: &str, delivery: u32) -> Request<Body> {
        let path = |name: &str| format!("/api/runtime/events/webhook/{name}/{connection_id}");
        match self {
            Self::Telegram => json_request(
                &path("telegram"),
                json!({
                    "update_id": delivery,
                    "message": {"chat": {"id": 42}, "from": {"id": 7}, "text": "hello"},
                }),
            ),
            Self::Slack => {
                let body = json!({
                    "type": "event_callback",
                    "event_id": format!("Ev{delivery}"),
                    "event": {"type": "message", "channel": "C1", "user": "U1", "text": "hello"},
                })
                .to_string();
                let timestamp = chrono::Utc::now().timestamp().to_string();
                let mut mac = Hmac::<Sha256>::new_from_slice(SLACK_SECRET.as_bytes()).unwrap();
                mac.update(format!("v0:{timestamp}:{body}").as_bytes());
                let signature = format!("v0={}", hex::encode(mac.finalize().into_bytes()));
                Request::post(path("slack"))
                    .header("content-type", "application/json")
                    .header("x-slack-request-timestamp", timestamp)
                    .header("x-slack-signature", signature)
                    .body(Body::from(body))
                    .unwrap()
            }
            Self::Teams => {
                let mut request = json_request(
                    &path("teams"),
                    json!({
                        "type": "message",
                        "id": format!("activity-{delivery}"),
                        "text": "hello",
                        "serviceUrl": TEAMS_SERVICE_URL,
                        "conversation": {"id": "conversation"},
                        "from": {"id": "user"},
                        "recipient": {"id": format!("28:{TEAMS_APP_ID}")},
                    }),
                );
                request.headers_mut().insert(
                    "authorization",
                    format!("Bearer {}", teams_token()).parse().unwrap(),
                );
                request
            }
            Self::Mailgun => Request::post(path("mailgun"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "sender=user%40example.test&body-plain=hello&Message-Id=%3Cm{delivery}%40example.test%3E"
                )))
                .unwrap(),
        }
    }
}

fn json_request(path: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Bot Framework metadata and keys, served for the whole test process: the
/// endpoint configuration and JWKS cache are process-global.
fn teams_authority() {
    static AUTHORITY: OnceLock<()> = OnceLock::new();
    AUTHORITY.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let base = format!("http://{}", listener.local_addr().unwrap());
                let openid = json!({"issuer": "mock", "jwks_uri": format!("{base}/keys")});
                let keys = json!({"keys": [{
                    "kid": TEAMS_KID, "kty": "RSA", "use": "sig",
                    "n": TEST_RSA_N, "e": TEST_RSA_E, "endorsements": ["msteams"],
                }]});
                let app = axum::Router::new()
                    .route(
                        "/openid",
                        axum::routing::get(move || async move { axum::Json(openid) }),
                    )
                    .route(
                        "/keys",
                        axum::routing::get(move || async move { axum::Json(keys) }),
                    );
                sender.send(base).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        let base = receiver.recv().unwrap();
        // Read once per process by the Teams validator and the egress guard.
        unsafe {
            std::env::set_var("RUNTARA_TEAMS_OPENID_CONFIG_URL", format!("{base}/openid"));
            std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost");
        }
    });
}

fn teams_token() -> String {
    let now = chrono::Utc::now().timestamp();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEAMS_KID.into());
    encode(
        &header,
        &json!({
            "iss": "https://api.botframework.com",
            "aud": TEAMS_APP_ID,
            "exp": now + 3600,
            "nbf": now - 60,
            "serviceurl": TEAMS_SERVICE_URL,
        }),
        &EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).unwrap(),
    )
    .unwrap()
}

struct Harness {
    pool: PgPool,
    tenant: String,
    workflow_id: String,
    connections: std::sync::Mutex<Vec<String>>,
    /// Runtime state outlives a router, as it outlives a server process.
    runtime: Arc<InMemoryPersistence>,
}

impl Harness {
    async fn new() -> Self {
        teams_authority();
        crate::config::init_for_test();
        let url = std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
            .expect("isolated server database required");
        let pool = PgPool::connect(&url).await.expect("server database");
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let tenant = crate::config::tenant_id().to_string();
        let workflow_id = Uuid::new_v4().to_string();
        let repo = WorkflowRepository::new(pool.clone());
        repo.create(&tenant, &workflow_id, None, &workflow_id, "")
            .await
            .unwrap();
        repo.create_initial_version(
            &tenant,
            &workflow_id,
            "Channel intake",
            "",
            Default::default(),
            false,
        )
        .await
        .unwrap();
        Self {
            pool,
            tenant,
            workflow_id,
            connections: Default::default(),
            runtime: Arc::new(InMemoryPersistence::new()),
        }
    }

    /// A fresh router over the same database, as after a process restart.
    async fn router(&self) -> Arc<ChannelRouter> {
        let persistence = self.runtime.clone();
        let client = Arc::new(RuntimeClient::new(
            Arc::new(EnvironmentHandlerState::new(
                sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy("postgresql://localhost:1/unused")
                    .unwrap(),
                persistence,
                Arc::new(MockRunner::new()),
                std::env::temp_dir(),
            )),
            RuntimeClientConfig::new(Default::default()),
        ));
        let (events, _) = tokio::sync::mpsc::channel(1);
        let engine = Arc::new(ExecutionEngine::new(
            self.pool.clone(),
            Arc::new(WorkflowRepository::new(self.pool.clone())),
            Some(client.clone()),
            None,
            crate::product_events::ProductEventSink::new(events),
        ));
        let connections = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
            ConnectionsConfig {
                db_pool: self.pool.clone(),
                redis_manager: None,
                public_base_url: "http://localhost".into(),
                http_client: runtara_connections::net::build_hardened_client(),
                cipher: Arc::new(NoOpCipher),
                compatibility: Arc::new(IntegrationCompatibility::new(Default::default())),
                agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                    Vec::new(),
                )),
                connection_events: None,
            },
        )));
        let valkey = crate::valkey::ValkeyConfig::from_env().expect("isolated Valkey required");
        let valkey = ConnectionManager::new(redis::Client::open(valkey.connection_url()).unwrap())
            .await
            .unwrap();
        Arc::new(ChannelRouter::new(
            client,
            self.pool.clone(),
            connections,
            engine,
            valkey,
        ))
    }

    /// A connection with an active Channel trigger for the harness workflow.
    async fn connection(&self, provider: Provider) -> String {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO connection_data_entity (id, tenant_id, title, integration_id, connection_parameters, status)
             VALUES ($1, $2, $3, $4, $5, 'ACTIVE')",
        )
        .bind(&id)
        .bind(&self.tenant)
        .bind(format!("{provider:?} intake {id}"))
        .bind(provider.integration_id())
        .bind(provider.parameters())
        .execute(&self.pool)
        .await
        .unwrap();
        let request: CreateInvocationTriggerRequest = serde_json::from_value(json!({
            "workflow_id": self.workflow_id,
            "trigger_type": TriggerType::Channel,
            "active": true,
            "configuration": {"connection_id": id},
        }))
        .unwrap();
        TriggerRepository::new(self.pool.clone())
            .create(&request, Some(&self.tenant), None)
            .await
            .unwrap();
        self.connections.lock().unwrap().push(id.clone());
        id
    }

    /// Remove everything this harness created. The server database is shared
    /// with other suites, and an unrelayed execution request here would be
    /// picked up by their outbox relays.
    async fn cleanup(self) {
        let connections = self.connections.into_inner().unwrap();
        sqlx::query(
            "WITH released AS (
                 DELETE FROM execution_admission_reservations
                 WHERE released_at IS NULL AND request_id IN
                     (SELECT request_id FROM execution_requests WHERE workflow_id = $1)
                 RETURNING tenant_id)
             UPDATE execution_admission_tenants t
             SET reserved_count = t.reserved_count - (SELECT count(*) FROM released)
             WHERE t.tenant_id = $2",
        )
        .bind(&self.workflow_id)
        .bind(&self.tenant)
        .execute(&self.pool)
        .await
        .unwrap();
        for sql in [
            "DELETE FROM execution_requests WHERE workflow_id = $1",
            "DELETE FROM invocation_trigger WHERE workflow_id = $1",
        ] {
            sqlx::query(sql)
                .bind(&self.workflow_id)
                .execute(&self.pool)
                .await
                .unwrap();
        }
        for sql in [
            "DELETE FROM channel_intake WHERE connection_id = ANY($1)",
            "DELETE FROM connection_data_entity WHERE id = ANY($1)",
        ] {
            sqlx::query(sql)
                .bind(&connections)
                .execute(&self.pool)
                .await
                .unwrap();
        }
    }

    async fn intake(&self, connection_id: &str) -> Vec<(Uuid, String, Option<String>)> {
        sqlx::query_as(
            "SELECT intake_id, status, instance_id FROM channel_intake WHERE connection_id = $1",
        )
        .bind(connection_id)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    async fn executions(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT instance_id FROM execution_requests WHERE workflow_id = $1")
            .bind(&self.workflow_id)
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    /// Wait until the connection's single intake row has been processed.
    async fn processed(&self, connection_id: &str) -> (Uuid, String) {
        for _ in 0..100 {
            if let [(id, status, Some(instance))] = self.intake(connection_id).await.as_slice()
                && status == "processed"
            {
                return (*id, instance.clone());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("intake for {connection_id} was not processed");
    }
}

async fn post(router: &Arc<ChannelRouter>, request: Request<Body>) -> StatusCode {
    super::routes(router.clone())
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

/// Run `sql` with `NAME` replaced by a unique object name, returning the name.
async fn install(pool: &PgPool, sql: &str) -> String {
    let name = format!("intake_fault_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&sql.replace("NAME", &name))
        .execute(pool)
        .await
        .unwrap();
    name
}

#[tokio::test]
async fn a_message_that_cannot_be_stored_is_not_acknowledged() {
    let harness = Harness::new().await;
    let router = harness.router().await;
    for provider in PROVIDERS {
        let connection = harness.connection(provider).await;
        let fault = install(
            &harness.pool,
            &format!(
                "CREATE FUNCTION NAME() RETURNS trigger LANGUAGE plpgsql AS $$
                 BEGIN RAISE EXCEPTION 'intake storage unavailable'; END $$;
                 CREATE TRIGGER NAME BEFORE INSERT ON channel_intake FOR EACH ROW
                 WHEN (NEW.connection_id = '{connection}') EXECUTE FUNCTION NAME();"
            ),
        )
        .await;

        let status = post(&router, provider.request(&connection, 1)).await;

        sqlx::raw_sql(&format!(
            "DROP TRIGGER {fault} ON channel_intake; DROP FUNCTION {fault}();"
        ))
        .execute(&harness.pool)
        .await
        .unwrap();
        assert!(status.is_server_error(), "{provider:?}: {status}");
        assert!(harness.intake(&connection).await.is_empty(), "{provider:?}");

        // The provider's retry is then accepted.
        let status = post(&router, provider.request(&connection, 1)).await;
        assert_eq!(status, StatusCode::OK, "{provider:?}");
        harness.processed(&connection).await;
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn an_acknowledged_message_is_launched_after_a_restart() {
    let harness = Harness::new().await;
    for provider in PROVIDERS {
        let connection = harness.connection(provider).await;
        // The first process acknowledges, then cannot launch before it "dies".
        let fault = block_launches(&harness).await;
        let first = harness.router().await;
        let status = post(&first, provider.request(&connection, 1)).await;
        assert_eq!(status, StatusCode::OK, "{provider:?}");
        wait_for_attempt(&harness, &fault).await;
        drop(first);
        unblock_launches(&harness, &fault).await;

        let rows = harness.intake(&connection).await;
        assert_eq!(rows.len(), 1, "{provider:?}");
        assert_eq!(rows[0].1, "pending", "{provider:?}");
        let before = harness.executions().await.len();

        let restarted = harness.router().await;
        restarted.clone().recover_pending().await;

        let (intake_id, instance) = harness.processed(&connection).await;
        assert_eq!(instance, intake_id.to_string(), "{provider:?}");
        let executions = harness.executions().await;
        assert_eq!(executions.len(), before + 1, "{provider:?}");
        assert!(executions.contains(&instance), "{provider:?}");
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn a_redelivered_message_starts_one_execution() {
    let harness = Harness::new().await;
    let router = harness.router().await;
    for provider in PROVIDERS {
        let connection = harness.connection(provider).await;
        let before = harness.executions().await.len();

        assert_eq!(
            post(&router, provider.request(&connection, 7)).await,
            StatusCode::OK,
            "{provider:?}"
        );
        let (intake_id, instance) = harness.processed(&connection).await;
        assert_eq!(
            post(&router, provider.request(&connection, 7)).await,
            StatusCode::OK,
            "{provider:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert_eq!(harness.intake(&connection).await.len(), 1, "{provider:?}");
        assert_eq!(instance, intake_id.to_string(), "{provider:?}");
        assert_eq!(harness.executions().await.len(), before + 1, "{provider:?}");
    }
    harness.cleanup().await;
}

/// Install a fault that fails every launch of the harness workflow, counting
/// attempts in a sequence (sequences are not rolled back with the failure).
async fn block_launches(harness: &Harness) -> String {
    install(
        &harness.pool,
        &format!(
            "CREATE SEQUENCE NAME;
             CREATE FUNCTION NAME() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN PERFORM nextval('NAME'); RAISE EXCEPTION 'launch unavailable'; END $$;
             CREATE TRIGGER NAME BEFORE INSERT ON execution_requests FOR EACH ROW
             WHEN (NEW.workflow_id = '{}') EXECUTE FUNCTION NAME();",
            harness.workflow_id
        ),
    )
    .await
}

async fn wait_for_attempt(harness: &Harness, fault: &str) {
    for _ in 0..100 {
        let (called,): (bool,) = sqlx::query_as(&format!("SELECT is_called FROM {fault}"))
            .fetch_one(&harness.pool)
            .await
            .unwrap();
        if called {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("launch was never attempted");
}

async fn unblock_launches(harness: &Harness, fault: &str) {
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {fault} ON execution_requests; DROP FUNCTION {fault}(); DROP SEQUENCE {fault};"
    ))
    .execute(&harness.pool)
    .await
    .unwrap();
}

/// Stands in for the handoff grace period elapsing.
async fn make_due(harness: &Harness, connection_id: &str) {
    sqlx::query("UPDATE channel_intake SET next_attempt_at = NOW() WHERE connection_id = $1")
        .bind(connection_id)
        .execute(&harness.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_transient_launch_failure_is_retried_without_a_restart() {
    let harness = Harness::new().await;
    let router = harness.router().await;
    for provider in PROVIDERS {
        let connection = harness.connection(provider).await;
        let fault = block_launches(&harness).await;
        assert_eq!(
            post(&router, provider.request(&connection, 3)).await,
            StatusCode::OK,
            "{provider:?}"
        );
        wait_for_attempt(&harness, &fault).await;
        unblock_launches(&harness, &fault).await;
        let before = harness.executions().await.len();

        // Within the handoff grace period the sweep leaves the row alone.
        router.sweep_intake().await;
        let rows = harness.intake(&connection).await;
        assert_eq!(rows[0].1, "pending", "{provider:?}");

        make_due(&harness, &connection).await;
        router.sweep_intake().await;

        let (intake_id, instance) = harness.processed(&connection).await;
        assert_eq!(instance, intake_id.to_string(), "{provider:?}");
        assert_eq!(harness.executions().await.len(), before + 1, "{provider:?}");
        let (attempts,): (i32,) =
            sqlx::query_as("SELECT attempts FROM channel_intake WHERE intake_id = $1")
                .bind(intake_id)
                .fetch_one(&harness.pool)
                .await
                .unwrap();
        assert_eq!(attempts, 1, "{provider:?}");
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn a_launch_uses_the_version_current_when_the_message_was_accepted() {
    let harness = Harness::new().await;
    let router = harness.router().await;
    let connection = harness.connection(Provider::Telegram).await;
    let fault = block_launches(&harness).await;
    assert_eq!(
        post(&router, Provider::Telegram.request(&connection, 1)).await,
        StatusCode::OK
    );
    wait_for_attempt(&harness, &fault).await;

    // A new version is published before the retried launch runs.
    let repo = WorkflowRepository::new(harness.pool.clone());
    let published = repo
        .create_version(&harness.tenant, &harness.workflow_id, &json!({}))
        .await
        .unwrap();
    assert!(published > 1);
    repo.set_current_version(&harness.tenant, &harness.workflow_id, published)
        .await
        .unwrap();
    unblock_launches(&harness, &fault).await;
    make_due(&harness, &connection).await;
    router.sweep_intake().await;

    let (_, instance) = harness.processed(&connection).await;
    let (version,): (Option<i32>,) =
        sqlx::query_as("SELECT workflow_version FROM execution_requests WHERE instance_id = $1")
            .bind(&instance)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
    assert_eq!(version, Some(1));

    // A message accepted after publication launches the new version.
    let later = harness.connection(Provider::Telegram).await;
    assert_eq!(
        post(&router, Provider::Telegram.request(&later, 2)).await,
        StatusCode::OK
    );
    let (_, instance) = harness.processed(&later).await;
    let (version,): (Option<i32>,) =
        sqlx::query_as("SELECT workflow_version FROM execution_requests WHERE instance_id = $1")
            .bind(&instance)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
    assert_eq!(version, Some(published));
    harness.cleanup().await;
}

#[tokio::test]
async fn handled_rows_are_purged_after_retention_and_stuck_rows_fail() {
    use super::intake::{IntakeStore, MAX_ATTEMPTS, PinnedWorkflow, RETENTION};
    let harness = Harness::new().await;
    let connection = harness.connection(Provider::Telegram).await;
    let store = IntakeStore::new(harness.pool.clone());
    let (trigger,): (String,) =
        sqlx::query_as("SELECT id FROM invocation_trigger WHERE workflow_id = $1")
            .bind(&harness.workflow_id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
    let workflow = PinnedWorkflow {
        id: harness.workflow_id.clone(),
        version: 1,
    };
    for delivery in [1, 2, 3] {
        let message = super::session::InboundMessage {
            text: "hello".into(),
            sender_id: "sender".into(),
            conv_id: "conversation".into(),
            channel: "telegram".into(),
            attachments: vec![],
            original_message: json!({}),
            target: None,
            activity_id: Some(delivery.to_string()),
            intake_id: None,
            workflow: None,
        };
        store
            .accept(&harness.tenant, &connection, &trigger, &workflow, &message)
            .await
            .unwrap();
    }
    let identity = |delivery: u32| format!("id:{delivery}");
    let age = |delivery: u32, status: &'static str, days: i32| {
        let pool = harness.pool.clone();
        let connection = connection.clone();
        async move {
            sqlx::query(
                "UPDATE channel_intake
                 SET status = $3, updated_at = NOW() - make_interval(days => $4)
                 WHERE connection_id = $1 AND identity = $2",
            )
            .bind(&connection)
            .bind(identity(delivery))
            .bind(status)
            .bind(days)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    // Expired, recent, and an old row that was never handled.
    age(1, "processed", 8).await;
    age(2, "processed", 1).await;
    age(3, "pending", 30).await;
    assert!(RETENTION < Duration::from_secs(8 * 24 * 3600));

    store.purge(&harness.tenant, RETENTION, 1000).await.unwrap();
    let mut left: Vec<String> =
        sqlx::query_scalar("SELECT identity FROM channel_intake WHERE connection_id = $1")
            .bind(&connection)
            .fetch_all(&harness.pool)
            .await
            .unwrap();
    left.sort();
    assert_eq!(left, vec![identity(2), identity(3)]);

    // A row that keeps failing is eventually failed, not retried forever.
    sqlx::query(
        "UPDATE channel_intake SET attempts = $3, next_attempt_at = NOW()
         WHERE connection_id = $1 AND identity = $2",
    )
    .bind(&connection)
    .bind(identity(3))
    .bind(MAX_ATTEMPTS)
    .execute(&harness.pool)
    .await
    .unwrap();
    assert!(
        store
            .claim_due(&harness.tenant, 100)
            .await
            .unwrap()
            .iter()
            .all(|row| row.connection_id != connection)
    );
    let (status,): (String,) = sqlx::query_as(
        "SELECT status FROM channel_intake WHERE connection_id = $1 AND identity = $2",
    )
    .bind(&connection)
    .bind(identity(3))
    .fetch_one(&harness.pool)
    .await
    .unwrap();
    assert_eq!(status, "failed");
    harness.cleanup().await;
}

#[tokio::test]
async fn a_reply_left_by_an_ended_session_is_delivered_not_relaunched() {
    use super::intake::{IntakeStore, PinnedWorkflow, ReplyBinding};
    use crate::api::services::session_queue::managed::{self, QueueScope};
    use runtara_core::domain::InstanceStatus;
    use runtara_core::persistence::{
        Persistence,
        inputs::{InputAuthority, InputRequestSpec},
    };

    let harness = Harness::new().await;
    let router = harness.router().await;
    let connection = harness.connection(Provider::Telegram).await;
    let store = IntakeStore::new(harness.pool.clone());
    let (trigger,): (String,) =
        sqlx::query_as("SELECT id FROM invocation_trigger WHERE workflow_id = $1")
            .bind(&harness.workflow_id)
            .fetch_one(&harness.pool)
            .await
            .unwrap();
    let valkey = crate::valkey::ValkeyConfig::from_env().unwrap();
    let mut valkey = ConnectionManager::new(redis::Client::open(valkey.connection_url()).unwrap())
        .await
        .unwrap();

    let instance = Uuid::new_v4().to_string();
    harness
        .runtime
        .register_instance(&instance, &harness.tenant)
        .await
        .unwrap();
    harness
        .runtime
        .update_instance_status(&instance, InstanceStatus::Running, None)
        .await
        .unwrap();
    let open = |schema: Value| {
        let runtime = harness.runtime.clone();
        let tenant = harness.tenant.clone();
        let instance = instance.clone();
        async move {
            runtime
                .input_requests()
                .unwrap()
                .register_input(
                    &InputAuthority::Root {
                        tenant_id: tenant,
                        instance_id: instance,
                    },
                    &InputRequestSpec {
                        signal_id: Uuid::new_v4().to_string(),
                        deadline: None,
                        response_schema: Some(schema),
                        metadata: json!({"message": "Answer"}),
                    },
                )
                .await
                .unwrap()
                .request_id
        }
    };
    let plain = open(json!({"message": {"type": "string", "required": true}})).await;
    let structured = open(json!({
        "name": {"type": "string", "required": true},
        "age": {"type": "number", "required": true},
    }))
    .await;
    let closed = Uuid::new_v4().to_string();

    // Each reply was bound and buffered, then its session ended before the
    // handoff to the managed queue.
    let session = Uuid::new_v4().to_string();
    let scope = QueueScope::new(&harness.tenant, &session).unwrap();
    let before = harness.executions().await.len();
    let mut replies = Vec::new();
    for (delivery, request) in [(1, &plain), (2, &structured), (3, &closed)] {
        let message = super::session::InboundMessage {
            text: format!("reply {delivery}"),
            sender_id: "sender".into(),
            conv_id: "conversation".into(),
            channel: "telegram".into(),
            attachments: vec![],
            original_message: json!({}),
            target: None,
            activity_id: Some(format!("reply-{delivery}")),
            intake_id: None,
            workflow: None,
        };
        let workflow = PinnedWorkflow {
            id: harness.workflow_id.clone(),
            version: 1,
        };
        let super::intake::Accepted::New(intake_id) = store
            .accept(&harness.tenant, &connection, &trigger, &workflow, &message)
            .await
            .unwrap()
        else {
            panic!("reply {delivery} was not new");
        };
        store
            .bind_reply(
                intake_id,
                &ReplyBinding {
                    session_id: session.clone(),
                    instance_id: instance.clone(),
                    request_id: request.clone(),
                    payload: json!({"message": format!("reply {delivery}")}),
                },
            )
            .await
            .unwrap();
        replies.push(intake_id);
    }

    for _ in 0..2 {
        make_due(&harness, &connection).await;
        router.sweep_intake().await;
    }

    let outcome = |intake_id: Uuid| {
        let pool = harness.pool.clone();
        async move {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT status, outcome FROM channel_intake WHERE intake_id = $1",
            )
            .bind(intake_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // The plain reply is handed to its own request, once.
    assert_eq!(
        outcome(replies[0]).await,
        ("processed".into(), Some("reply".into()))
    );
    let envelope = managed::get(&mut valkey, &scope, &replies[0].to_string())
        .await
        .unwrap();
    let envelope_enqueued_at = envelope.enqueued_at_ms;
    assert_eq!(
        envelope.target.map(|target| target.request_id),
        Some(plain.clone())
    );
    // Crash after the handoff but before the row was marked: recovery finds
    // the queued reply and does not queue it again.
    sqlx::query(
        "UPDATE channel_intake SET status = 'pending', outcome = NULL WHERE intake_id = $1",
    )
    .bind(replies[0])
    .execute(&harness.pool)
    .await
    .unwrap();
    make_due(&harness, &connection).await;
    router.sweep_intake().await;
    assert_eq!(
        outcome(replies[0]).await,
        ("processed".into(), Some("reply".into()))
    );
    assert_eq!(
        managed::get(&mut valkey, &scope, &replies[0].to_string())
            .await
            .unwrap()
            .enqueued_at_ms,
        envelope_enqueued_at
    );
    // A structured reply cannot finish collection alone; a closed request's
    // reply is not re-aimed. Neither is queued.
    assert_eq!(
        outcome(replies[1]).await,
        ("processed".into(), Some("interrupted".into()))
    );
    assert_eq!(
        outcome(replies[2]).await,
        ("processed".into(), Some("undelivered".into()))
    );
    for intake_id in &replies[1..] {
        assert!(matches!(
            managed::get(&mut valkey, &scope, &intake_id.to_string()).await,
            Err(managed::QueueError::NotFound)
        ));
    }
    // None of them started a run.
    assert_eq!(harness.executions().await.len(), before);
    harness.cleanup().await;
}
