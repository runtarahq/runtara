//! An authorization-code flow in a deployment that seals, against real
//! Postgres and Valkey and a wiremock provider: the tokens a flow brings are
//! stored sealed, a refresh goes through seal and its pair is stored sealed
//! under the rotation guard, and a disconnect revokes through seal.
//!
//! Own binary: sets the loopback egress allowlist (read-once env).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use runtara_connections::config::EveryTenant;
use runtara_connections::crypto::noop::NoOpCipher;
use runtara_connections::repository::connections::ConnectionRepository;
use runtara_connections::sealed::{Allowances, MemoryKeySource, SealedCredentials, is_sealed};
use runtara_connections::service::connections::ConnectionService;
use runtara_connections::{
    CallerChannel, ConnectionsFacade, ConnectionsState, IntegrationCompatibility,
    oauth_callback_router,
};
use serde_json::{Value, json};
use sqlx::Row;
use tower::ServiceExt;
use wiremock::matchers::{body_json, body_string_contains, header as request_header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod oauth_support;
mod valkey_support;
use oauth_support::*;

const TYPE: &str = "http_oauth2_authorization_code";

fn sealed_for(provider: &MockServer) -> Arc<SealedCredentials> {
    let address = provider.address();
    Arc::new(
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
    )
}

/// The row's parameters, how it says they are held, and its rotation guard.
async fn row(fixture: &Fixture, id: &str) -> (Value, String, Option<String>) {
    let row = sqlx::query(
        "SELECT connection_parameters, credential_format, refresh_token_hash \
         FROM connection_data_entity WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    (
        row.get("connection_parameters"),
        row.get("credential_format"),
        row.get("refresh_token_hash"),
    )
}

async fn post(api: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = api
        .clone()
        .oneshot(
            Request::post(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_flows_tokens_are_sealed_from_confirmation_to_disconnect() {
    allow_loopback_endpoints();
    let mut fixture = Fixture::start().await;
    let provider = MockServer::start().await;
    fixture.sealed = Some(sealed_for(&provider));

    // The provider: a code for the first pair, a refresh for the second, the
    // API, and revocation of the second pair's refresh token.
    provider_issuing(&provider, "code-1", "at-1").await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=rt-at-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-2", "refresh_token": "rt-at-2", "expires_in": 3600,
        })))
        .expect(1)
        .mount(&provider)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/orders"))
        .and(request_header("Authorization", "Bearer at-2"))
        .respond_with(ResponseTemplate::new(200).set_body_string("orders"))
        .expect(1)
        .mount(&provider)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/revoke"))
        .and(body_json(json!({"token": "rt-at-2"})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&provider)
        .await;

    let mut connection = params(&provider, Some("tenantless"));
    connection["base_url"] = json!(format!("{}/api", provider.uri()));
    connection["revocation_url"] = json!(format!("{}/oauth/revoke", provider.uri()));
    fixture.insert("c1", "acme", connection).await;

    // A link flow completes at the provider and waits for a member. Until
    // then its tokens are held apart, and are no connection's to use.
    let (status, link) = post(
        &api_as(&fixture, "acme", CallerChannel::ApiKey),
        "/connections/c1/oauth/link",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{link}");
    let state = query_param(link["authorizationUrl"].as_str().unwrap(), "state").unwrap();
    let callbacks = oauth_callback_router(fixture.config(Arc::new(EveryTenant)));
    call_back(
        &callbacks,
        &format!("/callback?code=code-1&state={state}"),
        None,
    )
    .await;
    let (pending, status) = fixture.stored("c1").await;
    assert_eq!(status, "PENDING_CONFIRMATION");
    assert!(pending.get("access_token").is_none());

    // Confirmed, the tokens become the connection's, sealed.
    let pending_id = fixture
        .service()
        .pending_confirmation("acme", "c1")
        .await
        .unwrap()
        .unwrap()
        .id;
    let (status, _) = post(
        &api_as(&fixture, "acme", CallerChannel::Member),
        "/connections/c1/oauth/confirm",
        json!({ "pendingId": pending_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (confirmed, format, _) = row(&fixture, "c1").await;
    assert_eq!(format, "seal");
    for token in ["access_token", "refresh_token"] {
        assert!(is_sealed(&confirmed[token]), "{token}: {confirmed}");
    }
    assert!(confirmed.get("pending_oauth").is_none());
    assert!(!confirmed.to_string().contains("at-1"), "{confirmed}");
    assert_eq!(fixture.stored("c1").await.1, "ACTIVE");

    // The stored token is fresh: a request presents it, and nothing is
    // refreshed.
    let facade = ConnectionsFacade::new(ConnectionsState::from_config(
        fixture.config(Arc::new(EveryTenant)),
    ));
    let resolve = |stored: Value| {
        let facade = &facade;
        async move {
            let mut headers = HashMap::new();
            let resolved = facade
                .resolve_connection_auth("c1", "acme", TYPE, &stored, &mut headers)
                .await
                .expect("the connection resolves");
            assert!(headers.is_empty());
            resolved
                .sealed_parameters
                .expect("sealed tokens are presented")
        }
    };
    assert_eq!(
        resolve(confirmed.clone()).await["access_token"],
        confirmed["access_token"]
    );

    // It runs out: seal refreshes, and the pair is stored sealed, under the
    // rotation guard.
    sqlx::query(
        "UPDATE connection_data_entity SET connection_parameters = jsonb_set(\
         connection_parameters, '{token_expires_at}', '\"2020-01-01T00:00:00Z\"') WHERE id = 'c1'",
    )
    .execute(&fixture.pool)
    .await
    .unwrap();
    let (expired, _, _) = row(&fixture, "c1").await;
    let presenting = resolve(expired.clone()).await;
    let (rotated, format, guard) = row(&fixture, "c1").await;
    assert_eq!(format, "seal");
    for token in ["access_token", "refresh_token"] {
        assert!(is_sealed(&rotated[token]), "{token}");
        assert_ne!(rotated[token], expired[token], "{token} was rotated");
    }
    assert!(guard.is_some(), "the rotation guard is set");
    assert!(!rotated.to_string().contains("at-2"), "{rotated}");
    assert_ne!(rotated["token_expires_at"], expired["token_expires_at"]);

    // The token seal holds reaches the API.
    let response = fixture
        .sealed
        .as_ref()
        .unwrap()
        .send_bounded(
            "acme",
            "c1",
            TYPE,
            &presenting,
            "GET",
            &format!("{}/api/orders", provider.uri()),
            [],
            None,
            65536,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);

    // Disconnecting revokes the refresh token the connection now holds.
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![]));
    ConnectionService::new(
        Arc::new(ConnectionRepository::new(
            fixture.pool.clone(),
            Arc::new(NoOpCipher),
        )),
        Arc::new(IntegrationCompatibility::from_catalog(&catalog)),
    )
    .with_sealed(fixture.sealed.clone())
    .delete_connection("c1", "acme")
    .await
    .expect("the connection is deleted");
    provider.verify().await;
}
