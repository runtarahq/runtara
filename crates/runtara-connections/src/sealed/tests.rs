use std::sync::Arc;

use http_body_util::BodyExt;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::testing::without_sealed;
use super::*;

const TENANT: &str = "acme";
const CONNECTION: &str = "6f1c5f0e-2a55-4b1c-9d0a-0d6a6a2f8f10";
const SECRET: &str = "s3cr3t-CANARY-token";

/// seal in this process with keys in memory, allowed to reach a local mock
/// server over plain HTTP.
fn sealed_for(server: &MockServer) -> SealedCredentials {
    sealed_standing_in(server, &[])
}

/// [`sealed_for`], with the mock server standing in for the APIs of `types`.
fn sealed_standing_in(server: &MockServer, types: &[&str]) -> SealedCredentials {
    let address = server.address();
    let allowances = Allowances {
        private_hosts: vec![format!("{}:{}", address.ip(), address.port())],
        plain_http_hosts: vec![address.ip().to_string()],
        api_bases: types
            .iter()
            .map(|integration_id| ((*integration_id).to_owned(), server.uri()))
            .collect(),
        ..Default::default()
    };
    SealedCredentials::in_process(
        Arc::new(MemoryKeySource::default()),
        &[7_u8; 32],
        allowances,
    )
    .unwrap()
}

fn bearer_params(server: &MockServer) -> Value {
    json!({"token": SECRET, "base_url": format!("{}/api", server.uri())})
}

async fn seal(sealed: &SealedCredentials, connection: &str, params: &Value) -> Value {
    sealed
        .seal_parameters(TENANT, connection, "http_bearer", params, None)
        .await
        .unwrap()
        .expect("http_bearer is sealed")
}

fn get(url: String) -> http::Request<Option<Vec<u8>>> {
    http::Request::builder()
        .method("GET")
        .uri(url)
        .body(None)
        .unwrap()
}

async fn text(response: SealedResponse) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn a_secret_is_stored_as_a_marker_and_nothing_else_changes() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let params = bearer_params(&server);

    let stored = seal(&sealed, CONNECTION, &params).await;

    assert_eq!(stored["base_url"], params["base_url"]);
    assert!(is_sealed(&stored["token"]));
    assert!(has_sealed_values(&stored));
    assert!(!without_sealed(&stored).to_string().contains(SECRET));
    assert_eq!(stored["token"][SEALED_MARKER]["envelope"]["keyset"], TENANT);
    assert_eq!(sealed.mode(), "in_process");
}

#[tokio::test]
async fn seal_attaches_the_secret_and_sends_the_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/orders"))
        .and(header("Authorization", format!("Bearer {SECRET}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string("orders"))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;

    let response = sealed
        .send(
            TENANT,
            CONNECTION,
            "http_bearer",
            &stored,
            get(format!("{}/api/orders", server.uri())),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(text(response).await, "orders");
}

#[tokio::test]
async fn an_api_keys_value_format_puts_text_around_the_key_seal_sends() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/orders"))
        .and(header(
            "Authorization",
            format!("Device {SECRET}:project").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let params = json!({
        "api_key": SECRET,
        "header_name": "Authorization",
        "value_format": "Device {}:project",
        "base_url": format!("{}/api", server.uri()),
    });
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "http_api_key", &params, None)
        .await
        .unwrap()
        .expect("http_api_key is sealed");
    assert!(is_sealed(&stored["api_key"]));
    assert_eq!(stored["value_format"], "Device {}:project");

    let response = sealed
        .send(
            TENANT,
            CONNECTION,
            "http_api_key",
            &stored,
            get(format!("{}/api/orders", server.uri())),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn a_request_body_is_sent_as_given() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/orders"))
        .and(wiremock::matchers::body_string("{\"n\":1}"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{}/api/orders", server.uri()))
        .header("content-type", "application/json")
        .body(Some(b"{\"n\":1}".to_vec()))
        .unwrap();

    let response = sealed
        .send(TENANT, CONNECTION, "http_bearer", &stored, request)
        .await
        .unwrap();

    assert_eq!(response.status(), 201);
}

#[tokio::test]
async fn seal_refuses_a_destination_outside_the_connections_base_url() {
    let server = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;

    for url in [
        // Another path on the same host.
        format!("{}/other", server.uri()),
        format!("{}/apiary", server.uri()),
        // Another host.
        format!("{}/api/orders", elsewhere.uri()),
        "https://example.com/api/orders".to_owned(),
    ] {
        let error = sealed
            .send(TENANT, CONNECTION, "http_bearer", &stored, get(url.clone()))
            .await
            .expect_err(&url);
        assert_eq!(error.code, SealErrorCode::PolicyDenied, "{url}");
        assert!(!error.detail.contains(SECRET));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_sealed_value_opens_only_for_its_own_connection_and_tenant() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;
    let url = format!("{}/api/orders", server.uri());

    // Presented as another connection of the same tenant.
    let error = sealed
        .send(
            TENANT,
            "another-connection",
            "http_bearer",
            &stored,
            get(url.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, SealErrorCode::OpenFailed);

    // Presented by another tenant, which has keys of its own.
    seal(&sealed, CONNECTION, &bearer_params(&server)).await;
    sealed
        .seal_parameters(
            "globex",
            "c-2",
            "http_bearer",
            &bearer_params(&server),
            None,
        )
        .await
        .unwrap();
    let error = sealed
        .send("globex", CONNECTION, "http_bearer", &stored, get(url))
        .await
        .unwrap_err();
    assert_eq!(error.code, SealErrorCode::PolicyDenied);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_type_seal_does_not_know_is_refused_with_whatever_it_holds() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let params = json!({"host": "db.example.com", "password": SECRET});

    let error = sealed
        .seal_parameters(TENANT, CONNECTION, "not_a_sealed_type", &params, None)
        .await
        .unwrap_err();

    assert!(matches!(error, SealingError::CannotSeal { .. }), "{error}");
    assert!(!error.to_string().contains(SECRET));
    // With nothing in it, there is nothing to refuse.
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "not_a_sealed_type", &json!({}), None)
        .await
        .unwrap();
    assert_eq!(stored, None);
}

#[tokio::test]
async fn an_oauth_connection_not_yet_authorized_has_its_client_secret_sealed() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let params = json!({"client_id": "id", "client_secret": SECRET});

    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "quickbooks_online", &params, None)
        .await
        .unwrap()
        .expect("the client secret is sealed");

    // It has no tokens yet; its client secret is seal's to send with a code.
    assert!(is_sealed(&stored["client_secret"]));
    assert_eq!(stored["client_id"], "id");
    assert!(!without_sealed(&stored).to_string().contains(SECRET));
    assert!(stored.get("access_token").is_none());

    // On a platform OAuth app it holds no client secret, and nothing is sealed.
    let on_app = json!({"oauth_app": "intuit"});
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "quickbooks_online", &on_app, None)
        .await
        .unwrap();
    assert_eq!(stored, Some(on_app));
}

#[tokio::test]
async fn a_secret_not_entered_yet_is_left_empty() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let params = json!({"base_url": format!("{}/api", server.uri())});

    let stored = seal(&sealed, CONNECTION, &params).await;

    assert_eq!(stored, params);
}

#[tokio::test]
async fn an_update_that_leaves_the_destination_keeps_the_sealed_secret() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;

    // The secret was not entered again: the update carries its marker.
    let mut update = stored.clone();
    update["note"] = json!("renamed");
    let updated = sealed
        .seal_parameters(TENANT, CONNECTION, "http_bearer", &update, Some(&stored))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(updated["token"], stored["token"]);
    assert_eq!(updated["note"], "renamed");
}

#[tokio::test]
async fn an_update_that_moves_the_destination_needs_the_secret_again() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;

    let mut moved = stored.clone();
    moved["base_url"] = json!(format!("{}/elsewhere", server.uri()));
    let error = sealed
        .seal_parameters(TENANT, CONNECTION, "http_bearer", &moved, Some(&stored))
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::SecretRequired { ref field } if field == "token"));

    // To a destination that cannot be sealed at all: it does not fall back to
    // being stored unsealed with a marker it can no longer use.
    moved["base_url"] = json!("http://not-allowed.example.com");
    let error = sealed
        .seal_parameters(TENANT, CONNECTION, "http_bearer", &moved, Some(&stored))
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::SecretRequired { .. }));

    // With the secret entered again, the move is sealed under the new policy.
    moved["base_url"] = json!(format!("{}/elsewhere", server.uri()));
    moved["token"] = json!(SECRET);
    let updated = sealed
        .seal_parameters(TENANT, CONNECTION, "http_bearer", &moved, Some(&stored))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(updated["token"], stored["token"]);
    assert_ne!(
        updated["token"][SEALED_MARKER]["policy"],
        stored["token"][SEALED_MARKER]["policy"]
    );
}

