// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Central HTTP client abstraction for runtara.
//!
//! Provides a blocking HTTP client that works on both native (via ureq)
//! and WASM (via the typed Runtara outbound host service) targets.

// Exactly one backend feature must be enabled by the consumer. `native`
// pulls ureq + the Rust TLS stack; `wasi` generates host bindings. The cfg
// gates below are written so the two cannot accidentally co-link.
#[cfg(not(any(feature = "native", feature = "wasi")))]
compile_error!(
    "runtara-http requires exactly one backend feature: `native` or `wasi`. \
     Native consumers should enable `native`; WASI workflows/agents should enable `wasi`."
);

#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::NativeHttpClient as HttpClient;

#[cfg(all(feature = "wasi", not(feature = "native")))]
mod host_io;
#[cfg(all(feature = "wasi", not(feature = "native")))]
mod wasi_backend;
#[cfg(all(feature = "wasi", not(feature = "native")))]
pub use wasi_backend::WasiHttpClient as HttpClient;

pub mod download;

use std::collections::HashMap;
use std::time::Duration;

/// Builder for an HTTP request.
pub struct RequestBuilder {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) query_params: Vec<(String, String)>,
    pub(crate) body: Option<Body>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) connection_id: Option<String>,
    pub(crate) endpoint: Option<String>,
    pub(crate) endpoint_ref: Option<String>,
    pub(crate) ai_provider: Option<String>,
    pub(crate) aws_service: Option<String>,
    pub(crate) max_response_bytes: Option<u64>,

    #[cfg(feature = "native")]
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
            connection_id: None,
            endpoint: None,
            endpoint_ref: None,
            ai_provider: None,
            aws_service: None,
            max_response_bytes: None,

            #[cfg(feature = "native")]
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

    /// Use credentials from this opaque connection ID, resolved by the host.
    pub fn connection_id(mut self, value: &str) -> Self {
        self.connection_id = Some(value.to_owned());
        self
    }

    /// Select a named endpoint declared by the connection type.
    pub fn endpoint(mut self, value: &str) -> Self {
        self.endpoint = Some(value.to_owned());
        self
    }

    /// Use a host-issued endpoint reference bound to the connection.
    pub fn endpoint_ref(mut self, value: &str) -> Self {
        self.endpoint_ref = Some(value.to_owned());
        self
    }

    /// Select the AI provider for connection compatibility checks.
    pub fn ai_provider(mut self, value: &str) -> Self {
        self.ai_provider = Some(value.to_owned());
        self
    }

    /// Select the AWS service used for outgoing request signing.
    pub fn aws_service(mut self, value: &str) -> Self {
        self.aws_service = Some(value.to_owned());
        self
    }

    /// Limit raw response bytes at the host boundary, subject to its ceiling.
    pub fn max_response_bytes(mut self, value: u64) -> Self {
        self.max_response_bytes = Some(value);
        self
    }

    /// Execute through the native SDK or the WASM outbound host service.
    /// All HTTP statuses are responses; transport failures return errors.
    pub fn call(self) -> Result<HttpResponse, HttpError> {
        #[cfg(feature = "native")]
        return native::execute(self);
        #[cfg(all(feature = "wasi", not(feature = "native")))]
        return host_io::execute(self);
    }

    /// Execute an agent request using the same outbound contract.
    pub fn call_agent(self) -> Result<HttpResponse, HttpError> {
        self.call()
    }

    /// Await the concurrent host service. Dropping the guest future cancels I/O.
    /// Native SDK builds retain their blocking ureq backend.
    pub async fn call_async(self) -> Result<HttpResponse, HttpError> {
        #[cfg(feature = "native")]
        return native::execute(self);
        #[cfg(all(feature = "wasi", not(feature = "native")))]
        return host_io::execute_async(self).await;
    }

    /// Await an agent request using the same outbound contract.
    pub async fn call_agent_async(self) -> Result<HttpResponse, HttpError> {
        self.call_async().await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "native")]
    #[test]
    fn native_connection_requests_fail_without_attempting_network() {
        let error = HttpClient::new()
            .request("GET", "http://127.0.0.1:1/unused")
            .connection_id("opaque-id")
            .call()
            .err()
            .expect("native SDK has no credential service");
        assert!(
            error
                .to_string()
                .contains("requires the outbound host service")
        );
    }

    #[test]
    fn test_request_starts_empty_from_either_constructor() {
        // The client-level timeout lives inside the backend agent and isn't
        // observable from here; what both constructors must guarantee is that
        // `request` seeds a builder with the verb and URL and nothing else.
        for client in [
            HttpClient::new(),
            HttpClient::with_timeout(Duration::from_secs(5)),
        ] {
            let req = client.request("GET", "http://example.com/path");
            assert_eq!(req.method, "GET");
            assert_eq!(req.url, "http://example.com/path");
            assert!(req.headers.is_empty());
            assert!(req.query_params.is_empty());
            assert!(req.body.is_none());
            #[cfg(feature = "native")]
            assert!(req.timeout.is_none());
        }
    }

    #[test]
    fn test_request_builder_headers() {
        let client = HttpClient::new();
        let req = client
            .request("GET", "http://example.com")
            .header("Authorization", "Bearer token")
            .header("Accept", "application/json");

        // Order is preserved: some servers are sensitive to header ordering,
        // and repeated keys must accumulate rather than overwrite.
        assert_eq!(
            req.headers,
            vec![
                ("Authorization".to_string(), "Bearer token".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ]
        );
    }

    #[test]
    fn test_request_builder_repeated_header_key_accumulates() {
        let client = HttpClient::new();
        let req = client
            .request("GET", "http://example.com")
            .header("Set-Cookie", "a=1")
            .header("Set-Cookie", "b=2");

        assert_eq!(
            req.headers,
            vec![
                ("Set-Cookie".to_string(), "a=1".to_string()),
                ("Set-Cookie".to_string(), "b=2".to_string()),
            ]
        );
    }

    #[test]
    fn test_request_builder_query_params() {
        let client = HttpClient::new();
        let req = client
            .request("GET", "http://example.com")
            .query("page", "1")
            .query("limit", "10");

        assert_eq!(
            req.query_params,
            vec![
                ("page".to_string(), "1".to_string()),
                ("limit".to_string(), "10".to_string()),
            ]
        );
        // Query params must not leak into the URL until the request executes.
        assert_eq!(req.url, "http://example.com");
    }

    #[test]
    fn test_request_builder_json_body() {
        let client = HttpClient::new();
        let body = serde_json::json!({"key": "value"});
        let req = client
            .request("POST", "http://example.com")
            .body_json(&body);

        match req.body {
            Some(Body::Json(ref value)) => assert_eq!(*value, body),
            Some(Body::Bytes(_)) => panic!("body_json stored a byte body"),
            None => panic!("body_json stored no body"),
        }
    }

    #[test]
    fn test_request_builder_bytes_body() {
        let client = HttpClient::new();
        let req = client
            .request("PUT", "http://example.com")
            .body_bytes(b"raw data");

        match req.body {
            Some(Body::Bytes(ref bytes)) => assert_eq!(bytes.as_slice(), b"raw data"),
            Some(Body::Json(_)) => panic!("body_bytes stored a JSON body"),
            None => panic!("body_bytes stored no body"),
        }
    }

    #[test]
    fn test_request_builder_last_body_wins() {
        let client = HttpClient::new();
        let req = client
            .request("POST", "http://example.com")
            .body_json(&serde_json::json!({"key": "value"}))
            .body_bytes(b"raw data");

        // A single request carries a single body; the later call replaces the
        // earlier one rather than sending both or silently keeping the first.
        match req.body {
            Some(Body::Bytes(ref bytes)) => assert_eq!(bytes.as_slice(), b"raw data"),
            Some(Body::Json(_)) => panic!("the earlier JSON body survived body_bytes"),
            None => panic!("no body was stored"),
        }
    }

    #[test]
    fn test_request_builder_timeout_is_recorded() {
        let client = HttpClient::new();
        let req = client
            .request("GET", "http://example.com")
            .timeout(Duration::from_secs(7));

        assert_eq!(req.timeout, Some(Duration::from_secs(7)));
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
