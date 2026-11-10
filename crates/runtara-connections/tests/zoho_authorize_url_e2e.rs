//! Integration test: a Zoho CRM connection authorizes at its own data centre's
//! login server, through the real `OAuthService::start` against a live
//! Postgres, and asks for a refresh token. The endpoint comes from the
//! descriptor's `hosts` function and the extra parameters from its
//! `authorize_params`, with no Zoho-specific code in the flow.

use std::sync::Arc;

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
async fn zoho_authorizes_at_its_data_centre_and_asks_for_a_refresh_token() {
    let fixture = PgFixture::start().await;
    let valkey = valkey_support::ValkeyFixture::start().await;

    for (id, data_center, auth_url) in [
        (
            "conn_zoho_eu",
            "eu",
            "https://accounts.zoho.eu/oauth/v2/auth?",
        ),
        (
            "conn_zoho_ca",
            "ca",
            "https://accounts.zohocloud.ca/oauth/v2/auth?",
        ),
    ] {
        sqlx::query(
            "INSERT INTO connection_data_entity (id, tenant_id, integration_id, connection_parameters) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind("tenant_zoho")
        .bind("zoho_crm_oauth")
        .bind(serde_json::json!({
            "client_id": "1000.ZOHOCID",
            "client_secret": "ZOHOSEC",
            "data_center": data_center,
            "environment": "developer",
        }))
        .execute(&fixture.pool)
        .await
        .expect("insert zoho connection");

        let service = OAuthService::new(
            fixture.pool.clone(),
            Arc::new(NoOpCipher),
            "https://platform.example.com".to_string(),
        )
        .with_flows(valkey.manager.clone());
        let url = service
            .start(id, "tenant_zoho", StartBinding::Browser)
            .await
            .expect("generate zoho authorize url")
            .authorization_url;

        assert!(url.starts_with(auth_url), "{data_center}: {url}");
        assert!(url.contains("client_id=1000.ZOHOCID"), "url: {url}");
        assert!(url.contains("scope=ZohoCRM.modules.ALL%20"), "url: {url}");
        // Zoho issues a refresh token only for offline access, and a new one
        // on reconnecting only when asked for consent again.
        assert!(
            url.ends_with("&access_type=offline&prompt=consent"),
            "url: {url}"
        );
        assert_eq!(url.matches('?').count(), 1, "url: {url}");
    }
}
