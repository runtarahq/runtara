// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The runtime raw SQL routes (`/api/runtime/object-model/sql/*`, also behind the
//! MCP SQL tools) driven through the real handlers and the real authorization
//! layer. The three query routes are `database:read`, so they must run read-only
//! and bounded; `sql/execute` must still write, under the statement timeout.
//!
//! Requires the explicit `db-integration-tests` feature and a live Postgres at
//! `TEST_RUNTARA_SERVER_DATABASE_URL` (the object model shares that database,
//! as in `outbound_http`). Own binary: the server config is process-global and
//! this sets a short `RUNTARA_RAW_SQL_STATEMENT_TIMEOUT_MS`.

use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::middleware::{Next, from_fn};
use axum::response::Response;
use axum::routing::post;
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::{
    ConnectionsConfig, ConnectionsFacade, ConnectionsState, IntegrationCompatibility,
};
use runtara_server::api::handlers::object_model::{self as handlers, ObjectModelState};
use runtara_server::api::repositories::object_model::ObjectStoreManager;
use runtara_server::auth::{AuthContext, AuthMethod, MembershipPolicy};
use runtara_server::authz::{ApiKeyScope, Role};
use runtara_server::middleware::authorization::authorize;
use runtara_server::product_events::ProductEventSink;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const TENANT: &str = "raw_sql_routes_test";
const STATEMENT_TIMEOUT_MS: u64 = 250;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
static CONFIG: Once = Once::new();

fn database_url() -> String {
    std::env::var("TEST_RUNTARA_SERVER_DATABASE_URL")
        .or_else(|_| std::env::var("RUNTARA_SERVER_DATABASE_URL"))
        .expect("db-integration-tests requires TEST_RUNTARA_SERVER_DATABASE_URL")
}

fn init_config() {
    CONFIG.call_once(|| {
        // SAFETY: runs once, before any test in this binary reads the config.
        unsafe {
            std::env::set_var("TENANT_ID", TENANT);
            std::env::set_var("OBJECT_MODEL_DATABASE_URL", database_url());
            std::env::set_var("RUNTARA_MCP_SESSION_STORE", "local");
            std::env::set_var(
                "RUNTARA_RAW_SQL_STATEMENT_TIMEOUT_MS",
                STATEMENT_TIMEOUT_MS.to_string(),
            );
        }
        runtara_server::config::init(
            runtara_server::config::Config::from_env().expect("raw SQL test config"),
        );
    });
}

struct Fixture {
    pool: PgPool,
    state: Arc<ObjectModelState>,
    table: String,
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

        let table = format!("raw_sql_{}", Uuid::new_v4().simple());
        sqlx::query(&format!(
            "CREATE TABLE {table} (id BIGINT PRIMARY KEY, name TEXT NOT NULL)"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {table} (id, name) VALUES (1, 'alpha'), (2, 'beta')"
        ))
        .execute(&pool)
        .await
        .unwrap();

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
        let state = Arc::new(ObjectModelState {
            manager: Arc::new(ObjectStoreManager::new(database_url())),
            pool: pool.clone(),
            connections,
            events: ProductEventSink::new(tokio::sync::mpsc::channel(8).0),
        });
        Self { pool, state, table }
    }

    /// The four SQL routes behind the production authorization layer, called
    /// as an API key with `scope` (issued by an Owner, so only the scope can
    /// narrow it).
    fn app(&self, scope: ApiKeyScope) -> Router {
        let inject = move |mut req: Request, next: Next| {
            let mut ctx = AuthContext::new(TENANT.into(), "user-1".into(), AuthMethod::ApiKey);
            ctx.role = Some(Role::Owner);
            ctx.api_key_scope = scope;
            req.extensions_mut().insert(ctx);
            next.run(req)
        };
        Router::new()
            .route(
                "/api/runtime/object-model/sql/query",
                post(handlers::query_sql),
            )
            .route(
                "/api/runtime/object-model/sql/query-one",
                post(handlers::query_sql_one),
            )
            .route(
                "/api/runtime/object-model/sql/query-raw",
                post(handlers::query_sql_raw),
            )
            .route(
                "/api/runtime/object-model/sql/execute",
                post(handlers::execute_sql),
            )
            .route_layer(from_fn(authorize(MembershipPolicy::Required)))
            .route_layer(from_fn(inject))
            .with_state(self.state.clone())
    }

    async fn row_count(&self) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {}", self.table))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn table_exists(&self, name: &str) -> bool {
        sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass($1)::text")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .unwrap()
            .is_some()
    }

    async fn cleanup(self) {
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS {}", self.table))
            .execute(&self.pool)
            .await;
    }
}

