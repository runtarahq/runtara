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
                is_default_file_storage BOOLEAN DEFAULT FALSE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .ok()?;
        sqlx::query(
            r#"
            CREATE TABLE connection_defaults (
                tenant_id VARCHAR(255) NOT NULL,
                default_for VARCHAR(255) NOT NULL,
                connection_id VARCHAR(255) NOT NULL,
                PRIMARY KEY (tenant_id, default_for)
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

/// Raw stored connection_parameters JSON (NoOpCipher = stored as plaintext JSONB).
async fn params_of(pool: &PgPool, id: &str) -> serde_json::Value {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT connection_parameters FROM connection_data_entity WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("fetch params")
}

async fn update(svc: &ConnectionService, id: &str, tenant: &str, body: serde_json::Value) {
    let req: runtara_connections::types::UpdateConnectionRequest =
        serde_json::from_value(body).expect("valid update request");
    svc.update_connection(id, tenant, req)
        .await
        .expect("update connection");
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

/// Authorization-sensitive fields are descriptor-owned. Editing one atomically
/// strips captured tokens and transitions health to REQUIRES_RECONNECTION;
/// unrelated edits keep the grant.
#[tokio::test]
async fn editing_params_applies_descriptor_owned_reauthorization() {
    let Some(fx) = PgFixture::start().await else {
        eprintln!("Skipping update-merge e2e: Docker/Postgres unavailable");
        return;
    };
    let svc = service(&fx.pool);

    // --- QuickBooks (curated): environment selects a different API host and
    // therefore invalidates the captured grant. ---
    let id = create(
        &svc,
        "t",
        json!({
            "title": "qb-prod", "integrationId": "quickbooks_online", "status": "ACTIVE",
            "connectionParameters": {
                "client_id": "QBCID", "client_secret": "QBSEC", "environment": "production",
                "realm_id": "123456789", "access_token": "at-live", "refresh_token": "rt-live"
            }
        }),
    )
    .await;

    let projection = svc
        .get_connection(&id, "t")
        .await
        .expect("safe edit projection")
        .edit_projection
        .expect("projection present");
    assert_eq!(projection.values["client_id"], "QBCID");
    assert_eq!(projection.values["realm_id"], "123456789");
    assert!(projection.values.get("client_secret").is_none());
    assert!(projection.secret_state["client_secret"].configured);
    let stale_version = projection.version.clone();

    // The canonical editor sends only changed ordinary fields. Untouched secrets
    // and provider-captured OAuth values are absent rather than blank placeholders.
    update(
        &svc,
        &id,
        "t",
        json!({
            "connectionParameterPatch": {
                "version": projection.version,
                "set": {"environment": "sandbox"},
                "replaceSecrets": {},
                "clear": []
            }
        }),
    )
    .await;

    let after = params_of(&fx.pool, &id).await;
    assert_eq!(
        after["environment"], "sandbox",
        "environment edit must persist"
    );
    assert!(after.get("access_token").is_none());
    assert!(after.get("refresh_token").is_none());
    assert_eq!(after["realm_id"], "123456789");
    assert_eq!(
        after["client_secret"], "QBSEC",
        "untouched secret keeps existing"
    );
    assert_eq!(status_of(&fx.pool, &id).await, "REQUIRES_RECONNECTION");

    let stale_request = serde_json::from_value(json!({
        "connectionParameterPatch": {
            "version": stale_version,
            "set": {"environment": "production"}
        }
    }))
    .expect("valid stale update");
    assert!(matches!(
        svc.update_connection(&id, "t", stale_request).await,
        Err(runtara_connections::service::connections::ServiceError::Conflict(_))
    ));

    // --- Generic params-driven authcode: an ENDPOINT change strips tokens ---
    let g = create(
        &svc,
        "t",
        json!({
            "title": "authcode", "integrationId": "http_oauth2_authorization_code", "status": "ACTIVE",
            "connectionParameters": {
                "auth_url": "https://a.example.com/authorize", "token_url": "https://a.example.com/token",
                "client_id": "c", "client_secret": "s", "base_url": "https://api.example.com",
                "access_token": "at-g", "refresh_token": "rt-g"
            }
        }),
    )
    .await;

    // Editing an unmarked operational preference keeps the tokens.
    update(
        &svc,
        &g,
        "t",
        json!({ "connectionParameters": { "pkce": false } }),
    )
    .await;
    let g1 = params_of(&fx.pool, &g).await;
    assert_eq!(g1["access_token"], "at-g", "unmarked edit keeps tokens");
    assert_eq!(g1["pkce"], false);

    // Changing the token_url (an endpoint) strips the captured tokens (rule E).
    update(
        &svc,
        &g,
        "t",
        json!({ "connectionParameters": { "token_url": "https://other.example.com/token" } }),
    )
    .await;
    let g2 = params_of(&fx.pool, &g).await;
    assert!(
        g2.get("access_token").is_none(),
        "endpoint change must strip tokens: {g2}"
    );
    assert!(g2.get("refresh_token").is_none());
    assert_eq!(
        status_of(&fx.pool, &g).await,
        "REQUIRES_RECONNECTION",
        "endpoint change forces reconnect"
    );
}

#[tokio::test]
async fn explicit_secret_replace_clear_and_forbidden_clear_are_enforced() {
    let Some(fx) = PgFixture::start().await else {
        eprintln!("Skipping secret lifecycle e2e: Docker/Postgres unavailable");
        return;
    };
    let svc = service(&fx.pool);

    let id = create(
        &svc,
        "t",
        json!({
            "title": "sftp", "integrationId": "sftp",
            "connectionParameters": {
                "host": "files.example.com", "port": 22, "username": "alice",
                "auth_mode": "password", "password": "old-password"
            }
        }),
    )
    .await;
    let projection = svc
        .get_connection(&id, "t")
        .await
        .unwrap()
        .edit_projection
        .unwrap();
    assert!(projection.secret_state["password"].configured);
    assert!(projection.secret_state["password"].clearable);

    update(
        &svc,
        &id,
        "t",
        json!({
            "connectionParameterPatch": {
                "version": projection.version,
                "replaceSecrets": {"password": "new-password"}
            }
        }),
    )
    .await;
    assert_eq!(params_of(&fx.pool, &id).await["password"], "new-password");

    let projection = svc
        .get_connection(&id, "t")
        .await
        .unwrap()
        .edit_projection
        .unwrap();
    update(
        &svc,
        &id,
        "t",
        json!({
            "connectionParameterPatch": {
                "version": projection.version,
                "set": {"auth_mode": "private_key"},
                "replaceSecrets": {"private_key": "key-material"},
                "clear": ["password"]
            }
        }),
    )
    .await;
    let switched = params_of(&fx.pool, &id).await;
    assert!(switched.get("password").is_none());
    assert_eq!(switched["private_key"], "key-material");
    assert_eq!(status_of(&fx.pool, &id).await, "ACTIVE");

    let qb = create(
        &svc,
        "t",
        json!({
            "title": "qb", "integrationId": "quickbooks_online",
            "connectionParameters": {
                "client_id": "client", "client_secret": "required-secret",
                "environment": "sandbox", "scopes": "com.intuit.quickbooks.accounting"
            }
        }),
    )
    .await;
    let version = svc
        .get_connection(&qb, "t")
        .await
        .unwrap()
        .edit_projection
        .unwrap()
        .version;
    let forbidden: runtara_connections::types::UpdateConnectionRequest =
        serde_json::from_value(json!({
            "connectionParameterPatch": {
                "version": version,
                "clear": ["client_secret"]
            }
        }))
        .unwrap();
    assert!(matches!(
        svc.update_connection(&qb, "t", forbidden).await,
        Err(runtara_connections::service::connections::ServiceError::ValidationError(_))
    ));
    assert_eq!(
        params_of(&fx.pool, &qb).await["client_secret"],
        "required-secret"
    );
}
