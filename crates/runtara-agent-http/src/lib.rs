//! HTTP request agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/http.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope, injecting `X-Runtara-Connection-Id` so the proxy can attach
//! credentials. The component never sees secrets.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::collections::HashMap;
use std::time::Duration;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// -----------------------------------------------------------------------------
// Schema (mirrors crates/runtara-agents/src/agents/http.rs)
// -----------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    fn as_str(&self) -> &'static str {
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ResponseType {
    #[default]
    Json,
    Text,
    Binary,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BodyType {
    #[default]
    Json,
    Text,
    Binary,
    Multipart,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
struct HttpBody(Value);

impl HttpBody {
    fn to_string_body(&self) -> Option<String> {
        match &self.0 {
            Value::Null => None,
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct HttpRequestInput {
    #[serde(default)]
    method: HttpMethod,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    query_parameters: HashMap<String, String>,
    #[serde(default)]
    body: HttpBody,
    #[serde(default)]
    #[allow(dead_code)]
    body_type: BodyType,
    #[serde(default)]
    response_type: ResponseType,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    #[serde(default = "default_true")]
    fail_on_error: bool,
}

fn default_timeout() -> u64 {
    30_000
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct HttpResponse {
    status_code: u16,
    headers: HashMap<String, String>,
    body: HttpResponseBody,
    success: bool,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum HttpResponseBody {
    Json(Value),
    Text(String),
    Binary {
        #[serde(rename = "base64")]
        base64: String,
    },
}

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "http".into(),
            display_name: "HTTP".into(),
            description: "Make HTTP requests via the runtara proxy.".into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["http_bearer".into(), "http_api_key".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![CapabilityInfo {
            id: "http-request".into(),
            function_name: "http_request".into(),
            display_name: Some("HTTP Request".into()),
            description: Some(
                "Execute an HTTP request with the specified method, URL, headers, and body. \
                 Credentials are injected server-side by the runtara HTTP proxy."
                    .into(),
            ),
            has_side_effects: true,
            is_idempotent: false,
            rate_limited: true,
            tags: vec!["http".into(), "io".into()],
            input_schema: HTTP_REQUEST_INPUT_SCHEMA.into(),
            output_schema: HTTP_REQUEST_OUTPUT_SCHEMA.into(),
            known_errors: vec![],
            compensation_hint: None,
        }]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "http-request" => http_request(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("http agent has no capability `{other}`"),
            )),
        }
    }
}

fn http_request(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: HttpRequestInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let mut headers = input.headers.clone();
    let mut url = input.url.clone();
    let query_parameters = input.query_parameters.clone();

    // Forward the connection id so the proxy can attach credentials.
    if let Some(conn) = connection
        && !conn.connection_id.is_empty()
    {
        headers
            .entry("X-Runtara-Connection-Id".to_string())
            .or_insert_with(|| conn.connection_id.clone());
    }

    // Append query parameters.
    if !query_parameters.is_empty() {
        let query_string: String = query_parameters
            .iter()
            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        if url.contains('?') {
            url = format!("{url}&{query_string}");
        } else {
            url = format!("{url}?{query_string}");
        }
    }

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(input.timeout_ms));
    let method_str = input.method.as_str();
    let mut request = client.request(method_str, &url);

    for (key, value) in &headers {
        request = request.header(key, value);
    }

    request = match input.method {
        HttpMethod::Get | HttpMethod::Head | HttpMethod::Options | HttpMethod::Delete => request,
        HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch => {
            if let Some(body_str) = input.body.to_string_body() {
                let has_content_type = headers
                    .keys()
                    .any(|k| k.eq_ignore_ascii_case("content-type"));
                if !has_content_type {
                    request = request.header("Content-Type", "application/json");
                }
                request.body_bytes(body_str.as_bytes())
            } else {
                request
            }
        }
    };

    let response = match request.call_agent() {
        Ok(r) => r,
        Err(e) => {
            return Err(transient_err(
                "NETWORK_ERROR",
                format!("request to {} failed: {e}", input.url),
            ));
        }
    };

    let status_code = response.status;
    let success = (200..300).contains(&status_code);
    let response_headers: HashMap<String, String> = response.headers.clone().into_iter().collect();

    if !success && input.fail_on_error {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let category = if status_code == 429 || (500..600).contains(&status_code) {
            "transient"
        } else {
            "permanent"
        };
        let code = if status_code == 429 {
            "HTTP_429"
        } else if (500..600).contains(&status_code) {
            "HTTP_5XX"
        } else {
            "HTTP_4XX"
        };
        return Err(ErrorInfo {
            code: code.into(),
            message: format!("HTTP {status_code}: {}", truncate(&body_text, 512)),
            category: category.into(),
            severity: "error".into(),
            retryable: category == "transient",
            retry_after_ms: response_headers
                .get("retry-after-ms")
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    response_headers
                        .get("retry-after")
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|s| s * 1000)
                }),
            attributes: serde_json::to_string(&serde_json::json!({
                "url": input.url,
                "status_code": status_code,
            }))
            .ok(),
        });
    }

    let body = match input.response_type {
        ResponseType::Json => {
            let text = String::from_utf8_lossy(&response.body).to_string();
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => HttpResponseBody::Json(v),
                Err(_) => HttpResponseBody::Text(text),
            }
        }
        ResponseType::Text => {
            HttpResponseBody::Text(String::from_utf8_lossy(&response.body).into_owned())
        }
        ResponseType::Binary => {
            use base64::Engine as _;
            HttpResponseBody::Binary {
                base64: base64::engine::general_purpose::STANDARD.encode(&response.body),
            }
        }
    };

    serde_json::to_string(&HttpResponse {
        status_code,
        headers: response_headers,
        body,
        success,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            _ => {
                for byte in c.to_string().as_bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push_str("…");
        t
    }
}

fn permanent_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn transient_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "transient".into(),
        severity: "warning".into(),
        retryable: true,
        retry_after_ms: None,
        attributes: None,
    }
}

const HTTP_REQUEST_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["url"],
    "properties": {
        "method":           { "type": "string", "enum": ["GET","POST","PUT","DELETE","PATCH","HEAD","OPTIONS"], "default": "GET" },
        "url":              { "type": "string", "description": "Full URL" },
        "headers":          { "type": "object", "additionalProperties": { "type": "string" } },
        "query_parameters": { "type": "object", "additionalProperties": { "type": "string" } },
        "body":             { "description": "Request body (any JSON value)" },
        "body_type":        { "type": "string", "enum": ["json","text","binary","multipart"], "default": "json" },
        "response_type":    { "type": "string", "enum": ["json","text","binary"], "default": "json" },
        "timeout_ms":       { "type": "integer", "default": 30000 },
        "fail_on_error":    { "type": "boolean", "default": true }
    }
}"#;

const HTTP_REQUEST_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "status_code": { "type": "integer" },
        "headers":     { "type": "object", "additionalProperties": { "type": "string" } },
        "body":        { "description": "JSON value, text string, or {\"base64\": \"...\"} for binary" },
        "success":     { "type": "boolean" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
