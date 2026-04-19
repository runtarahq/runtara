//! Internal HTTP Proxy Handler
//!
//! Proxies HTTP requests on behalf of WASM workflows, injecting connection
//! credentials server-side so that WASM modules never see secrets directly.
//!
//! Mounted at `POST /api/internal/proxy` without authentication middleware —
//! the tenant_id is passed via the `X-Org-Id` header without JWT validation.

use axum::{extract::State, http::StatusCode, response::Json};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use runtara_connections::{AwsSigningParams, ConnectionsFacade, RateLimitEventType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

// ============================================================================
// State
// ============================================================================

pub struct ProxyState {
    pub facade: Arc<ConnectionsFacade>,
    pub client: reqwest::Client,
}

// ============================================================================
// Request / Response DTOs
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ProxyRequest {
    /// HTTP method (GET, POST, PUT, DELETE, PATCH, etc.)
    pub method: String,
    /// Target URL — full URL or relative path (prepended with connection base URL)
    pub url: String,
    /// Request headers to forward
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// JSON request body
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// Base64-encoded binary body (takes precedence over `body` if both set)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_raw: Option<String>,
    /// Connection ID — when set, credentials are injected and base URL is resolved
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    /// Request timeout in milliseconds (default: 30 000)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ProxyResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    /// Parsed JSON body (if the response was valid JSON)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// Base64-encoded raw body (always present)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_raw: Option<String>,
}

// ============================================================================
// Handler
// ============================================================================

/// POST /api/internal/proxy
pub async fn proxy_handler(
    headers: axum::http::HeaderMap,
    State(state): State<Arc<ProxyState>>,
    Json(request): Json<ProxyRequest>,
) -> Result<(StatusCode, Json<ProxyResponse>), (StatusCode, Json<Value>)> {
    let tenant_id = extract_tenant_id(&headers)?;
    execute_proxy_request(&tenant_id, &state.facade, &state.client, request).await
}

/// Core proxy logic shared between internal and authenticated debug endpoints
pub async fn execute_proxy_request(
    tenant_id: &str,
    facade: &ConnectionsFacade,
    client: &reqwest::Client,
    request: ProxyRequest,
) -> Result<(StatusCode, Json<ProxyResponse>), (StatusCode, Json<Value>)> {
    // Mutable copies we'll enrich with connection data
    let mut final_headers = request.headers.clone();
    let mut final_url = request.url.clone();
    let mut aws_signing: Option<AwsSigningParams> = None;

    // ── Connection credential injection ──────────────────────────────────
    if let Some(ref connection_id) = request.connection_id {
        let conn = facade
            .get_with_parameters(connection_id, tenant_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("Database error fetching connection: {}", e)})),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": format!("Connection '{}' not found", connection_id)})),
                )
            })?;

        let integration_id = conn.integration_id.as_deref().unwrap_or("");
        let params = conn
            .connection_parameters
            .as_ref()
            .cloned()
            .unwrap_or(json!({}));

        let resolved = facade
            .resolve_connection_auth(connection_id, integration_id, &params, &mut final_headers)
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": format!("Credential resolution failed: {}", e)})),
                )
            })?;

        // Record rate limit event for analytics tracking
        let _ = facade
            .record_credential_request(connection_id, tenant_id, &RateLimitEventType::Request, None)
            .await;

        // Resolve URL against connection base URL
        if let Some(base) = resolved.base_url {
            if final_url.starts_with('/') {
                // Relative path -> prepend base URL
                final_url = format!("{}{}", base.trim_end_matches('/'), final_url);
            } else {
                // Absolute URL -> replace host with connection's base URL for security
                // (credentials should only be sent to the connection's domain)
                if let (Ok(mut parsed), Ok(base_parsed)) =
                    (url::Url::parse(&final_url), url::Url::parse(&base))
                {
                    let _ = parsed.set_host(base_parsed.host_str());
                    let _ = parsed.set_scheme(base_parsed.scheme());
                    if let Some(port) = base_parsed.port() {
                        let _ = parsed.set_port(Some(port));
                    } else {
                        let _ = parsed.set_port(None);
                    }
                    final_url = parsed.to_string();
                }
            }
        } else if final_url.starts_with('/') {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Relative URL requires a connection with a base URL"})),
            ));
        }

        aws_signing = resolved.aws_signing;

        // ── Pre-flight rate limit check ────────────────────────────────────
        // If adaptive rate limiting is enabled, check the token bucket before
        // dispatching upstream. When tokens are exhausted, return a synthetic
        // 429 with Retry-After so the agent's #[durable] retry loop can use
        // durable_sleep() (suspendable/resumable) to wait.
        if crate::config::adaptive_rate_limiting_enabled()
            && let Err(retry_after_ms) = facade
                .check_rate_limit(connection_id, &conn.rate_limit_config)
                .await
        {
            // Record rate_limited event for analytics
            let _ = facade
                .record_credential_request(
                    connection_id,
                    tenant_id,
                    &RateLimitEventType::RateLimited,
                    Some(json!({
                        "source": "preflight",
                        "retry_after_ms": retry_after_ms
                    })),
                )
                .await;

            tracing::info!(
                target: "proxy",
                connection_id = connection_id,
                retry_after_ms = retry_after_ms,
                "Pre-flight rate limit: returning 429 for durable retry"
            );

            // Return synthetic 429 with precise Retry-After in milliseconds.
            let retry_after_secs = (retry_after_ms / 1000).max(1);
            let mut headers = HashMap::new();
            headers.insert("retry-after".to_string(), retry_after_secs.to_string());
            headers.insert("retry-after-ms".to_string(), retry_after_ms.to_string());
            return Ok((
                StatusCode::OK,
                Json(ProxyResponse {
                    status: 429,
                    headers,
                    body: Some(json!({
                        "error": "Rate limited (pre-flight)",
                        "retry_after_ms": retry_after_ms
                    })),
                    body_raw: None,
                }),
            ));
        }
    }

    // ── SSRF protection: block private/internal IP ranges ─────────────────
    reject_private_url(&final_url)?;

    tracing::info!(
        target: "proxy",
        method = %request.method,
        url = %final_url,
        connection_id = ?request.connection_id,
        header_count = final_headers.len(),
        "Proxy forwarding request"
    );

    // ── Build outbound request ───────────────────────────────────────────
    let method = request.method.to_uppercase();
    let reqwest_method = method.parse::<reqwest::Method>().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Invalid HTTP method: {}", method)})),
        )
    })?;

    let timeout = Duration::from_millis(request.timeout_ms.unwrap_or(30_000));

    // Resolve body bytes for SigV4 signing (we need them before building the request)
    let body_bytes: Option<Vec<u8>> = if let Some(ref raw) = request.body_raw {
        Some(BASE64.decode(raw).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid base64 in body_raw: {}", e)})),
            )
        })?)
    } else {
        request
            .body
            .as_ref()
            .map(|json_body| serde_json::to_vec(json_body).unwrap_or_default())
    };

    // ── AWS SigV4 signing (if needed) ───────────────────────────────────
    if let Some(ref aws) = aws_signing {
        let parsed_url = url::Url::parse(&final_url).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid URL for SigV4 signing: {}", e)})),
            )
        })?;
        let payload = body_bytes.as_deref().unwrap_or(b"");
        runtara_connections::auth::aws_signing::sign_request_v4(
            &method,
            &parsed_url,
            &mut final_headers,
            payload,
            &aws.access_key_id,
            &aws.secret_access_key,
            &aws.region,
            &aws.service,
            aws.session_token.as_deref(),
        );
    }

    let mut req_builder = client.request(reqwest_method, &final_url).timeout(timeout);

    // Set headers
    let mut header_map = HeaderMap::new();
    for (k, v) in &final_headers {
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            header_map.insert(name, val);
        }
    }
    req_builder = req_builder.headers(header_map);

    // Set body
    if let Some(bytes) = body_bytes {
        // If JSON body was provided and no Content-Type set, add it
        if request.body.is_some()
            && request.body_raw.is_none()
            && !final_headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("content-type"))
        {
            req_builder = req_builder.header("Content-Type", "application/json");
        }
        req_builder = req_builder.body(bytes);
    }

    // ── Execute request ──────────────────────────────────────────────────
    let response = req_builder.send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Upstream request failed: {}", e)})),
        )
    })?;

    let status = response.status().as_u16();

    // Collect response headers
    let mut resp_headers = HashMap::new();
    for (name, value) in response.headers().iter() {
        if let Ok(v) = value.to_str() {
            resp_headers.insert(name.to_string(), v.to_string());
        }
    }

    // Read response body
    let resp_body_bytes = response.bytes().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("Failed to read upstream response body: {}", e)})),
        )
    })?;

    // Track upstream 429 responses for analytics
    if status == 429
        && let Some(ref connection_id) = request.connection_id
    {
        let retry_after = resp_headers
            .get("retry-after")
            .or_else(|| resp_headers.get("Retry-After"))
            .and_then(|v| v.parse::<u64>().ok());

        let _ = facade
            .record_credential_request(
                connection_id,
                tenant_id,
                &RateLimitEventType::RateLimited,
                Some(json!({
                    "source": "upstream_429",
                    "retry_after_secs": retry_after
                })),
            )
            .await;

        tracing::warn!(
            target: "proxy",
            connection_id = connection_id.as_str(),
            retry_after_secs = ?retry_after,
            "Upstream returned 429 — rate limited"
        );
    }

    // Try to parse as JSON; always provide base64 raw body too
    let json_body = serde_json::from_slice::<Value>(&resp_body_bytes).ok();
    let raw_body = BASE64.encode(&resp_body_bytes);

    Ok((
        StatusCode::OK,
        Json(ProxyResponse {
            status,
            headers: resp_headers,
            body: json_body,
            body_raw: Some(raw_body),
        }),
    ))
}

