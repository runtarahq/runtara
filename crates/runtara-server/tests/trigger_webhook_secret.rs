// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A Channel trigger's `webhook_secret` authenticates inbound platform calls,
//! so the trigger API must never return it and never take it from a client.
//!
//! These drive the real trigger handlers through a router: the secret is
//! stripped from every response, ignored in create and update requests, and
//! kept across updates even though clients can no longer send it back.
//! Triggers stay inactive, so no webhook is registered and no platform is
//! contacted.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres.

use std::sync::{Arc, Once};

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRef, Request};
use axum::http::{Request as HttpRequest, StatusCode};
use axum::middleware::{Next, from_fn};
use axum::routing::get;
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, IntegrationCompatibility,
};
use runtara_server::api::handlers::triggers as handlers;
use runtara_server::api::repositories::triggers::TriggerRepository;
use runtara_server::auth::{AuthContext, AuthMethod};
use runtara_server::authz::Role;
use runtara_server::product_events::ProductEventSink;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const TENANT: &str = "trigger_webhook_secret_test";

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
static CONFIG: Once = Once::new();

fn database_url() -> String {
    std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL")
}

fn init_config() {
    CONFIG.call_once(|| {
        unsafe {
            std::env::set_var("TENANT_ID", TENANT);
            std::env::set_var("OBJECT_MODEL_DATABASE_URL", database_url());
            std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
        }
        runtara_server::config::init(
            runtara_server::config::Config::from_env().expect("trigger test config"),
        );
    });
}

#[derive(Clone)]
struct TestState {
    pool: PgPool,
    connections: Arc<ConnectionsFacade>,
    events: ProductEventSink,
}

impl FromRef<TestState> for PgPool {
    fn from_ref(state: &TestState) -> Self {
        state.pool.clone()
    }
}

impl FromRef<TestState> for Arc<ConnectionsFacade> {
    fn from_ref(state: &TestState) -> Self {
        state.connections.clone()
    }
}

impl FromRef<TestState> for ProductEventSink {
    fn from_ref(state: &TestState) -> Self {
        state.events.clone()
    }
}

struct Fixture {
    pool: PgPool,
    app: Router,
}

impl Fixture {
    async fn start() -> Self {
        init_config();
        let pool = PgPool::connect(&database_url())
            .await
            .expect("required server test database must accept connections");
        MIGRATOR
            .run(&pool)
            .await
            .expect("required server migrations must succeed");

        let connections = Arc::new(ConnectionsFacade::new(ConnectionsState::from_config(
            ConnectionsConfig {
                db_pool: pool.clone(),
                redis_manager: None,
                public_base_url: "http://localhost".to_string(),
                http_client: reqwest::Client::new(),
                cipher: Arc::new(NoOpCipher),
                compatibility: Arc::new(IntegrationCompatibility::new(Default::default())),
                agent_catalog: Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
                    Vec::new(),
                )),
                connection_events: None,
            },
        )));
        let state = TestState {
            pool: pool.clone(),
            connections,
            events: ProductEventSink::new(tokio::sync::mpsc::channel(8).0),
        };

        let inject = move |mut req: Request, next: Next| {
            let mut ctx = AuthContext::new(TENANT.into(), "user-1".into(), AuthMethod::ApiKey);
            ctx.role = Some(Role::Owner);
            req.extensions_mut().insert(ctx);
            next.run(req)
        };
        let app = Router::new()
            .route(
                "/api/runtime/triggers",
                get(handlers::list_invocation_triggers).post(handlers::create_invocation_trigger),
            )
            .route(
                "/api/runtime/triggers/{id}",
                get(handlers::get_invocation_trigger).put(handlers::update_invocation_trigger),
            )
            .route_layer(from_fn(inject))
            .with_state(state);

        Self { pool, app }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn stored_configuration(&self, id: &str) -> Value {
        TriggerRepository::new(self.pool.clone())
            .get_by_id(id, Some(TENANT))
            .await
            .unwrap()
            .expect("trigger exists")
            .configuration
            .unwrap_or(Value::Null)
    }

    async fn cleanup(&self, id: &str) {
        let _ = sqlx::query("DELETE FROM invocation_trigger WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await;
    }
}

#[tokio::test]
async fn the_webhook_secret_is_never_returned_and_never_taken_from_clients() {
    let fixture = Fixture::start().await;
    let workflow_id = Uuid::new_v4().to_string();
    let connection_id = Uuid::new_v4().to_string();
    let channel_request = |configuration: Value| {
        json!({
            "workflow_id": workflow_id,
            "trigger_type": "CHANNEL",
            "active": false,
            "configuration": configuration,
            "single_instance": false,
        })
    };

    // Create: a client-chosen secret is dropped.
    let (status, body) = fixture
        .call(
            "POST",
            "/api/runtime/triggers",
            Some(channel_request(json!({
                "connection_id": connection_id,
                "webhook_secret": "client-chosen",
            }))),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["data"]["id"].as_str().expect("trigger id").to_string();
    assert_eq!(
        body["data"]["configuration"],
        json!({"connection_id": connection_id})
    );
    assert_eq!(
        fixture.stored_configuration(&id).await,
        json!({"connection_id": connection_id}),
        "a client cannot pick the secret"
    );

    // Registration stores the secret and platform.
    TriggerRepository::new(fixture.pool.clone())
        .update_configuration(
            &id,
            &json!({
                "connection_id": connection_id,
                "platform": "telegram",
                "webhook_secret": "registered-secret",
            }),
        )
        .await
        .unwrap();
    let visible = json!({"connection_id": connection_id, "platform": "telegram"});

    // Get and list never return it.
    let (status, body) = fixture
        .call("GET", &format!("/api/runtime/triggers/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["configuration"], visible);

    let (status, body) = fixture.call("GET", "/api/runtime/triggers", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed = body["data"]
        .as_array()
        .expect("trigger list")
        .iter()
        .find(|t| t["id"] == json!(id))
        .expect("trigger listed");
    assert_eq!(listed["configuration"], visible);

    // A save that sends back what the client read keeps the stored secret.
    let (status, body) = fixture
        .call(
            "PUT",
            &format!("/api/runtime/triggers/{id}"),
            Some(channel_request(json!({
                "connection_id": connection_id,
                "platform": "telegram",
                "session_mode": "per_conversation",
            }))),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["configuration"],
        json!({
            "connection_id": connection_id,
            "platform": "telegram",
            "session_mode": "per_conversation",
        })
    );
    assert_eq!(
        fixture.stored_configuration(&id).await,
        json!({
            "connection_id": connection_id,
            "platform": "telegram",
            "session_mode": "per_conversation",
            "webhook_secret": "registered-secret",
        }),
        "the edit applies and the secret survives it"
    );

    // An update cannot overwrite it either.
    let (status, body) = fixture
        .call(
            "PUT",
            &format!("/api/runtime/triggers/{id}"),
            Some(channel_request(json!({
                "connection_id": connection_id,
                "webhook_secret": "client-chosen",
            }))),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["configuration"],
        json!({"connection_id": connection_id})
    );
    assert_eq!(
        fixture.stored_configuration(&id).await["webhook_secret"],
        json!("registered-secret")
    );

    fixture.cleanup(&id).await;
}