#[tokio::test]
async fn a_marker_the_connection_does_not_hold_is_refused() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let other = seal(&sealed, "another-connection", &bearer_params(&server)).await;
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;

    // Another connection's marker, submitted for this one.
    let mut forged = stored.clone();
    forged["token"] = other["token"].clone();
    for held in [Some(&stored), None] {
        let error = sealed
            .seal_parameters(TENANT, CONNECTION, "http_bearer", &forged, held)
            .await
            .unwrap_err();
        assert!(matches!(error, SealingError::SecretRequired { .. }));
    }

    // A marker in a field that is not a secret.
    let mut misplaced = bearer_params(&server);
    misplaced["base_url_note"] = stored["token"].clone();
    let error = sealed
        .seal_parameters(TENANT, CONNECTION, "http_bearer", &misplaced, Some(&stored))
        .await
        .unwrap_err();
    assert!(
        matches!(error, SealingError::SecretRequired { ref field } if field == "base_url_note")
    );
}

#[tokio::test]
async fn sealing_the_same_secret_twice_gives_different_envelopes_under_one_policy() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let first = seal(&sealed, CONNECTION, &bearer_params(&server)).await;
    let second = seal(&sealed, CONNECTION, &bearer_params(&server)).await;

    assert_ne!(
        first["token"][SEALED_MARKER]["envelope"],
        second["token"][SEALED_MARKER]["envelope"]
    );
    assert_eq!(
        first["token"][SEALED_MARKER]["policy"],
        second["token"][SEALED_MARKER]["policy"]
    );
    // One key was made for the tenant, and both are sealed to it.
    assert_eq!(
        first["token"][SEALED_MARKER]["envelope"]["kid"],
        second["token"][SEALED_MARKER]["envelope"]["kid"]
    );
}

fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[tokio::test]
async fn the_proxys_request_keeps_its_headers_but_not_one_seal_sets() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/orders"))
        .and(header("Authorization", format!("Bearer {SECRET}").as_str()))
        .and(header("x-trace", "t-1"))
        .and(wiremock::matchers::body_string("body"))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("x-request-id", "r-9")
                .set_body_string("accepted"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;

    let response = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "http_bearer",
            &stored,
            "POST",
            &format!("{}/api/orders", server.uri()),
            // What an agent might send: its own credential, and headers that
            // describe a request other than the one seal makes.
            headers(&[
                ("authorization", "Bearer agent-supplied"),
                ("Host", "elsewhere.example.com"),
                ("Content-Length", "999"),
                ("x-trace", "t-1"),
            ]),
            Some(b"body".to_vec()),
            64 * 1024,
        )
        .await
        .unwrap();

    assert_eq!(response.status, 202);
    assert_eq!(response.body, b"accepted");
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| name == "x-request-id" && value == "r-9")
    );
}

#[tokio::test]
async fn a_response_over_the_limit_is_refused_whole() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 4096]))
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;
    let url = format!("{}/api/big", server.uri());
    let send = |limit| {
        sealed.send_bounded(
            TENANT,
            CONNECTION,
            "http_bearer",
            &stored,
            "GET",
            &url,
            headers(&[]),
            None,
            limit,
        )
    };

    assert!(matches!(send(1024).await, Err(SendError::TooLarge)));
    // Not even room for the headers.
    assert!(matches!(send(8).await, Err(SendError::TooLarge)));
    assert_eq!(send(64 * 1024).await.unwrap().body.len(), 4096);
}

