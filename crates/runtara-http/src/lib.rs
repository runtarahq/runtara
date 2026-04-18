// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Central HTTP client abstraction for runtara.
//!
//! Provides a blocking HTTP client that works on both native (via ureq)
//! and WASI (via wasi-http, future) targets.

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub use native::NativeHttpClient as HttpClient;

#[cfg(target_family = "wasm")]
mod wasi_backend;
#[cfg(target_family = "wasm")]
pub use wasi_backend::WasiHttpClient as HttpClient;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

/// Builder for an HTTP request.
pub struct RequestBuilder {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) query_params: Vec<(String, String)>,
    pub(crate) body: Option<Body>,
    pub(crate) timeout: Option<Duration>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) agent: Option<ureq::Agent>,
}

pub(crate) enum Body {
    Json(serde_json::Value),
    Bytes(Vec<u8>),
}

/// Response from an HTTP request.
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Raw response body bytes.
    pub body: Vec<u8>,
    /// Response headers (lowercase keys).
    pub headers: HashMap<String, String>,
}

/// HTTP error.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// Non-2xx HTTP response.
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },

    /// Transport-level error (DNS, connection, timeout).
    #[error("Transport error: {0}")]
    Transport(String),

    /// IO error reading the response body.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl HttpResponse {
    /// Consume the response and return the body as a UTF-8 string.
    pub fn into_string(self) -> Result<String, HttpError> {
        String::from_utf8(self.body)
            .map_err(|e| HttpError::Transport(format!("Response is not valid UTF-8: {}", e)))
    }

    /// Consume the response and deserialize the body as JSON.
    pub fn into_json<T: serde::de::DeserializeOwned>(self) -> Result<T, HttpError> {
        serde_json::from_slice(&self.body).map_err(HttpError::Json)
    }

    /// Get a response header by name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(|s| s.as_str())
    }
}

impl RequestBuilder {
    pub(crate) fn new(method: &str, url: &str) -> Self {
        Self {
            method: method.to_string(),
            url: url.to_string(),
            headers: Vec::new(),
            query_params: Vec::new(),
            body: None,
            timeout: None,
            #[cfg(not(target_family = "wasm"))]
            agent: None,
        }
    }

    /// Add a header to the request.
    pub fn header(mut self, key: &str, value: &str) -> Self {
        self.headers.push((key.to_string(), value.to_string()));
        self
    }

    /// Set a JSON body (serializes the value).
    pub fn body_json(mut self, value: &serde_json::Value) -> Self {
        self.body = Some(Body::Json(value.clone()));
        self
    }

    /// Set a raw byte body.
    pub fn body_bytes(mut self, data: &[u8]) -> Self {
        self.body = Some(Body::Bytes(data.to_vec()));
        self
    }

    /// Add a query parameter.
    pub fn query(mut self, key: &str, value: &str) -> Self {
        self.query_params.push((key.to_string(), value.to_string()));
        self
    }

    /// Set a per-request timeout (overrides client default).
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// Execute the request and return the response.
    ///
    /// Note: Unlike ureq, this does NOT treat non-2xx as errors.
    /// All valid HTTP responses are returned as `Ok(HttpResponse)`.
    /// Only transport-level failures return `Err`.
    ///
    /// When the `RUNTARA_HTTP_PROXY_URL` environment variable is set, the request
    /// is serialized as JSON and POSTed to the proxy endpoint instead of being
    /// executed directly.
    /// Execute the request directly (no proxy). Used by SDK and internal APIs.
    pub fn call(self) -> Result<HttpResponse, HttpError> {
        #[cfg(not(target_family = "wasm"))]
        return native::execute(self);
        #[cfg(target_family = "wasm")]
        return wasi_backend::execute(self);
    }

    /// Execute the request through the HTTP proxy (if configured).
    /// Used by agent capabilities — the proxy handles credential injection
    /// and URL rewriting for requests with X-Runtara-Connection-Id.
    /// Falls back to direct call if no proxy is configured.
    pub fn call_agent(self) -> Result<HttpResponse, HttpError> {
        static PROXY_URL: OnceLock<Option<String>> = OnceLock::new();
        let proxy_url = PROXY_URL.get_or_init(|| std::env::var("RUNTARA_HTTP_PROXY_URL").ok());

        if let Some(proxy) = proxy_url {
            return self.call_via_proxy(proxy);
        }

        // No proxy configured — fall back to direct call
        self.call()
    }

