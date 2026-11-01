//! OAuth flows started without a browser, which a member confirms in the
//! console, against real Postgres and Valkey and a wiremock provider.
//!
//! - A link flow (an API key or an MCP tool) completes at the provider with no
//!   cookie; the connection then waits as `PENDING_CONFIRMATION`, and its
//!   tokens are held where nothing that makes requests finds them.
//! - Only a member at the console may confirm or reject, within 15 minutes of
//!   the callback; rejecting, or the time running out, drops the tokens and
//!   revokes them at the provider.
//!
//! Own binary: sets the loopback egress allowlist (read-once env).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use runtara_connections::CallerChannel;
use runtara_connections::config::{EveryTenant, OAuthOptions};
use runtara_connections::oauth_callback_router;
use runtara_connections::service::oauth::{OAuthError, StartBinding};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod oauth_support;
mod valkey_support;
use oauth_support::*;

async fn post(api: &Router, uri: &str) -> (StatusCode, Value) {
    let response = api
        .clone()
        .oneshot(Request::post(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    assert!(
        response.headers().get(header::SET_COOKIE).is_none(),
        "{uri}: no cookie"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn get(api: &Router, uri: &str) -> Value {
    let response = api
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// A connection with a revocation endpoint, and the provider's mocks: the
/// token endpoint answers `code-1`, and revocation is expected `revocations`
/// times.
async fn revocable(fixture: &Fixture, id: &str, revocations: u64) -> MockServer {
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-1", "at-pending").await;
    Mock::given(method("POST"))
        .and(path("/oauth/revoke"))
        .and(body_string_contains("rt-at-pending"))
        .respond_with(ResponseTemplate::new(200))
        .expect(revocations)
        .mount(&provider)
        .await;
    let mut params = params(&provider, Some("tenantless"));
    params["revocation_url"] = json!(format!("{}/oauth/revoke", provider.uri()));
    fixture.insert(id, "acme", params).await;
    provider
}

/// Start a link flow as an API key and complete it at the tenant-less
/// callback, with no cookie: the connection is left pending.
async fn link_and_return(fixture: &Fixture, id: &str) {
    let (status, link) = post(
        &api_as(fixture, "acme", CallerChannel::ApiKey),
        &format!("/connections/{id}/oauth/link"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{link}");
    assert!(link["expiresAt"].is_string());
    let url = link["authorizationUrl"].as_str().unwrap();
    let state = query_param(url, "state").unwrap();
    let callbacks = oauth_callback_router(fixture.config(Arc::new(EveryTenant)));
    let response = call_back(
        &callbacks,
        &format!("/callback?code=code-1&state={state}"),
        None,
    )
    .await;
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("https://app.example.com/ui/oauth-complete.html?result=pending&connection={id}")
    );
}

/// Move a pending connection's deadline into the past.
async fn expire(fixture: &Fixture, id: &str) {
    sqlx::query(
        "UPDATE connection_data_entity SET connection_parameters = jsonb_set(\
         connection_parameters, '{pending_oauth,pending_until}', '\"2020-01-01T00:00:00Z\"') \
         WHERE id = $1",
    )
    .bind(id)
    .execute(&fixture.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_link_flow_waits_unusable_until_a_member_confirms() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let _provider = revocable(&fixture, "c1", 0).await;
    link_and_return(&fixture, "c1").await;

    let (stored, status) = fixture.stored("c1").await;
    assert_eq!(status, "PENDING_CONFIRMATION");
    for live in ["access_token", "refresh_token", "authorized_at"] {
        assert!(stored.get(live).is_none(), "{live} is held apart: {stored}");
    }
    assert_eq!(
        stored["pending_oauth"]["fields"]["access_token"],
        "at-pending"
    );

    // Nothing that makes requests with the connection finds a token.
    let mut headers = HashMap::new();
    let resolved = runtara_connections::auth::provider_auth::resolve_connection_auth(
        &reqwest::Client::new(),
        "acme",
        "c1",
        "http_oauth2_authorization_code",
        &stored,
        &mut headers,
        &runtara_connections::events::ConnectionEvents::default(),
    )
    .await;
    assert!(resolved.is_err(), "a pending connection resolves no auth");
    assert!(
        !headers.values().any(|value| value.contains("at-pending")),
        "{headers:?}"
    );

    // The console shows who started it and until when, without a secret.
    let pending = get(&api_for(&fixture, "acme"), "/connections/c1/oauth/pending").await;
    assert_eq!(pending["pending"]["startedBy"], "api_key:user-1");
    let until =
        chrono::DateTime::parse_from_rfc3339(pending["pending"]["pendingUntil"].as_str().unwrap())
            .unwrap();
    let left = until
        .signed_duration_since(chrono::Utc::now())
        .num_seconds();
    assert!((14 * 60..=15 * 60).contains(&left), "{left}");
    assert!(!pending.to_string().contains("at-pending"), "{pending}");

    // Neither an API key nor an MCP client may confirm.
    for channel in [CallerChannel::ApiKey, CallerChannel::Mcp] {
        let (status, _) = post(
            &api_as(&fixture, "acme", channel),
            "/connections/c1/oauth/confirm",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{channel:?}");
        let (status, _) = post(
            &api_as(&fixture, "acme", channel),
            "/connections/c1/oauth/reject",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{channel:?}");
    }
    assert_eq!(fixture.stored("c1").await.1, "PENDING_CONFIRMATION");

    // Any member who may edit connections may.
    let (status, _) = post(&api_for(&fixture, "acme"), "/connections/c1/oauth/confirm").await;
    assert_eq!(status, StatusCode::OK);
    let (stored, status) = fixture.stored("c1").await;
    assert_eq!(status, "ACTIVE");
    assert_eq!(stored["access_token"], "at-pending");
    assert_eq!(stored["refresh_token"], "rt-at-pending");
    assert!(stored.get("pending_oauth").is_none());

    // Nothing is left to confirm.
    let (status, _) = post(&api_for(&fixture, "acme"), "/connections/c1/oauth/confirm").await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn rejecting_drops_and_revokes_the_tokens() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = revocable(&fixture, "c1", 1).await;
    link_and_return(&fixture, "c1").await;

    let (status, _) = post(&api_for(&fixture, "acme"), "/connections/c1/oauth/reject").await;
    assert_eq!(status, StatusCode::OK);
    let (stored, status) = fixture.stored("c1").await;
    assert_ne!(status, "PENDING_CONFIRMATION");
    assert_ne!(status, "ACTIVE");
    assert!(stored.get("pending_oauth").is_none());
    assert!(stored.get("access_token").is_none());
    provider.verify().await;
}

#[tokio::test]
async fn confirming_after_fifteen_minutes_is_refused_and_drops_the_tokens() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = revocable(&fixture, "c1", 1).await;
    link_and_return(&fixture, "c1").await;
    expire(&fixture, "c1").await;

    let (status, body) = post(&api_for(&fixture, "acme"), "/connections/c1/oauth/confirm").await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    let (stored, status) = fixture.stored("c1").await;
    assert_ne!(status, "ACTIVE");
    assert!(stored.get("pending_oauth").is_none());
    assert!(stored.get("access_token").is_none());
    provider.verify().await;
}

#[tokio::test]
async fn the_sweep_drops_only_expired_authorizations() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let expired = revocable(&fixture, "late", 1).await;
    let fresh = revocable(&fixture, "fresh", 0).await;
    link_and_return(&fixture, "late").await;
    link_and_return(&fixture, "fresh").await;
    expire(&fixture, "late").await;

    assert_eq!(fixture.service().expire_pending().await.unwrap(), 1);
    assert_ne!(fixture.stored("late").await.1, "PENDING_CONFIRMATION");
    assert_eq!(fixture.stored("fresh").await.1, "PENDING_CONFIRMATION");
    expired.verify().await;
    fresh.verify().await;
}

#[tokio::test]
async fn a_reconnection_keeps_the_old_tokens_until_confirmed() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-1", "at-new").await;
    let mut params = params(&provider, Some("tenantless"));
    params["access_token"] = json!("at-old");
    fixture.insert("c1", "acme", params).await;
    sqlx::query("UPDATE connection_data_entity SET status = 'ACTIVE' WHERE id = 'c1'")
        .execute(&fixture.pool)
        .await
        .unwrap();
    link_and_return(&fixture, "c1").await;

    let (stored, _) = fixture.stored("c1").await;
    assert_eq!(stored["access_token"], "at-old");
    let service = fixture.service();
    service.confirm("acme", "c1").await.unwrap();
    assert_eq!(fixture.stored("c1").await.0["access_token"], "at-new");

    // Rejecting a later reconnection restores the status it had.
    provider_issuing(&provider, "code-2", "at-newer").await;
    let flow = service
        .start(
            "c1",
            "acme",
            StartBinding::Confirm {
                started_by: "mcp:user-1".into(),
            },
        )
        .await
        .unwrap();
    service
        .complete("acme", &flow.state, "code-2", &HashMap::new(), None)
        .await
        .unwrap();
    service.reject("acme", "c1").await.unwrap();
    let (stored, status) = fixture.stored("c1").await;
    assert_eq!(status, "ACTIVE");
    assert_eq!(stored["access_token"], "at-new");
}

#[tokio::test]
async fn a_flows_binding_is_fixed_when_it_starts() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-1", "at-1").await;
    fixture
        .insert("c1", "acme", params(&provider, Some("tenantless")))
        .await;
    let service = fixture.service();

    // A browser flow stays one: without its cookie it is refused, not left
    // pending for someone to confirm.
    let flow = service
        .start("c1", "acme", StartBinding::Browser)
        .await
        .unwrap();
    assert!(matches!(
        service
            .complete("acme", &flow.state, "code-1", &HashMap::new(), None)
            .await,
        Err(OAuthError::BrowserMismatch)
    ));
    // A link flow stays one: a cookie does not make it active.
    let flow = service
        .start(
            "c1",
            "acme",
            StartBinding::Confirm {
                started_by: "api_key:user-1".into(),
            },
        )
        .await
        .unwrap();
    assert!(flow.browser_nonce.is_none());
    let completed = service
        .complete("acme", &flow.state, "code-1", &HashMap::new(), Some("any"))
        .await
        .unwrap();
    assert!(completed.pending_confirmation);
}

#[tokio::test]
async fn without_browser_binding_console_flows_wait_for_confirmation() {
    allow_loopback_endpoints();
    let fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    provider_issuing(&provider, "code-1", "at-1").await;
    fixture
        .insert("c1", "acme", params(&provider, Some("tenantless")))
        .await;
    let mut config = fixture.config(Arc::new(EveryTenant));
    config.oauth = OAuthOptions {
        browser_binding: false,
        ..OAuthOptions::default()
    };
    let api = runtara_connections::connections_router(config).layer(axum::middleware::from_fn(
        |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
            request
                .extensions_mut()
                .insert(runtara_connections::TenantId("acme".to_string()));
            next.run(request).await
        },
    ));
    let response = api
        .oneshot(
            Request::get("/connections/c1/oauth/authorize")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    let state = query_param(body["authorizationUrl"].as_str().unwrap(), "state").unwrap();
    let completed = fixture
        .service()
        .complete("acme", &state, "code-1", &HashMap::new(), None)
        .await
        .unwrap();
    assert!(completed.pending_confirmation);
    let pending = fixture
        .service()
        .pending_confirmation("acme", "c1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.started_by, "unknown");
}