#[tokio::test]
async fn a_refusal_names_seals_code_and_an_invalid_request_is_not_sent() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;
    let send = |method: &'static str, url: String| {
        let (sealed, stored) = (&sealed, &stored);
        async move {
            sealed
                .send_bounded(
                    TENANT,
                    CONNECTION,
                    "http_bearer",
                    stored,
                    method,
                    &url,
                    headers(&[]),
                    None,
                    1024,
                )
                .await
        }
    };

    let refused = send("GET", format!("{}/outside", server.uri())).await;
    assert!(
        matches!(&refused, Err(SendError::Seal(error)) if error.code == SealErrorCode::PolicyDenied),
        "{refused:?}"
    );
    let invalid = send("NOT A METHOD", format!("{}/api", server.uri())).await;
    assert!(
        matches!(invalid, Err(SendError::InvalidRequest)),
        "{invalid:?}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

fn hmac_sha256(key: &str, message: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

#[tokio::test]
async fn a_signing_secret_checks_a_webhooks_signature_without_being_read() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let params = json!({"bot_token": "xoxb-CANARY", "signing_secret": SECRET});
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "slack_bot", &params, None)
        .await
        .unwrap()
        .expect("slack_bot is sealed");
    assert!(is_sealed(&stored["bot_token"]) && is_sealed(&stored["signing_secret"]));
    assert!(!without_sealed(&stored).to_string().contains("CANARY"));

    let message = b"v0:1700000000:{\"type\":\"event_callback\"}".to_vec();
    let signature = hmac_sha256(SECRET, &message);
    let check = |message: Vec<u8>, mac: Vec<u8>| {
        sealed.verify(TENANT, CONNECTION, &stored, "signing_secret", message, mac)
    };

    assert!(check(message.clone(), signature.clone()).await.unwrap());
    // Another message, another secret's code, or a shortened code: no.
    assert!(
        !check(b"v0:1700000001:{}".to_vec(), signature.clone())
            .await
            .unwrap()
    );
    assert!(
        !check(message.clone(), hmac_sha256("other", &message))
            .await
            .unwrap()
    );
    assert!(
        !check(message.clone(), signature[..16].to_vec())
            .await
            .unwrap()
    );

    // A secret that is sent cannot be made to check messages, and one the
    // connection does not hold sealed cannot be asked about at all.
    let token = sealed
        .verify(
            TENANT,
            CONNECTION,
            &stored,
            "bot_token",
            message.clone(),
            signature.clone(),
        )
        .await
        .unwrap_err();
    assert_eq!(token.code, SealErrorCode::PolicyDenied);
    let missing = sealed
        .verify(
            TENANT,
            CONNECTION,
            &params,
            "signing_secret",
            message.clone(),
            signature.clone(),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code, SealErrorCode::BadRequest);
    // Nor does it check for another connection.
    let other = sealed
        .verify(
            TENANT,
            "another-connection",
            &stored,
            "signing_secret",
            message,
            signature,
        )
        .await
        .unwrap_err();
    assert_eq!(other.code, SealErrorCode::OpenFailed);

    // Requests present the token only: the signing secret goes nowhere.
    assert_eq!(request_fields("slack_bot", &stored), ["bot_token"]);
}

#[tokio::test]
async fn a_request_presents_only_the_secret_the_connections_settings_use() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("Authorization", "Bearer bearer-CANARY"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    // Both secrets are held; the mode names the bearer token.
    let params = json!({
        "url": format!("{}/mcp", server.uri()),
        "auth_mode": "bearer",
        "bearer_token": "bearer-CANARY",
        "api_key_value": "key-CANARY",
    });
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "mcp", &params, None)
        .await
        .unwrap()
        .expect("mcp is sealed");
    assert!(is_sealed(&stored["bearer_token"]) && is_sealed(&stored["api_key_value"]));
    assert_eq!(request_fields("mcp", &stored), ["bearer_token"]);

    let response = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "mcp",
            &stored,
            "POST",
            &format!("{}/mcp", server.uri()),
            headers(&[]),
            Some(b"{}".to_vec()),
            64 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    let received = &server.received_requests().await.unwrap()[0];
    assert!(!received.headers.contains_key("x-api-key"));

    // With no auth mode, nothing is presented: the request needs no seal.
    let mut open = stored.clone();
    open["auth_mode"] = json!("none");
    assert!(request_fields("mcp", &open).is_empty());
    // And a connection whose secrets are not sealed presents nothing either.
    assert!(request_fields("mcp", &params).is_empty());
}