    /// Execute the request by forwarding it through an HTTP proxy.
    ///
    /// The original request is serialized as JSON and POSTed to the proxy URL.
    /// The proxy response is deserialized back into an `HttpResponse`.
    fn call_via_proxy(self, proxy_url: &str) -> Result<HttpResponse, HttpError> {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD as BASE64;

        // Extract connection_id from headers if present
        let connection_id = self
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-runtara-connection-id"))
            .map(|(_, v)| v.clone());

        // Remove X-Runtara-* headers from forwarded headers
        let clean_headers: Vec<(String, String)> = self
            .headers
            .iter()
            .filter(|(k, _)| !k.to_lowercase().starts_with("x-runtara-"))
            .cloned()
            .collect();

        // Build the full URL with query params
        let full_url = build_url_with_query(&self.url, &self.query_params);

        // Serialize body
        let (body_json, body_raw, _body_type) = match &self.body {
            Some(Body::Json(v)) => (Some(v.clone()), None::<String>, "json"),
            Some(Body::Bytes(b)) => (None, Some(BASE64.encode(b)), "binary"),
            None => (None, None, "none"),
        };

        // Build proxy request payload
        let proxy_body = serde_json::json!({
            "method": self.method,
            "url": full_url,
            "headers": headers_to_map(&clean_headers),
            "body": body_json,
            "body_raw": body_raw,
            "connection_id": connection_id,
            "timeout_ms": self.timeout.map(|t| t.as_millis() as u64),
        });

        // Create a new request to the proxy
        let mut proxy_request = RequestBuilder::new("POST", proxy_url);
        proxy_request.body = Some(Body::Json(proxy_body));
        proxy_request
            .headers
            .push(("Content-Type".to_string(), "application/json".to_string()));

        // Forward tenant ID header (X-Org-Id)
        // Try from original request headers first, then from RUNTARA_TENANT_ID env var
        // (cached on first read — env is stable for scenario lifetime).
        static TENANT_ID: OnceLock<Option<String>> = OnceLock::new();
        if let Some(tenant) = self
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-org-id"))
        {
            proxy_request.headers.push(tenant.clone());
        } else if let Some(tenant_id) =
            TENANT_ID.get_or_init(|| std::env::var("RUNTARA_TENANT_ID").ok())
        {
            proxy_request
                .headers
                .push(("X-Org-Id".to_string(), tenant_id.clone()));
        }

        // Execute directly (bypass proxy check to avoid recursion)
        #[cfg(not(target_family = "wasm"))]
        let proxy_response = native::execute(proxy_request)?;
        #[cfg(target_family = "wasm")]
        let proxy_response = wasi_backend::execute(proxy_request)?;

        // Parse proxy response
        let resp_json: serde_json::Value = serde_json::from_slice(&proxy_response.body)
            .map_err(|e| HttpError::Transport(format!("Failed to parse proxy response: {}", e)))?;

        // Reconstruct HttpResponse
        let status = resp_json["status"].as_u64().unwrap_or(502) as u16;
        let resp_headers: HashMap<String, String> = resp_json["headers"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Decode body — prefer body_raw (base64) if present, otherwise body (JSON)
        let body = if let Some(raw) = resp_json["body_raw"].as_str() {
            BASE64.decode(raw).map_err(|e| {
                HttpError::Transport(format!("Invalid base64 in proxy response: {e}"))
            })?
        } else if let Some(body_val) = resp_json.get("body") {
            if body_val.is_null() {
                Vec::new()
            } else {
                serde_json::to_vec(body_val).unwrap_or_default()
            }
        } else {
            Vec::new()
        };

        Ok(HttpResponse {
            status,
            body,
            headers: resp_headers,
        })
    }
}

/// Build a full URL by appending query parameters.
fn build_url_with_query(url: &str, query_params: &[(String, String)]) -> String {
    if query_params.is_empty() {
        return url.to_string();
    }
    let qs: String = query_params
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    if url.contains('?') {
        format!("{url}&{qs}")
    } else {
        format!("{url}?{qs}")
    }
}

/// Simple percent-encoding for query parameter keys/values.
fn url_encode(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
            _ => {
                for byte in c.to_string().as_bytes() {
                    result.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    result
}

/// Convert a header list to a map (last value wins for duplicate keys).
fn headers_to_map(headers: &[(String, String)]) -> HashMap<String, String> {
    headers.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_client() {
        let _client = HttpClient::new();
    }

    #[test]
    fn test_create_client_with_timeout() {
        let _client = HttpClient::with_timeout(Duration::from_secs(5));
    }

    #[test]
    fn test_request_builder_headers() {
        let client = HttpClient::new();
        let _req = client
            .request("GET", "http://example.com")
            .header("Authorization", "Bearer token")
            .header("Accept", "application/json");
    }

    #[test]
    fn test_request_builder_query_params() {
        let client = HttpClient::new();
        let _req = client
            .request("GET", "http://example.com")
            .query("page", "1")
            .query("limit", "10");
    }

    #[test]
    fn test_request_builder_json_body() {
        let client = HttpClient::new();
        let body = serde_json::json!({"key": "value"});
        let _req = client
            .request("POST", "http://example.com")
            .body_json(&body);
    }

    #[test]
    fn test_request_builder_bytes_body() {
        let client = HttpClient::new();
        let _req = client
            .request("PUT", "http://example.com")
            .body_bytes(b"raw data");
    }

    #[test]
    fn test_http_response_into_string() {
        let resp = HttpResponse {
            status: 200,
            body: b"hello world".to_vec(),
            headers: HashMap::new(),
        };
        assert_eq!(resp.into_string().unwrap(), "hello world");
    }

    #[test]
    fn test_http_response_into_json() {
        let resp = HttpResponse {
            status: 200,
            body: br#"{"key":"value"}"#.to_vec(),
            headers: HashMap::new(),
        };
        let val: serde_json::Value = resp.into_json().unwrap();
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn test_http_response_header() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        let resp = HttpResponse {
            status: 200,
            body: vec![],
            headers,
        };
        assert_eq!(resp.header("Content-Type"), Some("application/json"));
        assert_eq!(resp.header("x-missing"), None);
    }

    #[test]
    fn test_http_error_display() {
        let err = HttpError::Status {
            status: 404,
            body: "Not Found".to_string(),
        };
        assert!(err.to_string().contains("404"));

        let err = HttpError::Transport("connection refused".to_string());
        assert!(err.to_string().contains("connection refused"));
    }
}