// ============================================================================
// Helpers
// ============================================================================

/// Extract tenant_id from X-Org-Id header (no JWT validation)
fn extract_tenant_id(headers: &axum::http::HeaderMap) -> Result<String, (StatusCode, Json<Value>)> {
    headers
        .get("X-Org-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "Missing X-Org-Id header"})),
            )
        })
}

/// SSRF protection: reject URLs targeting private/internal IP ranges.
///
/// Blocks: loopback (127.0.0.0/8), private (10/8, 172.16/12, 192.168/16),
/// link-local (169.254/16), and IPv6 equivalents (::1, fc00::/7, fe80::/10).
fn reject_private_url(url: &str) -> Result<(), (StatusCode, Json<Value>)> {
    let parsed = url::Url::parse(url).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid URL"})),
        )
    })?;

    let host = parsed.host_str().unwrap_or("");

    // Block localhost by name
    if host == "localhost" || host.ends_with(".localhost") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Requests to localhost are not allowed"})),
        ));
    }

    // Resolve hostname to IP and check
    if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&(host, 80)) {
        for addr in addrs {
            let ip = addr.ip();
            if ip.is_loopback() || is_private_ip(&ip) {
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(
                        json!({"error": format!("Requests to private/internal addresses are not allowed: {}", ip)}),
                    ),
                ));
            }
        }
    }

    Ok(())
}

fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()                       // 127.0.0.0/8
                || v4.is_private()                  // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()               // 169.254/16
                || v4.is_broadcast()                // 255.255.255.255
                || v4.is_unspecified()              // 0.0.0.0
                || v4.octets()[0] == 100 && v4.octets()[1] >= 64 && v4.octets()[1] <= 127 // CGNAT 100.64/10
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()                       // ::1
                || v6.is_unspecified()              // ::
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 (unique local)
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 (link-local)
        }
    }
}