/// The extra headers of an MCP connection are secrets of their own: seal
/// sets each on the connection's requests, and nothing else holds a value.
#[tokio::test]
async fn an_mcp_connections_extra_headers_are_sent_by_seal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("Authorization", "Bearer bearer-CANARY"))
        .and(header("X-Team", "team-CANARY"))
        .and(header("X-Api-Key", "key-CANARY"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/open"))
        .and(header("X-Team", "team-CANARY"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    let send = |connection: &'static str, stored: Value, route: &'static str| {
        let (sealed, url) = (&sealed, format!("{}{route}", server.uri()));
        async move {
            sealed
                .send_bounded(
                    TENANT,
                    connection,
                    "mcp",
                    &stored,
                    "POST",
                    &url,
                    headers(&[]),
                    Some(b"{}".to_vec()),
                    64 * 1024,
                )
                .await
        }
    };

    let stored = sealed
        .seal_parameters(
            TENANT,
            CONNECTION,
            "mcp",
            &json!({
                "url": format!("{}/mcp", server.uri()),
                "auth_mode": "bearer",
                "bearer_token": "bearer-CANARY",
                "extra_headers": {"X-Team": "team-CANARY", "X-Api-Key": "key-CANARY"},
            }),
            None,
        )
        .await
        .unwrap()
        .expect("mcp is sealed");
    assert!(!without_sealed(&stored).to_string().contains("CANARY"));
    assert_eq!(
        request_fields("mcp", &stored),
        [
            "bearer_token",
            "extra_headers.X-Api-Key",
            "extra_headers.X-Team"
        ]
    );
    assert_eq!(
        send(CONNECTION, stored.clone(), "/mcp")
            .await
            .unwrap()
            .status,
        200
    );
    // They go to the connection's server, and nowhere else.
    let elsewhere = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "mcp",
            &stored,
            "POST",
            "https://elsewhere.example.com/mcp",
            headers(&[]),
            None,
            64 * 1024,
        )
        .await;
    assert!(
        matches!(elsewhere, Err(SendError::Seal(_))),
        "{elsewhere:?}"
    );

    // With no auth mode, the headers are still seal's to send.
    let open = sealed
        .seal_parameters(
            TENANT,
            "c-open",
            "mcp",
            &json!({
                "url": format!("{}/open", server.uri()),
                "auth_mode": "none",
                "extra_headers": {"X-Team": "team-CANARY"},
            }),
            None,
        )
        .await
        .unwrap()
        .expect("its headers are sealed");
    assert!(has_sealed_values(&open));
    assert_eq!(request_fields("mcp", &open), ["extra_headers.X-Team"]);
    assert_eq!(send("c-open", open, "/open").await.unwrap().status, 200);
}

#[test]
fn the_wrapping_key_is_its_own_setting_or_derived_from_the_encryption_key() {
    use base64::Engine;
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let own = encode(&[1_u8; 32]);
    let encryption = encode(&[2_u8; 32]);

    // Its own setting is used as it is.
    assert_eq!(
        *wrapping_key(Some(&own), Some(&encryption)).unwrap(),
        [1_u8; 32]
    );
    // Otherwise it is derived, and is not the encryption key itself.
    let derived = wrapping_key(None, Some(&encryption)).unwrap();
    assert_eq!(
        *derived,
        *keys::wrapping_key_from_encryption_key(&[2_u8; 32])
    );
    assert_ne!(*derived, [2_u8; 32]);
    assert_eq!(
        *wrapping_key(Some("  "), Some(&encryption)).unwrap(),
        *derived
    );

    for (own, encryption) in [
        (None, None),
        (Some("not base64!"), None),
        (Some(encode(&[1_u8; 16]).as_str()), None),
        (None, Some("not base64!")),
    ] {
        let error = wrapping_key(own, encryption).unwrap_err();
        assert!(error.contains("RUNTARA_"), "{error}");
    }
}

#[test]
fn only_a_lone_marker_object_is_a_sealed_value() {
    assert!(is_sealed(&json!({ SEALED_MARKER: {} })));
    for value in [
        json!("text"),
        json!(null),
        json!({}),
        json!({ SEALED_MARKER: {}, "other": 1 }),
        json!({ "envelope": {} }),
        json!([{ SEALED_MARKER: {} }]),
    ] {
        assert!(!is_sealed(&value), "{value}");
    }
    assert!(!has_sealed_values(&json!({"a": "b"})));
    assert!(!has_sealed_values(&json!("text")));
}

#[tokio::test]
async fn a_telegram_token_is_put_in_the_path_by_seal() {
    let token = "123456:ABC-DEF_canary";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/bot{token}/sendMessage")))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"ok\":true}"))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_standing_in(&server, &["telegram_bot"]);
    let stored = sealed
        .seal_parameters(
            TENANT,
            CONNECTION,
            "telegram_bot",
            &json!({"bot_token": token}),
            None,
        )
        .await
        .unwrap()
        .expect("telegram_bot is sealed");
    assert!(!without_sealed(&stored).to_string().contains(token));

    // The request names the method only.
    let response = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "telegram_bot",
            &stored,
            "POST",
            &format!("{}/sendMessage", server.uri()),
            [("content-type".to_owned(), "application/json".to_owned())],
            Some(b"{\"chat_id\":\"1\",\"text\":\"hi\"}".to_vec()),
            64 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert!(!format!("{response:?}").contains(token));

    // The token goes nowhere but the Bot API.
    let elsewhere = MockServer::start().await;
    let refused = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "telegram_bot",
            &stored,
            "POST",
            &format!("{}/sendMessage", elsewhere.uri()),
            [],
            None,
            64 * 1024,
        )
        .await;
    assert!(matches!(refused, Err(SendError::Seal(_))), "{refused:?}");
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

fn received_headers(request: &wiremock::Request) -> Vec<(String, String)> {
    request
        .headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
        .collect()
}

#[tokio::test]
async fn an_azure_account_key_signs_the_request_seal_sends() {
    use base64::Engine;
    let account_key = base64::engine::general_purpose::STANDARD.encode("account-key-CANARY");
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);
    // An endpoint addressed by path, as Azurite's is.
    let params = json!({
        "account_name": "devstore",
        "account_key": account_key,
        "endpoint_override": server.uri(),
    });
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "azure_blob_storage", &params, None)
        .await
        .unwrap()
        .expect("azure_blob_storage is sealed");
    assert!(is_sealed(&stored["account_key"]));
    assert!(!without_sealed(&stored).to_string().contains(&account_key));

    let url = format!("{}/devstore/container/blob.txt", server.uri());
    let response = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "azure_blob_storage",
            &stored,
            "PUT",
            &url,
            [
                ("content-type".to_owned(), "text/plain".to_owned()),
                ("x-ms-blob-type".to_owned(), "BlockBlob".to_owned()),
                // What an agent might send of its own is not seal's signature.
                (
                    "Authorization".to_owned(),
                    "SharedKey devstore:forged".to_owned(),
                ),
                (
                    "x-ms-date".to_owned(),
                    "Thu, 01 Jan 1970 00:00:00 GMT".to_owned(),
                ),
            ],
            Some(b"hello".to_vec()),
            64 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 201);

    let received = &server.received_requests().await.unwrap()[0];
    let headers = received_headers(received);
    testing::check_shared_key(
        "PUT",
        &url.parse().unwrap(),
        &headers,
        b"hello",
        "devstore",
        &account_key,
    )
    .unwrap();
    assert!(!format!("{headers:?}").contains("forged"));

    // The key signs for the account's endpoint only.
    let elsewhere = MockServer::start().await;
    let refused = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "azure_blob_storage",
            &stored,
            "GET",
            &format!("{}/devstore/container/blob.txt", elsewhere.uri()),
            [],
            None,
            64 * 1024,
        )
        .await;
    assert!(matches!(refused, Err(SendError::Seal(_))), "{refused:?}");
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

/// An AWS connection with temporary credentials, whose endpoint is the mock
/// server.
fn aws_params(server: &MockServer, service: &str) -> Value {
    json!({
        "aws_access_key_id": "AKIAIOSFODNN7EXAMPLE",
        "aws_secret_access_key": SECRET,
        "aws_session_token": "session-CANARY",
        "aws_region": "eu-west-1",
        "endpoint": server.uri(),
        "service": service,
    })
}

