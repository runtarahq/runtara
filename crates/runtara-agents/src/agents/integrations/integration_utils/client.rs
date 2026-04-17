// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared proxy-aware HTTP client for integrations.
//!
//! Integrations today each reimplement the same call pattern on top of
//! `crate::http::http_request`:
//!
//! 1. Attach `X-Runtara-Connection-Id` from the connection.
//! 2. Build an `HttpRequestInput` with JSON or form body.
//! 3. Call `http::http_request`.
//! 4. Check `response.success` and on failure wrap the body in a structured error.
//! 5. On success unwrap `HttpResponseBody::Json`.
//!
//! `ProxyHttpClient` + `ProxyRequest` make that pattern first-class while
//! staying on top of the existing `http_request` boundary — it's a shape
//! over that boundary, not a replacement for it.
//!
//! The client does **not** retry. Retries are owned by the `#[durable]`
//! runtime. However, when the upstream returns 429 this module parses
//! `Retry-After` / `Retry-After-Ms` response headers (via
//! `crate::types::parse_retry_after_header`) and embeds the value as
//! `retry_after_ms` in the structured error's attributes so the retry
//! loop can honor the server's hint — which fixes a latent bug where
//! integration-layer 429s historically dropped that signal.

use std::collections::HashMap;

use serde_json::Value;

use crate::connections::RawConnection;
use crate::http::{
    self, BodyType, HttpBody, HttpMethod, HttpRequestInput, HttpResponse, HttpResponseBody,
    ResponseType,
};
use crate::types::parse_retry_after_header;

use super::error::IntegrationError;
use super::url::urlencoded;

/// Default timeout applied to every proxy request unless overridden.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Reusable, connection-bound HTTP client that calls the host's HTTP boundary
/// with structured error mapping.
pub struct ProxyHttpClient<'a> {
    connection: &'a RawConnection,
    integration_prefix: &'static str,
    default_timeout_ms: u64,
    extra_headers: Vec<(String, String)>,
}

impl<'a> ProxyHttpClient<'a> {
    /// Create a new client bound to `connection` and tagged with
    /// `integration_prefix` (used in structured error codes, e.g.
    /// `"HUBSPOT"` -> `HUBSPOT_UNAUTHORIZED`).
    pub fn new(connection: &'a RawConnection, integration_prefix: &'static str) -> Self {
        Self {
            connection,
            integration_prefix,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            extra_headers: Vec::new(),
        }
    }

    /// Override the default timeout applied to every request built from
    /// this client.
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.default_timeout_ms = ms;
        self
    }

    /// Attach an extra header that will be present on every request built
    /// from this client (e.g. `"Accept"`).
    pub fn with_header(mut self, k: &str, v: &str) -> Self {
        self.extra_headers.push((k.to_string(), v.to_string()));
        self
    }

    /// The integration prefix used for structured error codes.
    pub fn prefix(&self) -> &'static str {
        self.integration_prefix
    }

    /// Start a request with an explicit method.
    pub fn request(&self, method: HttpMethod, path: impl Into<String>) -> ProxyRequest<'_> {
        ProxyRequest {
            client: self,
            method,
            path: path.into(),
            query: HashMap::new(),
            headers: HashMap::new(),
            body: RequestBody::None,
            timeout_ms: self.default_timeout_ms,
        }
    }

    pub fn get(&self, path: impl Into<String>) -> ProxyRequest<'_> {
        self.request(HttpMethod::Get, path)
    }

    pub fn post(&self, path: impl Into<String>) -> ProxyRequest<'_> {
        self.request(HttpMethod::Post, path)
    }

    pub fn patch(&self, path: impl Into<String>) -> ProxyRequest<'_> {
        self.request(HttpMethod::Patch, path)
    }

    pub fn put(&self, path: impl Into<String>) -> ProxyRequest<'_> {
        self.request(HttpMethod::Put, path)
    }

    pub fn delete(&self, path: impl Into<String>) -> ProxyRequest<'_> {
        self.request(HttpMethod::Delete, path)
    }
}

enum RequestBody {
    None,
    Json(Value),
    /// `application/x-www-form-urlencoded` body (already encoded).
    FormUrlEncoded(String),
    /// Raw text body with a content-type already set via `.header`.
    Text(String),
    /// Raw binary body.
    Binary(Vec<u8>),
}