async fn call(app: &Router, route: &str, body: Value) -> (StatusCode, Value) {
    let response: Response = app
        .clone()
        .oneshot(
            HttpRequest::post(format!("/api/runtime/object-model/sql/{route}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn assert_read_only_rejection(status: StatusCode, body: &Value, what: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("read-only"), "{what}: {body}");
    assert!(error.contains("25006"), "{what}: {body}");
}

#[tokio::test]
async fn read_routes_reject_writes_and_ddl_without_effect() {
    let fixture = Fixture::start().await;
    let app = fixture.app(ApiKeyScope::Full);
    let table = fixture.table.clone();
    let created = format!("{table}_created");

    let writes = [
        format!("WITH d AS (DELETE FROM {table} RETURNING 1) SELECT count(*)::bigint AS n FROM d"),
        format!("DROP TABLE {table}"),
        format!("CREATE TABLE {created} (id BIGINT)"),
        format!("INSERT INTO {table} (id, name) VALUES (99, 'x') RETURNING id"),
    ];
    for sql in &writes {
        let (status, body) = call(&app, "query-raw", json!({ "sql": sql })).await;
        assert_read_only_rejection(status, &body, &format!("query-raw {sql}"));

        let (status, body) = call(
            &app,
            "query",
            json!({ "sql": sql, "resultSchema": [{ "name": "n", "type": "integer" }] }),
        )
        .await;
        assert_read_only_rejection(status, &body, &format!("query {sql}"));
    }
    let (status, body) = call(
        &app,
        "query-one",
        json!({ "sql": writes[0], "resultSchema": [{ "name": "n", "type": "integer" }] }),
    )
    .await;
    assert_read_only_rejection(status, &body, "query-one data-modifying CTE");

    assert_eq!(fixture.row_count().await, 2, "no row was deleted or added");
    assert!(fixture.table_exists(&table).await, "table was not dropped");
    assert!(
        !fixture.table_exists(&created).await,
        "no table was created"
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn read_routes_serve_selects_and_execute_still_writes() {
    let fixture = Fixture::start().await;
    let app = fixture.app(ApiKeyScope::Full);
    let table = fixture.table.clone();
    let schema = json!([
        { "name": "id", "type": "integer" },
        { "name": "name", "type": "string" }
    ]);

    let (status, body) = call(
        &app,
        "query",
        json!({
            "sql": format!("SELECT id, name FROM {table} WHERE id >= $1 ORDER BY id"),
            "params": [{ "type": "integer", "value": 1 }],
            "resultSchema": schema,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rowCount"], 2);
    assert_eq!(body["rows"][1], json!({ "id": 2, "name": "beta" }));

    let (status, body) = call(
        &app,
        "query-raw",
        json!({
            "sql": format!("SELECT name FROM {table} WHERE id = $1"),
            "params": [{ "type": "integer", "value": 2 }],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([{ "name": "beta" }]));

    let (status, body) = call(
        &app,
        "query-one",
        json!({
            "sql": format!("SELECT id, name FROM {table} WHERE name = $1"),
            "params": [{ "type": "string", "value": "alpha" }],
            "resultSchema": schema,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["row"], json!({ "id": 1, "name": "alpha" }));

    for (sql, got) in [
        (format!("SELECT id, name FROM {table}"), 2),
        (format!("SELECT id, name FROM {table} WHERE id < 0"), 0),
    ] {
        let (status, body) = call(
            &app,
            "query-one",
            json!({ "sql": sql, "resultSchema": schema }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("exactly one row, got {got}")),
            "{body}"
        );
    }

    let (status, body) = call(
        &app,
        "execute",
        json!({
            "sql": format!("UPDATE {table} SET name = $1 WHERE id = $2"),
            "params": [
                { "type": "string", "value": "gamma" },
                { "type": "integer", "value": 2 }
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rowsAffected"], 1);
    let name: String = sqlx::query_scalar(&format!("SELECT name FROM {table} WHERE id = 2"))
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
    assert_eq!(name, "gamma", "execute committed its write");
    fixture.cleanup().await;
}

#[tokio::test]
async fn statement_timeout_cancels_reads_and_commands() {
    let fixture = Fixture::start().await;
    let app = fixture.app(ApiKeyScope::Full);

    for (route, body) in [
        (
            "query-raw",
            json!({ "sql": "SELECT pg_sleep(5)::text AS slept" }),
        ),
        (
            "query",
            json!({
                "sql": "SELECT pg_sleep(5)::text AS slept",
                "resultSchema": [{ "name": "slept", "type": "string" }],
            }),
        ),
        ("execute", json!({ "sql": "SELECT pg_sleep(5)" })),
    ] {
        let started = Instant::now();
        let (status, body) = call(&app, route, body).await;
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{route} was not cancelled by the statement timeout"
        );
        assert_eq!(status, StatusCode::BAD_REQUEST, "{route}: {body}");
        let error = body["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("statement timeout") && error.contains("57014"),
            "{route}: {body}"
        );
        assert!(
            error.contains(&format!("{STATEMENT_TIMEOUT_MS} ms")),
            "{route}: {body}"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn read_only_api_key_reads_but_cannot_write_or_execute() {
    let fixture = Fixture::start().await;
    let app = fixture.app(ApiKeyScope::ReadOnly);
    let table = fixture.table.clone();

    let (status, body) = call(
        &app,
        "query-raw",
        json!({ "sql": format!("SELECT count(*)::bigint AS n FROM {table}") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([{ "n": 2 }]));

    let (status, body) = call(
        &app,
        "query-raw",
        json!({ "sql": format!("WITH d AS (DELETE FROM {table} RETURNING 1) SELECT 1 AS n") }),
    )
    .await;
    assert_read_only_rejection(status, &body, "read-only key write via query-raw");

    let (status, body) = call(
        &app,
        "execute",
        json!({ "sql": format!("DELETE FROM {table}") }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    assert_eq!(fixture.row_count().await, 2, "nothing was deleted");
    fixture.cleanup().await;
}

#[tokio::test]
async fn boot_privilege_check_detects_a_shared_database() {
    let fixture = Fixture::start().await;
    let facts =
        runtara_server::object_model_privileges::collect_facts(&fixture.pool, &fixture.pool)
            .await
            .unwrap();
    assert!(facts.shares_server_database);
    let superuser: bool =
        sqlx::query_scalar("SELECT rolsuper FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(facts.superuser, superuser);
    fixture.cleanup().await;
}
