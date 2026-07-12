//! Full params-driven flow for the generic http_oauth2_authorization_code type
//! against real Postgres + a wiremock OAuth provider: authorize URL built from
//! connection params (PKCE on by default, off on request), the code exchange
//! hitting the param token_url with HTTP Basic creds, token capture on the
//! connection, and a refresh resolving a new Bearer from the param endpoint.
//! Own binary: sets the loopback egress allowlist (read-once env).

use std::sync::Arc;

use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::service::oauth::OAuthService;
use serde_json::json;
use sqlx::PgPool;
use sqlx::Row;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
        sqlx::query(
            r#"
            CREATE TABLE connection_data_entity (
                id VARCHAR(255) PRIMARY KEY,
                tenant_id VARCHAR(255) NOT NULL,
                title VARCHAR(255) NOT NULL DEFAULT 'conn',
                integration_id VARCHAR(255),
                connection_subtype VARCHAR(255),
                connection_parameters JSONB,
                rate_limit_config JSONB,
                status VARCHAR(64) DEFAULT 'UNKNOWN',
                refresh_token_hash TEXT DEFAULT NULL,
                is_default_file_storage BOOLEAN NOT NULL DEFAULT FALSE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                valid_until TIMESTAMPTZ DEFAULT NULL,
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
        sqlx::query(
            r#"
            CREATE TABLE oauth_state (
                state TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                connection_id TEXT NOT NULL,
                integration_id TEXT NOT NULL,
                redirect_uri TEXT NOT NULL,
                code_verifier TEXT,
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

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find_map(|(k, v)| if k == key { Some(v.into_owned()) } else { None })
}

#[tokio::test]
async fn generic_authcode_full_flow_from_params() {
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
    let Some(fixture) = PgFixture::start().await else {
        eprintln!("Skipping generic authcode flow test: Docker/Postgres unavailable");
        return;
    };
    let provider = MockServer::start().await;

    // Token endpoint: requires HTTP Basic (token_auth = "basic"); asserts the
    // PKCE verifier and the code arrive; returns tokens.
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(header("Authorization", "Basic Y2lkOmNzZWM=")) // base64("cid:csec")
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=authcode-123"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-1",
            "refresh_token": "rt-1",
            "token_type": "Bearer",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&provider)
        .await;

    // Connection with ALL OAuth config in params (bring-your-own endpoints).
    let params = json!({
        "auth_url": format!("{}/oauth/authorize", provider.uri()),
        "token_url": format!("{}/oauth/token", provider.uri()),
        "client_id": "cid",
        "client_secret": "csec",
        "scopes": "read write",
        "base_url": "https://api.example.com",
        "token_auth": "basic"
    });
    sqlx::query(
        "INSERT INTO connection_data_entity (id, tenant_id, integration_id, connection_parameters) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind("conn_gen")
    .bind("tenant_gen")
    .bind("http_oauth2_authorization_code")
    .bind(&params)
    .execute(&fixture.pool)
    .await
    .expect("insert connection");

    let service = OAuthService::new(
        fixture.pool.clone(),
        Arc::new(NoOpCipher),
        "https://platform.example.com".to_string(),
    );

    // 1. Authorize URL comes from the PARAM auth_url, with PKCE on by default.
    let auth_url = service
        .generate_authorization_url("conn_gen", "tenant_gen")
        .await
        .expect("authorize url");
    assert!(
        auth_url.starts_with(&format!("{}/oauth/authorize?", provider.uri())),
        "authorize URL must be rooted at the param endpoint: {auth_url}"
    );
    assert!(auth_url.contains("client_id=cid"), "{auth_url}");
    assert!(auth_url.contains("scope=read%20write"), "{auth_url}");
    assert!(
        auth_url.contains("code_challenge_method=S256"),
        "PKCE must default ON for the generic type: {auth_url}"
    );

    // 2. Callback: exchange the code at the param token_url (Basic auth), store tokens.
    let state = extract_query_param(&auth_url, "state").expect("state in authorize url");
    let connection_id = service
        .handle_callback(&state, "authcode-123", &std::collections::HashMap::new())
        .await
        .expect("callback exchange");
    assert_eq!(connection_id, "conn_gen");

    let row = sqlx::query(
        "SELECT connection_parameters, status FROM connection_data_entity WHERE id = 'conn_gen'",
    )
    .fetch_one(&fixture.pool)
    .await
    .expect("read back");
    let stored: serde_json::Value = row.get("connection_parameters");
    let status: String = row.get("status");
    assert_eq!(stored["access_token"], "at-1");
    assert_eq!(stored["refresh_token"], "rt-1");
    assert!(stored["token_expires_at"].is_string());
    assert_eq!(status, "ACTIVE");

    // 3. Refresh: with the access token expired, resolving auth must hit the PARAM
    //    token_url with grant_type=refresh_token (Basic style again) and inject the
    //    fresh Bearer; the rotated refresh token is surfaced for persistence.
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(header("Authorization", "Basic Y2lkOmNzZWM="))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=rt-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-2",
            "refresh_token": "rt-2",
            "token_type": "Bearer",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&provider)
        .await;

    let mut expired = stored.clone();
    expired["token_expires_at"] = json!("2020-01-01T00:00:00Z");

    let client = reqwest::Client::new();
    let mut headers = std::collections::HashMap::new();
    let resolved = runtara_connections::auth::provider_auth::resolve_connection_auth(
        &client,
        "conn_gen",
        "http_oauth2_authorization_code",
        &expired,
        &mut headers,
        &runtara_connections::events::ConnectionEvents::default(),
    )
    .await
    .expect("refresh should succeed");

    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer at-2"),
        "fresh Bearer from the param token endpoint must be injected"
    );
    assert_eq!(
        resolved.base_url.as_deref(),
        Some("https://api.example.com"),
        "base_url pins to the connection's declared host"
    );
    let rotated = resolved
        .rotated_credentials
        .expect("rotated credentials must surface for persistence");
    assert_eq!(rotated.refresh_token.as_deref(), Some("rt-2"));
}

#[tokio::test]
async fn generic_authcode_pkce_can_be_disabled() {
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
    let Some(fixture) = PgFixture::start().await else {
        eprintln!("Skipping pkce-off test: Docker/Postgres unavailable");
        return;
    };
    sqlx::query(
        "INSERT INTO connection_data_entity (id, tenant_id, integration_id, connection_parameters) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind("conn_nopkce")
    .bind("tenant_gen")
    .bind("http_oauth2_authorization_code")
    .bind(json!({
        "auth_url": "https://auth.example.com/authorize",
        "token_url": "https://auth.example.com/token",
        "client_id": "cid",
        "client_secret": "csec",
        "base_url": "https://api.example.com",
        "pkce": false
    }))
    .execute(&fixture.pool)
    .await
    .expect("insert connection");

    let service = OAuthService::new(
        fixture.pool.clone(),
        Arc::new(NoOpCipher),
        "https://platform.example.com".to_string(),
    );
    let url = service
        .generate_authorization_url("conn_nopkce", "tenant_gen")
        .await
        .expect("authorize url");
    assert!(
        !url.contains("code_challenge"),
        "pkce=false must disable the code challenge: {url}"
    );
}

#[tokio::test]
async fn rule_e_endpoint_edit_clears_tokens_and_evicts_cache() {
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
    // Save-time https gate: allow the loopback wiremock endpoints for this PATCH.
    unsafe { std::env::set_var("RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS", "127.0.0.1,localhost") };
    let Some(fixture) = PgFixture::start().await else {
        eprintln!("Skipping rule E test: Docker/Postgres unavailable");
        return;
    };
    let provider = MockServer::start().await;
    // Refresh endpoint: each mint hits it once — expect exactly 2 (warm + post-evict).
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh", "token_type": "Bearer", "expires_in": 3600
        })))
        .expect(2)
        .mount(&provider)
        .await;

    let params = json!({
        "auth_url": format!("{}/oauth/authorize", provider.uri()),
        "token_url": format!("{}/oauth/token", provider.uri()),
        "client_id": "cid",
        "client_secret": "csec",
        "base_url": "https://api.example.com",
        "access_token": "expired",
        "refresh_token": "rt-1",
        "token_expires_at": "2020-01-01T00:00:00Z"
    });
    sqlx::query(
        "INSERT INTO connection_data_entity (id, tenant_id, title, integration_id, connection_parameters, status) \
         VALUES ($1, $2, 'rule-e', $3, $4, 'ACTIVE')",
    )
    .bind("conn_rule_e")
    .bind("tenant_gen")
    .bind("http_oauth2_authorization_code")
    .bind(&params)
    .execute(&fixture.pool)
    .await
    .expect("insert connection");

    // Warm the in-memory cache: expired stored token forces a mint (wiremock hit #1).
    let client = reqwest::Client::new();
    let mut headers = std::collections::HashMap::new();
    runtara_connections::auth::provider_auth::resolve_connection_auth(
        &client,
        "conn_rule_e",
        "http_oauth2_authorization_code",
        &params,
        &mut headers,
        &runtara_connections::events::ConnectionEvents::default(),
    )
    .await
    .expect("warm mint");
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer fresh")
    );

    // PATCH an endpoint param through the SERVICE (rule E path).
    let repo = Arc::new(
        runtara_connections::repository::connections::ConnectionRepository::new(
            fixture.pool.clone(),
            Arc::new(NoOpCipher),
        ),
    );
    let compatibility = Arc::new(
        runtara_connections::integration_compatibility::IntegrationCompatibility::new(
            Default::default(),
        ),
    );
    let service =
        runtara_connections::service::connections::ConnectionService::new(repo, compatibility);

    let mut new_params = params.clone();
    new_params["base_url"] = json!("https://api.other-host.example");
    let update = runtara_connections::types::UpdateConnectionRequest {
        title: None,
        connection_subtype: None,
        connection_parameters: Some(new_params),
        connection_parameter_patch: None,
        integration_id: None,
        rate_limit_config: None,
        valid_until: None,
        status: None,
        is_default_file_storage: None,
        default_for: None,
    };
    let dto = service
        .update_connection("conn_rule_e", "tenant_gen", update)
        .await
        .expect("rule E update");
    assert_eq!(
        dto.status.as_str(),
        "REQUIRES_RECONNECTION",
        "endpoint edit must force reconnect"
    );

    // Captured tokens stripped from the stored params.
    let row = sqlx::query(
        "SELECT connection_parameters FROM connection_data_entity WHERE id = 'conn_rule_e'",
    )
    .fetch_one(&fixture.pool)
    .await
    .expect("read back");
    let stored: serde_json::Value = row.get("connection_parameters");
    assert!(
        stored.get("access_token").is_none(),
        "access_token must be cleared"
    );
    assert!(
        stored.get("refresh_token").is_none(),
        "refresh_token must be cleared"
    );
    assert!(stored.get("token_expires_at").is_none());
    assert_eq!(stored["base_url"], "https://api.other-host.example");

    // Cache evicted: resolving with the OLD params again (an in-flight caller)
    // must MISS the cache and re-mint (wiremock hit #2), not serve the stale token.
    let mut headers2 = std::collections::HashMap::new();
    runtara_connections::auth::provider_auth::resolve_connection_auth(
        &client,
        "conn_rule_e",
        "http_oauth2_authorization_code",
        &params,
        &mut headers2,
        &runtara_connections::events::ConnectionEvents::default(),
    )
    .await
    .expect("post-evict mint");
    // Mock .expect(2) verifies the second network mint on drop.
}