/// Builder for a single request tied to a `ProxyHttpClient`.
pub struct ProxyRequest<'c> {
    client: &'c ProxyHttpClient<'c>,
    method: HttpMethod,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: RequestBody,
    timeout_ms: u64,
}

impl<'c> ProxyRequest<'c> {
    /// Attach query string parameters.
    pub fn query<K, V>(mut self, params: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        for (k, v) in params {
            self.query.insert(k.into(), v.into());
        }
        self
    }

    /// Attach a JSON request body. Sets `Content-Type: application/json` if
    /// not already set.
    pub fn json_body(mut self, body: Value) -> Self {
        self.body = RequestBody::Json(body);
        self
    }

    /// Attach a form-urlencoded body. Sets
    /// `Content-Type: application/x-www-form-urlencoded` if not already set.
    pub fn form_body<K: AsRef<str>, V: AsRef<str>>(mut self, parts: &[(K, V)]) -> Self {
        let body = parts
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoded(k.as_ref()), urlencoded(v.as_ref())))
            .collect::<Vec<_>>()
            .join("&");
        self.body = RequestBody::FormUrlEncoded(body);
        self
    }

    /// Attach a raw text body. The caller is responsible for setting
    /// `Content-Type` via `.header(..)`.
    pub fn body_text(mut self, s: String) -> Self {
        self.body = RequestBody::Text(s);
        self
    }

    /// Attach a raw binary body. The caller is responsible for setting
    /// `Content-Type` via `.header(..)`.
    pub fn body_binary(mut self, bytes: Vec<u8>) -> Self {
        self.body = RequestBody::Binary(bytes);
        self
    }

    /// Override / add a header on this specific request.
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.insert(k.to_string(), v.to_string());
        self
    }

    /// Override the request timeout for this specific request.
    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Send the request and parse the response as JSON.
    pub fn send_json(self) -> Result<Value, IntegrationError> {
        let prefix = self.client.integration_prefix;
        let response = self.send_raw()?;
        match response.body {
            HttpResponseBody::Json(v) => Ok(v),
            HttpResponseBody::Text(t) if t.is_empty() => Ok(Value::Null),
            _ => Err(IntegrationError::Deserialization {
                prefix,
                message: "expected JSON response".to_string(),
            }),
        }
    }

    /// Send the request, returning the raw `HttpResponse` without JSON
    /// parsing. Still maps non-2xx into `IntegrationError`.
    pub fn send_raw(self) -> Result<HttpResponse, IntegrationError> {
        let prefix = self.client.integration_prefix;
        let input = self.into_http_input();

        let response = match http::http_request(input) {
            Ok(r) => r,
            Err(e) => {
                // `http::http_request` already returns a JSON-as-string
                // structured error (via `types::http_error_with_headers`).
                // For a shared integration layer we prefer to re-wrap it
                // under the integration's prefix — but we cannot afford
                // to silently drop the existing `retry_after_ms` embedded
                // by the http agent. Translate by parsing the JSON and
                // remapping the code prefix; fall back to Network on
                // parse failure.
                return Err(translate_http_agent_error(prefix, &e));
            }
        };

        if !response.success {
            return Err(classify_response(prefix, &response));
        }

        Ok(response)
    }

    fn into_http_input(self) -> HttpRequestInput {
        let connection_id = self.client.connection.connection_id.clone();

        let mut headers: HashMap<String, String> = HashMap::new();
        // Always forward the connection id so the proxy can inject credentials.
        if !connection_id.is_empty() {
            headers.insert("X-Runtara-Connection-Id".to_string(), connection_id);
        }
        // Client-wide headers come next; per-request headers override below.
        for (k, v) in &self.client.extra_headers {
            headers.insert(k.clone(), v.clone());
        }
        for (k, v) in self.headers {
            headers.insert(k, v);
        }

        // Body + Content-Type resolution.
        let (body, body_type) = match self.body {
            RequestBody::None => (HttpBody(Value::Null), BodyType::default()),
            RequestBody::Json(v) => {
                headers
                    .entry("Content-Type".to_string())
                    .or_insert_with(|| "application/json".to_string());
                (HttpBody(v), BodyType::Json)
            }
            RequestBody::FormUrlEncoded(s) => {
                headers
                    .entry("Content-Type".to_string())
                    .or_insert_with(|| "application/x-www-form-urlencoded".to_string());
                (HttpBody(Value::String(s)), BodyType::Text)
            }
            RequestBody::Text(s) => (HttpBody(Value::String(s)), BodyType::Text),
            RequestBody::Binary(bytes) => {
                // Base64-encode into the `HttpBody` string slot: that's the
                // contract `BodyType::Binary` already uses elsewhere.
                use base64::Engine as _;
                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                (HttpBody(Value::String(encoded)), BodyType::Binary)
            }
        };

        HttpRequestInput {
            method: self.method,
            url: self.path,
            headers,
            query_parameters: self.query,
            body,
            body_type,
            response_type: ResponseType::Json,
            timeout_ms: self.timeout_ms,
            ..Default::default()
        }
    }
}

