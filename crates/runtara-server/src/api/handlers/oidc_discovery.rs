use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Instant;

/// Cache for the OIDC discovery response proxied from the authorization server.
pub struct OidcDiscoveryCache {
    cached: RwLock<Option<(String, Instant)>>,
    client: reqwest::Client,
    discovery_url: String,
    ttl: Duration,
}

impl OidcDiscoveryCache {
    pub fn new(issuer_uri: String) -> Self {
        let base = issuer_uri.trim_end_matches('/');
        let discovery_url = format!("{base}/.well-known/openid-configuration");

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("failed to build reqwest client");

        Self {
            cached: RwLock::new(None),
            client,
            discovery_url,
            ttl: Duration::from_secs(3600),
        }
    }

    /// Return the cached discovery document, fetching from upstream if expired or missing.
    pub async fn get_or_fetch(&self) -> Result<String, String> {
        // Fast path: read lock
        {
            let guard = self.cached.read().await;
            if let Some((ref body, fetched_at)) = *guard
                && fetched_at.elapsed() < self.ttl
            {
                return Ok(body.clone());
            }
        }

        // Slow path: write lock with double-check
        let mut guard = self.cached.write().await;
        if let Some((ref body, fetched_at)) = *guard
            && fetched_at.elapsed() < self.ttl
        {
            return Ok(body.clone());
        }

        let res = self
            .client
            .get(&self.discovery_url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch OIDC discovery: {e}"))?;

        if !res.status().is_success() {
            return Err(format!("OIDC discovery returned status {}", res.status()));
        }

        let body = res
            .text()
            .await
            .map_err(|e| format!("Failed to read OIDC discovery body: {e}"))?;

        *guard = Some((body.clone(), Instant::now()));
        Ok(body)
    }
}

/// GET /.well-known/oauth-protected-resource
///
/// Returns OAuth 2.0 Protected Resource Metadata (RFC 9728) for MCP clients.
/// Requires `MCP_RESOURCE_URI` and `OAUTH2_ISSUER` env vars to be set; when
/// either is missing the metadata simply doesn't exist for this deployment,
/// so respond 404 instead of panicking the worker (SYN-523).
pub async fn oauth_protected_resource_handler() -> axum::response::Response {
    let Ok(resource_uri) = std::env::var("MCP_RESOURCE_URI") else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "OAuth protected resource metadata is not configured (MCP_RESOURCE_URI unset)"})),
        )
            .into_response();
    };
    let Ok(issuer_uri) = std::env::var("OAUTH2_ISSUER") else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "OAuth protected resource metadata is not configured (OAUTH2_ISSUER unset)"})),
        )
            .into_response();
    };

    let body = json!({
        "resource": resource_uri,
        "authorization_servers": [issuer_uri],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["openid", "profile", "email"]
    });

    (
        [(header::CACHE_CONTROL, "public, max-age=3600")],
        Json(body),
    )
        .into_response()
}

/// GET /.well-known/openid-configuration
/// GET /.well-known/oauth-authorization-server
///
/// Proxies to the upstream authorization server with 1-hour caching.
pub async fn openid_configuration_handler(
    State(cache): State<Arc<OidcDiscoveryCache>>,
) -> impl IntoResponse {
    match cache.get_or_fetch().await {
        Ok(body) => (
            StatusCode::OK,
            [
                (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
                (header::CONTENT_TYPE, "application/json".to_string()),
            ],
            body,
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": "Failed to fetch authorization server metadata"})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::{ENV_MUTEX, EnvGuard};

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .expect("read body");
        serde_json::from_slice(&bytes).expect("parse JSON body")
    }

    #[tokio::test]
    async fn protected_resource_returns_404_when_env_unset() {
        // Regression guard for SYN-523: with MCP_RESOURCE_URI / OAUTH2_ISSUER
        // unset the handler must answer 404 — not panic the worker task.
        let _lock = ENV_MUTEX.lock().await;
        let mut guard = EnvGuard::new();
        guard.remove("MCP_RESOURCE_URI");
        guard.remove("OAUTH2_ISSUER");

        let resp = oauth_protected_resource_handler().await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert!(body["error"].as_str().unwrap().contains("MCP_RESOURCE_URI"));

        // Partial config (resource set, issuer missing) is still 404.
        guard.set("MCP_RESOURCE_URI", "https://mcp.example.com/mcp");
        let resp = oauth_protected_resource_handler().await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert!(body["error"].as_str().unwrap().contains("OAUTH2_ISSUER"));
    }

    #[tokio::test]
    async fn protected_resource_returns_metadata_when_configured() {
        let _lock = ENV_MUTEX.lock().await;
        let mut guard = EnvGuard::new();
        guard.set("MCP_RESOURCE_URI", "https://mcp.example.com/mcp");
        guard.set("OAUTH2_ISSUER", "https://idp.example.com/");

        let resp = oauth_protected_resource_handler().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["resource"], "https://mcp.example.com/mcp");
        assert_eq!(body["authorization_servers"][0], "https://idp.example.com/");
    }
}
