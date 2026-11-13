//! A client-credentials connection that a refused token request flagged
//! `REQUIRES_RECONNECTION` is `ACTIVE` again once a token is minted for it.
//!
//! The flag comes from the token endpoint refusing the credentials (a wrong
//! secret: `401 invalid_client`). There is no grant behind the connection and
//! no consent to go through again: the remedy is correcting the credentials,
//! or the provider accepting them again. Nothing cleared the flag, so it stayed
//! after the credentials were fixed and every call worked. An
//! authorization-code connection is the other case: its flag asks for a new
//! consent, and a refresh does not stand in for one.
//!
//! Drives `ConnectionsFacade::resolve_connection_auth` against a real Postgres
//! with encrypted parameters, seal, and a wiremock token endpoint.
//!
//! Own binary: sets the loopback egress allowlist (read-once env).

use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::auth::provider_auth::ResolvedConnectionAuth;
use runtara_connections::config::OAuthOptions;
use runtara_connections::crypto::aes_gcm::AesGcmCipher;
use runtara_connections::crypto::{CipherContext, CredentialCipher};
use runtara_connections::repository::connections::{ConnectionRepository, refresh_token_guard};
use runtara_connections::sealed::{Allowances, MemoryKeySource, SealedCredentials};
use runtara_connections::types::{ConnectionStatus, CreateConnectionRequest};
use runtara_connections::{
    ConnectionsConfig, ConnectionsError, ConnectionsFacade, ConnectionsState, EveryAgent,
    IntegrationCompatibility,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zeroize::Zeroizing;

const TENANT: &str = "acme";
const CLIENT_CREDENTIALS: &str = "http_oauth2_client_credentials";
const AUTHORIZATION_CODE: &str = "http_oauth2_authorization_code";
const RIGHT_SECRET: &str = "right-secret";
const WRONG_SECRET: &str = "wrong-secret";

fn allow_loopback() {
    // SAFETY: every test in this binary sets the same values.
    unsafe {
        std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost");
        std::env::set_var("RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS", "127.0.0.1,localhost");
    }
}

struct Fixture {
    pool: PgPool,
    /// A real cipher: the stored parameters are encrypted, as in a
    /// deployment, so they never equal the parameters a request reads.
    cipher: Arc<dyn CredentialCipher>,
    sealed: Arc<SealedCredentials>,
    _pg: ContainerAsync<Postgres>,
}

impl Fixture {
    async fn start(provider: &MockServer) -> Self {
        let pg = Postgres::default()
            .start()
            .await
            .expect("required Docker Postgres container must start");
        let host = pg.get_host().await.expect("Postgres host");
        let port = pg.get_host_port_ipv4(5432).await.expect("Postgres port");
        let pool = PgPool::connect(&format!(
            "postgres://postgres:postgres@{host}:{port}/postgres"
        ))
        .await
        .expect("Postgres connection");
        sqlx::query(
            r#"CREATE TABLE connection_data_entity (
                id VARCHAR(255) PRIMARY KEY,
                tenant_id VARCHAR(255) NOT NULL,
                title VARCHAR(255) NOT NULL DEFAULT 'conn',
                integration_id VARCHAR(255),
                connection_subtype VARCHAR(255),
                connection_parameters JSONB,
                credential_format TEXT NOT NULL DEFAULT 'cipher',
                rate_limit_config JSONB,
                status VARCHAR(64) DEFAULT 'UNKNOWN'
                    -- As the server's schema allows (its migrations).
                    CHECK (status IN ('UNKNOWN', 'ACTIVE', 'REQUIRES_RECONNECTION',
                        'INVALID_CREDENTIALS', 'PENDING_CONFIRMATION')),
                refresh_token_hash TEXT DEFAULT NULL,
                is_default_file_storage BOOLEAN NOT NULL DEFAULT FALSE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                valid_until TIMESTAMPTZ DEFAULT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )"#,
        )
        .execute(&pool)
        .await
        .expect("create the connection table");
        let address = provider.address();
        let sealed = Arc::new(
            SealedCredentials::in_process(
                Arc::new(MemoryKeySource::default()),
                &[7_u8; 32],
                Allowances {
                    private_hosts: vec![format!("{}:{}", address.ip(), address.port())],
                    plain_http_hosts: vec![address.ip().to_string()],
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        Self {
            pool,
            cipher: Arc::new(AesGcmCipher::new(Zeroizing::new(vec![9_u8; 32]), "k1").unwrap()),
            sealed,
            _pg: pg,
        }
    }

    fn repo(&self) -> ConnectionRepository {
        ConnectionRepository::new(self.pool.clone(), self.cipher.clone())
    }

    fn facade(&self) -> ConnectionsFacade {
        let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![]));
        ConnectionsFacade::new(ConnectionsState::from_config(ConnectionsConfig {
            sealed: self.sealed.clone(),
            agent_access: Arc::new(EveryAgent),
            db_pool: self.pool.clone(),
            redis_manager: None,
            public_base_url: "https://platform.example.com".to_string(),
            oauth_complete_url: None,
            http_client: reqwest::Client::new(),
            cipher: self.cipher.clone(),
            compatibility: Arc::new(IntegrationCompatibility::from_catalog(&catalog)),
            agent_catalog: catalog,
            connection_events: None,
            oauth: OAuthOptions::default(),
        }))
    }

    /// Save a connection as create does: its secrets sealed, its parameters
    /// encrypted. Returns the parameters as a request reads them.
    async fn create(
        &self,
        id: &str,
        integration_id: &str,
        plain: Value,
        status: ConnectionStatus,
    ) -> Value {
        let parameters = self.seal(id, integration_id, &plain).await;
        let request: CreateConnectionRequest = serde_json::from_value(json!({
            "title": id,
            "integrationId": integration_id,
            "connectionParameters": parameters,
        }))
        .unwrap();
        self.repo()
            .create(&request, TENANT, id, &status, &[])
            .await
            .expect("create the connection");
        self.read(id).await
    }

    /// Replace a connection's parameters, as an edit of its credentials does
    /// for a client-credentials type: the status is left as it is. Returns the
    /// parameters as a request reads them.
    async fn edit(&self, id: &str, integration_id: &str, plain: Value) -> Value {
        let parameters = self.seal(id, integration_id, &plain).await;
        let encrypted = self
            .cipher
            .encrypt(
                &parameters,
                &CipherContext {
                    tenant_id: TENANT,
                    connection_id: id,
                },
            )
            .unwrap();
        sqlx::query(
            "UPDATE connection_data_entity \
             SET connection_parameters = $2, credential_format = 'seal', updated_at = NOW() \
             WHERE id = $1",
        )
        .bind(id)
        .bind(encrypted)
        .execute(&self.pool)
        .await
        .expect("edit the connection");
        self.read(id).await
    }

    async fn seal(&self, id: &str, integration_id: &str, plain: &Value) -> Value {
        self.sealed
            .seal_parameters(TENANT, id, integration_id, plain, None)
            .await
            .expect("the connection's secrets are sealed")
            .expect("the connection has secrets to seal")
    }

    async fn read(&self, id: &str) -> Value {
        self.repo()
            .get_with_parameters(id, TENANT)
            .await
            .expect("read the connection")
            .and_then(|connection| connection.connection_parameters)
            .expect("the connection has parameters")
    }

    async fn status(&self, id: &str) -> String {
        sqlx::query_scalar("SELECT status FROM connection_data_entity WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .expect("read the connection status")
    }

    async fn set_status(&self, id: &str, status: ConnectionStatus) {
        sqlx::query("UPDATE connection_data_entity SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status.as_str())
            .execute(&self.pool)
            .await
            .expect("set the connection status");
    }

    async fn resolve(
        &self,
        id: &str,
        integration_id: &str,
        parameters: &Value,
    ) -> Result<ResolvedConnectionAuth, ConnectionsError> {
        self.facade()
            .resolve_connection_auth(id, TENANT, integration_id, parameters, &mut HashMap::new())
            .await
    }
}

/// A token endpoint that refuses the wrong secret and grants the right one.
async fn token_endpoint(provider: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!(
            "client_secret={WRONG_SECRET}"
        )))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_client", "error_description": "Client authentication failed"
        })))
        .mount(provider)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!(
            "client_secret={RIGHT_SECRET}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "minted", "token_type": "Bearer", "expires_in": 3600
        })))
        .mount(provider)
        .await;
}