fn classify_response(prefix: &'static str, response: &HttpResponse) -> IntegrationError {
    let body = describe_body(&response.body);
    let status = response.status_code;

    match status {
        401 => IntegrationError::Unauthorized {
            prefix,
            status,
            body,
        },
        403 => IntegrationError::Forbidden {
            prefix,
            status,
            body,
        },
        404 => IntegrationError::NotFound {
            prefix,
            status,
            body,
        },
        429 => {
            let retry_after_ms = parse_retry_after_header(&response.headers);
            IntegrationError::RateLimited {
                prefix,
                status,
                body,
                retry_after_ms,
            }
        }
        408 | 500..=599 => IntegrationError::Upstream {
            prefix,
            status,
            body,
        },
        // Generic 4xx falls through as Unauthorized? No — keep "Upstream" for
        // unknown 5xx; for unknown 4xx map to NotFound-style permanent by
        // using the `http_status_error` path via Validation wouldn't be
        // right either. The cleanest option here is to preserve the
        // historical wire format (a generic CLIENT_ERROR code), which the
        // errors module produces for any unmapped status.
        _ => IntegrationError::Unknown {
            prefix,
            code: format!("{}_{}", prefix, status_suffix(status)),
            message: format!("{} API error: {}", prefix, body),
            category: if super::super::errors::is_transient_status(status) {
                super::error::ErrorCategory::Transient
            } else {
                super::error::ErrorCategory::Permanent
            },
            attributes: serde_json::json!({ "status_code": status, "body": body }),
        },
    }
}

fn status_suffix(status: u16) -> &'static str {
    match status {
        401 => "UNAUTHORIZED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        408 => "TIMEOUT",
        429 => "RATE_LIMITED",
        500..=599 => "SERVER_ERROR",
        _ => "CLIENT_ERROR",
    }
}

fn describe_body(body: &HttpResponseBody) -> String {
    // Matches the historical `format!("{:?}", response.body)` behavior so
    // the wire format of errors is byte-identical to the pre-migration
    // shape.
    format!("{:?}", body)
}

