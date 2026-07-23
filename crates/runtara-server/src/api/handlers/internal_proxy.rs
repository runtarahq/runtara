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
    /// Opaque, tenant+connection-bound endpoint reference (a signed token
    /// minted after the inbound activity that produced it was authenticated).
    /// When present and valid, it supplies the request's base URL — used for
    /// providers whose base is per-request (e.g. a Teams conversation's
    /// serviceUrl) rather than a static connection base. The ref must belong
    /// to the current tenant and the request's connection, or the request is
    /// rejected. See [`crate::api::services::endpoint_ref`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<String>,
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

/// Map a credential-resolution failure onto the proxy's HTTP error contract.
///
/// A PERMANENT failure (the identity provider rejected the credentials/grant —
/// wrong client secret, dead refresh token) becomes **401** with
/// `{"code": "CREDENTIAL_RESOLUTION_FAILED", "permanent": true}` so agents
/// classify it permanent and stop durable-retrying; a transient failure
/// (transport, provider 5xx/429) keeps the legacy **502**. The `error` string
/// shape is preserved for compatibility with existing consumers.
fn map_credential_resolution_error(
    e: &runtara_connections::ConnectionsError,
) -> (StatusCode, Json<Value>) {
    let permanent = matches!(
        e,
        runtara_connections::ConnectionsError::AuthResolution(err) if err.permanent
    );
    let status = if permanent {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::BAD_GATEWAY
    };
    (
        status,
        Json(json!({
            "error": format!("Credential resolution failed: {}", e),
            "code": "CREDENTIAL_RESOLUTION_FAILED",
            "permanent": permanent,
        })),
    )
}

/// Resolve an agent-declared endpoint ref into the request's base URL.
///
/// The ref is a signed token binding a validated base URL to a specific
/// `(tenant, connection)`. This verifies the signature and enforces that the
/// ref belongs to the current tenant and the request's connection, then pins
/// `resolved.base_url` to the ref's URL. As defense in depth, when the ref
/// carries a conversation id it must appear in the request path (the ref for
/// conversation A cannot be used to POST to conversation B). Any failure is a
/// hard reject (mapped to 403) — the credential must not egress.
///
/// No-op when no ref is supplied. A connection with no static base (e.g.
/// `teams_bot`) then reaches `pin_url_to_base` with `base_url = None` and is
/// rejected there (fail-closed): such a connection cannot egress without a ref.
fn apply_endpoint_ref_override(
    endpoint_ref: Option<&str>,
    tenant_id: &str,
    connection_id: &str,
    agent_url: &str,
    resolved: &mut ResolvedConnectionAuth,
) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(token) = endpoint_ref else {
        return Ok(());
    };
    let reject = |msg: &str| {
        tracing::warn!(
            target: "proxy",
            connection_id,
            reason = msg,
            "proxy.endpoint_ref.rejected"
        );
        (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("Endpoint reference rejected: {msg}") })),
        )
    };

    let keyring =
        crate::api::services::endpoint_ref::EndpointRefKeyring::from_env().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Endpoint reference key unavailable: {e}") })),
            )
        })?;
    let binding = crate::api::services::endpoint_ref::verify(keyring, token)
        .map_err(|e| reject(&e.to_string()))?;

    if binding.tenant_id != tenant_id {
        return Err(reject("tenant mismatch"));
    }
    if binding.connection_id != connection_id {
        return Err(reject("connection mismatch"));
    }

    // Defense in depth: the ref's conversation must be the one being targeted.
    // A substring check is NOT enough — an attacker who controls the agent URL
    // could stuff the bound conversation id anywhere in the path (a query-like
    // tail, a sibling resource, a different conversation whose id is a prefix)
    // while actually addressing a different `conversations/{id}`. Bot Connector
    // reply URLs are `.../v3/conversations/{conversationId}/activities...`, so
    // we require the segment immediately after `conversations` to EXACTLY equal
    // the bound conversation id.
    if let Some(conversation_id) = binding.conversation_id.as_deref()
        && !path_targets_conversation(agent_url, conversation_id)
    {
        return Err(reject(
            "request path does not target the bound conversation",
        ));
    }

    resolved.base_url = Some(binding.base_url);
    Ok(())
}

