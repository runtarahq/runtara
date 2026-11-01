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

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use redis::AsyncCommands;
use runtara_connections::config::EveryTenant;
use runtara_connections::oauth_callback_router;
use runtara_connections::repository::oauth::OAuthRepository;
use runtara_connections::service::oauth::{OAuthError, StartBinding};
use runtara_connections::service::oauth_flow::{self, sha256_hex};
use serde_json::json;
use tower::ServiceExt;
use wiremock::MockServer;

mod oauth_support;
mod valkey_support;
use oauth_support::*;

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
