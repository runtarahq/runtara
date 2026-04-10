//! Internal HTTP Proxy Handler
//!
//! Proxies HTTP requests on behalf of WASM scenarios, injecting connection
//! credentials server-side so that WASM modules never see secrets directly.
//!
//! Mounted at `POST /api/internal/proxy` without authentication middleware —
//! the tenant_id is passed via the `X-Org-Id` header without JWT validation.

use axum::{extract::State, http::StatusCode, response::Json};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use crate::api::dto::rate_limits::RateLimitEventType;
use crate::api::repositories::connections::ConnectionRepository;
use crate::api::services::rate_limits::RateLimitService;

type HmacSha256 = Hmac<Sha256>;

// ============================================================================
// State
// ============================================================================

pub struct ProxyState {
    pub pool: PgPool,
    pub client: reqwest::Client,
    pub redis_url: Option<String>,
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

use crate::api::services::proxy_auth;

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
    execute_proxy_request(
        &tenant_id,
        &state.pool,
        &state.client,
        request,
        state.redis_url.as_deref(),
    )
    .await
}

/// Core proxy logic shared between internal and authenticated debug endpoints
pub async fn execute_proxy_request(
    tenant_id: &str,
    pool: &PgPool,
    client: &reqwest::Client,
    request: ProxyRequest,
    redis_url: Option<&str>,
) -> Result<(StatusCode, Json<ProxyResponse>), (StatusCode, Json<Value>)> {
    // Mutable copies we'll enrich with connection data
    let mut final_headers = request.headers.clone();
    let mut final_url = request.url.clone();
    let mut aws_signing: Option<proxy_auth::AwsSigningParams> = None;

    // ── Connection credential injection ──────────────────────────────────
    if let Some(ref connection_id) = request.connection_id {
        let repo = ConnectionRepository::new(pool.clone());
        let conn = repo
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

        let resolved = proxy_auth::resolve_connection_auth(
            client,
            connection_id,
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

        // Record rate limit event for analytics tracking
        let rate_limit_service = RateLimitService::with_db_pool(
            Arc::new(ConnectionRepository::new(pool.clone())),
            pool.clone(),
        );
        let _ = rate_limit_service
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
        // If adaptive rate limiting is enabled and we have Redis, check the
        // token bucket before dispatching upstream. When tokens are exhausted,
        // return a synthetic 429 with Retry-After so the agent's #[durable]
        // retry loop can use durable_sleep() (suspendable/resumable) to wait.
        if crate::config::adaptive_rate_limiting_enabled()
            && let Some(redis_url) = redis_url
            && let Err(retry_after_ms) =
                check_rate_limit(pool, redis_url, connection_id, &conn.rate_limit_config).await
        {
            // Record rate_limited event for analytics
            let rate_limit_service = RateLimitService::with_db_pool(
                Arc::new(ConnectionRepository::new(pool.clone())),
                pool.clone(),
            );
            let _ = rate_limit_service
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
            // We use a custom header `retry-after-ms` for sub-second precision,
            // plus the standard `retry-after` in seconds as fallback.
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
        sign_request_v4(
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

        let rate_limit_service = RateLimitService::with_db_pool(
            Arc::new(ConnectionRepository::new(pool.clone())),
            pool.clone(),
        );
        let _ = rate_limit_service
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
// Token bucket rate limit check
// ============================================================================

/// Lua script for atomic token bucket check-and-decrement.
///
/// KEYS[1] = rate_limit:{connection_id}
/// ARGV[1] = requests_per_second (refill rate)
/// ARGV[2] = burst_size (max tokens)
/// ARGV[3] = now_ms (current timestamp in milliseconds)
///
/// Returns:
///   1 if the request is allowed (token consumed)
///   Negative value = -retry_after_ms if rate limited
const TOKEN_BUCKET_LUA: &str = r#"
local key = KEYS[1]
local rps = tonumber(ARGV[1])
local burst = tonumber(ARGV[2])
local now_ms = tonumber(ARGV[3])

local tokens = tonumber(redis.call('hget', key, 'tokens') or burst)
local last_refill = tonumber(redis.call('hget', key, 'last_refill') or now_ms)

-- Refill tokens based on elapsed time
local elapsed_ms = now_ms - last_refill
if elapsed_ms > 0 then
    local refill = (elapsed_ms / 1000.0) * rps
    tokens = math.min(burst, tokens + refill)
end

if tokens >= 1 then
    tokens = tokens - 1
    redis.call('hset', key, 'tokens', tostring(tokens))
    redis.call('hset', key, 'last_refill', tostring(now_ms))
    return 1
else
    redis.call('hset', key, 'tokens', tostring(tokens))
    redis.call('hset', key, 'last_refill', tostring(now_ms))
    -- Compute wait time until one token is available
    local wait_ms = math.ceil((1 - tokens) / rps * 1000)
    return -wait_ms
end
"#;

/// Check the token bucket for a connection and consume a token if available.
///
/// Returns `Ok(())` if the request is allowed (or if rate limiting is not
/// configured for this connection). Returns `Err(retry_after_ms)` if the
/// caller should wait before retrying.
async fn check_rate_limit(
    _pool: &PgPool,
    redis_url: &str,
    connection_id: &str,
    rate_limit_config_json: &Option<serde_json::Value>,
) -> Result<(), u64> {
    // Parse rate limit config — if absent or invalid, allow the request
    let config: crate::api::dto::rate_limits::RateLimitConfigDto = match rate_limit_config_json {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        },
        None => return Ok(()),
    };

    // Skip if requests_per_second is 0 (unconfigured)
    if config.requests_per_second == 0 {
        return Ok(());
    }

    // Connect to Redis
    let client = match redis::Client::open(redis_url) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                connection_id = connection_id,
                error = %e,
                "Redis connect failed for rate limit check — allowing request"
            );
            return Ok(());
        }
    };

    let mut conn = match client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                connection_id = connection_id,
                error = %e,
                "Redis connection failed for rate limit check — allowing request"
            );
            return Ok(());
        }
    };

    let key = format!("rate_limit:{}", connection_id);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Execute atomic Lua script
    let result: Result<i64, redis::RedisError> = redis::Script::new(TOKEN_BUCKET_LUA)
        .key(&key)
        .arg(config.requests_per_second)
        .arg(config.burst_size)
        .arg(now_ms)
        .invoke_async(&mut conn)
        .await;

    match result {
        Ok(v) if v > 0 => Ok(()),
        Ok(v) => {
            // Negative value = -retry_after_ms
            let retry_after_ms = (-v) as u64;
            Err(retry_after_ms)
        }
        Err(e) => {
            // Redis error — fail open, allow the request
            tracing::warn!(
                connection_id = connection_id,
                error = %e,
                "Token bucket Lua script failed — allowing request"
            );
            Ok(())
        }
    }
}

