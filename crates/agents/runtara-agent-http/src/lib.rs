//! Generic HTTP request agent — WebAssembly Component.
//!
//! This is the workspace-wide generic HTTP client. Unlike a single-service
//! integration agent (mailgun, hubspot, slack, …), the http agent serves a
//! *family* of connection types: any registered `HttpConnectionExtractor` on
//! the server side counts as a valid `integration_id` for this agent. That
//! means the `integration_ids` list is dynamic and lives server-side
//! (`runtara_agents::extractors::get_http_extractor_ids()` — see
//! `crates/runtara-server/src/api/services/operators.rs::http_integration_ids`).
//! The wasm component therefore ships an empty `integration_ids: vec![]`;
//! the server augments it at request time.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions
//! that the wasm cdylib's `invoke` dispatcher calls into. The workspace
//! binary `runtara-agent-bundle-emit` reads these macro-emitted `&'static`
//! statics on the host architecture and writes
//! `runtara_agent_http.meta.json` next to the `.wasm` — the JSON is a build
//! artifact, never hand-edited.
//!
//! Routing model: all WASM requests use the typed outbound host service.
//! Connection destinations select host-side credentials and base URL pinning;
//! public destinations apply the same egress policy without stored credentials.
//! Tenant identity comes from host execution context. Native connection-aware
//! requests require a host service and fail explicitly in the ordinary SDK.

#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use strum::VariantNames;

// ============================================================================
// Local AgentError shim
// ============================================================================
//
// The host crate's `runtara_agents::types::AgentError` pulls in `tracing` and
// other host-only baggage. We only need the on-the-wire JSON shape that the
// `#[capability]` macro expects (`Into<String>` returning
// `{"code","message","category","severity",...}`), so we inline a minimal
// version here. Mirrors the shim in `runtara-agent-mailgun`.

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, Value>,
}

impl AgentError {
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "permanent",
            severity: "error",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "transient",
            severity: "warning",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }

    pub fn with_retry_after_ms(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }
}

/// Serialize into the canonical JSON envelope so the `#[capability]` macro
/// executor passes us straight through to `error_string_to_error_info` on the
/// wasm side (which parses the JSON back into a typed `ErrorInfo`).
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================
//
// The host crate's `RawConnection` lives in `runtara-agents` and isn't a
// wasm-compatible dependency. We mirror just the struct so the macro-derived
// executor can deserialize what the wasm Guest::invoke wrapper injects into
// the input JSON under the `_connection` key.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConnection {
    #[serde(default)]
    pub connection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    pub integration_id: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_config: Option<Value>,
}

// ============================================================================
// Enums (with VariantNames + EnumVariants so the macro can record allowed values)
// ============================================================================

/// HTTP method for the request.
#[derive(Debug, Default, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "UPPERCASE")]
#[strum(serialize_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl EnumVariants for HttpMethod {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
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

/// Expected format of the HTTP response body.
#[derive(Debug, Default, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ResponseType {
    #[default]
    Json,
    Text,
    Binary,
}

impl EnumVariants for ResponseType {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// Body type for HTTP requests.
#[derive(Debug, Default, Clone, Serialize, Deserialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum BodyType {
    #[default]
    Json,
    Text,
    Binary,
    Multipart,
}

