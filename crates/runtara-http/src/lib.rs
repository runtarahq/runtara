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
    pub fn call(self) -> Result<HttpResponse, HttpError> {
        #[cfg(not(target_family = "wasm"))]
        {
            native::execute(self)
        }
        #[cfg(target_family = "wasm")]
        {
            wasi_backend::execute(self)
        }
    }
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
