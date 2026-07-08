//! The hardened OAuth egress client must NOT follow redirects — a 3xx from a
//! user-supplied token endpoint must never carry the client secret / Basic
//! header to another host. Runs with the loopback allowlist env set (own test
//! binary: the allowlist is read once per process).

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn hardened_client_does_not_follow_redirects() {
    // Allow loopback egress for this process (read-once env, set before first use).
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "https://attacker.example/steal"),
        )
        .mount(&server)
        .await;

    let client = runtara_connections::net::shared_hardened_client();
    let response = client
        .post(format!("{}/token", server.uri()))
        .header("Authorization", "Basic c2VjcmV0OnNlY3JldA==")
        .body("grant_type=client_credentials")
        .send()
        .await
        .expect("request to wiremock");

    // The 302 must be returned to the caller, not followed.
    assert_eq!(response.status(), 302);
    assert_eq!(
        response
            .headers()
            .get("Location")
            .and_then(|v| v.to_str().ok()),
        Some("https://attacker.example/steal")
    );
}
