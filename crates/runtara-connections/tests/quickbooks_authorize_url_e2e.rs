//! Integration test: the QuickBooks connection type drives a correct Intuit
//! authorization URL through the real `OAuthService::generate_authorization_url`
//! against a live Postgres. Proves the P1 descriptor wiring end-to-end: the
//! authorize endpoint, scopes, and tenant-scoped callback all come from the
//! `quickbooks_online` OAuthConfig, with no QuickBooks-specific code in the flow.

use std::sync::Arc;

use redis::AsyncCommands;
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::service::oauth::{OAuthService, StartBinding};
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

mod valkey_support;

struct PgFixture {
    pool: PgPool,
    _container: ContainerAsync<Postgres>,
}

impl PgFixture {
    async fn start() -> Self {
        let container = Postgres::default()
            .start()
            .await
            .expect("required Docker Postgres container must start");
        let host = container.get_host().await.expect("required Postgres host");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("required Postgres port");
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let pool = PgPool::connect(&url)
            .await
            .expect("required Postgres connection");
        // Minimal schema for starting a flow: the columns get_with_parameters reads.
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
        .expect("create required connection table");
        Self {
            pool,
            _container: container,
        }
    }
}

#[tokio::test]
async fn quickbooks_authorize_url_is_descriptor_driven() {
    let fixture = PgFixture::start().await;
    let valkey = valkey_support::ValkeyFixture::start().await;

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
    )
    .with_flows(valkey.manager.clone());

    let flow = service
        .start("conn_qb", "tenant_qb", StartBinding::Browser)
        .await
        .expect("generate quickbooks authorize url");
    let url = flow.authorization_url.clone();

    // Authorization endpoint comes from the descriptor, not any QuickBooks-specific code.
    assert!(
        url.starts_with("https://appcenter.intuit.com/connect/oauth2"),
        "unexpected authorize URL: {url}"
    );
    // response_type=code is mandatory for the auth-code grant (RFC 6749 §4.1.1);
    // Intuit rejects the request without it ("response_type query parameter is missing").
    assert!(url.contains("response_type=code"), "url: {url}");
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
    // PKCE (RFC 7636): the descriptor enables it, so the authorize URL carries an
    // S256 code_challenge.
    assert!(url.contains("code_challenge="), "url: {url}");
    assert!(url.contains("code_challenge_method=S256"), "url: {url}");

    // The flow was stored, under the tenant, with a PKCE verifier for the
    // callback to send.
    // The flow is this Valkey's only one.
    let mut valkey = valkey.manager.clone();
    let records: Vec<String> = valkey
        .keys("tenant_qb:oauth_state:*")
        .await
        .expect("list flow records");
    let [record_key] = records.as_slice() else {
        panic!("one flow record under tenant_qb: {records:?}");
    };
    let record: String = valkey.get(record_key).await.expect("read flow record");
    let record: serde_json::Value = serde_json::from_str(&record).expect("flow record");
    assert_eq!(record["connection_id"], "conn_qb");
    assert!(
        record["code_verifier"]
            .as_str()
            .is_some_and(|v| v.len() == 43),
        "a PKCE verifier must be kept with the flow"
    );
}