#[tokio::test]
async fn an_aws_key_signs_the_request_seal_sends() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let params = aws_params(&server, "sqs");
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "aws_credentials", &params, None)
        .await
        .unwrap();
    // seal before it took `X-Amz-Target` cannot call SQS, so the connection
    // is left readable.
    let Some(stored) = stored else {
        assert!(!policy::seal_takes_amz_target());
        return;
    };
    assert!(is_sealed(&stored["aws_secret_access_key"]));
    assert!(is_sealed(&stored["aws_session_token"]));
    assert!(!without_sealed(&stored).to_string().contains(SECRET));
    assert!(
        !without_sealed(&stored)
            .to_string()
            .contains("session-CANARY")
    );

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .expect(1)
        .mount(&server)
        .await;
    let body = b"{\"MaxResults\":10}".to_vec();
    let response = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "aws_credentials",
            &stored,
            "POST",
            &format!("{}/", server.uri()),
            [
                (
                    "Content-Type".to_owned(),
                    "application/x-amz-json-1.0".to_owned(),
                ),
                ("X-Amz-Target".to_owned(), "AmazonSQS.ListQueues".to_owned()),
                // An agent's own signing headers are dropped, not sent on.
                (
                    "Authorization".to_owned(),
                    "AWS4-HMAC-SHA256 forged".to_owned(),
                ),
                ("X-Amz-Date".to_owned(), "19700101T000000Z".to_owned()),
                ("X-Amz-Security-Token".to_owned(), "forged".to_owned()),
                ("X-Amz-Content-Sha256".to_owned(), "forged".to_owned()),
            ],
            Some(body.clone()),
            64 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);

    let received = &server.received_requests().await.unwrap()[0];
    let headers = received_headers(received);
    let scope = testing::check_sigv4("POST", "/", None, &headers, &body, SECRET).unwrap();
    assert_eq!(scope.access_key_id, "AKIAIOSFODNN7EXAMPLE");
    assert_eq!(
        (scope.region.as_str(), scope.service.as_str()),
        ("eu-west-1", "sqs")
    );
    for signed in ["host", "x-amz-target", "x-amz-security-token", "x-amz-date"] {
        assert!(
            scope.signed_headers.iter().any(|name| name == signed),
            "{signed}"
        );
    }
    let token = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-amz-security-token"))
        .map(|(_, value)| value.as_str());
    assert_eq!(token, Some("session-CANARY"));
    assert!(!format!("{headers:?}").contains("forged"));
}

#[tokio::test]
async fn an_s3_key_signs_requests_and_copies_between_buckets() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let params = json!({
        "access_key_id": "AKIAIOSFODNN7EXAMPLE",
        "secret_access_key": SECRET,
        "region": "us-east-1",
        "endpoint": server.uri(),
    });
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "s3_compatible", &params, None)
        .await
        .unwrap();
    // seal before it let a whole endpoint copy between buckets cannot do what
    // the storage agent does, so the connection is left readable.
    let Some(stored) = stored else {
        assert!(!policy::seal_copies_from_any_bucket());
        return;
    };
    assert!(is_sealed(&stored["secret_access_key"]));

    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let send = |method: &'static str, path: &'static str, headers: Vec<(String, String)>| {
        let (sealed, stored, url) = (&sealed, &stored, format!("{}{path}", server.uri()));
        async move {
            sealed
                .send_bounded(
                    TENANT,
                    CONNECTION,
                    "s3_compatible",
                    stored,
                    method,
                    &url,
                    headers,
                    None,
                    64 * 1024,
                )
                .await
        }
    };
    send("GET", "/bucket/object.txt", Vec::new()).await.unwrap();
    // A method no URL is presigned for goes by the connection's other rule.
    send("POST", "/bucket/object.txt?uploads", Vec::new())
        .await
        .unwrap();
    send(
        "PUT",
        "/bucket/copy.txt",
        vec![(
            "x-amz-copy-source".to_owned(),
            "/other-bucket/original.txt".to_owned(),
        )],
    )
    .await
    .unwrap();

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 3);
    for request in &received {
        let headers = received_headers(request);
        let scope = testing::check_sigv4(
            request.method.as_str(),
            request.url.path(),
            request.url.query(),
            &headers,
            &[],
            SECRET,
        )
        .unwrap();
        assert_eq!(scope.service, "s3");
    }
    assert!(
        received_headers(&received[2])
            .iter()
            .any(|(name, value)| name == "x-amz-copy-source"
                && value == "/other-bucket/original.txt")
    );
}

#[tokio::test]
async fn a_database_url_is_stored_as_its_sealed_password_and_its_target() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let url = format!("postgres://app:{SECRET}@db.example.com:6432/orders?sslmode=verify-full");

    let stored = sealed
        .seal_parameters(
            TENANT,
            CONNECTION,
            "postgres",
            &json!({"database_url": url}),
            None,
        )
        .await
        .unwrap()
        .expect("a verified Postgres connection is sealed");
    assert!(is_sealed(&stored["database_url"]));
    assert_eq!(
        stored["database_target"],
        "postgres://app@db.example.com:6432/orders"
    );
    assert!(!without_sealed(&stored).to_string().contains(SECRET));

    // Saved again untouched, it stays as it is.
    let again = sealed
        .seal_parameters(TENANT, CONNECTION, "postgres", &stored, Some(&stored))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, stored);

    // A URL seal could not log in for is refused: it is not stored whole.
    let odd = json!({
        "database_url": format!(
            "postgres://app:{SECRET}@db.example.com/orders?sslmode=verify-full&sslrootcert=/ca.pem"
        ),
    });
    let error = sealed
        .seal_parameters(TENANT, CONNECTION, "postgres", &odd, None)
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::CannotSeal { .. }), "{error}");
    assert!(!error.to_string().contains(SECRET));

    // A URL that does not ask for a verified server is sealed for what it
    // asks, by a seal that can log in so; an earlier seal leaves it whole.
    let unverified = json!({
        "database_url": format!("postgres://app:{SECRET}@db.example.com/orders?sslmode=require"),
    });
    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "postgres", &unverified, None)
        .await
        .unwrap();
    match stored {
        Some(stored) => {
            assert!(policy::seal_logs_in_unverified());
            assert!(is_sealed(&stored["database_url"]));
            assert_eq!(
                stored["database_target"],
                "postgres://app@db.example.com:5432/orders?sslmode=require"
            );
            assert!(!without_sealed(&stored).to_string().contains(SECRET));
        }
        None => assert!(!policy::seal_logs_in_unverified()),
    }
}

