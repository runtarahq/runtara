// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP agent for making web requests
//!
//! This module provides HTTP request operations with support for:
//! - Multiple HTTP methods (GET, POST, PUT, DELETE, PATCH, etc.)
//! - Custom headers and query parameters
//! - JSON and binary request/response bodies
//! - Response body as JSON or raw bytes/text
//!
//! The actual HTTP execution happens on the host side via host functions,
//! while this module handles request preparation and response parsing.

use crate::types::{http_error, network_error};
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use strum::VariantNames;

// ============================================================================
// Enums
// ============================================================================

/// HTTP method for the request
#[derive(Debug, Default, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "UPPERCASE")]
#[strum(serialize_all = "UPPERCASE")]
pub enum HttpMethod {
    /// GET request - retrieve data
    #[default]
    Get,
    /// POST request - create or submit data
    Post,
    /// PUT request - update or replace data
    Put,
    /// DELETE request - remove data
    Delete,
    /// PATCH request - partially update data
    Patch,
    /// HEAD request - retrieve headers only
    Head,
    /// OPTIONS request - query supported methods
    Options,
}

impl EnumVariants for HttpMethod {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

impl HttpMethod {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }
}

/// Expected format of the HTTP response body
#[derive(Debug, Default, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ResponseType {
    /// Parse response as JSON
    #[default]
    Json,
    /// Return response as plain text
    Text,
    /// Return response as raw binary data
    Binary,
}

impl EnumVariants for ResponseType {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

impl ResponseType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
            Self::Binary => "binary",
        }
    }
}

// ============================================================================
// Input/Output Types
// ============================================================================

/// Represents the body of an HTTP request
///
/// Note: This is now just a Value wrapper to handle all input types uniformly.
/// The actual conversion to JSON/text/binary happens when sending the request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct HttpBody(pub Value);

impl HttpBody {
    /// Check if body is empty/null
    pub fn is_empty(&self) -> bool {
        self.0.is_null()
    }

    /// Convert to string for sending in request
    pub fn to_string_body(&self) -> Option<String> {
        match &self.0 {
            Value::Null => None,
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }
}

/// Represents the body of an HTTP response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HttpResponseBody {
    /// Binary response body (base64 encoded)
    #[serde(with = "base64_string")]
    Binary(Vec<u8>),
    /// Text response body
    Text(String),
    /// JSON response body
    Json(Value),
}

/// Body type for HTTP requests
#[derive(Debug, Default, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum BodyType {
    /// JSON body (default)
    #[default]
    Json,
    /// Plain text body
    Text,
    /// Raw binary body (base64 encoded in input)
    Binary,
    /// Multipart form data (for file uploads)
    Multipart,
}

impl EnumVariants for BodyType {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// A part of a multipart form request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultipartPart {
    /// Field name
    pub name: String,

    /// Field value (string) or file data
    #[serde(flatten)]
    pub content: MultipartContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MultipartContent {
    /// Text field
    Text { value: String },

    /// File field (base64 encoded)
    File {
        /// Base64 encoded file content
        content: String,
        /// Filename for Content-Disposition header
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// Content-Type for this part
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "contentType")]
        content_type: Option<String>,
    },
}

/// Input structure for HTTP request operation
#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "HTTP Request Input")]
pub struct HttpRequestInput {
    /// HTTP method
    #[field(
        display_name = "Method",
        description = "HTTP verb for the request",
        example = "GET",
        default = "GET",
        enum_type = "HttpMethod"
    )]
    #[serde(default)]
    pub method: HttpMethod,

    /// Target URL
    #[field(
        display_name = "URL",
        description = "Full URL to send the request to",
        example = "https://api.example.com/v1/users"
    )]
    pub url: String,

    /// HTTP headers
    #[field(
        display_name = "Headers",
        description = "Custom HTTP headers",
        example = r#"{"Authorization": "Bearer token123"}"#,
        default = "{}"
    )]
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,

    /// Query parameters
    #[field(
        display_name = "Query Parameters",
        description = "URL query parameters",
        example = r#"{"page": "1", "limit": "100"}"#,
        default = "{}"
    )]
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub query_parameters: HashMap<String, String>,

    /// Request body
    #[field(
        display_name = "Body",
        description = "Request payload",
        example = r#"{"name": "John Doe", "email": "john@example.com"}"#,
        default = "null"
    )]
    #[serde(default, skip_serializing_if = "HttpBody::is_empty")]
    pub body: HttpBody,

    /// Body type for the request
    #[field(
        display_name = "Body Type",
        description = "How to encode the request body",
        example = "json",
        default = "json",
        enum_type = "BodyType"
    )]
    #[serde(default)]
    pub body_type: BodyType,

    /// Multipart form parts (used when body_type is "multipart")
    #[field(
        display_name = "Multipart Parts",
        description = "Form fields and files to include in multipart requests",
        default = "[]"
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub multipart: Vec<MultipartPart>,

    /// Response body type
    #[field(
        display_name = "Response Type",
        description = "Expected response format",
        example = "json",
        default = "json",
        enum_type = "ResponseType"
    )]
    #[serde(default)]
    pub response_type: ResponseType,

    /// Request timeout in milliseconds
    #[field(
        display_name = "Timeout (ms)",
        description = "Maximum time to wait for response",
        example = "5000",
        default = "30000"
    )]
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,

    /// Whether to fail the step on non-2xx responses
    #[field(
        display_name = "Fail on Error",
        description = "If true (default), non-2xx responses will fail the step. If false, non-2xx responses are returned normally.",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_fail_on_error")]
    pub fail_on_error: bool,

    /// Connection data injected by workflow runtime (internal use)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[field(skip)]
    pub _connection: Option<crate::connections::RawConnection>,
}

