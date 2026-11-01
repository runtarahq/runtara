//! OAuth flows held in Valkey, the callback that names no tenant, and the
//! browser binding, against real Postgres and Valkey and a wiremock provider.
//!
//! - A flow's record lives under its tenant's prefix; the global index holds
//!   only the tenant, keyed by the state's hash. A state redeems once and
//!   expires with the flow.
//! - A flow started in the console completes only with its own cookie, and
//!   the callback clears that cookie whatever the outcome.
//! - Two tenants' flows through one tenant-less callback each land on their
//!   own connection; a tenant served elsewhere is sent to its own callback.
//! - A connection made before the tenant-less callback keeps its tenant's
//!   redirect URI; a flow left in the old `oauth_state` table still completes.
//!
//! Own binary: sets the loopback egress allowlist (read-once env).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use redis::AsyncCommands;
use runtara_connections::config::{EveryTenant, HostFuture, OAuthOptions, TenantHost};
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::oauth::OAuthRepository;
use runtara_connections::service::oauth::{OAuthError, OAuthService, StartBinding};
use runtara_connections::service::oauth_flow::{self, sha256_hex};
use runtara_connections::{
    ConnectionsConfig, IntegrationCompatibility, TenantId, connections_router,
    oauth_callback_router,
};
use serde_json::json;
use sqlx::{PgPool, Row};
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod valkey_support;
use valkey_support::ValkeyFixture;

const BASE: &str = "https://platform.example.com";

struct Fixture {
    pool: PgPool,
    valkey: redis::aio::ConnectionManager,
    _pg: ContainerAsync<Postgres>,
    _valkey: ValkeyFixture,
}