impl EnumVariants for BodyType {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// Represents the body of an HTTP request — opaque Value passthrough.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct HttpBody(pub Value);

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

// ============================================================================
// HTTP Request capability
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "HTTP Request Input")]
pub struct HttpRequestInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (the UI/runtime fills it from the
    /// configured connection, not from user input). Optional — the http
    /// agent supports plain unauthenticated requests as well as
    /// proxy-routed connection-bound requests.
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Method",
        description = "HTTP verb for the request",
        example = "GET",
        default = "GET",
        enum_type = "HttpMethod"
    )]
    #[serde(default)]
    pub method: HttpMethod,

    #[field(
        display_name = "URL",
        description = "Full URL to send the request to",
        example = "https://api.example.com/v1/users"
    )]
    pub url: String,

    #[field(
        display_name = "Connection Endpoint",
        description = "Name of an alternate endpoint declared by the connection type, for \
                       providers that split one credential across several API hosts \
                       (e.g. \"graphql\" on a QuickBooks Online connection). Leave empty to \
                       use the connection's default base URL. Ignored without a connection.",
        example = "graphql"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_endpoint: Option<String>,

    #[field(
        display_name = "Headers",
        description = "Custom HTTP headers",
        example = r#"{"Authorization": "Bearer token123"}"#,
        default = "{}"
    )]
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,

    #[field(
        display_name = "Query Parameters",
        description = "URL query parameters",
        example = r#"{"page": "1", "limit": "100"}"#,
        default = "{}"
    )]
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub query_parameters: HashMap<String, String>,

    #[field(
        display_name = "Body",
        description = "Request payload (any JSON value, or string for non-JSON bodies)",
        example = r#"{"name": "John Doe", "email": "john@example.com"}"#,
        default = "null"
    )]
    #[serde(default)]
    pub body: HttpBody,

    #[field(
        display_name = "Body Type",
        description = "How to encode the request body",
        example = "json",
        default = "json",
        enum_type = "BodyType"
    )]
    #[serde(default)]
    #[allow(dead_code)]
    pub body_type: BodyType,

    #[field(
        display_name = "Response Type",
        description = "Expected response format",
        example = "json",
        default = "json",
        enum_type = "ResponseType"
    )]
    #[serde(default)]
    pub response_type: ResponseType,

    #[field(
        display_name = "Timeout (ms)",
        description = "Maximum time to wait for response",
        example = "5000",
        default = "30000"
    )]
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,

    #[field(
        display_name = "Fail on Error",
        description = "If true (default), non-2xx responses will fail the step. If false, non-2xx responses are returned normally.",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_fail_on_error")]
    pub fail_on_error: bool,

    #[field(
        display_name = "Follow Redirects",
        description = "If true, a 3xx answer with a Location header is followed with a new request, each one \
                       checked like the first, up to Max Redirects. Off by default: the 3xx is returned (or \
                       fails the step, with Fail on Error). Not with a connection: its answers are returned \
                       as they are, so its credentials go nowhere but its own host.",
        example = "true",
        default = "false"
    )]
    #[serde(default)]
    pub follow_redirects: bool,

    #[field(
        display_name = "Max Redirects",
        description = "How many redirects to follow at most, with Follow Redirects. One more fails the step \
                       (TOO_MANY_REDIRECTS).",
        example = "5",
        default = "5"
    )]
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u32,
}

fn default_timeout() -> u64 {
    30_000
}

fn default_max_redirects() -> u32 {
    5
}

fn default_fail_on_error() -> bool {
    true
}

impl Default for HttpRequestInput {
    fn default() -> Self {
        HttpRequestInput {
            _connection: None,
            method: HttpMethod::default(),
            url: String::new(),
            connection_endpoint: None,
            headers: HashMap::new(),
            query_parameters: HashMap::new(),
            body: HttpBody(Value::Null),
            body_type: BodyType::default(),
            response_type: ResponseType::default(),
            timeout_ms: default_timeout(),
            fail_on_error: default_fail_on_error(),
            follow_redirects: false,
            max_redirects: default_max_redirects(),
        }
    }
}

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
        description = "Response body (JSON object, text string, or {\"base64\": \"...\"} for binary depending on response_type)"
    )]
    pub body: HttpResponseBody,

    #[field(
        display_name = "Success",
        description = "True if the status code is in the 2xx range",
        example = "true"
    )]
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HttpResponseBody {
    Json(Value),
    Text(String),
    Binary {
        #[serde(rename = "base64")]
        base64: String,
    },
}