impl Default for HttpRequestInput {
    fn default() -> Self {
        HttpRequestInput {
            method: HttpMethod::default(),
            url: String::new(),
            headers: HashMap::new(),
            query_parameters: HashMap::new(),
            body: HttpBody(Value::Null),
            response_type: ResponseType::default(),
            timeout_ms: default_timeout(),
            body_type: BodyType::default(),
            multipart: Vec::new(),
            fail_on_error: default_fail_on_error(),
            _connection: None,
        }
    }
}

fn default_timeout() -> u64 {
    30000
}

fn default_fail_on_error() -> bool {
    true
}

/// HTTP response metadata (without body)
#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct HttpResponseMetadata {
    /// HTTP status code (e.g., 200, 404, 500)
    pub status_code: u16,

    /// Response headers
    pub headers: HashMap<String, String>,

    /// Length of the response body in bytes
    pub body_length: usize,

    /// Response type: "json", "text", or "binary"
    pub response_type: String,

    /// Whether the request was successful (2xx status code)
    pub success: bool,
}

/// HTTP response structure
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "HTTP Response",
    description = "Response from an HTTP request"
)]
pub struct HttpResponse {
    #[field(
        display_name = "Status Code",
        description = "HTTP status code (e.g., 200, 404, 500)",
        example = "200"
    )]
    pub status_code: u16,

    #[field(
        display_name = "Headers",
        description = "Response headers as key-value pairs"
    )]
    pub headers: HashMap<String, String>,

    #[field(
        display_name = "Body",
        description = "Response body (JSON object, text string, or base64-encoded binary depending on response_type)"
    )]
    pub body: HttpResponseBody,

    #[field(
        display_name = "Success",
        description = "True if the status code is in the 2xx range",
        example = "true"
    )]
    pub success: bool,
}

// Re-export HttpConnectionConfig from extractors for convenience
pub use crate::extractors::HttpConnectionConfig;

/// Extract HTTP connection config from a raw connection using registered extractors
pub fn extract_connection_config(
    raw: &crate::connections::RawConnection,
) -> Result<HttpConnectionConfig, String> {
    crate::extractors::extract_http_config(
        &raw.integration_id,
        &raw.parameters,
        raw.rate_limit_config.clone(),
    )
}

// ============================================================================
// Operations
// ============================================================================