impl Fixture {
    async fn start() -> Self {
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

    async fn insert(&self, id: &str, tenant: &str, params: serde_json::Value) {
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

    async fn stored(&self, id: &str) -> (serde_json::Value, String) {
        let row = sqlx::query(
            "SELECT connection_parameters, status FROM connection_data_entity WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .expect("read connection");
        (row.get("connection_parameters"), row.get("status"))
    }

    fn service(&self) -> OAuthService {
        OAuthService::new(self.pool.clone(), Arc::new(NoOpCipher), BASE.to_string())
            .with_flows(self.valkey.clone())
    }

    fn config(&self, tenants: Arc<dyn TenantHost>) -> ConnectionsConfig {
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
fn allow_loopback_endpoints() {
    // SAFETY: every test in this binary sets the same values.
    unsafe {
        std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost");
        std::env::set_var("RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS", "127.0.0.1,localhost");
    }
}

/// A provider whose token endpoint answers `code` with `access_token`.
async fn provider_issuing(server: &MockServer, code: &str, access_token: &str) {
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

fn params(provider: &MockServer, callback: Option<&str>) -> serde_json::Value {
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

fn query_param(url: &str, key: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find_map(|(k, v)| (k == key).then(|| v.into_owned()))
}

#[tokio::test]
async fn a_flows_record_is_under_its_tenant_and_redeems_once() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-1", "at-1").await;
    fixture
        .insert("c1", "acme", params(&provider, Some("tenantless")))
        .await;
    let service = fixture.service();

    let flow = service
        .start("c1", "acme", StartBinding::Browser)
        .await
        .expect("start");
    assert_eq!(flow.redirect_uri, format!("{BASE}/api/oauth/callback"));
    assert_eq!(
        query_param(&flow.authorization_url, "redirect_uri").as_deref(),
        Some(flow.redirect_uri.as_str())
    );
    let nonce = flow
        .browser_nonce
        .clone()
        .expect("a browser flow has a nonce");

    // The index holds the tenant alone, keyed by the state's hash, and the
    // record lives under the tenant's prefix; both expire with the flow.
    let hash = sha256_hex(&flow.state);
    let mut valkey = fixture.valkey.clone();
    let index: String = valkey
        .get(format!("runtara:oauth_state:{hash}"))
        .await
        .unwrap();
    assert_eq!(index, "acme");
    let record: String = valkey
        .get(format!("acme:oauth_state:{hash}"))
        .await
        .unwrap();
    assert!(record.contains("\"connection_id\":\"c1\""));
    assert!(!record.contains(&nonce), "only the nonce's hash is stored");
    for key in [
        format!("runtara:oauth_state:{hash}"),
        format!("acme:oauth_state:{hash}"),
    ] {
        let ttl: i64 = valkey.ttl(&key).await.unwrap();
        assert!((590..=600).contains(&ttl), "{key}: ttl {ttl}");
    }
    let keys: Vec<String> = valkey.keys("*").await.unwrap();
    assert!(
        keys.iter().all(|key| !key.contains(&flow.state)),
        "no key names the state itself: {keys:?}"
    );
    assert_eq!(
        service.tenant_of(&flow.state).await.unwrap().as_deref(),
        Some("acme")
    );

    // Another tenant cannot redeem it, and the state is not spent by trying.
    assert!(matches!(
        service
            .complete("beta", &flow.state, "code-1", &HashMap::new(), Some(&nonce))
            .await,
        Err(OAuthError::InvalidState)
    ));
    let completed = service
        .complete("acme", &flow.state, "code-1", &HashMap::new(), Some(&nonce))
        .await
        .expect("complete");
    assert_eq!(completed.connection_id, "c1");
    let (stored, status) = fixture.stored("c1").await;
    assert_eq!(stored["access_token"], "at-1");
    assert_eq!(status, "ACTIVE");

    // Redeemed once: both keys are gone and a replay is refused.
    let left: Vec<String> = valkey.keys("*oauth_state*").await.unwrap();
    assert!(left.is_empty(), "{left:?}");
    assert!(matches!(
        service
            .complete("acme", &flow.state, "code-1", &HashMap::new(), Some(&nonce))
            .await,
        Err(OAuthError::InvalidState)
    ));
}

#[tokio::test]
async fn a_browser_flow_needs_its_own_cookie() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-1", "at-1").await;
    fixture.insert("c1", "acme", params(&provider, None)).await;
    let service = fixture.service();

    for presented in [None, Some("another-flows-nonce".to_string())] {
        let flow = service
            .start("c1", "acme", StartBinding::Browser)
            .await
            .unwrap();
        let other = service
            .start("c1", "acme", StartBinding::Browser)
            .await
            .unwrap();
        let presented = presented.map(|_| other.browser_nonce.clone().unwrap());
        assert!(matches!(
            service
                .complete(
                    "acme",
                    &flow.state,
                    "code-1",
                    &HashMap::new(),
                    presented.as_deref()
                )
                .await,
            Err(OAuthError::BrowserMismatch)
        ));
        // The flow is spent: the right cookie cannot finish it afterwards.
        assert!(matches!(
            service
                .complete(
                    "acme",
                    &flow.state,
                    "code-1",
                    &HashMap::new(),
                    flow.browser_nonce.as_deref()
                )
                .await,
            Err(OAuthError::InvalidState)
        ));
    }
    let (stored, _) = fixture.stored("c1").await;
    assert!(stored.get("access_token").is_none(), "no token was stored");
}

#[tokio::test]
async fn an_existing_connection_keeps_its_tenants_redirect_uri() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    fixture.insert("old", "acme", params(&provider, None)).await;
    fixture
        .insert("new", "acme", params(&provider, Some("tenantless")))
        .await;

    let service = fixture.service();
    let old = service
        .start("old", "acme", StartBinding::Browser)
        .await
        .unwrap();
    assert_eq!(old.redirect_uri, format!("{BASE}/api/oauth/acme/callback"));
    let new = service
        .start("new", "acme", StartBinding::Browser)
        .await
        .unwrap();
    assert_eq!(new.redirect_uri, format!("{BASE}/api/oauth/callback"));

    // With the tenant-less callback off, every connection uses its tenant's.
    let off = fixture.service().with_tenantless_callback(false);
    let new_off = off
        .start("new", "acme", StartBinding::Browser)
        .await
        .unwrap();
    assert_eq!(
        new_off.redirect_uri,
        format!("{BASE}/api/oauth/acme/callback")
    );
}

#[tokio::test]
async fn a_flow_left_in_the_old_table_still_completes() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-legacy", "at-legacy").await;
    fixture.insert("c1", "acme", params(&provider, None)).await;
    let repo = OAuthRepository::new(fixture.pool.clone());
    let redirect = format!("{BASE}/api/oauth/acme/callback");
    for state in ["legacy-1", "legacy-2"] {
        repo.create_state(
            state,
            "acme",
            "c1",
            "http_oauth2_authorization_code",
            &redirect,
            None,
        )
        .await
        .unwrap();
    }
    let service = fixture.service();

    // Redeemed only for the tenant it was issued for, and spent either way.
    assert!(matches!(
        service
            .complete("beta", "legacy-1", "code-legacy", &HashMap::new(), None)
            .await,
        Err(OAuthError::InvalidState)
    ));
    assert!(matches!(
        service
            .complete("acme", "legacy-1", "code-legacy", &HashMap::new(), None)
            .await,
        Err(OAuthError::InvalidState)
    ));
    // Started before flows were bound, so no cookie is asked of it.
    service
        .complete("acme", "legacy-2", "code-legacy", &HashMap::new(), None)
        .await
        .expect("a flow from the old table completes");
    assert_eq!(fixture.stored("c1").await.0["access_token"], "at-legacy");
}

/// The connections API as a tenant's member reaches it.
fn api_for(fixture: &Fixture, tenant: &'static str) -> Router {
    connections_router(fixture.config(Arc::new(EveryTenant))).layer(axum::middleware::from_fn(
        move |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
            request
                .extensions_mut()
                .insert(TenantId(tenant.to_string()));
            next.run(request).await
        },
    ))
}

/// Start a flow through the API: the authorization URL and the cookie set.
async fn authorize(api: &Router, connection: &str) -> (String, String) {
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

async fn call_back(
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
fn cookie_pair(set_cookie: &str) -> &str {
    set_cookie.split(';').next().unwrap()
}

struct ServesOnly(&'static str);

impl TenantHost for ServesOnly {
    fn serves(&self, tenant: &str) -> bool {
        tenant == self.0
    }
    fn scoped(&self, _tenant: String, work: HostFuture) -> HostFuture {
        work
    }
}

#[tokio::test]
async fn two_tenants_flows_through_one_tenantless_callback() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-acme", "at-acme").await;
    provider_issuing(&provider, "code-beta", "at-beta").await;
    fixture
        .insert("acme-conn", "acme", params(&provider, Some("tenantless")))
        .await;
    fixture
        .insert("beta-conn", "beta", params(&provider, Some("tenantless")))
        .await;
    let callbacks = oauth_callback_router(fixture.config(Arc::new(EveryTenant)));

    let (acme_url, acme_cookie) = authorize(&api_for(&fixture, "acme"), "acme-conn").await;
    let (beta_url, beta_cookie) = authorize(&api_for(&fixture, "beta"), "beta-conn").await;
    for (url, cookie) in [(&acme_url, &acme_cookie), (&beta_url, &beta_cookie)] {
        assert!(cookie.starts_with("__Host-runtara-oauth-"), "{cookie}");
        for attribute in [
            "Path=/",
            "Secure",
            "HttpOnly",
            "SameSite=Lax",
            "Max-Age=600",
        ] {
            assert!(cookie.contains(attribute), "{cookie}");
        }
        assert!(
            url.contains(
                "redirect_uri=https%3A%2F%2Fplatform.example.com%2Fapi%2Foauth%2Fcallback"
            ),
            "{url}"
        );
    }

    // Each tenant's flow, with its own cookie, lands on its own connection.
    for (url, cookie, code, connection) in [
        (&acme_url, &acme_cookie, "code-acme", "acme-conn"),
        (&beta_url, &beta_cookie, "code-beta", "beta-conn"),
    ] {
        let state = query_param(url, "state").unwrap();
        let response = call_back(
            &callbacks,
            &format!("/callback?code={code}&state={state}"),
            Some(cookie_pair(cookie)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            format!(
                "https://app.example.com/ui/oauth-complete.html?result=ok&connection={connection}"
            )
        );
        let cleared = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cleared.starts_with(&format!("{}=;", oauth_flow::cookie_name(&state, true))));
        assert!(cleared.contains("Max-Age=0"), "{cleared}");
    }
    assert_eq!(
        fixture.stored("acme-conn").await.0["access_token"],
        "at-acme"
    );
    assert_eq!(
        fixture.stored("beta-conn").await.0["access_token"],
        "at-beta"
    );
}

#[tokio::test]
async fn the_callback_refuses_a_flow_without_its_cookie_and_clears_it() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-1", "at-1").await;
    fixture
        .insert("c1", "acme", params(&provider, Some("tenantless")))
        .await;
    let callbacks = oauth_callback_router(fixture.config(Arc::new(EveryTenant)));
    let api = api_for(&fixture, "acme");

    let (url, _) = authorize(&api, "c1").await;
    let (_, other_cookie) = authorize(&api, "c1").await;
    let state = query_param(&url, "state").unwrap();
    // Another flow's cookie, even renamed to this flow's, does not do.
    let renamed = format!(
        "{}={}",
        oauth_flow::cookie_name(&state, true),
        cookie_pair(&other_cookie).split_once('=').unwrap().1
    );
    let response = call_back(
        &callbacks,
        &format!("/callback?code=code-1&state={state}"),
        Some(&renamed),
    )
    .await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://app.example.com/ui/oauth-complete.html?result=error&error=browser_mismatch"
    );
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    // A provider error with a state clears the cookie too.
    let (url, _) = authorize(&api, "c1").await;
    let state = query_param(&url, "state").unwrap();
    let response = call_back(
        &callbacks,
        &format!("/callback?error=access_denied&state={state}"),
        None,
    )
    .await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://app.example.com/ui/oauth-complete.html?result=error&error=access_denied"
    );
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    // An unknown state names no tenant.
    let response = call_back(&callbacks, "/callback?code=x&state=unknown", None).await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://app.example.com/ui/oauth-complete.html?result=error&error=invalid_state"
    );
    assert!(fixture.stored("c1").await.0.get("access_token").is_none());
}

