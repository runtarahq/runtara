//! Integration test: the QuickBooks connection type drives a correct Intuit
//! authorization URL through the real `OAuthService::generate_authorization_url`
//! against a live Postgres. Proves the P1 descriptor wiring end-to-end: the
//! authorize endpoint, scopes, and tenant-scoped callback all come from the
//! `quickbooks_online` OAuthConfig, with no QuickBooks-specific code in the flow.

use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::service::oauth::OAuthService;
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
        // Minimal schema for generate_authorization_url: the columns get_with_parameters
        // reads, plus the oauth_state table create_state writes.
        sqlx::query(
            r#"
            CREATE TABLE connection_data_entity (
                id VARCHAR(255) PRIMARY KEY,
                tenant_id VARCHAR(255) NOT NULL,
                integration_id VARCHAR(255),
                connection_subtype VARCHAR(255),
                connection_parameters JSONB,
                rate_limit_config JSONB
            )
            "#,
        )
        .execute(&pool)
        .await
        .ok()?;
        sqlx::query(
            r#"
            CREATE TABLE oauth_state (
                state TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                connection_id TEXT NOT NULL,
                integration_id TEXT NOT NULL,
                redirect_uri TEXT NOT NULL,
                expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '10 minutes')
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

#[tokio::test]
async fn quickbooks_authorize_url_is_descriptor_driven() {
    let Some(fixture) = PgFixture::start().await else {
        eprintln!("Skipping quickbooks authorize url e2e: Docker/Postgres unavailable");
        return;
    };

    sqlx::query(
        "INSERT INTO connection_data_entity (id, tenant_id, integration_id, connection_parameters) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind("conn_qb")
    .bind("tenant_qb")
    .bind("quickbooks_online")
    .bind(serde_json::json!({ "client_id": "QBCID", "client_secret": "QBSEC" }))
    .execute(&fixture.pool)
    .await
    .expect("insert quickbooks connection");

    let service = OAuthService::new(
        fixture.pool.clone(),
        Arc::new(NoOpCipher),
        "https://platform.example.com".to_string(),
    );

    let url = service
        .generate_authorization_url("conn_qb", "tenant_qb")
        .await
        .expect("generate quickbooks authorize url");

    // Authorization endpoint comes from the descriptor, not any QuickBooks-specific code.
    assert!(
        url.starts_with("https://appcenter.intuit.com/connect/oauth2"),
        "unexpected authorize URL: {url}"
    );
    assert!(url.contains("client_id=QBCID"), "url: {url}");
    // Default scopes from the descriptor (dots are URL-unreserved, so unencoded).
    assert!(
        url.contains("scope=com.intuit.quickbooks.accounting"),
        "url: {url}"
    );
    // Tenant-scoped callback built from public_base_url + tenant id.
    assert!(
        url.contains(
            "redirect_uri=https%3A%2F%2Fplatform.example.com%2Fapi%2Foauth%2Ftenant_qb%2Fcallback"
        ),
        "url: {url}"
    );
    assert!(url.contains("state="), "url: {url}");

    // The CSRF state row was persisted for the callback to consume.
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM oauth_state WHERE connection_id = $1")
            .bind("conn_qb")
            .fetch_one(&fixture.pool)
            .await
            .expect("count state rows");
    assert_eq!(count, 1, "authorize must persist exactly one state row");
}
