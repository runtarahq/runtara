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
use runtara_connections::{
    AwsSigningParams, AzureSigningParams, ConnectionsFacade, RateLimitEventType,
    ResolvedConnectionAuth,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use super::proxy_url::{self, ProxyReject};

/// Rollout posture for the base-URL pin (`RUNTARA_PROXY_STRICT_BASE_URL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyStrictMode {
    /// Reject a request whose URL cannot be pinned to the connection base
    /// (default, fail-closed).
    Enforce,
    /// Forward with the legacy host-only rewrite but log `proxy.pin.violation`.
    Warn,
    /// Forward with the legacy host-only rewrite, no logging.
    Off,
}

/// Read `RUNTARA_PROXY_STRICT_BASE_URL` once. Default = enforce (fail-closed).
pub(crate) fn proxy_strict_mode() -> ProxyStrictMode {
    static MODE: OnceLock<ProxyStrictMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        match std::env::var("RUNTARA_PROXY_STRICT_BASE_URL")
            .ok()
            .map(|s| s.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("warn") => ProxyStrictMode::Warn,
            Some("off") => ProxyStrictMode::Off,
            _ => ProxyStrictMode::Enforce,
        }
    })
}

/// Hosts allowed to use an `http://` (non-TLS) base URL
/// (`RUNTARA_PROXY_ALLOW_HTTP_HOSTS`). Empty = https-only.
pub(crate) fn proxy_allow_http_hosts() -> Vec<String> {
    static HOSTS: OnceLock<Vec<String>> = OnceLock::new();
    HOSTS
        .get_or_init(|| {
            std::env::var("RUNTARA_PROXY_ALLOW_HTTP_HOSTS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .clone()
}

/// The pre-WP1 host-only rewrite, kept for warn/off rollout modes so behavior
/// is byte-for-byte identical to before enforcement. Does NOT enforce the base
/// path. New deployments default to enforce and never reach this.
fn apply_legacy_pin(
    final_url: &str,
    base: Option<&str>,
) -> Result<String, (StatusCode, Json<Value>)> {
    match base {
        Some(base) => {
            if final_url.starts_with('/') {
                Ok(format!("{}{}", base.trim_end_matches('/'), final_url))
            } else {
                let mut out = final_url.to_string();
                if let (Ok(mut parsed), Ok(base_parsed)) =
                    (url::Url::parse(final_url), url::Url::parse(base))
                {
                    let _ = parsed.set_host(base_parsed.host_str());
                    let _ = parsed.set_scheme(base_parsed.scheme());
                    match base_parsed.port() {
                        Some(port) => {
                            let _ = parsed.set_port(Some(port));
                        }
                        None => {
                            let _ = parsed.set_port(None);
                        }
                    }
                    out = parsed.to_string();
                }
                Ok(out)
            }
        }
        None if final_url.starts_with('/') => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Relative URL requires a connection with a base URL"})),
        )),
        None => Ok(final_url.to_string()),
    }
}

/// Map a [`ProxyReject`] to the proxy's HTTP error contract.
fn map_proxy_reject(reject: &ProxyReject, connection_id: &str) -> (StatusCode, Json<Value>) {
    match reject {
        ProxyReject::NoBaseUrl | ProxyReject::EmptyBaseUrl => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "CONNECTION_BASE_URL_REQUIRED",
                "message": format!(
                    "Connection '{connection_id}' has no base URL; the proxy refuses to forward \
                     injected credentials to an unpinned host. Set an https base URL on the connection."
                ),
                "connection_id": connection_id,
            })),
        ),
        ProxyReject::UnparseableBaseUrl(detail) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "CONNECTION_BASE_URL_INVALID",
                "message": format!("Connection '{connection_id}' base URL is not a valid URL: {detail}"),
                "connection_id": connection_id,
            })),
        ),
        ProxyReject::NonHttpsBaseUrl(base) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "CONNECTION_BASE_URL_NOT_HTTPS",
                "message": format!("Connection '{connection_id}' base URL must use https: {base}"),
                "connection_id": connection_id,
            })),
        ),
        ProxyReject::UnparseableAgentUrl(detail) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "INVALID_REQUEST_URL",
                "message": format!("Request URL is not valid: {detail}"),
            })),
        ),
        ProxyReject::PathEscape {
            final_path,
            base_path,
        } => (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "PATH_OUTSIDE_BASE",
                "message": format!(
                    "Request path '{final_path}' is outside the connection's base path '{base_path}'"
                ),
                "connection_id": connection_id,
            })),
        ),
    }
}

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
    /// Explicit AI provider requested by the caller. When present, the proxy
    /// verifies the connection's integration is compatible before credentials
    /// are applied to the outgoing request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_provider: Option<String>,
    /// AWS service the calling agent is signing for (e.g. "sqs", "dynamodb").
    /// When set, it overrides the connection's resolved signing service and, if
    /// the connection pinned no explicit endpoint, selects the regional
    /// endpoint `https://{service}.{region}.amazonaws.com`. This lets one
    /// generic `aws_credentials` connection serve any AWS service.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_service: Option<String>,
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

