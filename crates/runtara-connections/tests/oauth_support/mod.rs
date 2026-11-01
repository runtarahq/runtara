//! Postgres and Valkey, a connection table, and the connections API and
//! callbacks over them, for the OAuth flow tests.
#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use runtara_connections::config::{EveryTenant, HostFuture, OAuthOptions, TenantHost};
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::service::oauth::OAuthService;
use runtara_connections::{
    Caller, CallerChannel, ConnectionsConfig, IntegrationCompatibility, TenantId,
    connections_router,
};
use serde_json::json;
use sqlx::{PgPool, Row};
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::valkey_support::ValkeyFixture;

pub const BASE: &str = "https://platform.example.com";

pub struct Fixture {
    pub pool: PgPool,
    pub valkey: redis::aio::ConnectionManager,
    _pg: ContainerAsync<Postgres>,
    _valkey: ValkeyFixture,
}

impl Fixture {
    pub async fn start() -> Self {
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
        for statement in [
            r#"CREATE TABLE connection_data_entity (
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
            )"#,
            r#"CREATE TABLE connection_defaults (
                tenant_id VARCHAR(255) NOT NULL,
                default_for VARCHAR(255) NOT NULL,
                connection_id VARCHAR(255) NOT NULL,
                PRIMARY KEY (tenant_id, default_for)
            )"#,
            r#"CREATE TABLE oauth_state (
                state TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                connection_id TEXT NOT NULL,
                integration_id TEXT NOT NULL,
                redirect_uri TEXT NOT NULL,
                code_verifier TEXT,
                expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '10 minutes')
            )"#,
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create table");
        }

        let valkey = ValkeyFixture::start().await;
        Self {
            pool,
            valkey: valkey.manager.clone(),
            _pg: pg,
            _valkey: valkey,
        }
    }

    pub async fn insert(&self, id: &str, tenant: &str, params: serde_json::Value) {
        sqlx::query(
            "INSERT INTO connection_data_entity (id, tenant_id, integration_id, connection_parameters) \
             VALUES ($1, $2, 'http_oauth2_authorization_code', $3)",
        )
        .bind(id)
        .bind(tenant)
        .bind(params)
        .execute(&self.pool)
        .await
        .expect("insert connection");
    }

    pub async fn stored(&self, id: &str) -> (serde_json::Value, String) {
        let row = sqlx::query(
            "SELECT connection_parameters, status FROM connection_data_entity WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .expect("read connection");
        (row.get("connection_parameters"), row.get("status"))
    }

    pub fn service(&self) -> OAuthService {
        OAuthService::new(self.pool.clone(), Arc::new(NoOpCipher), BASE.to_string())
            .with_flows(self.valkey.clone())
    }

    pub fn config(&self, tenants: Arc<dyn TenantHost>) -> ConnectionsConfig {
        let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![]));
        ConnectionsConfig {
            db_pool: self.pool.clone(),
            redis_manager: Some(self.valkey.clone()),
            public_base_url: BASE.to_string(),
            oauth_complete_url: Some(
                url::Url::parse("https://app.example.com/ui/oauth-complete.html").unwrap(),
            ),
            http_client: reqwest::Client::new(),
            cipher: Arc::new(NoOpCipher),
            compatibility: Arc::new(IntegrationCompatibility::from_catalog(&catalog)),
            agent_catalog: catalog,
            connection_events: None,
            oauth: OAuthOptions {
                tenants,
                ..OAuthOptions::default()
            },
        }
    }
}

/// Allow the loopback wiremock endpoints through both dev allow-lists, read
/// once per process: every test sets the same values first.
pub fn allow_loopback_endpoints() {
    // SAFETY: every test in this binary sets the same values.
    unsafe {
        std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost");
        std::env::set_var("RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS", "127.0.0.1,localhost");
    }
}

/// A provider whose token endpoint answers `code` with `access_token`.
pub async fn provider_issuing(server: &MockServer, code: &str, access_token: &str) {
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains(format!("code={code}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": access_token,
            "refresh_token": format!("rt-{access_token}"),
            "expires_in": 3600
        })))
        .mount(server)
        .await;
}

pub fn params(provider: &MockServer, callback: Option<&str>) -> serde_json::Value {
    let mut params = json!({
        "auth_url": format!("{}/oauth/authorize", provider.uri()),
        "token_url": format!("{}/oauth/token", provider.uri()),
        "client_id": "cid",
        "client_secret": "csec",
        "base_url": "https://api.example.com"
    });
    if let Some(callback) = callback {
        params["oauth_callback"] = json!(callback);
    }
    params
}

pub fn query_param(url: &str, key: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find_map(|(k, v)| (k == key).then(|| v.into_owned()))
}

/// The connections API as a tenant's member reaches it.
pub fn api_for(fixture: &Fixture, tenant: &'static str) -> Router {
    api_as(fixture, tenant, CallerChannel::Member)
}

/// The connections API as a caller reaching it by `channel` sees it.
pub fn api_as(fixture: &Fixture, tenant: &'static str, channel: CallerChannel) -> Router {
    connections_router(fixture.config(Arc::new(EveryTenant))).layer(axum::middleware::from_fn(
        move |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
            request
                .extensions_mut()
                .insert(TenantId(tenant.to_string()));
            request.extensions_mut().insert(Caller {
                id: "user-1".to_string(),
                channel,
            });
            next.run(request).await
        },
    ))
}

/// Start a flow through the API: the authorization URL and the cookie set.
pub async fn authorize(api: &Router, connection: &str) -> (String, String) {
    let response = api
        .clone()
        .oneshot(
            Request::get(format!("/connections/{connection}/oauth/authorize"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_string();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (
        body["authorizationUrl"].as_str().unwrap().to_string(),
        cookie,
    )
}

pub async fn call_back(
    callbacks: &Router,
    uri: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::get(uri);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    callbacks
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// `name=value` from a `Set-Cookie` header.
pub fn cookie_pair(set_cookie: &str) -> &str {
    set_cookie.split(';').next().unwrap()
}

pub struct ServesOnly(pub &'static str);

impl TenantHost for ServesOnly {
    fn serves(&self, tenant: &str) -> bool {
        tenant == self.0
    }
    fn scoped(&self, _tenant: String, work: HostFuture) -> HostFuture {
        work
    }
}