#[capability(
    module = "http",
    display_name = "HTTP Request",
    description = "Execute an HTTP request with the specified method, URL, headers, and body. \
                   When a connection is configured, credentials are injected server-side by the \
                   runtara HTTP proxy; otherwise the request is sent directly to the URL.",
    module_display_name = "HTTP",
    module_description = "Generic HTTP client.",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_secure = true
)]
pub async fn http_request(input: HttpRequestInput) -> Result<HttpResponse, AgentError> {
    let mut hop = Hop::first(&input);
    let mut redirects = 0u32;
    loop {
        let response = prepare_http_request(&input, &hop).call_agent_async().await;
        let Some(next) = response
            .as_ref()
            .ok()
            .and_then(|r| redirect_to(&input, &hop, r))
        else {
            return finish_http_request(&input, response);
        };
        if redirects >= input.max_redirects {
            return Err(AgentError::permanent(
                "TOO_MANY_REDIRECTS",
                format!(
                    "{} redirected more than {} times (max_redirects)",
                    input.url, input.max_redirects
                ),
            )
            .with_attr("url", input.url.clone())
            .with_attr("last_url", hop.url.clone()));
        }
        redirects += 1;
        hop = next;
    }
}

/// One request of a call: the first, or one a redirect asked for.
struct Hop {
    url: String,
    method: HttpMethod,
    /// Whether the request body and its content headers are still sent
    /// (dropped once a redirect turns the request into a GET).
    with_body: bool,
    /// Whether the caller's credential headers are still sent (dropped once
    /// a redirect leaves the first URL's origin).
    with_credentials: bool,
}

impl Hop {
    fn first(input: &HttpRequestInput) -> Self {
        Hop {
            url: with_query(&input.url, &input.query_parameters),
            method: input.method.clone(),
            with_body: true,
            with_credentials: true,
        }
    }
}

/// The request a redirect asks for, when the step follows redirects and
/// `response` is one.
fn redirect_to(
    input: &HttpRequestInput,
    hop: &Hop,
    response: &runtara_http::HttpResponse,
) -> Option<Hop> {
    if !input.follow_redirects || has_connection(input) {
        return None;
    }
    let status = response.status;
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = response.header("location")?;
    let current = url::Url::parse(&hop.url).ok()?;
    let next = current.join(location).ok()?;
    if !matches!(next.scheme(), "http" | "https") {
        return None;
    }
    // As browsers do: 303 always, and 301/302 after a POST, become a GET
    // without the body; 307 and 308 repeat the request as it was.
    let becomes_get = status == 303 && !matches!(hop.method, HttpMethod::Head)
        || matches!(status, 301 | 302) && matches!(hop.method, HttpMethod::Post);
    let first = url::Url::parse(&with_query(&input.url, &input.query_parameters)).ok();
    Some(Hop {
        method: if becomes_get {
            HttpMethod::Get
        } else {
            hop.method.clone()
        },
        with_body: hop.with_body && !becomes_get,
        with_credentials: hop.with_credentials
            && first.is_some_and(|f| f.origin() == next.origin()),
        url: next.into(),
    })
}

fn has_connection(input: &HttpRequestInput) -> bool {
    input
        ._connection
        .as_ref()
        .is_some_and(|c| !c.connection_id.is_empty())
}

/// Headers that carry the caller's credentials: not sent to another origin.
fn is_credential_header(name: &str) -> bool {
    ["authorization", "proxy-authorization", "cookie"]
        .iter()
        .any(|h| name.eq_ignore_ascii_case(h))
}

fn with_query(url: &str, query_parameters: &HashMap<String, String>) -> String {
    if query_parameters.is_empty() {
        return url.to_string();
    }
    let query_string: String = query_parameters
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    if url.contains('?') {
        format!("{url}&{query_string}")
    } else {
        format!("{url}?{query_string}")
    }
}