/// Apply an agent-declared AWS service to a resolved connection descriptor.
///
/// AWS credentials are service-agnostic — the calling agent names the service
/// it is signing for via the `X-Runtara-Aws-Service` header. This overrides the
/// resolved SigV4 signing service and, when the connection pinned no explicit
/// endpoint, synthesizes the default regional endpoint
/// (`https://{service}.{region}.amazonaws.com`). The net effect: one generic
/// `aws_credentials` connection can serve SQS, DynamoDB, SNS, … without a
/// per-service connection type. No-op unless the connection actually resolved
/// AWS SigV4 signing (so a stray header on a non-AWS connection does nothing).
fn apply_aws_service_override(aws_service: Option<&str>, resolved: &mut ResolvedConnectionAuth) {
    if let Some(service) = aws_service
        && let Some(aws) = resolved.aws_signing.as_mut()
    {
        aws.service = service.to_string();
        if resolved.base_url.is_none() {
            resolved.base_url = Some(
                runtara_connections::auth::provider_auth::aws_default_endpoint(
                    service,
                    &aws.region,
                ),
            );
        }
    }
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
    let mut azure_signing: Option<AzureSigningParams> = None;

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
        ensure_ai_provider_connection_compatible(
            request.ai_provider.as_deref(),
            connection_id,
            integration_id,
        )?;
        let params = conn
            .connection_parameters
            .as_ref()
            .cloned()
            .unwrap_or(json!({}));

        let mut resolved = facade
            .resolve_connection_auth(
                connection_id,
                tenant_id,
                integration_id,
                &params,
                &mut final_headers,
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": format!("Credential resolution failed: {}", e)})),
                )
            })?;

        // Agent-declared AWS service (generic AWS credentials) — see
        // `apply_aws_service_override`.
        apply_aws_service_override(request.aws_service.as_deref(), &mut resolved);

        // Record analytics off the request path. This can hit Redis and
        // PostgreSQL, and should not delay the upstream call.
        record_credential_request_async(
            facade,
            connection_id,
            tenant_id,
            RateLimitEventType::Request,
            None,
        );

        // ── Pin the request URL to the connection base URL (fail-closed) ─────
        // A connection-scoped request must never forward the injected
        // credential to a host the connection did not declare, nor escape its
        // base path. `pin_url_to_base` (proxy_url.rs) is the single decision
        // point. Path enforcement is relaxed where the path is the payload
        // (object stores, MCP single endpoint, signed requests). Signed
        // integrations always enforce — warn mode would sign+send a request
        // bound to an unpinned URL.
        let relax_path = integration_id == "mcp"
            || resolved.aws_signing.is_some()
            || resolved.azure_signing.is_some();
        let effective_mode = if resolved.aws_signing.is_some() || resolved.azure_signing.is_some() {
            ProxyStrictMode::Enforce
        } else {
            proxy_strict_mode()
        };
        let pin_opts = proxy_url::PinOptions {
            enforce_path_prefix: !relax_path,
            allow_http_base_hosts: proxy_allow_http_hosts(),
        };
        match proxy_url::pin_url_to_base(&final_url, resolved.base_url.as_deref(), true, &pin_opts)
        {
            Ok(pinned) => final_url = pinned,
            Err(reject) => match effective_mode {
                ProxyStrictMode::Enforce => {
                    return Err(map_proxy_reject(&reject, connection_id));
                }
                ProxyStrictMode::Warn => {
                    tracing::warn!(
                        target: "proxy",
                        connection_id = connection_id.as_str(),
                        agent_url = %request.url,
                        base_url = ?resolved.base_url,
                        reject = ?reject,
                        "proxy.pin.violation (warn mode — forwarding legacy-pinned; will be REJECTED under enforce)"
                    );
                    final_url = apply_legacy_pin(&final_url, resolved.base_url.as_deref())?;
                }
                ProxyStrictMode::Off => {
                    final_url = apply_legacy_pin(&final_url, resolved.base_url.as_deref())?;
                }
            },
        }

        aws_signing = resolved.aws_signing;
        azure_signing = resolved.azure_signing;

        // ── Pre-flight rate limit check ────────────────────────────────────
        // If adaptive rate limiting is enabled, check the token bucket before
        // dispatching upstream. When tokens are exhausted, return a synthetic
        // 429 with Retry-After so the agent's #[resilient] retry loop can use
        // durable_sleep() (suspendable/resumable) to wait.
        if crate::config::adaptive_rate_limiting_enabled()
            && let Err(retry_after_ms) = facade
                .check_rate_limit(connection_id, &conn.rate_limit_config)
                .await
        {
            record_credential_request_async(
                facade,
                connection_id,
                tenant_id,
                RateLimitEventType::RateLimited,
                Some(json!({
                    "source": "preflight",
                    "retry_after_ms": retry_after_ms
                })),
            );

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

    // ── Azure Storage Shared Key signing (if needed) ────────────────────
    if let Some(ref azure) = azure_signing {
        let parsed_url = url::Url::parse(&final_url).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Invalid URL for Azure signing: {}", e)})),
            )
        })?;

        // Rewrite a relative x-ms-copy-source header to an absolute URL on the
        // same storage account before signing, so the canonical resource and the
        // header value stay in sync.
        let copy_source_key = final_headers
            .keys()
            .find(|k| k.eq_ignore_ascii_case("x-ms-copy-source"))
            .cloned();
        if let Some(key) = copy_source_key
            && let Some(value) = final_headers.get(&key).cloned()
            && value.starts_with('/')
        {
            let scheme = parsed_url.scheme();
            let host = parsed_url.host_str().unwrap_or("");
            let absolute = match parsed_url.port() {
                Some(port) => format!("{}://{}:{}{}", scheme, host, port, value),
                None => format!("{}://{}{}", scheme, host, value),
            };
            final_headers.insert(key, absolute);
        }

        let payload = body_bytes.as_deref().unwrap_or(b"");
        runtara_connections::auth::azure_signing::sign_request_shared_key(
            &method,
            &parsed_url,
            &mut final_headers,
            payload,
            &azure.account_name,
            &azure.account_key,
        )
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("Azure Shared Key signing failed: {}", e)})),
            )
        })?;
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

        record_credential_request_async(
            facade,
            connection_id,
            tenant_id,
            RateLimitEventType::RateLimited,
            Some(json!({
                "source": "upstream_429",
                "retry_after_secs": retry_after
            })),
        );

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