// ============================================================================
// AWS Signature V4 signing
// ============================================================================

/// Sign an HTTP request using AWS Signature V4.
///
/// Computes the SigV4 signature and sets the `Authorization`, `X-Amz-Date`,
/// `X-Amz-Content-Sha256`, and optionally `X-Amz-Security-Token` headers.
#[allow(clippy::too_many_arguments)]
fn sign_request_v4(
    method: &str,
    url: &url::Url,
    headers: &mut HashMap<String, String>,
    body: &[u8],
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
    session_token: Option<&str>,
) {
    let now = Utc::now();
    let date_stamp = now.format("%Y%m%d").to_string();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();

    // Payload hash
    let payload_hash = hex::encode(Sha256::digest(body));

    // Host header
    let host = url.host_str().unwrap_or("localhost");
    let host_with_port = if let Some(port) = url.port() {
        format!("{}:{}", host, port)
    } else {
        host.to_string()
    };

    // Canonical URI (URL-encoded path)
    let canonical_uri = if url.path().is_empty() {
        "/".to_string()
    } else {
        url.path().to_string()
    };

    // Canonical query string (sorted)
    let canonical_querystring = {
        let mut pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        pairs
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    };

    // Build sorted headers map for signing
    let mut headers_map = BTreeMap::new();
    headers_map.insert("host".to_string(), host_with_port.clone());
    headers_map.insert("x-amz-content-sha256".to_string(), payload_hash.clone());
    headers_map.insert("x-amz-date".to_string(), amz_date.clone());

    // Include content-type in signing if present in request headers
    for (k, v) in headers.iter() {
        let lk = k.to_lowercase();
        if lk == "content-type" {
            headers_map.insert(lk, v.trim().to_string());
        }
    }

    if let Some(token) = session_token {
        headers_map.insert("x-amz-security-token".to_string(), token.to_string());
    }

    // Include extra S3 headers (x-amz-copy-source, etc.) in signing
    for (k, v) in headers.iter() {
        let lk = k.to_lowercase();
        if lk.starts_with("x-amz-") && !headers_map.contains_key(&lk) {
            headers_map.insert(lk, v.trim().to_string());
        }
    }

    let signed_headers: Vec<String> = headers_map.keys().cloned().collect();
    let signed_headers_str = signed_headers.join(";");

    let canonical_headers: String = headers_map
        .iter()
        .map(|(k, v)| format!("{}:{}\n", k, v.trim()))
        .collect();

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        canonical_uri,
        canonical_querystring,
        canonical_headers,
        signed_headers_str,
        payload_hash
    );

    let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, region, service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        amz_date,
        credential_scope,
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    // Calculate signing key
    let k_date = hmac_sha256(
        format!("AWS4{}", secret_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");

    let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        access_key, credential_scope, signed_headers_str, signature
    );

    // Set the signing headers on the outbound request
    headers.insert("Authorization".into(), authorization);
    headers.insert("X-Amz-Date".into(), amz_date);
    headers.insert("X-Amz-Content-Sha256".into(), payload_hash);
    headers.insert("Host".into(), host_with_port);
    if let Some(token) = session_token {
        headers.insert("X-Amz-Security-Token".into(), token.to_string());
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
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
