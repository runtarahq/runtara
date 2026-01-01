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

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use strum::VariantNames;

// ============================================================================
// Enums
// ============================================================================

/// HTTP method for the request
#[derive(Debug, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "UPPERCASE")]
#[strum(serialize_all = "UPPERCASE")]
pub enum HttpMethod {
    /// GET request - retrieve data
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

impl Default for HttpMethod {
    fn default() -> Self {
        Self::Get
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
#[derive(Debug, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ResponseType {
    /// Parse response as JSON
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

impl Default for ResponseType {
    fn default() -> Self {
        Self::Json
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
#[derive(Debug, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum BodyType {
    /// JSON body (default)
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

impl Default for BodyType {
    fn default() -> Self {
        Self::Json
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

/// Execute an HTTP request using native ureq
#[capability(
    module = "http",
    display_name = "HTTP Request",
    description = "Execute an HTTP request with the specified method, URL, headers, and body",
    side_effects = true
)]
pub fn http_request(input: HttpRequestInput) -> Result<HttpResponse, String> {
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

    // Create ureq agent with timeout
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_millis(input.timeout_ms))
        .build();

    // Build request based on method
    let request = match input.method {
        HttpMethod::Get => agent.get(&url),
        HttpMethod::Post => agent.post(&url),
        HttpMethod::Put => agent.put(&url),
        HttpMethod::Delete => agent.delete(&url),
        HttpMethod::Patch => agent.patch(&url),
        HttpMethod::Head => agent.head(&url),
        HttpMethod::Options => agent.request("OPTIONS", &url),
    };

    // Add headers
    let mut request = request;
    for (key, value) in &headers {
        request = request.set(key, value);
    }

    // Execute request with body if applicable
    let response = match input.method {
        HttpMethod::Get | HttpMethod::Head | HttpMethod::Options | HttpMethod::Delete => {
            request.call()
        }
        HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch => {
            if let Some(body_str) = input.body.to_string_body() {
                // Set content-type if not already set
                if !headers.contains_key("Content-Type") && !headers.contains_key("content-type") {
                    request = request.set("Content-Type", "application/json");
                }
                request.send_string(&body_str)
            } else {
                request.call()
            }
        }
    };

    // Handle response
    match response {
        Ok(resp) => {
            let status_code = resp.status();

            // Extract headers
            let mut headers = HashMap::new();
            for name in resp.headers_names() {
                if let Some(value) = resp.header(&name) {
                    headers.insert(name, value.to_string());
                }
            }

            // Read body
            let body = match input.response_type {
                ResponseType::Json => match resp.into_string() {
                    Ok(text) => match serde_json::from_str(&text) {
                        Ok(json_value) => HttpResponseBody::Json(json_value),
                        Err(_) => HttpResponseBody::Text(text),
                    },
                    Err(e) => return Err(format!("Failed to read response body: {}", e)),
                },
                ResponseType::Text => match resp.into_string() {
                    Ok(text) => HttpResponseBody::Text(text),
                    Err(e) => return Err(format!("Failed to read response body: {}", e)),
                },
                ResponseType::Binary => {
                    let mut bytes = Vec::new();
                    match resp.into_reader().read_to_end(&mut bytes) {
                        Ok(_) => HttpResponseBody::Binary(bytes),
                        Err(e) => return Err(format!("Failed to read response body: {}", e)),
                    }
                }
            };

            let success = status_code >= 200 && status_code < 300;

            Ok(HttpResponse {
                status_code,
                headers,
                body,
                success,
            })
        }
        Err(ureq::Error::Status(status_code, resp)) => {
            // HTTP error response (4xx, 5xx)
            let mut headers = HashMap::new();
            for name in resp.headers_names() {
                if let Some(value) = resp.header(&name) {
                    headers.insert(name, value.to_string());
                }
            }

            let body_text = resp.into_string().unwrap_or_default();
            let body = match serde_json::from_str(&body_text) {
                Ok(json_value) => HttpResponseBody::Json(json_value),
                Err(_) => HttpResponseBody::Text(body_text.clone()),
            };

            // If fail_on_error is true, return an error instead of a response
            if input.fail_on_error {
                return Err(format!(
                    "HTTP request failed with status {}: {}",
                    status_code, body_text
                ));
            }

            Ok(HttpResponse {
                status_code,
                headers,
                body,
                success: false,
            })
        }
        Err(ureq::Error::Transport(transport)) => Err(format!(
            "HTTP request to {} failed: {}",
            input.url, transport
        )),
    }
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