fn ensure_ai_provider_connection_compatible(
    provider: Option<&str>,
    connection_id: &str,
    integration_id: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(provider) = provider else {
        return Ok(());
    };
    if runtara_ai::provider::provider_supports_integration(provider, integration_id) {
        return Ok(());
    }
    Err((
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "AI_PROVIDER_CONNECTION_MISMATCH",
            "message": format!(
                "AI provider '{}' is not compatible with connection '{}' integration '{}'",
                provider, connection_id, integration_id
            ),
            "provider": provider,
            "connection_id": connection_id,
            "integration_id": integration_id
        })),
    ))
}

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

fn record_credential_request_async(
    facade: &ConnectionsFacade,
    connection_id: &str,
    tenant_id: &str,
    event_type: RateLimitEventType,
    metadata: Option<Value>,
) {
    let facade = facade.clone();
    let connection_id = connection_id.to_string();
    let tenant_id = tenant_id.to_string();

    tokio::spawn(async move {
        if let Err(e) = facade
            .record_credential_request(&connection_id, &tenant_id, &event_type, metadata)
            .await
        {
            tracing::warn!(
                connection_id = %connection_id,
                event_type = %event_type,
                error = ?e,
                "Failed to record rate limit event"
            );
        }
    });
}

/// SSRF protection: reject URLs targeting private/internal IP ranges.
///
/// Blocks: loopback (127.0.0.0/8), private (10/8, 172.16/12, 192.168/16),
/// link-local (169.254/16), and IPv6 equivalents (::1, fc00::/7, fe80::/10).
///
/// **Escape hatch for dev/test environments only:** the
/// `RUNTARA_PROXY_ALLOWED_HOSTS` env var accepts a comma-separated list of
/// `host` or `host:port` entries that bypass the SSRF guard. The list is read
/// once at process start and is intended for local Azurite/MinIO-style
/// emulators reachable on loopback. **Do not set this in production** — it
/// re-opens the SSRF surface for every connection routed through the proxy.
fn reject_private_url(url: &str) -> Result<(), (StatusCode, Json<Value>)> {
    let parsed = url::Url::parse(url).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid URL"})),
        )
    })?;

    if is_explicitly_allowed_host(&parsed) {
        tracing::warn!(
            target: "proxy",
            url = %parsed,
            "SSRF guard bypassed by RUNTARA_PROXY_ALLOWED_HOSTS — dev/test only"
        );
        return Ok(());
    }

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
            if proxy_url::is_private_ip(&ip) {
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

// The egress allowlist + host check moved to `runtara_connections::net` so the
// OAuth token/refresh/revoke egress can share them (the egress client now uses
// them from there directly).
pub(crate) use runtara_connections::net::allowed_private_hosts;

fn is_explicitly_allowed_host(url: &url::Url) -> bool {
    let allowed = allowed_private_hosts();
    if allowed.is_empty() {
        return false;
    }
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    let host_with_port = match url.port_or_known_default() {
        Some(p) => format!("{}:{}", host, p),
        None => host.clone(),
    };
    allowed
        .iter()
        .any(|entry| entry == &host || entry == &host_with_port)
}

// `is_private_ip` now lives in `proxy_url` (hardened to decode IPv4-mapped /
// IPv4-compatible IPv6) and is shared by the pre-flight check here and the
// egress client's DNS resolver.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_provider_connection_mismatch_is_rejected_before_proxying() {
        let err = ensure_ai_provider_connection_compatible(
            Some(runtara_ai::provider::PROVIDER_OPENAI),
            "conn-aws",
            "aws_credentials",
        )
        .expect_err("provider mismatch should fail");

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        let body = err.1.0;
        assert_eq!(
            body.get("error").and_then(Value::as_str),
            Some("AI_PROVIDER_CONNECTION_MISMATCH")
        );
        assert_eq!(
            body.get("provider").and_then(Value::as_str),
            Some(runtara_ai::provider::PROVIDER_OPENAI)
        );
        assert_eq!(
            body.get("integration_id").and_then(Value::as_str),
            Some("aws_credentials")
        );
    }

    #[test]
    fn ai_provider_connection_match_is_allowed() {
        ensure_ai_provider_connection_compatible(
            Some(runtara_ai::provider::PROVIDER_BEDROCK),
            "conn-aws",
            "aws_credentials",
        )
        .expect("provider match should pass");
    }

    fn aws_resolved(base_url: Option<&str>, region: &str, service: &str) -> ResolvedConnectionAuth {
        ResolvedConnectionAuth {
            base_url: base_url.map(str::to_string),
            aws_signing: Some(AwsSigningParams {
                access_key_id: "AKIA".into(),
                secret_access_key: "secret".into(),
                region: region.into(),
                service: service.into(),
                session_token: None,
            }),
            azure_signing: None,
            rotated_credentials: None,
        }
    }

    #[test]
    fn aws_default_endpoint_uses_uniform_regional_host() {
        use runtara_connections::auth::provider_auth::aws_default_endpoint;
        assert_eq!(
            aws_default_endpoint("sqs", "us-east-1"),
            "https://sqs.us-east-1.amazonaws.com"
        );
        assert_eq!(
            aws_default_endpoint("dynamodb", "eu-west-1"),
            "https://dynamodb.eu-west-1.amazonaws.com"
        );
    }

    #[test]
    fn aws_service_override_sets_service_and_default_endpoint() {
        // Generic aws_credentials connection: no explicit endpoint, service
        // defaulted to "bedrock" during resolution. The SQS agent's header
        // must flip both the signing service and the regional endpoint.
        let mut resolved = aws_resolved(None, "us-east-1", "bedrock");
        apply_aws_service_override(Some("sqs"), &mut resolved);

        assert_eq!(resolved.aws_signing.as_ref().unwrap().service, "sqs");
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://sqs.us-east-1.amazonaws.com")
        );
    }

    #[test]
    fn aws_service_override_preserves_explicit_endpoint() {
        // A LocalStack / VPC / GovCloud endpoint on the connection must win over
        // the synthesized default, but the signing service is still overridden.
        let mut resolved = aws_resolved(Some("https://localhost:4566"), "eu-west-1", "bedrock");
        apply_aws_service_override(Some("sqs"), &mut resolved);

        assert_eq!(resolved.aws_signing.as_ref().unwrap().service, "sqs");
        assert_eq!(resolved.base_url.as_deref(), Some("https://localhost:4566"));
    }

    #[test]
    fn aws_service_override_noop_without_header() {
        // No agent-declared service → nothing changes (Bedrock/S3 path).
        let mut resolved = aws_resolved(None, "us-east-1", "bedrock");
        apply_aws_service_override(None, &mut resolved);

        assert_eq!(resolved.aws_signing.as_ref().unwrap().service, "bedrock");
        assert_eq!(resolved.base_url, None);
    }

    #[test]
    fn aws_service_override_noop_on_non_aws_connection() {
        // A stray header on a connection that resolved no SigV4 signing must not
        // invent an endpoint or otherwise mutate the descriptor.
        let mut resolved = ResolvedConnectionAuth {
            base_url: Some("https://api.example.com".into()),
            aws_signing: None,
            azure_signing: None,
            rotated_credentials: None,
        };
        apply_aws_service_override(Some("sqs"), &mut resolved);

        assert!(resolved.aws_signing.is_none());
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://api.example.com")
        );
    }
}