/// `http::http_request` already returns a JSON-encoded structured error.
/// Translate it into an `IntegrationError` under our prefix while keeping
/// `retry_after_ms` (if present) intact.
fn translate_http_agent_error(prefix: &'static str, raw: &str) -> IntegrationError {
    // Parse the embedded JSON.
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        let status = v
            .get("attributes")
            .and_then(|a| a.get("status_code"))
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<u16>().ok())
            .or_else(|| {
                v.get("attributes")
                    .and_then(|a| a.get("status_code"))
                    .and_then(|s| s.as_u64())
                    .and_then(|n| u16::try_from(n).ok())
            });
        let body = v
            .get("attributes")
            .and_then(|a| a.get("body"))
            .and_then(|b| b.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        if let Some(status) = status {
            return match status {
                401 => IntegrationError::Unauthorized {
                    prefix,
                    status,
                    body,
                },
                403 => IntegrationError::Forbidden {
                    prefix,
                    status,
                    body,
                },
                404 => IntegrationError::NotFound {
                    prefix,
                    status,
                    body,
                },
                429 => {
                    let retry_after_ms = v
                        .get("attributes")
                        .and_then(|a| a.get("retry_after_ms"))
                        .and_then(|x| {
                            x.as_u64()
                                .or_else(|| x.as_str().and_then(|s| s.parse::<u64>().ok()))
                        });
                    IntegrationError::RateLimited {
                        prefix,
                        status,
                        body,
                        retry_after_ms,
                    }
                }
                408 | 500..=599 => IntegrationError::Upstream {
                    prefix,
                    status,
                    body,
                },
                _ => IntegrationError::Network {
                    prefix,
                    message: v
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or(raw)
                        .to_string(),
                },
            };
        }
    }

    // Fallback: treat as a network-level failure.
    IntegrationError::Network {
        prefix,
        message: raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::RawConnection;
    use crate::http::{HttpResponse, HttpResponseBody};

    fn conn() -> RawConnection {
        RawConnection {
            connection_id: "conn-1".into(),
            connection_subtype: None,
            integration_id: "test".into(),
            parameters: serde_json::json!({}),
            rate_limit_config: None,
        }
    }

    #[test]
    fn builder_sets_defaults() {
        let c = conn();
        let client = ProxyHttpClient::new(&c, "TEST");
        assert_eq!(client.prefix(), "TEST");
        assert_eq!(client.default_timeout_ms, DEFAULT_TIMEOUT_MS);
    }

    #[test]
    fn builder_with_timeout_and_header() {
        let c = conn();
        let client = ProxyHttpClient::new(&c, "TEST")
            .with_timeout_ms(5_000)
            .with_header("Accept", "application/json");
        assert_eq!(client.default_timeout_ms, 5_000);
        assert_eq!(client.extra_headers.len(), 1);
    }

    #[test]
    fn classify_response_401_unauthorized() {
        let resp = HttpResponse {
            status_code: 401,
            headers: HashMap::new(),
            body: HttpResponseBody::Text("nope".into()),
            success: false,
        };
        let err = classify_response("HUBSPOT", &resp);
        let v: serde_json::Value = serde_json::from_str(&err.into_structured()).unwrap();
        assert_eq!(v["code"], "HUBSPOT_UNAUTHORIZED");
        assert_eq!(v["category"], "permanent");
    }

    #[test]
    fn classify_response_429_carries_retry_after_ms() {
        let mut headers = HashMap::new();
        headers.insert("retry-after".to_string(), "2".to_string());
        let resp = HttpResponse {
            status_code: 429,
            headers,
            body: HttpResponseBody::Text("slow down".into()),
            success: false,
        };
        let err = classify_response("OPENAI", &resp);
        let v: serde_json::Value = serde_json::from_str(&err.into_structured()).unwrap();
        assert_eq!(v["code"], "OPENAI_RATE_LIMITED");
        assert_eq!(v["attributes"]["retry_after_ms"], 2000);
    }

    #[test]
    fn classify_response_429_prefers_retry_after_ms_header() {
        let mut headers = HashMap::new();
        headers.insert("retry-after-ms".to_string(), "750".to_string());
        headers.insert("retry-after".to_string(), "5".to_string());
        let resp = HttpResponse {
            status_code: 429,
            headers,
            body: HttpResponseBody::Text("".into()),
            success: false,
        };
        let err = classify_response("STRIPE", &resp);
        let v: serde_json::Value = serde_json::from_str(&err.into_structured()).unwrap();
        assert_eq!(v["attributes"]["retry_after_ms"], 750);
    }

    #[test]
    fn classify_response_503_is_transient_upstream() {
        let resp = HttpResponse {
            status_code: 503,
            headers: HashMap::new(),
            body: HttpResponseBody::Text("down".into()),
            success: false,
        };
        let err = classify_response("BEDROCK", &resp);
        let v: serde_json::Value = serde_json::from_str(&err.into_structured()).unwrap();
        assert_eq!(v["code"], "BEDROCK_SERVER_ERROR");
        assert_eq!(v["category"], "transient");
    }

    #[test]
    fn translate_http_agent_error_preserves_retry_after_ms() {
        // Shape matches what `types::http_error_with_headers` emits after
        // `serde_json::to_string`.
        let raw = r#"{
            "code": "HTTP_RATE_LIMITED",
            "message": "HTTP 429 error: slow",
            "category": "transient",
            "severity": "warning",
            "attributes": {"status_code": "429", "retry_after_ms": "1500"}
        }"#;
        let err = translate_http_agent_error("OPENAI", raw);
        let v: serde_json::Value = serde_json::from_str(&err.into_structured()).unwrap();
        assert_eq!(v["code"], "OPENAI_RATE_LIMITED");
        assert_eq!(v["attributes"]["retry_after_ms"], 1500);
    }

    #[test]
    fn translate_http_agent_error_falls_back_to_network_on_unparseable() {
        let err = translate_http_agent_error("MAILGUN", "not json");
        let v: serde_json::Value = serde_json::from_str(&err.into_structured()).unwrap();
        assert_eq!(v["code"], "MAILGUN_NETWORK_ERROR");
        assert_eq!(v["category"], "transient");
    }
}
