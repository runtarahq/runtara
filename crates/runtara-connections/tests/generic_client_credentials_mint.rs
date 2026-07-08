//! End-to-end mint for the generic http_oauth2_client_credentials connection:
//! resolve_connection_auth against a wiremock token endpoint, both token_auth
//! styles. Own binary: sets the loopback egress allowlist (read-once env).

use serde_json::json;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn events() -> runtara_connections::events::ConnectionEvents {
    runtara_connections::events::ConnectionEvents::default()
}

#[tokio::test]
async fn generic_client_credentials_mints_via_form_body_and_basic() {
    unsafe { std::env::set_var("RUNTARA_PROXY_ALLOWED_HOSTS", "127.0.0.1,localhost") };
    let server = MockServer::start().await;

    // form_body: creds must arrive in the body.
    Mock::given(method("POST"))
        .and(path("/form/token"))
        .and(body_string_contains("grant_type=client_credentials"))
        .and(body_string_contains("client_id=cid"))
        .and(body_string_contains("client_secret=csec"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "form-token", "token_type": "Bearer", "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    // basic: creds must arrive as the Basic header and NOT in the body.
    Mock::given(method("POST"))
        .and(path("/basic/token"))
        .and(header("Authorization", "Basic Y2lkOmNzZWM=")) // base64("cid:csec")
        .and(body_string_contains("grant_type=client_credentials"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "basic-token", "token_type": "Bearer", "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = reqwest::Client::new();

    for (style, path_part, expected_token) in [
        ("form_body", "form", "form-token"),
        ("basic", "basic", "basic-token"),
    ] {
        let params = json!({
            "token_url": format!("{}/{}/token", server.uri(), path_part),
            "client_id": "cid",
            "client_secret": "csec",
            "scope": "read",
            "base_url": "https://api.example.com",
            "token_auth": style
        });
        let mut headers = std::collections::HashMap::new();
        let resolved = runtara_connections::auth::provider_auth::resolve_connection_auth(
            &client,
            &format!("conn-{style}"),
            "http_oauth2_client_credentials",
            &params,
            &mut headers,
            &events(),
        )
        .await
        .expect("mint should succeed");

        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some(format!("Bearer {expected_token}").as_str()),
            "style {style}: minted Bearer must be injected"
        );
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://api.example.com"),
            "style {style}: base_url pins to the connection's declared host"
        );
    }

    // Mock .expect(1) each verifies exactly one mint per style (cache keys differ
    // per connection id + token_url, and each loop iteration used a distinct pair).
}