#[tokio::test]
async fn a_webhooks_secret_token_registers_one_url_and_is_checked_without_being_read() {
    let token = "123456:ABC-DEF_canary";
    let secret = "0123456789abcdef-webhook-CANARY";
    let webhook_url = "https://runtime.example.com/api/events/acme/webhook/telegram/c-1";
    let server = MockServer::start().await;
    // seal puts the bot token in the path and the secret token in the form.
    Mock::given(method("POST"))
        .and(path(format!("/bot{token}/setWebhook")))
        .and(wiremock::matchers::body_string_contains(format!(
            "secret_token={secret}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"ok\":true}"))
        .expect(1)
        .mount(&server)
        .await;
    let sealed = sealed_standing_in(&server, &["telegram_bot"]);
    let mut connection = sealed
        .seal_parameters(
            TENANT,
            CONNECTION,
            "telegram_bot",
            &json!({"bot_token": token}),
            None,
        )
        .await
        .unwrap()
        .unwrap();
    let marker = sealed
        .seal_webhook_secret(TENANT, CONNECTION, Some(webhook_url), secret)
        .await
        .unwrap();
    assert!(is_sealed(&marker));
    assert!(!marker.to_string().contains(secret));
    assert!(sealed.webhook_secret_is_current(CONNECTION, Some(webhook_url), &marker));
    // Another connection's, or one for a URL since changed, is not.
    assert!(!sealed.webhook_secret_is_current("another-connection", Some(webhook_url), &marker));
    assert!(!sealed.webhook_secret_is_current(
        CONNECTION,
        Some("https://elsewhere.example.com/hook"),
        &marker
    ));
    assert!(!sealed.webhook_secret_is_current(CONNECTION, None, &marker));

    // Sealed with no URL to register, a token is only ever checked: it can
    // be sent nowhere, registration included.
    let checked_only = sealed
        .seal_webhook_secret(TENANT, CONNECTION, None, secret)
        .await
        .unwrap();
    assert!(sealed.webhook_secret_is_current(CONNECTION, None, &checked_only));
    assert!(!sealed.webhook_secret_is_current(CONNECTION, Some(webhook_url), &checked_only));
    assert_eq!(
        sealed
            .verify_webhook_secret(TENANT, CONNECTION, &checked_only, secret)
            .await,
        Ok(true)
    );
    assert_eq!(
        sealed
            .verify_webhook_secret(TENANT, CONNECTION, &checked_only, "another-token")
            .await,
        Ok(false)
    );

    // Registration presents both, for that URL only.
    connection[policy::WEBHOOK_SECRET_FIELD] = marker.clone();
    let register = |url: &'static str| {
        let (sealed, connection, endpoint) =
            (&sealed, &connection, format!("{}/setWebhook", server.uri()));
        async move {
            let form = format!(
                "url={}&allowed_updates=%5B%22message%22%5D",
                urlencoding::encode(url)
            );
            sealed
                .send_bounded(
                    TENANT,
                    CONNECTION,
                    "telegram_bot",
                    connection,
                    "POST",
                    &endpoint,
                    [(
                        "content-type".to_owned(),
                        "application/x-www-form-urlencoded".to_owned(),
                    )],
                    Some(form.into_bytes()),
                    64 * 1024,
                )
                .await
        }
    };
    assert_eq!(register(webhook_url).await.unwrap().status, 200);
    // Telegram would send the token to whatever URL is registered.
    let elsewhere = register("https://attacker.example.com/hook").await;
    assert!(
        matches!(elsewhere, Err(SendError::Seal(_))),
        "{elsewhere:?}"
    );

    // A call's token is checked against it; nothing reads it.
    let check = |presented: &'static str| {
        let (sealed, marker) = (&sealed, &marker);
        async move {
            sealed
                .verify_webhook_secret(TENANT, CONNECTION, marker, presented)
                .await
        }
    };
    assert_eq!(check(secret).await, Ok(true));
    assert_eq!(check("0123456789abcdef-webhook-CANARX").await, Ok(false));
    assert_eq!(check("").await, Ok(false));
    // It is this connection's: presented for another, it does not open.
    assert!(
        sealed
            .verify_webhook_secret(TENANT, "another-connection", &marker, secret)
            .await
            .is_err()
    );
    // A request that is not the webhook's registration does not present it.
    let reply = sealed
        .send_bounded(
            TENANT,
            CONNECTION,
            "telegram_bot",
            &connection,
            "POST",
            &format!("{}/sendMessage", server.uri()),
            [],
            None,
            64 * 1024,
        )
        .await;
    assert!(matches!(reply, Err(SendError::Seal(_))), "{reply:?}");
}

/// seal in this process gives an answer as long as the host gives the
/// request: a model answering a long prompt is not cut off at seal's 60 s.
#[test]
fn seal_waits_as_long_as_the_host_lets_a_request_wait() {
    let mut limits = seal_broker::broker::Limits::default();
    super::allow_the_callers_deadline(&mut limits);
    assert_eq!(limits.first_byte_timeout, super::REQUEST_DEADLINE);
    assert_eq!(limits.buffered_total_timeout, super::REQUEST_DEADLINE);
    // A longer one is kept.
    limits.first_byte_timeout = std::time::Duration::from_secs(600);
    super::allow_the_callers_deadline(&mut limits);
    assert_eq!(
        limits.first_byte_timeout,
        std::time::Duration::from_secs(600)
    );
}

// ---- Secrets sealed where they are entered ---------------------------------