/// Execute an HTTP request using async reqwest
#[capability(
    module = "http",
    display_name = "HTTP Request",
    description = "Execute an HTTP request with the specified method, URL, headers, and body",
    side_effects = true
)]
pub async fn http_request(input: HttpRequestInput) -> Result<HttpResponse, String> {
    // Start with input values
    let mut headers = input.headers.clone();
    let mut query_parameters = input.query_parameters.clone();
    let mut url = input.url.clone();

    // If connection data is provided, extract config and merge
    if let Some(ref raw) = input._connection {
        let config = extract_connection_config(raw)?;

        // Prepend url_prefix if URL is relative (doesn't start with http)
        if !url.starts_with("http://") && !url.starts_with("https://") {
            url = format!("{}{}", config.url_prefix, url);
        }

        // Merge headers (input headers override connection headers)
        for (k, v) in config.headers {
            headers.entry(k).or_insert(v);
        }

        // Merge query parameters (input params override connection params)
        for (k, v) in config.query_parameters {
            query_parameters.entry(k).or_insert(v);
        }

        // TODO: Apply rate limiting using config.rate_limit_config
    }

    // Build URL with query parameters
    if !query_parameters.is_empty() {
        let query_string: String = query_parameters
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        if url.contains('?') {
            url = format!("{}&{}", url, query_string);
        } else {
            url = format!("{}?{}", url, query_string);
        }
    }

    // Create reqwest client with timeout
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(input.timeout_ms))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // Build request based on method
    let method = match input.method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Delete => reqwest::Method::DELETE,
        HttpMethod::Patch => reqwest::Method::PATCH,
        HttpMethod::Head => reqwest::Method::HEAD,
        HttpMethod::Options => reqwest::Method::OPTIONS,
    };

    let mut request = client.request(method, &url);

    // Add headers
    for (key, value) in &headers {
        request = request.header(key, value);
    }

    // Add body if applicable
    request = match input.method {
        HttpMethod::Get | HttpMethod::Head | HttpMethod::Options | HttpMethod::Delete => request,
        HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch => {
            if let Some(body_str) = input.body.to_string_body() {
                // Set content-type if not already set
                if !headers.contains_key("Content-Type") && !headers.contains_key("content-type") {
                    request = request.header("Content-Type", "application/json");
                }
                request.body(body_str)
            } else {
                request
            }
        }
    };

    // Execute request
    let response = request.send().await.map_err(|e| {
        let err = network_error(format!("HTTP request to {} failed: {}", input.url, e))
            .with_attr("url", &input.url);
        // Serialize as JSON to preserve structured error info
        serde_json::to_string(&err).unwrap_or_else(|_| err.to_string())
    })?;

    let status_code = response.status().as_u16();
    let success = response.status().is_success();

    // Extract headers
    let mut response_headers = HashMap::new();
    for (name, value) in response.headers() {
        if let Ok(v) = value.to_str() {
            response_headers.insert(name.to_string(), v.to_string());
        }
    }

    // Check for error status before consuming body
    if !success && input.fail_on_error {
        let body_text = response.text().await.unwrap_or_else(|_| String::new());
        let err = http_error(status_code, &body_text).with_attr("url", &input.url);
        // Serialize as JSON to preserve structured error info
        return Err(serde_json::to_string(&err).unwrap_or_else(|_| err.to_string()));
    }

    // Read body based on response type
    let body = match input.response_type {
        ResponseType::Json => {
            let text = response
                .text()
                .await
                .map_err(|e| format!("Failed to read response body: {}", e))?;
            match serde_json::from_str(&text) {
                Ok(json_value) => HttpResponseBody::Json(json_value),
                Err(_) => HttpResponseBody::Text(text),
            }
        }
        ResponseType::Text => {
            let text = response
                .text()
                .await
                .map_err(|e| format!("Failed to read response body: {}", e))?;
            HttpResponseBody::Text(text)
        }
        ResponseType::Binary => {
            let bytes = response
                .bytes()
                .await
                .map_err(|e| format!("Failed to read response body: {}", e))?;
            HttpResponseBody::Binary(bytes.to_vec())
        }
    };

    Ok(HttpResponse {
        status_code,
        headers: response_headers,
        body,
        success,
    })
}

/// URL encoding helper module
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::new();
        for c in s.chars() {
            match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
                _ => {
                    for byte in c.to_string().as_bytes() {
                        result.push_str(&format!("%{:02X}", byte));
                    }
                }
            }
        }
        result
    }
}

