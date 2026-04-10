use axum::{
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    Json,
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
            return Err(format!(
                "OIDC discovery returned status {}",
                res.status()
            ));
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
/// Requires `MCP_RESOURCE_URI` and `OAUTH2_ISSUER` env vars to be set.
pub async fn oauth_protected_resource_handler() -> impl IntoResponse {
    let resource_uri = std::env::var("MCP_RESOURCE_URI")
        .expect("MCP_RESOURCE_URI must be set");
    let issuer_uri = std::env::var("OAUTH2_ISSUER")
        .expect("OAUTH2_ISSUER must be set");

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
                (
                    header::CONTENT_TYPE,
                    "application/json".to_string(),
                ),
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
