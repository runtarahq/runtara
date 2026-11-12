//! Integration test: an eBay connection authorizes at its environment's login
//! server, through the real `OAuthService::start` against a live Postgres and
//! Valkey, and sends its RuName as `redirect_uri`, in the authorize URL and in
//! the flow the code exchange reads. The user still comes back to runtara's
//! callback, the URL registered under that RuName. No eBay-specific code is in
//! the flow: the descriptor names the field (`redirect_uri_param`).

use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::service::oauth::{OAuthError, OAuthService, StartBinding};
use runtara_connections::service::oauth_flow::FlowStore;
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

mod valkey_support;

const TENANT: &str = "tenant_ebay";
const RU_NAME: &str = "Runtara_Test-RuntaraT-Runtar-abcdefgh";

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
                credential_format TEXT NOT NULL DEFAULT 'cipher',
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

    async fn insert(&self, id: &str, params: serde_json::Value) {
        sqlx::query(
            "INSERT INTO connection_data_entity (id, tenant_id, integration_id, connection_parameters) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(TENANT)
        .bind("ebay_oauth")
        .bind(params)
        .execute(&self.pool)
        .await
        .expect("insert ebay connection");
    }
}

#[tokio::test]
async fn ebay_sends_its_runame_as_the_redirect_uri_and_comes_back_to_runtara() {
    let fixture = PgFixture::start().await;
    let valkey = valkey_support::ValkeyFixture::start().await;
    let service = OAuthService::new(
        fixture.pool.clone(),
        Arc::new(NoOpCipher),
        "https://platform.example.com".to_string(),
        runtara_connections::sealed::SealedCredentials::for_tests(),
    )
    .with_flows(valkey.manager.clone());

    for (id, environment, auth_url) in [
        (
            "conn_ebay_sandbox",
            "sandbox",
            "https://auth.sandbox.ebay.com/oauth2/authorize?",
        ),
        (
            "conn_ebay_production",
            "production",
            "https://auth.ebay.com/oauth2/authorize?",
        ),
    ] {
        fixture
            .insert(
                id,
                serde_json::json!({
                    "client_id": "Runtara-Test-SBX-0123456789-abcdef01",
                    "client_secret": "SBX-secret",
                    "ru_name": format!(" {RU_NAME} "),
                    "environment": environment,
                }),
            )
            .await;

        let started = service
            .start(id, TENANT, StartBinding::Browser)
            .await
            .expect("generate ebay authorize url");
        let url = &started.authorization_url;
        assert!(url.starts_with(auth_url), "{environment}: {url}");
        assert!(
            url.contains(&format!("&redirect_uri={RU_NAME}&")),
            "url: {url}"
        );
        assert!(!url.contains("platform.example.com"), "url: {url}");
        assert!(
            url.contains("scope=https%3A%2F%2Fapi.ebay.com%2Foauth%2Fapi_scope%20"),
            "url: {url}"
        );
        // What to enter as the RuName's Auth Accepted URL.
        assert_eq!(
            started.redirect_uri,
            format!("https://platform.example.com/api/oauth/{TENANT}/callback")
        );
        // The code exchange sends what the authorize URL sent.
        let flow = FlowStore::new(valkey.manager.clone())
            .take(TENANT, &started.state)
            .await
            .expect("read the flow")
            .expect("the flow is stored");
        assert_eq!(flow.redirect_uri, RU_NAME);
    }

    // Without its RuName, eBay would refuse the authorization: nothing starts.
    for (id, params) in [
        (
            "conn_ebay_no_runame",
            serde_json::json!({ "client_id": "App", "environment": "sandbox" }),
        ),
        (
            "conn_ebay_blank_runame",
            serde_json::json!({ "client_id": "App", "environment": "sandbox", "ru_name": "  " }),
        ),
    ] {
        fixture.insert(id, params).await;
        let err = service
            .start(id, TENANT, StartBinding::Browser)
            .await
            .expect_err("no RuName");
        assert!(
            matches!(&err, OAuthError::MissingParameter(field) if field == "ru_name"),
            "{id}: {err:?}"
        );
    }
}
