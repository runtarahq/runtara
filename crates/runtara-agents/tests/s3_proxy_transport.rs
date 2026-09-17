//! The server's S3 client must retain native HTTP after native agents are removed.
//! This separate test binary owns the proxy environment and its OnceLock caches.

#[tokio::test]
async fn server_s3_client_still_uses_the_credential_proxy() {
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let proxy = MockServer::start().await;
    // No other test or task in this binary reads these keys before the call.
    unsafe {
        std::env::set_var(
            "RUNTARA_HTTP_PROXY_URL",
            format!("{}/api/internal/proxy", proxy.uri()),
        );
        std::env::set_var("RUNTARA_TENANT_ID", "s3-transport-test");
    }
    Mock::given(method("POST"))
        .and(path("/api/internal/proxy"))
        .and(header("X-Org-Id", "s3-transport-test"))
        .and(body_partial_json(json!({
            "method": "PUT",
            "url": "/retained-storage",
            "connection_id": "opaque-s3-reference"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 200, "headers": {}, "body": ""
        })))
        .expect(1)
        .mount(&proxy)
        .await;
    tokio::task::spawn_blocking(|| {
        runtara_agents::s3_client::S3Client::new("opaque-s3-reference".into(), true)
            .create_bucket("retained-storage")
    })
    .await
    .unwrap()
    .unwrap();
}