#[tokio::test]
async fn a_tenant_served_elsewhere_is_sent_to_its_own_callback() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    fixture
        .insert("c1", "beta", params(&provider, Some("tenantless")))
        .await;
    let (url, _) = authorize(&api_for(&fixture, "beta"), "c1").await;
    let state = query_param(&url, "state").unwrap();

    let callbacks = oauth_callback_router(fixture.config(Arc::new(ServesOnly("acme"))));
    let query = format!("code=code-1&state={state}");
    let response = call_back(&callbacks, &format!("/callback?{query}"), None).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("{BASE}/api/oauth/beta/callback?{query}")
    );
    // Nothing was redeemed here: the flow is still there for its tenant.
    assert_eq!(
        fixture
            .service()
            .tenant_of(&state)
            .await
            .unwrap()
            .as_deref(),
        Some("beta")
    );
}

#[tokio::test]
async fn the_tenant_path_callback_completes_a_flow_with_its_cookie() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-1", "at-1").await;
    fixture.insert("old", "acme", params(&provider, None)).await;
    let callbacks = oauth_callback_router(fixture.config(Arc::new(EveryTenant)));
    let (url, cookie) = authorize(&api_for(&fixture, "acme"), "old").await;
    assert!(
        url.contains(
            "redirect_uri=https%3A%2F%2Fplatform.example.com%2Fapi%2Foauth%2Facme%2Fcallback"
        ),
        "{url}"
    );
    let state = query_param(&url, "state").unwrap();

    // At another tenant's callback the flow is not found.
    let response = call_back(
        &callbacks,
        &format!("/beta/callback?code=code-1&state={state}"),
        Some(cookie_pair(&cookie)),
    )
    .await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://app.example.com/ui/oauth-complete.html?result=error&error=invalid_state"
    );
    let response = call_back(
        &callbacks,
        &format!("/acme/callback?code=code-1&state={state}"),
        Some(cookie_pair(&cookie)),
    )
    .await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://app.example.com/ui/oauth-complete.html?result=ok&connection=old"
    );
    assert_eq!(fixture.stored("old").await.0["access_token"], "at-1");
}

#[tokio::test]
async fn new_authorization_code_connections_get_the_tenantless_callback() {
    let fixture = Fixture::start().await;
    let api = api_for(&fixture, "acme");
    let response = api
        .clone()
        .oneshot(
            Request::post("/connections")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "title": "Generic",
                        "integrationId": "http_oauth2_authorization_code",
                        "connectionParameters": {
                            "auth_url": "https://auth.example.com/authorize",
                            "token_url": "https://auth.example.com/token",
                            "client_id": "cid",
                            "client_secret": "csec",
                            "base_url": "https://api.example.com"
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = created["connectionId"]
        .as_str()
        .or(created["id"].as_str())
        .expect("created id")
        .to_string();
    assert_eq!(fixture.stored(&id).await.0["oauth_callback"], "tenantless");
}
