//! A connection whose OAuth client secret is sealed: seal sends the token
//! request, the access token comes back sealed for the connection's API, and
//! this process never reads either.
//!
//! Own binary: sets the loopback egress allowlist (read-once env).

use std::collections::HashMap;
use std::sync::Arc;

use runtara_connections::auth::provider_auth::resolve_connection_auth_with_seal;
use runtara_connections::sealed::{
    Allowances, MemoryKeySource, SealedCredentials, SendError, is_sealed,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TENANT: &str = "acme";
const SECRET: &str = "client-secret-CANARY";
const TOKEN: &str = "access-token-CANARY";

fn allow_loopback() {
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
}

fn sealed_for(server: &MockServer, stand_in_for: &[&str]) -> SealedCredentials {
    let address = server.address();
    SealedCredentials::in_process(
        Arc::new(MemoryKeySource::default()),
        &[7_u8; 32],
        Allowances {
            private_hosts: vec![format!("{}:{}", address.ip(), address.port())],
            plain_http_hosts: vec![address.ip().to_string()],
            api_bases: stand_in_for
                .iter()
                .map(|integration_id| ((*integration_id).to_owned(), server.uri()))
                .collect(),
            ..Default::default()
        },
    )
    .unwrap()
}

/// The connection's parameters as a request presents them: with the access
/// token got through seal.
async fn presenting(
    sealed: Option<&SealedCredentials>,
    connection: &str,
    integration_id: &str,
    stored: &Value,
) -> Result<Value, String> {
    let mut headers = HashMap::new();
    let resolved = resolve_connection_auth_with_seal(
        &reqwest::Client::new(),
        TENANT,
        connection,
        integration_id,
        stored,
        &mut headers,
        &Default::default(),
        sealed,
    )
    .await
    .map_err(|error| format!("{} (permanent: {})", error.message, error.permanent))?;
    // No credential is this process's to attach.
    assert!(headers.is_empty(), "{:?}", headers.keys());
    Ok(resolved
        .sealed_parameters
        .expect("a sealed client secret gets a sealed token"))
}

async fn token_endpoint(server: &MockServer, route: &str, token: &str) {
    Mock::given(method("POST"))
        .and(path(route))
        .and(body_string_contains("grant_type=client_credentials"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": token, "token_type": "Bearer", "expires_in": 3600
        })))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_sealed_client_secret_gets_a_token_only_the_api_ever_sees() {
    allow_loopback();
    let server = MockServer::start().await;
    token_endpoint(&server, "/form/token", TOKEN).await;
    Mock::given(method("GET"))
        .and(path("/api/orders"))
        .and(header("Authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string("orders"))
        .expect(2)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server, &[]);
    let integration_id = "http_oauth2_client_credentials";
    let connection = "conn-sealed-form";
    let stored = sealed
        .seal_parameters(
            TENANT,
            connection,
            integration_id,
            &json!({
                "token_url": format!("{}/form/token", server.uri()),
                "base_url": format!("{}/api", server.uri()),
                "client_id": "cid",
                "client_secret": SECRET,
                "scope": "read",
            }),
            None,
        )
        .await
        .unwrap()
        .expect("the connection is sealed");
    assert!(is_sealed(&stored["client_secret"]));
    assert!(!stored.to_string().contains(SECRET));

    // Twice: the token is got once, and kept.
    for _ in 0..2 {
        let parameters = presenting(Some(&sealed), connection, integration_id, &stored)
            .await
            .unwrap();
        assert!(is_sealed(&parameters["access_token"]));
        assert!(!parameters.to_string().contains(TOKEN));
        let response = sealed
            .send_bounded(
                TENANT,
                connection,
                integration_id,
                &parameters,
                "GET",
                &format!("{}/api/orders", server.uri()),
                // An agent's own credential is dropped.
                [("Authorization".to_owned(), "Bearer forged".to_owned())],
                None,
                64 * 1024,
            )
            .await
            .unwrap();
        assert_eq!(
            (response.status, response.body.as_slice()),
            (200, &b"orders"[..])
        );
    }

    // The secret reached the token endpoint, put there by seal.
    let requests = server.received_requests().await.unwrap();
    let token_request = requests
        .iter()
        .find(|request| request.url.path() == "/form/token")
        .unwrap();
    let body = String::from_utf8_lossy(&token_request.body);
    assert!(body.contains(&format!("client_secret={SECRET}")), "{body}");
    assert!(
        body.contains("client_id=cid") && body.contains("scope=read"),
        "{body}"
    );

    // The token goes to the API, and nowhere else.
    let parameters = presenting(Some(&sealed), connection, integration_id, &stored)
        .await
        .unwrap();
    let outside = sealed
        .send_bounded(
            TENANT,
            connection,
            integration_id,
            &parameters,
            "GET",
            &format!("{}/elsewhere", server.uri()),
            [],
            None,
            64 * 1024,
        )
        .await;
    assert!(matches!(outside, Err(SendError::Seal(_))), "{outside:?}");

    // A deployment without seal cannot use the connection.
    let unavailable = presenting(None, connection, integration_id, &stored)
        .await
        .unwrap_err();
    assert!(unavailable.contains("permanent: true"), "{unavailable}");
}

#[tokio::test]
async fn a_sealed_client_secret_is_sent_as_the_type_sends_it() {
    allow_loopback();
    let server = MockServer::start().await;
    // "cid:client-secret-CANARY", and not in the body.
    Mock::given(method("POST"))
        .and(path("/basic/token"))
        .and(header(
            "Authorization",
            "Basic Y2lkOmNsaWVudC1zZWNyZXQtQ0FOQVJZ",
        ))
        .and(body_string_contains("grant_type=client_credentials"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": TOKEN, "expires_in": "3600"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server, &[]);
    let integration_id = "http_oauth2_client_credentials";
    let connection = "conn-sealed-basic";
    let stored = sealed
        .seal_parameters(
            TENANT,
            connection,
            integration_id,
            &json!({
                "token_url": format!("{}/basic/token", server.uri()),
                "base_url": format!("{}/api", server.uri()),
                "client_id": "cid",
                "client_secret": SECRET,
                "token_auth": "basic",
            }),
            None,
        )
        .await
        .unwrap()
        .unwrap();
    let parameters = presenting(Some(&sealed), connection, integration_id, &stored)
        .await
        .unwrap();
    assert!(is_sealed(&parameters["access_token"]));
    let token_request = &server.received_requests().await.unwrap()[0];
    assert!(!String::from_utf8_lossy(&token_request.body).contains(SECRET));
}

#[tokio::test]
async fn a_token_endpoints_refusal_is_reported_as_before() {
    allow_loopback();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/denied/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_client", "error_description": "bad secret",
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/down/token"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({"error": "unavailable"})))
        .mount(&server)
        .await;
    let sealed = sealed_for(&server, &[]);
    let integration_id = "http_oauth2_client_credentials";
    for (route, connection, permanent) in [
        ("denied", "conn-sealed-denied", true),
        ("down", "conn-sealed-down", false),
    ] {
        let stored = sealed
            .seal_parameters(
                TENANT,
                connection,
                integration_id,
                &json!({
                    "token_url": format!("{}/{route}/token", server.uri()),
                    "base_url": format!("{}/api", server.uri()),
                    "client_id": "cid",
                    "client_secret": SECRET,
                }),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        let refused = presenting(Some(&sealed), connection, integration_id, &stored)
            .await
            .unwrap_err();
        assert!(
            refused.contains(&format!("permanent: {permanent}")),
            "{refused}"
        );
        assert!(!refused.contains(SECRET));
    }
}

#[tokio::test]
async fn a_teams_bots_app_password_gets_a_token_for_the_bot_connector() {
    allow_loopback();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tenant-1/oauth2/v2.0/token"))
        .and(body_string_contains("client_id=app-1"))
        .and(body_string_contains(
            format!("client_secret={SECRET}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": TOKEN, "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/amer/v3/conversations/c-1/activities"))
        .and(header("Authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(201).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;
    // The mock server stands in for the Bot Connector.
    let sealed = sealed_for(&server, &["teams_bot"]);
    let connection = "conn-sealed-teams";
    let stored = sealed
        .seal_parameters(
            TENANT,
            connection,
            "teams_bot",
            &json!({
                "app_id": "app-1",
                "app_password": SECRET,
                "azure_tenant_id": "tenant-1",
                "authority_host": server.uri(),
            }),
            None,
        )
        .await
        .unwrap()
        .expect("teams_bot is sealed");
    assert!(is_sealed(&stored["app_password"]));

    let parameters = presenting(Some(&sealed), connection, "teams_bot", &stored)
        .await
        .unwrap();
    let response = sealed
        .send_bounded(
            TENANT,
            connection,
            "teams_bot",
            &parameters,
            "POST",
            &format!("{}/amer/v3/conversations/c-1/activities", server.uri()),
            [("content-type".to_owned(), "application/json".to_owned())],
            Some(b"{\"type\":\"message\"}".to_vec()),
            64 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 201);
}
