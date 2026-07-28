//! Deleting a connection must drop its cached OAuth credentials, not just its row.
//!
//! The access-token cache is process-global and survives the delete on its own, so
//! without eviction a still-fresh token keeps being served for a connection that no
//! longer exists. This drives the real wiring — `ConnectionService::delete_connection`
//! against a live Postgres — rather than calling the eviction helper directly, because
//! the bug is precisely that nothing called it.
//!
//! Uses the client-credentials grant deliberately: the refresh-token grant's first fast
//! path reads the stored `access_token` off the connection row and would never consult
//! the cache. Own binary: sets the loopback egress allowlist (read-once env).

use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::IntegrationCompatibility;
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::service::connections::{ConnectionService, ServiceError};
use serde_json::{Value, json};
use sqlx::PgPool;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TENANT_ID: &str = "tenant-delete-evicts";

struct PgFixture {
    pool: PgPool,
    _container: ContainerAsync<Postgres>,
}

impl PgFixture {
    /// A Postgres holding a single connection row for `connection_id`.
    async fn start(connection_id: &str) -> Self {
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
        // Minimal schema: the columns the delete path reads and writes. `integration_id`
        // stays NULL so the best-effort provider revocation short-circuits — this test is
        // about cache eviction, not about the revoke call.
        sqlx::query(
            r#"
            CREATE TABLE connection_data_entity (
                id VARCHAR(255) PRIMARY KEY,
                tenant_id VARCHAR(255) NOT NULL,
                integration_id VARCHAR(255) DEFAULT NULL,
                connection_subtype VARCHAR(255) DEFAULT NULL,
                connection_parameters JSONB DEFAULT NULL,
                rate_limit_config JSONB DEFAULT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create required connection table");
        sqlx::query("INSERT INTO connection_data_entity (id, tenant_id) VALUES ($1, $2)")
            .bind(connection_id)
            .bind(TENANT_ID)
            .execute(&pool)
            .await
            .expect("seed the connection row");
        Self {
            pool,
            _container: container,
        }
    }

    fn service(&self) -> ConnectionService {
        ConnectionService::new(
            Arc::new(ConnectionRepository::new(
                self.pool.clone(),
                Arc::new(NoOpCipher),
            )),
            Arc::new(IntegrationCompatibility::new(HashMap::new())),
        )
    }
}

/// A token endpoint that mints `token-before` exactly once and `token-after` from then
/// on. Which one comes back is a direct readout of whether the cache was consulted.
async fn token_endpoint() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "token-before", "token_type": "Bearer", "expires_in": 3600
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "token-after", "token_type": "Bearer", "expires_in": 3600
        })))
        .mount(&server)
        .await;
    server
}

fn client_credentials_params(server: &MockServer) -> Value {
    json!({
        "token_url": format!("{}/token", server.uri()),
        "client_id": "cid",
        "client_secret": "csec",
        "scope": "read",
        "base_url": "https://api.example.com",
        "token_auth": "form_body"
    })
}

/// Resolve the connection's auth and return the minted `Authorization` header value.
async fn resolve_bearer(connection_id: &str, params: &Value) -> String {
    let mut headers = HashMap::new();
    runtara_connections::auth::provider_auth::resolve_connection_auth(
        &reqwest::Client::new(),
        connection_id,
        "http_oauth2_client_credentials",
        params,
        &mut headers,
        &runtara_connections::events::ConnectionEvents::default(),
    )
    .await
    .expect("mint should succeed");
    headers
        .remove("Authorization")
        .expect("an Authorization header must be injected")
}

#[tokio::test]
async fn deleting_a_connection_evicts_its_cached_access_token() {
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
    let connection_id = "conn-delete-evicts";
    let server = token_endpoint().await;
    let params = client_credentials_params(&server);
    let fixture = PgFixture::start(connection_id).await;

    assert_eq!(
        resolve_bearer(connection_id, &params).await,
        "Bearer token-before",
        "the first resolution mints from the provider"
    );
    // Proves the cache is actually in play — without a hit here the assertion after the
    // delete would pass for the wrong reason.
    assert_eq!(
        resolve_bearer(connection_id, &params).await,
        "Bearer token-before",
        "the second resolution must be served from cache, not re-minted"
    );

    fixture
        .service()
        .delete_connection(connection_id, TENANT_ID)
        .await
        .expect("delete should succeed");

    assert_eq!(
        resolve_bearer(connection_id, &params).await,
        "Bearer token-after",
        "the deleted connection's cached token must not outlive its row"
    );
}

#[tokio::test]
async fn one_connection_still_caches_its_grants_separately() {
    // Carrying the connection id on the key must not stop the rest of the key from
    // discriminating: one connection can mint several tokens (per scope, endpoint, base
    // URL), and collapsing them onto one entry would serve a token for the wrong scope.
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
    let connection_id = "conn-per-scope";
    let server = MockServer::start().await;
    for scope in ["read", "write"] {
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(format!("scope={scope}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": format!("token-{scope}"), "token_type": "Bearer", "expires_in": 3600
            })))
            .mount(&server)
            .await;
    }

    let params_for = |scope: &str| {
        json!({
            "token_url": format!("{}/token", server.uri()),
            "client_id": "cid",
            "client_secret": "csec",
            "scope": scope,
            "base_url": "https://api.example.com",
            "token_auth": "form_body"
        })
    };

    assert_eq!(
        resolve_bearer(connection_id, &params_for("read")).await,
        "Bearer token-read"
    );
    assert_eq!(
        resolve_bearer(connection_id, &params_for("write")).await,
        "Bearer token-write",
        "a second scope on the same connection must not collide with the first"
    );
    // And the first entry is still there afterwards — the two coexist rather than
    // evicting each other.
    assert_eq!(
        resolve_bearer(connection_id, &params_for("read")).await,
        "Bearer token-read"
    );
}

#[tokio::test]
async fn a_delete_that_matches_no_row_leaves_the_cache_alone() {
    // Eviction runs only after the guarded delete reports a row actually gone. A delete
    // scoped to the wrong tenant matches nothing, so it must not discard the credentials
    // of a connection that is still live.
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
    let connection_id = "conn-delete-noop";
    let server = token_endpoint().await;
    let params = client_credentials_params(&server);
    let fixture = PgFixture::start(connection_id).await;

    assert_eq!(
        resolve_bearer(connection_id, &params).await,
        "Bearer token-before"
    );

    let err = fixture
        .service()
        .delete_connection(connection_id, "some-other-tenant")
        .await
        .expect_err("deleting another tenant's connection must not succeed");
    assert!(
        matches!(err, ServiceError::NotFound(_)),
        "a non-matching delete reports not-found, got {err:?}"
    );

    assert_eq!(
        resolve_bearer(connection_id, &params).await,
        "Bearer token-before",
        "a delete that removed nothing must leave the cached token in place"
    );
}
