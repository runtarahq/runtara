use std::sync::Arc;

use http_body_util::BodyExt;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const TENANT: &str = "acme";
const CONNECTION: &str = "6f1c5f0e-2a55-4b1c-9d0a-0d6a6a2f8f10";
const SECRET: &str = "s3cr3t-CANARY-token";

/// seal in this process with keys in memory, allowed to reach a local mock
/// server over plain HTTP.
fn sealed_for(server: &MockServer) -> SealedCredentials {
    let address = server.address();
    let allowances = Allowances {
        private_hosts: vec![format!("{}:{}", address.ip(), address.port())],
        plain_http_hosts: vec![address.ip().to_string()],
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
    assert!(!stored.to_string().contains(SECRET));
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
            &stored,
            get(format!("{}/api/orders", server.uri())),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(text(response).await, "orders");
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
        .send(TENANT, CONNECTION, &stored, request)
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
            .send(TENANT, CONNECTION, &stored, get(url.clone()))
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
        .send(TENANT, "another-connection", &stored, get(url.clone()))
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
        .send("globex", CONNECTION, &stored, get(url))
        .await
        .unwrap_err();
    assert_eq!(error.code, SealErrorCode::PolicyDenied);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_type_that_is_not_sealed_is_left_alone() {
    let server = MockServer::start().await;
    let sealed = sealed_for(&server);
    let params = json!({"client_id": "id", "client_secret": SECRET});

    let stored = sealed
        .seal_parameters(TENANT, CONNECTION, "quickbooks_online", &params, None)
        .await
        .unwrap();

    assert_eq!(stored, None);
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