fn prepare_http_request(input: &HttpRequestInput, hop: &Hop) -> runtara_http::RequestBuilder {
    let headers: HashMap<String, String> = input
        .headers
        .iter()
        .filter(|(k, _)| hop.with_credentials || !is_credential_header(k))
        .filter(|(k, _)| {
            hop.with_body
                || !(k.eq_ignore_ascii_case("content-type")
                    || k.eq_ignore_ascii_case("content-length"))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(input.timeout_ms));
    let method_str = hop.method.as_str();
    let mut request = client.request(method_str, &hop.url);

    if let Some(connection) = input
        ._connection
        .as_ref()
        .filter(|c| !c.connection_id.is_empty())
    {
        request = request.connection_id(&connection.connection_id);
        if let Some(endpoint) = input
            .connection_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            request = request.endpoint(endpoint);
        }
    }

    for (key, value) in &headers {
        request = request.header(key, value);
    }

    request = match hop.method {
        HttpMethod::Get | HttpMethod::Head | HttpMethod::Options | HttpMethod::Delete => request,
        HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch if !hop.with_body => request,
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

    request
}

fn finish_http_request(
    input: &HttpRequestInput,
    response_result: Result<runtara_http::HttpResponse, runtara_http::HttpError>,
) -> Result<HttpResponse, AgentError> {
    let response = match response_result {
        Ok(r) => r,
        Err(e) => {
            return Err(AgentError::transient(
                "NETWORK_ERROR",
                format!("request to {} failed: {e}", input.url),
            )
            .with_attr("url", input.url.clone()));
        }
    };

    let status_code = response.status;
    let success = (200..300).contains(&status_code);
    let response_headers: HashMap<String, String> = response.headers.clone().into_iter().collect();

    if !success && input.fail_on_error {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (code, category, severity) = if status_code == 429 {
            ("HTTP_429", "transient", "warning")
        } else if (500..600).contains(&status_code) {
            ("HTTP_5XX", "transient", "warning")
        } else {
            ("HTTP_4XX", "permanent", "error")
        };
        let mut err = AgentError {
            code: code.into(),
            message: format!("HTTP {status_code}: {}", truncate(&body_text, 512)),
            category,
            severity,
            retry_after_ms: None,
            attributes: HashMap::new(),
        };
        err = err
            .with_attr("url", input.url.clone())
            .with_attr("status_code", status_code.to_string())
            .with_attr("body", truncate(&body_text, 512));
        if status_code == 429 {
            let retry_after_ms = response_headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("retry-after-ms"))
                .and_then(|(_, v)| v.parse::<u64>().ok())
                .or_else(|| {
                    response_headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                        .and_then(|(_, v)| v.parse::<u64>().ok())
                        .map(|s| s * 1000)
                });
            if let Some(ms) = retry_after_ms {
                err = err.with_retry_after_ms(ms);
            }
        }
        return Err(err);
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

    Ok(HttpResponse {
        status_code,
        headers: response_headers,
        body,
        success,
    })
}

// ============================================================================
// Helpers
// ============================================================================

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
        t.push('…');
        t
    }
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================
//
// `integration_ids` is intentionally empty here. The http agent is the
// generic HTTP client: any registered `HttpConnectionExtractor` on the
// server-side is a valid integration for it. The server augments this list
// at runtime via `runtara_agents::extractors::get_http_extractor_ids()` —
// see `crates/runtara-server/src/api/services/operators.rs`.