/// True iff `agent_url`'s path addresses the Bot Connector conversation
/// `conversation_id` — i.e. the path segment right after a `conversations`
/// segment, percent-decoded, is exactly `conversation_id`.
///
/// The agent encodes the conversation id as a single RFC-3986 path segment
/// (any `/` inside becomes `%2F`), so splitting the RAW path on `/` keeps the
/// whole encoded id in one segment; decoding that one segment recovers the
/// exact original id. There is no `conversations` segment ⇒ not a
/// conversation-scoped call ⇒ rejected (the ref exists to pin a reply).
fn path_targets_conversation(agent_url: &str, conversation_id: &str) -> bool {
    let raw_path = match url::Url::parse(agent_url) {
        Ok(u) => u.path().to_string(),
        // Relative path-only agent URL (the Teams agent sends these).
        Err(_) => agent_url
            .split(['?', '#'])
            .next()
            .unwrap_or(agent_url)
            .to_string(),
    };
    let mut segments = raw_path.split('/');
    while let Some(seg) = segments.next() {
        if seg == "conversations" {
            let Some(next) = segments.next() else {
                return false;
            };
            let decoded = urlencoding::decode(next)
                .map(|s| s.into_owned())
                .unwrap_or_else(|_| next.to_string());
            return decoded == conversation_id;
        }
    }
    false
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
                    Json(json!({
                        "error": format!("Connection '{}' not found", connection_id),
                        "code": "CONNECTION_NOT_FOUND",
                    })),
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
            .map_err(|e| map_credential_resolution_error(&e))?;

        // Agent-declared AWS service (generic AWS credentials) — see
        // `apply_aws_service_override`.
        apply_aws_service_override(request.aws_service.as_deref(), &mut resolved);

        // Agent-declared endpoint ref (generic per-request base URL binding) —
        // see `apply_endpoint_ref_override`. Runs before the pin so the ref's
        // validated URL becomes the pin base.
        apply_endpoint_ref_override(
            request.endpoint_ref.as_deref(),
            tenant_id,
            connection_id,
            &request.url,
            &mut resolved,
        )?;

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

    // ── Endpoint-ref override ────────────────────────────────────────────
    use crate::api::services::endpoint_ref::{EndpointBinding, EndpointRefKeyring, sign};

    const CONV_ID: &str = "19:abc@thread.tacv2;messageid=1";
    const SVC_URL: &str = "https://smba.trafficmanager.net/amer/";

    fn ref_keyring() -> EndpointRefKeyring {
        EndpointRefKeyring::new("1", b"internal-proxy-test-secret".to_vec())
    }

    fn teams_binding(tenant: &str, connection: &str) -> EndpointBinding {
        EndpointBinding {
            v: EndpointBinding::CURRENT_VERSION,
            tenant_id: tenant.into(),
            connection_id: connection.into(),
            base_url: SVC_URL.into(),
            conversation_id: Some(CONV_ID.into()),
            conversation_type: Some("channel".into()),
            ms_tenant_id: Some("ms-tenant".into()),
            iat: 1_700_000_000,
        }
    }

    /// The Teams agent percent-encodes the conversation id into a relative path.
    fn teams_agent_url() -> String {
        format!(
            "/v3/conversations/{}/activities",
            urlencoding::encode(CONV_ID)
        )
    }

    fn empty_resolved() -> ResolvedConnectionAuth {
        ResolvedConnectionAuth {
            base_url: None,
            aws_signing: None,
            azure_signing: None,
            rotated_credentials: None,
        }
    }

    fn set_ref_secret() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| unsafe {
            std::env::set_var("RUNTARA_ENDPOINT_REF_SECRET", "internal-proxy-test-secret");
        });
    }

    #[test]
    fn endpoint_ref_noop_without_ref() {
        let mut resolved = empty_resolved();
        apply_endpoint_ref_override(None, "tenant-a", "conn-1", "/v3/x", &mut resolved).unwrap();
        assert_eq!(resolved.base_url, None);
    }

    #[test]
    fn endpoint_ref_pins_base_url_and_then_joins_under_service_url() {
        set_ref_secret();
        let token = sign(&ref_keyring(), &teams_binding("tenant-a", "conn-1"));
        let agent_url = teams_agent_url();
        let mut resolved = empty_resolved();
        apply_endpoint_ref_override(
            Some(&token),
            "tenant-a",
            "conn-1",
            &agent_url,
            &mut resolved,
        )
        .expect("valid ref accepted");
        assert_eq!(resolved.base_url.as_deref(), Some(SVC_URL));

        // The full pin must place the request UNDER the serviceUrl base path
        // (/amer/), not replace it, and stay contained there.
        let pinned = proxy_url::pin_url_to_base(
            &agent_url,
            resolved.base_url.as_deref(),
            true,
            &proxy_url::PinOptions::strict(),
        )
        .expect("relative agent path pins under the service url");
        assert!(
            pinned.starts_with("https://smba.trafficmanager.net/amer/v3/conversations/"),
            "unexpected pinned url: {pinned}"
        );
        assert!(pinned.ends_with("/activities"));
    }

    #[test]
    fn endpoint_ref_rejects_tenant_mismatch() {
        set_ref_secret();
        let token = sign(&ref_keyring(), &teams_binding("tenant-a", "conn-1"));
        let mut resolved = empty_resolved();
        let err = apply_endpoint_ref_override(
            Some(&token),
            "tenant-B",
            "conn-1",
            &teams_agent_url(),
            &mut resolved,
        )
        .expect_err("foreign tenant rejected");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(resolved.base_url, None);
    }

    #[test]
    fn endpoint_ref_rejects_connection_mismatch() {
        set_ref_secret();
        let token = sign(&ref_keyring(), &teams_binding("tenant-a", "conn-1"));
        let mut resolved = empty_resolved();
        let err = apply_endpoint_ref_override(
            Some(&token),
            "tenant-a",
            "conn-OTHER",
            &teams_agent_url(),
            &mut resolved,
        )
        .expect_err("foreign connection rejected");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn endpoint_ref_rejects_forged_signature() {
        set_ref_secret();
        let forged = EndpointRefKeyring::new("1", b"attacker-secret".to_vec());
        let token = sign(&forged, &teams_binding("tenant-a", "conn-1"));
        let mut resolved = empty_resolved();
        let err = apply_endpoint_ref_override(
            Some(&token),
            "tenant-a",
            "conn-1",
            &teams_agent_url(),
            &mut resolved,
        )
        .expect_err("forged signature rejected");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn endpoint_ref_rejects_conversation_id_not_in_path() {
        set_ref_secret();
        let token = sign(&ref_keyring(), &teams_binding("tenant-a", "conn-1"));
        let mut resolved = empty_resolved();
        // A ref for CONV_ID used to POST to a different conversation.
        let wrong_path = "/v3/conversations/19:zzz@thread.tacv2/activities";
        let err = apply_endpoint_ref_override(
            Some(&token),
            "tenant-a",
            "conn-1",
            wrong_path,
            &mut resolved,
        )
        .expect_err("conversation-path mismatch rejected");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn endpoint_ref_rejects_conversation_id_as_non_target_segment() {
        // The substring-check bypass: address a DIFFERENT conversation but slip
        // the bound id into the activity-id position so the old `contains` test
        // passed. The exact-segment check must reject this.
        set_ref_secret();
        let token = sign(&ref_keyring(), &teams_binding("tenant-a", "conn-1"));
        let mut resolved = empty_resolved();
        let evil = urlencoding::encode("19:evil@thread.tacv2");
        let bound = urlencoding::encode(CONV_ID);
        let sneaky_path = format!("/v3/conversations/{evil}/activities/{bound}");
        let err = apply_endpoint_ref_override(
            Some(&token),
            "tenant-a",
            "conn-1",
            &sneaky_path,
            &mut resolved,
        )
        .expect_err("bound id in a non-conversation segment must be rejected");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn endpoint_ref_rejects_prefix_conversation_id() {
        // A conversation whose id merely has the bound id as a prefix must not
        // be accepted (exact match, not prefix/substring).
        set_ref_secret();
        let token = sign(&ref_keyring(), &teams_binding("tenant-a", "conn-1"));
        let mut resolved = empty_resolved();
        let longer = urlencoding::encode(&format!("{CONV_ID}-attacker")).into_owned();
        let path = format!("/v3/conversations/{longer}/activities");
        let err =
            apply_endpoint_ref_override(Some(&token), "tenant-a", "conn-1", &path, &mut resolved)
                .expect_err("prefix conversation id must be rejected");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn path_targets_conversation_matches_only_exact_segment() {
        let enc = urlencoding::encode(CONV_ID).into_owned();
        // Exact target segment (with and without an activity id).
        assert!(path_targets_conversation(
            &format!("/v3/conversations/{enc}/activities"),
            CONV_ID
        ));
        assert!(path_targets_conversation(
            &format!("/v3/conversations/{enc}/activities/1:2:3"),
            CONV_ID
        ));
        // Absolute URL form.
        assert!(path_targets_conversation(
            &format!("https://smba.trafficmanager.net/amer/v3/conversations/{enc}/activities"),
            CONV_ID
        ));
        // Bound id present but not as the conversation segment.
        assert!(!path_targets_conversation(
            &format!("/v3/conversations/19%3Aevil/activities/{enc}"),
            CONV_ID
        ));
        // No conversations segment at all.
        assert!(!path_targets_conversation("/v3/attachments/xyz", CONV_ID));
        // Trailing conversations with nothing after it.
        assert!(!path_targets_conversation("/v3/conversations", CONV_ID));
    }

    #[test]
    fn teams_connection_without_ref_fails_closed_at_pin() {
        // teams_bot resolves base_url = None; with no endpoint ref the pin must
        // reject (the injected Bearer token cannot egress to an unpinned host).
        let resolved = empty_resolved();
        let result = proxy_url::pin_url_to_base(
            &teams_agent_url(),
            resolved.base_url.as_deref(),
            true,
            &proxy_url::PinOptions::strict(),
        );
        assert!(matches!(result, Err(proxy_url::ProxyReject::NoBaseUrl)));
    }
}