/// What a caller does with secrets it enters: marks each in the parameters,
/// asks for an offer, seals them under it, and saves with the envelope.
async fn entered_and_saved(
    sealed: &SealedCredentials,
    connection: &str,
    integration_id: &str,
    readable: &Value,
) -> Result<Value, SealingError> {
    let (mut parameters, secrets) = testing::enter(integration_id, readable);
    // As the service takes a database's target from a caller.
    if let Some(target) = parameters
        .get(policy::PG_TARGET_FIELD)
        .and_then(Value::as_str)
    {
        parameters[policy::PG_TARGET_FIELD] =
            json!(policy::target_as_entered(target).expect("a target"));
    }
    let offered = sealed
        .offer(TENANT, connection, integration_id, &parameters)
        .await?
        .expect("a secret was entered");
    let intake = testing::seal_under(
        &serde_json::to_value(&offered.offer).unwrap(),
        &offered.fields,
        &secrets,
    );
    sealed
        .admit_parameters(
            TENANT,
            connection,
            integration_id,
            &parameters,
            None,
            Some(&intake),
        )
        .await
        .map(|stored| stored.expect("the connection is sealed"))
}

#[tokio::test]
async fn a_secret_sealed_where_it_was_entered_is_stored_and_sent_by_seal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/orders"))
        .and(header("authorization", format!("Bearer {SECRET}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string("orders"))
        .mount(&server)
        .await;
    let sealed = sealed_for(&server);

    let stored = entered_and_saved(&sealed, CONNECTION, "http_bearer", &bearer_params(&server))
        .await
        .unwrap();

    assert!(is_sealed(&stored["token"]));
    assert!(!has_entered_values(&stored));
    assert!(!without_sealed(&stored).to_string().contains(SECRET));
    // What seal made is what sealing here makes: the same policy.
    let here = seal(&sealed, CONNECTION, &bearer_params(&server)).await;
    assert_eq!(
        stored["token"][SEALED_MARKER]["policy"],
        here["token"][SEALED_MARKER]["policy"]
    );
    let response = sealed
        .send(
            TENANT,
            CONNECTION,
            "http_bearer",
            &stored,
            get(format!("{}/api/orders", server.uri())),
        )
        .await
        .unwrap();
    assert_eq!(text(response).await, "orders");
}

#[tokio::test]
async fn a_readable_secret_from_a_caller_is_refused() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);

    let error = sealed
        .admit_parameters(
            TENANT,
            CONNECTION,
            "http_bearer",
            &bearer_params(&server),
            None,
            None,
        )
        .await
        .unwrap_err();

    assert!(
        matches!(&error, SealingError::MustBeSealed { field } if field == "token"),
        "{error}"
    );
    assert!(!error.to_string().contains(SECRET));
}

#[tokio::test]
async fn an_offer_names_the_connection_and_one_policy_for_each_secret() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let (parameters, _) = testing::enter("http_bearer", &bearer_params(&server));

    let offered = sealed
        .offer(TENANT, CONNECTION, "http_bearer", &parameters)
        .await
        .unwrap()
        .unwrap();

    let offer = serde_json::to_value(&offered.offer).unwrap();
    assert_eq!(offer["keyset"], TENANT);
    assert_eq!(offer["item"], CONNECTION);
    assert_eq!(offer["field"], seal_format::INTAKE_FIELD);
    assert_eq!(
        offer["policy"]["intake"]["outputs"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["token"]
    );
    assert_eq!(
        offered.fields.get("token").map(String::as_str),
        Some("token")
    );
    assert!(!offer.to_string().contains(SECRET));

    // Parameters that enter no secret need no offer.
    assert!(
        sealed
            .offer(
                TENANT,
                CONNECTION,
                "http_bearer",
                &json!({"base_url": "https://api.example.com"})
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn secrets_sealed_for_one_destination_are_not_stored_for_another() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let (parameters, secrets) = testing::enter("http_bearer", &bearer_params(&server));
    let offered = sealed
        .offer(TENANT, CONNECTION, "http_bearer", &parameters)
        .await
        .unwrap()
        .unwrap();
    let intake = testing::seal_under(
        &serde_json::to_value(&offered.offer).unwrap(),
        &offered.fields,
        &secrets,
    );

    // The envelope, saved beside another base URL: a captured request with
    // its destination changed.
    let mut moved = parameters.clone();
    moved["base_url"] = json!(format!("{}/elsewhere", server.uri()));
    let error = sealed
        .admit_parameters(
            TENANT,
            CONNECTION,
            "http_bearer",
            &moved,
            None,
            Some(&intake),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Stale), "{error}");

    // Nor for another connection, or another tenant.
    let other = "0b0e2c1a-58a1-4f0e-8f0b-1d4d6f1f3a55";
    let error = sealed
        .admit_parameters(
            TENANT,
            other,
            "http_bearer",
            &parameters,
            None,
            Some(&intake),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Intake(_)), "{error}");
    let error = sealed
        .admit_parameters(
            "another-tenant",
            CONNECTION,
            "http_bearer",
            &parameters,
            None,
            Some(&intake),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Intake(_)), "{error}");

    // As it was offered, it is stored.
    sealed
        .admit_parameters(
            TENANT,
            CONNECTION,
            "http_bearer",
            &parameters,
            None,
            Some(&intake),
        )
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn an_envelope_and_the_secrets_marked_as_entered_go_together() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let (parameters, secrets) = testing::enter("http_bearer", &bearer_params(&server));
    let offered = sealed
        .offer(TENANT, CONNECTION, "http_bearer", &parameters)
        .await
        .unwrap()
        .unwrap();
    let offer = serde_json::to_value(&offered.offer).unwrap();
    let intake = testing::seal_under(&offer, &offered.fields, &secrets);

    // Marked as entered, and no envelope.
    let error = sealed
        .admit_parameters(TENANT, CONNECTION, "http_bearer", &parameters, None, None)
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Intake(_)), "{error}");

    // An envelope, and nothing marked as entered.
    let error = sealed
        .admit_parameters(
            TENANT,
            CONNECTION,
            "http_bearer",
            &json!({"base_url": parameters["base_url"]}),
            None,
            Some(&intake),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Intake(_)), "{error}");

    // An envelope that leaves out a secret marked as entered.
    let empty = testing::seal_under(&offer, &offered.fields, &Default::default());
    let error = sealed
        .admit_parameters(
            TENANT,
            CONNECTION,
            "http_bearer",
            &parameters,
            None,
            Some(&empty),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Intake(_)), "{error}");

    // What is not an envelope at all.
    let error = sealed
        .admit_parameters(
            TENANT,
            CONNECTION,
            "http_bearer",
            &parameters,
            None,
            Some(&json!({"token": SECRET})),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Intake(_)), "{error}");
    assert!(!error.to_string().contains(SECRET));

    // A stored value is no intake envelope: it cannot be given a new policy.
    let stored = seal(&sealed, CONNECTION, &bearer_params(&server)).await;
    let error = sealed
        .admit_parameters(
            TENANT,
            CONNECTION,
            "http_bearer",
            &parameters,
            None,
            Some(&stored["token"][SEALED_MARKER]["envelope"]),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Intake(_)), "{error}");

    // A secret entered at the engine is readable there, and a marker is not
    // a secret.
    let error = sealed
        .seal_parameters(TENANT, CONNECTION, "http_bearer", &parameters, None)
        .await
        .unwrap_err();
    assert!(matches!(error, SealingError::Intake(_)), "{error}");
}