#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[&__CAPABILITY_META_HTTP_REQUEST];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [(
        "HttpRequestInput",
        &__INPUT_META_HttpRequestInput as &InputTypeMeta,
    )]
    .into_iter()
    .collect();
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [(
        "HttpResponse",
        &__OUTPUT_META_HttpResponse as &OutputTypeMeta,
    )]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api_with_types(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
                &output_types,
            )
        })
        .collect();

    AgentInfo {
        id: "http".into(),
        name: "HTTP".into(),
        description: "Generic HTTP client.".into(),
        has_side_effects: true,
        supports_connections: true,
        // Dynamic on the server — see module-level docs and operators.rs.
        integration_ids: vec![],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

runtara_agent_macro::agent_component!(agent = "http", capabilities = [http_request,],);

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
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
        assert!(matches!(response.body, HttpResponseBody::Binary { .. }));

        if let HttpResponseBody::Binary { base64 } = response.body {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&base64)
                .unwrap();
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
        assert!(result.unwrap_err().message.contains("404"));
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
        assert!(response.headers.contains_key("allow"));
    }

    async fn redirecting(status: u16, from: &str, to: &str, server: &MockServer) {
        Mock::given(path(from.to_string()))
            .respond_with(ResponseTemplate::new(status).insert_header("location", to))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed_unless_asked() {
        let server = MockServer::start().await;
        redirecting(301, "/old", "/new", &server).await;
        Mock::given(path("/new"))
            .respond_with(ResponseTemplate::new(200).set_body_string("moved here"))
            .expect(0)
            .mount(&server)
            .await;

        let response = http_request(HttpRequestInput {
            url: format!("{}/old", server.uri()),
            fail_on_error: false,
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(response.status_code, 301);
        assert_eq!(response.headers.get("location").unwrap(), "/new");

        let refused = http_request(HttpRequestInput {
            url: format!("{}/old", server.uri()),
            ..Default::default()
        })
        .await;
        assert!(refused.unwrap_err().message.contains("301"));
    }

    #[tokio::test]
    async fn redirects_are_followed_when_asked() {
        let server = MockServer::start().await;
        redirecting(301, "/a", "/b?x=1", &server).await;
        redirecting(307, "/b", "/c", &server).await;
        Mock::given(method("GET"))
            .and(path("/c"))
            .respond_with(ResponseTemplate::new(200).set_body_string("the page"))
            .expect(1)
            .mount(&server)
            .await;

        let response = http_request(HttpRequestInput {
            url: format!("{}/a", server.uri()),
            response_type: ResponseType::Text,
            follow_redirects: true,
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(response.status_code, 200);
        assert!(matches!(response.body, HttpResponseBody::Text(ref t) if t == "the page"));
    }

    #[tokio::test]
    async fn one_redirect_more_than_max_redirects_fails() {
        let server = MockServer::start().await;
        redirecting(302, "/1", "/2", &server).await;
        redirecting(302, "/2", "/3", &server).await;
        redirecting(302, "/3", "/4", &server).await;
        Mock::given(path("/4"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let input = |max_redirects| HttpRequestInput {
            url: format!("{}/1", server.uri()),
            follow_redirects: true,
            max_redirects,
            ..Default::default()
        };

        let error = http_request(input(2)).await.unwrap_err();
        assert_eq!(error.code, "TOO_MANY_REDIRECTS");
        assert_eq!(http_request(input(3)).await.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn credentials_are_not_sent_to_another_origin() {
        let first = MockServer::start().await;
        let other = MockServer::start().await;
        redirecting(302, "/start", &format!("{}/landing", other.uri()), &first).await;
        Mock::given(path("/landing"))
            .and(header("x-kept", "yes"))
            .respond_with(|request: &wiremock::Request| {
                let leaked = ["authorization", "cookie"]
                    .iter()
                    .any(|h| request.headers.contains_key(*h));
                ResponseTemplate::new(if leaked { 400 } else { 200 })
            })
            .expect(1)
            .mount(&other)
            .await;

        let headers = HashMap::from([
            ("Authorization".to_string(), "Bearer secret".to_string()),
            ("Cookie".to_string(), "session=1".to_string()),
            ("X-Kept".to_string(), "yes".to_string()),
        ]);
        let response = http_request(HttpRequestInput {
            url: format!("{}/start", first.uri()),
            headers,
            follow_redirects: true,
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(response.status_code, 200);
    }

    #[tokio::test]
    async fn a_see_other_after_a_post_is_a_get_without_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/orders"))
            .respond_with(ResponseTemplate::new(303).insert_header("location", "/orders/7"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/orders/7"))
            .and(body_string(""))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 7})))
            .expect(1)
            .mount(&server)
            .await;

        let response = http_request(HttpRequestInput {
            method: HttpMethod::Post,
            url: format!("{}/orders", server.uri()),
            body: HttpBody(serde_json::json!({"item": "a"})),
            follow_redirects: true,
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(response.status_code, 200);
    }

    #[test]
    fn a_connections_redirect_is_not_followed() {
        let input = HttpRequestInput {
            _connection: Some(RawConnection {
                connection_id: "conn-1".into(),
                connection_subtype: None,
                integration_id: "http_bearer".into(),
                parameters: Value::Null,
                rate_limit_config: None,
            }),
            url: "https://api.example.com/v1/a".into(),
            follow_redirects: true,
            ..Default::default()
        };
        let response = runtara_http::HttpResponse {
            status: 302,
            headers: HashMap::from([("location".into(), "https://elsewhere.example/".into())]),
            body: Vec::new(),
        };
        assert!(redirect_to(&input, &Hop::first(&input), &response).is_none());
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