fn client_credentials(provider: &MockServer, secret: &str) -> Value {
    json!({
        "token_url": format!("{}/token", provider.uri()),
        "base_url": format!("{}/api", provider.uri()),
        "client_id": "cid",
        "client_secret": secret,
        "scope": "read",
    })
}

async fn mint_count(provider: &MockServer) -> usize {
    provider
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == "/token")
        .count()
}

#[tokio::test]
async fn corrected_credentials_make_the_connection_active_on_its_next_mint() {
    allow_loopback();
    let provider = MockServer::start().await;
    token_endpoint(&provider).await;
    let fixture = Fixture::start(&provider).await;
    let id = "conn-cc-corrected";

    let wrong = fixture
        .create(
            id,
            CLIENT_CREDENTIALS,
            client_credentials(&provider, WRONG_SECRET),
            ConnectionStatus::Active,
        )
        .await;
    let Err(error) = fixture.resolve(id, CLIENT_CREDENTIALS, &wrong).await else {
        panic!("a wrong secret must not get a token");
    };
    assert!(error.to_string().contains("invalid_client"), "{error}");
    assert_eq!(
        fixture.status(id).await,
        "REQUIRES_RECONNECTION",
        "a refused secret flags the connection"
    );

    // The secret is corrected; the edit leaves the flag.
    let right = fixture
        .edit(
            id,
            CLIENT_CREDENTIALS,
            client_credentials(&provider, RIGHT_SECRET),
        )
        .await;
    assert_eq!(fixture.status(id).await, "REQUIRES_RECONNECTION");

    let resolved = fixture
        .resolve(id, CLIENT_CREDENTIALS, &right)
        .await
        .expect("the corrected secret gets a token");
    assert!(resolved.token_minted);
    assert_eq!(
        fixture.status(id).await,
        "ACTIVE",
        "a token minted for the connection's credentials clears the flag"
    );
}