#[tokio::test]
async fn an_update_enters_one_secret_and_keeps_another_sealed() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let readable = json!({"bot_token": "xoxb-first", "signing_secret": SECRET});
    let stored = entered_and_saved(&sealed, CONNECTION, "slack_bot", &readable)
        .await
        .unwrap();
    assert!(is_sealed(&stored["bot_token"]) && is_sealed(&stored["signing_secret"]));

    // The token is entered again; the signing secret stays as it is held.
    let mut merged = stored.clone();
    merged["bot_token"] = entered();
    let offered = sealed
        .offer(TENANT, CONNECTION, "slack_bot", &merged)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(offered.fields.keys().collect::<Vec<_>>(), ["bot_token"]);
    let intake = testing::seal_under(
        &serde_json::to_value(&offered.offer).unwrap(),
        &offered.fields,
        &[("bot_token".to_owned(), "xoxb-second".to_owned())].into(),
    );
    let updated = sealed
        .admit_parameters(
            TENANT,
            CONNECTION,
            "slack_bot",
            &merged,
            Some(&stored),
            Some(&intake),
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(updated["signing_secret"], stored["signing_secret"]);
    assert!(is_sealed(&updated["bot_token"]));
    assert_ne!(updated["bot_token"], stored["bot_token"]);
}

#[tokio::test]
async fn a_database_password_is_entered_beside_where_it_may_go() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let url = format!("postgres://app:{SECRET}@db.example.com:5432/orders?sslmode=verify-full");

    // A caller splits the URL: the password is the secret.
    let (parameters, secrets) = testing::enter("postgres", &json!({"database_url": url}));
    assert!(is_entered(&parameters["database_url"]));
    assert_eq!(
        secrets.get("database_url").map(String::as_str),
        Some(SECRET)
    );
    assert!(!parameters.to_string().contains(SECRET));
    // The URL goes beside it without the password, as it was typed, and is
    // held to the rules of a whole one.
    assert_eq!(
        parameters["database_target"],
        "postgres://app@db.example.com:5432/orders?sslmode=verify-full"
    );
    let target = policy::target_as_entered(parameters["database_target"].as_str().unwrap());
    assert_eq!(
        target.as_deref(),
        Ok("postgres://app@db.example.com:5432/orders")
    );
    assert_eq!(
        policy::target_as_entered(&url),
        Err(policy::TargetRefused::HoldsThePassword)
    );
    for refused in [
        // Does not say how its session is protected, or asks for what seal
        // has no mode for.
        "postgres://app@db.example.com/orders",
        "postgres://app@db.example.com/orders?sslmode=prefer",
        "postgres://app@db.example.com/orders?sslmode=require&options=x",
        "https://app@db.example.com/orders?sslmode=require",
        "postgres://db.example.com/orders?sslmode=require",
    ] {
        assert_eq!(
            policy::target_as_entered(refused),
            Err(policy::TargetRefused::NotATarget),
            "{refused}"
        );
    }

    let stored = entered_and_saved(
        &sealed,
        CONNECTION,
        "postgres",
        &json!({"database_url": url}),
    )
    .await
    .unwrap();
    assert!(is_sealed(&stored["database_url"]));
    assert_eq!(stored["database_target"], target.unwrap());
    // As the engine stores one entered at the server.
    let here = sealed
        .seal_parameters(
            TENANT,
            CONNECTION,
            "postgres",
            &json!({"database_url": url}),
            None,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored["database_target"], here["database_target"]);
    assert_eq!(
        stored["database_url"][SEALED_MARKER]["policy"],
        here["database_url"][SEALED_MARKER]["policy"]
    );
}

#[tokio::test]
async fn an_mcp_connections_extra_headers_are_entered_under_names_the_offer_gives() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let readable = json!({
        "url": format!("{}/mcp", server.uri()),
        "auth_mode": "bearer",
        "bearer_token": SECRET,
        "extra_headers": {"X-Tenant-Key": "header-CANARY"},
    });
    let (parameters, secrets) = testing::enter("mcp", &readable);
    assert!(is_entered(&parameters["extra_headers"]["X-Tenant-Key"]));
    assert_eq!(secrets.len(), 2);

    let offered = sealed
        .offer(TENANT, CONNECTION, "mcp", &parameters)
        .await
        .unwrap()
        .unwrap();
    let field = &offered.fields["extra_headers.X-Tenant-Key"];
    assert_eq!(field, &policy::extra_header_field("X-Tenant-Key"));

    let stored = entered_and_saved(&sealed, CONNECTION, "mcp", &readable)
        .await
        .unwrap();
    assert!(is_sealed(&stored["bearer_token"]));
    assert!(is_sealed(&stored["extra_headers"]["X-Tenant-Key"]));
    let clear = without_sealed(&stored).to_string();
    assert!(!clear.contains(SECRET) && !clear.contains("header-CANARY"));
}

#[tokio::test]
async fn one_save_enters_no_more_secrets_than_an_envelope_holds() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let headers: serde_json::Map<String, Value> = (0..=MAX_ENTERED_SECRETS)
        .map(|n| (format!("X-Header-{n}"), entered()))
        .collect();
    let parameters = json!({
        "url": format!("{}/mcp", server.uri()),
        "auth_mode": "none",
        "extra_headers": headers,
    });

    let error = sealed
        .offer(TENANT, CONNECTION, "mcp", &parameters)
        .await
        .unwrap_err();

    assert!(matches!(error, SealingError::Intake(_)), "{error}");
}
