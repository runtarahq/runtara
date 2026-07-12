//! Integration test: `create_connection` assigns the correct initial status for
//! interactive-OAuth (authorization-code) connection types, driven through the
//! real `ConnectionService` against a live Postgres.
//!
//! An authorization-code connection (QuickBooks, or the generic
//! `http_oauth2_authorization_code`) created WITHOUT tokens must start
//! `REQUIRES_RECONNECTION`, so the UI surfaces the reconnect affordance instead of
//! a misleading "Connected". A connection seeded with tokens, a client-credentials
//! OAuth type (no interactive consent step), and an explicit caller-supplied status
//! all stay/settle on their expected value.

use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::integration_compatibility::IntegrationCompatibility;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::service::connections::ConnectionService;
use runtara_connections::types::CreateConnectionRequest;
use serde_json::json;
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

struct PgFixture {
    pool: PgPool,
    _container: ContainerAsync<Postgres>,
}

impl PgFixture {
    async fn start() -> Option<Self> {
        let container = Postgres::default().start().await.ok()?;
        let host = container.get_host().await.ok()?;
        let port = container.get_host_port_ipv4(5432).await.ok()?;
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let pool = PgPool::connect(&url).await.ok()?;
        // Full column set that ConnectionRepository::create writes.
        sqlx::query(
            r#"
            CREATE TABLE connection_data_entity (
                id VARCHAR(255) PRIMARY KEY,
                tenant_id VARCHAR(255) NOT NULL,
                title VARCHAR(255),
                connection_subtype VARCHAR(255),
                connection_parameters JSONB,
                integration_id VARCHAR(255),
                valid_until TIMESTAMPTZ,
                status VARCHAR(64),
                rate_limit_config JSONB,
                is_default_file_storage BOOLEAN DEFAULT FALSE
            )
            "#,
        )
        .execute(&pool)
        .await
        .ok()?;
        Some(Self {
            pool,
            _container: container,
        })
    }
}

fn service(pool: &PgPool) -> ConnectionService {
    let repo = Arc::new(ConnectionRepository::new(
        pool.clone(),
        Arc::new(NoOpCipher),
    ));
    let compat = Arc::new(IntegrationCompatibility::new(HashMap::new()));
    ConnectionService::new(repo, compat)
}

async fn create(svc: &ConnectionService, tenant: &str, body: serde_json::Value) -> String {
    let req: CreateConnectionRequest = serde_json::from_value(body).expect("valid request");
    svc.create_connection(req, tenant)
        .await
        .expect("create connection")
}

async fn status_of(pool: &PgPool, id: &str) -> String {
    sqlx::query_scalar::<_, String>("SELECT status FROM connection_data_entity WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("fetch status")
}

#[tokio::test]
async fn create_assigns_reconnection_status_for_unauthorized_oauth() {
    let Some(fx) = PgFixture::start().await else {
        eprintln!("Skipping create_connection status e2e: Docker/Postgres unavailable");
        return;
    };
    let svc = service(&fx.pool);

    // A. QuickBooks, no tokens → REQUIRES_RECONNECTION (the reported bug: was ACTIVE).
    let a = create(
        &svc,
        "t",
        json!({
            "title": "qb-noauth", "integrationId": "quickbooks_online",
            "connectionParameters": {"client_id": "C", "client_secret": "S", "environment": "sandbox"}
        }),
    )
    .await;
    assert_eq!(status_of(&fx.pool, &a).await, "REQUIRES_RECONNECTION");

    // B. QuickBooks pre-seeded with tokens → ACTIVE (usable straight away).
    let b = create(
        &svc,
        "t",
        json!({
            "title": "qb-seeded", "integrationId": "quickbooks_online",
            "connectionParameters": {"client_id": "C", "client_secret": "S", "access_token": "at", "refresh_token": "rt"}
        }),
    )
    .await;
    assert_eq!(status_of(&fx.pool, &b).await, "ACTIVE");

    // C. Generic authorization-code type, no tokens → REQUIRES_RECONNECTION
    //    (params-driven: static auth_url is empty, so oauth_config presence gates it).
    let c = create(
        &svc,
        "t",
        json!({
            "title": "authcode-noauth", "integrationId": "http_oauth2_authorization_code",
            "connectionParameters": {
                "auth_url": "https://p.example.com/authorize", "token_url": "https://p.example.com/token",
                "client_id": "c", "client_secret": "s", "base_url": "https://api.example.com"
            }
        }),
    )
    .await;
    assert_eq!(status_of(&fx.pool, &c).await, "REQUIRES_RECONNECTION");

    // D. Client-credentials OAuth (no interactive consent) → ACTIVE.
    let d = create(
        &svc,
        "t",
        json!({
            "title": "clientcreds", "integrationId": "http_oauth2_client_credentials",
            "connectionParameters": {
                "token_url": "https://p.example.com/token", "client_id": "c",
                "client_secret": "s", "base_url": "https://api.example.com"
            }
        }),
    )
    .await;
    assert_eq!(status_of(&fx.pool, &d).await, "ACTIVE");

    // E. An explicit caller-supplied status is respected, never overridden.
    let e = create(
        &svc,
        "t",
        json!({
            "title": "qb-explicit", "integrationId": "quickbooks_online", "status": "ACTIVE",
            "connectionParameters": {"client_id": "C", "client_secret": "S"}
        }),
    )
    .await;
    assert_eq!(status_of(&fx.pool, &e).await, "ACTIVE");
}