#[tokio::test]
async fn a_mint_for_parameters_the_connection_no_longer_holds_leaves_it_flagged() {
    // A request that read the parameters before an edit can mint after it:
    // that says nothing about the credentials the connection holds now.
    allow_loopback();
    let provider = MockServer::start().await;
    token_endpoint(&provider).await;
    let fixture = Fixture::start(&provider).await;
    let id = "conn-cc-stale-read";

    let read_before_the_edit = fixture
        .create(
            id,
            CLIENT_CREDENTIALS,
            client_credentials(&provider, RIGHT_SECRET),
            ConnectionStatus::RequiresReconnection,
        )
        .await;
    fixture
        .edit(
            id,
            CLIENT_CREDENTIALS,
            client_credentials(&provider, WRONG_SECRET),
        )
        .await;

    let resolved = fixture
        .resolve(id, CLIENT_CREDENTIALS, &read_before_the_edit)
        .await
        .expect("the secret read before the edit gets a token");
    assert!(resolved.token_minted);
    assert_eq!(fixture.status(id).await, "REQUIRES_RECONNECTION");
}

#[tokio::test]
async fn a_kept_token_leaves_the_status_alone() {
    // Only a token the endpoint issues now says the credentials work; a kept
    // one was minted earlier. It also keeps requests free of status writes.
    allow_loopback();
    let provider = MockServer::start().await;
    token_endpoint(&provider).await;
    let fixture = Fixture::start(&provider).await;
    let id = "conn-cc-kept-token";

    let parameters = fixture
        .create(
            id,
            CLIENT_CREDENTIALS,
            client_credentials(&provider, RIGHT_SECRET),
            ConnectionStatus::Active,
        )
        .await;
    let first = fixture
        .resolve(id, CLIENT_CREDENTIALS, &parameters)
        .await
        .expect("the first request mints");
    assert!(first.token_minted);

    fixture
        .set_status(id, ConnectionStatus::RequiresReconnection)
        .await;
    let second = fixture
        .resolve(id, CLIENT_CREDENTIALS, &parameters)
        .await
        .expect("the second request uses the kept token");
    assert!(!second.token_minted);
    assert_eq!(mint_count(&provider).await, 1);
    assert_eq!(fixture.status(id).await, "REQUIRES_RECONNECTION");
}

#[tokio::test]
async fn an_authorization_code_connection_stays_flagged_after_a_refresh() {
    allow_loopback();
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-2", "refresh_token": "rt-2", "expires_in": 3600
        })))
        .expect(1)
        .mount(&provider)
        .await;
    let fixture = Fixture::start(&provider).await;
    let id = "conn-ac-flagged";

    let parameters = fixture
        .create(
            id,
            AUTHORIZATION_CODE,
            json!({
                "auth_url": format!("{}/oauth/authorize", provider.uri()),
                "token_url": format!("{}/oauth/token", provider.uri()),
                "client_id": "cid",
                "client_secret": "csec",
                "base_url": format!("{}/api", provider.uri()),
                "access_token": "at-1",
                "refresh_token": "rt-1",
                "token_expires_at": "2020-01-01T00:00:00Z",
            }),
            ConnectionStatus::RequiresReconnection,
        )
        .await;
    // The rotation guard a stored refresh token carries.
    sqlx::query("UPDATE connection_data_entity SET refresh_token_hash = $2 WHERE id = $1")
        .bind(id)
        .bind(refresh_token_guard(&parameters["refresh_token"]))
        .execute(&fixture.pool)
        .await
        .unwrap();

    let resolved = fixture
        .resolve(id, AUTHORIZATION_CODE, &parameters)
        .await
        .expect("the refresh succeeds");
    assert!(
        resolved.rotated_sealed.is_some(),
        "the token endpoint must have been asked for this test to mean anything"
    );
    assert!(!resolved.token_minted);
    assert_eq!(
        fixture.status(id).await,
        "REQUIRES_RECONNECTION",
        "only a new consent clears an authorization-code connection's flag"
    );
}