mod base64_string {
    use base64::{Engine as _, engine::general_purpose};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let encoded = general_purpose::STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_get_request_json_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/users"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": 1, "name": "John"})),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/users", mock_server.uri()),
            response_type: ResponseType::Json,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.status_code, 200);
        assert!(response.success);
        assert!(matches!(response.body, HttpResponseBody::Json(_)));

        if let HttpResponseBody::Json(json) = response.body {
            assert_eq!(json["id"], 1);
            assert_eq!(json["name"], "John");
        }
    }

    #[tokio::test]
    async fn test_post_request_with_json_body() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/users"))
            .and(header("Content-Type", "application/json"))
            .and(body_string(r#"{"name":"Jane"}"#))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"id": 2, "name": "Jane"})),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Post,
            url: format!("{}/users", mock_server.uri()),
            body: HttpBody(serde_json::json!({"name": "Jane"})),
            response_type: ResponseType::Json,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.status_code, 201);
        assert!(response.success);
    }

    #[tokio::test]
    async fn test_get_request_with_query_parameters() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "rust"))
            .and(query_param("page", "1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"results": []})),
            )
            .mount(&mock_server)
            .await;

        let mut query_params = HashMap::new();
        query_params.insert("q".to_string(), "rust".to_string());
        query_params.insert("page".to_string(), "1".to_string());

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/search", mock_server.uri()),
            query_parameters: query_params,
            response_type: ResponseType::Json,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn test_get_request_with_custom_headers() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("Authorization", "Bearer token123"))
            .and(header("X-Custom-Header", "custom-value"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());
        headers.insert("X-Custom-Header".to_string(), "custom-value".to_string());

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/protected", mock_server.uri()),
            headers,
            response_type: ResponseType::Json,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn test_text_response_type() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/text"))
            .respond_with(ResponseTemplate::new(200).set_body_string("Hello, World!"))
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/text", mock_server.uri()),
            response_type: ResponseType::Text,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(matches!(response.body, HttpResponseBody::Text(_)));

        if let HttpResponseBody::Text(text) = response.body {
            assert_eq!(text, "Hello, World!");
        }
    }

    #[tokio::test]
    async fn test_binary_response_type() {
        let mock_server = MockServer::start().await;

        let binary_data = vec![0x89, 0x50, 0x4E, 0x47]; // PNG header bytes

        Mock::given(method("GET"))
            .and(path("/image"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(binary_data.clone()))
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/image", mock_server.uri()),
            response_type: ResponseType::Binary,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert!(matches!(response.body, HttpResponseBody::Binary(_)));

        if let HttpResponseBody::Binary(bytes) = response.body {
            assert_eq!(bytes, binary_data);
        }
    }

    #[tokio::test]
    async fn test_put_request() {
        let mock_server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/users/1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"updated": true})),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Put,
            url: format!("{}/users/1", mock_server.uri()),
            body: HttpBody(serde_json::json!({"name": "Updated"})),
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn test_delete_request() {
        let mock_server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/users/1"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Delete,
            url: format!("{}/users/1", mock_server.uri()),
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 204);
    }

    #[tokio::test]
    async fn test_patch_request() {
        let mock_server = MockServer::start().await;

        Mock::given(method("PATCH"))
            .and(path("/users/1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"patched": true})),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Patch,
            url: format!("{}/users/1", mock_server.uri()),
            body: HttpBody(serde_json::json!({"status": "active"})),
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn test_error_response_with_fail_on_error_true() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/not-found"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(serde_json::json!({"error": "Not found"})),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/not-found", mock_server.uri()),
            fail_on_error: true,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("404"));
    }

    #[tokio::test]
    async fn test_error_response_with_fail_on_error_false() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/not-found"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(serde_json::json!({"error": "Not found"})),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/not-found", mock_server.uri()),
            fail_on_error: false,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.status_code, 404);
        assert!(!response.success);
    }

    #[tokio::test]
    async fn test_server_error_with_fail_on_error_false() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/error"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_json(serde_json::json!({"error": "Internal server error"})),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/error", mock_server.uri()),
            fail_on_error: false,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.status_code, 500);
        assert!(!response.success);
    }

    #[tokio::test]
    async fn test_response_headers_captured() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/headers"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("X-Custom-Response", "custom-value")
                    .insert_header("X-Request-Id", "12345")
                    .set_body_json(serde_json::json!({})),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/headers", mock_server.uri()),
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(
            response.headers.get("x-custom-response"),
            Some(&"custom-value".to_string())
        );
        assert_eq!(
            response.headers.get("x-request-id"),
            Some(&"12345".to_string())
        );
    }

    #[tokio::test]
    async fn test_head_request() {
        let mock_server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .and(path("/resource"))
            .respond_with(ResponseTemplate::new(200).insert_header("Content-Length", "1024"))
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Head,
            url: format!("{}/resource", mock_server.uri()),
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.status_code, 200);
    }

    #[tokio::test]
    async fn test_options_request() {
        let mock_server = MockServer::start().await;

        Mock::given(method("OPTIONS"))
            .and(path("/api"))
            .respond_with(
                ResponseTemplate::new(200).insert_header("Allow", "GET, POST, PUT, DELETE"),
            )
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Options,
            url: format!("{}/api", mock_server.uri()),
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        assert_eq!(response.status_code, 200);
        assert!(response.headers.get("allow").is_some());
    }

    #[tokio::test]
    async fn test_json_response_fallback_to_text() {
        let mock_server = MockServer::start().await;

        // Return invalid JSON when JSON response is expected
        Mock::given(method("GET"))
            .and(path("/invalid-json"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not valid json"))
            .mount(&mock_server)
            .await;

        let input = HttpRequestInput {
            method: HttpMethod::Get,
            url: format!("{}/invalid-json", mock_server.uri()),
            response_type: ResponseType::Json,
            ..Default::default()
        };

        let result = http_request(input).await;
        assert!(result.is_ok());

        let response = result.unwrap();
        // Should fall back to text when JSON parsing fails
        assert!(matches!(response.body, HttpResponseBody::Text(_)));

        if let HttpResponseBody::Text(text) = response.body {
            assert_eq!(text, "not valid json");
        }
    }
}
