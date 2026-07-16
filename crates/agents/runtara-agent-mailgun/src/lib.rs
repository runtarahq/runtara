//! Mailgun email agent — WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_mailgun.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to attach Basic auth
//! (derived from `api_key`) and resolve the base URL (`https://api.mailgun.net`
//! or `https://api.eu.mailgun.net` depending on the `region` parameter). The
//! component never sees secrets.
//!
//! The `domain` connection parameter is a non-credential config value exposed
//! in `connection.parameters` (JSON object); the capability reads it to build
//! the request path and the default sender address.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// ============================================================================
// Local AgentError shim
// ============================================================================
//
// The host crate's `runtara_agents::types::AgentError` pulls in `tracing` and
// other host-only baggage. We only need the on-the-wire JSON shape that the
// `#[capability]` macro expects (`Into<String>` returning
// `{"code","message","category","severity",...}`), so we inline a minimal
// version here. Mirrors the shim in `runtara-agent-transform`.

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
// Send Email
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Email Input")]
pub struct SendEmailInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (the UI/runtime fills it from the
    /// configured connection, not from user input).
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "To",
        description = "Recipient email address(es), comma-separated for multiple",
        example = "user@example.com"
    )]
    pub to: String,

    #[field(
        display_name = "Subject",
        description = "Email subject line",
        example = "Order Confirmation"
    )]
    pub subject: String,

    #[field(display_name = "Text Body", description = "Plain text email body")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[field(
        display_name = "HTML Body",
        description = "HTML email body (takes precedence over text when both provided)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,

    #[field(
        display_name = "From",
        description = "Sender email address (defaults to noreply@{domain})"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    #[field(display_name = "CC", description = "CC recipients, comma-separated")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,

    #[field(display_name = "BCC", description = "BCC recipients, comma-separated")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bcc: Option<String>,

    #[field(display_name = "Reply-To", description = "Reply-To email address")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,

    #[field(
        display_name = "Tags",
        description = "Comma-separated tags for tracking"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Send Email Output")]
pub struct SendEmailOutput {
    #[field(
        display_name = "Message ID",
        description = "Mailgun message ID for tracking",
        example = "<20210101.0123456789.ABCD@mailgun.org>"
    )]
    pub id: String,

    #[field(
        display_name = "Message",
        description = "Mailgun response message",
        example = "Queued. Thank you."
    )]
    pub message: String,
}

#[capability(
    module = "mailgun",
    display_name = "Send Email (Mailgun)",
    description = "Send an email via Mailgun REST API. Credentials are injected server-side by the runtara HTTP proxy.",
    module_display_name = "Mailgun",
    module_description = "Mailgun email service for sending transactional and marketing emails.",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "mailgun",
    module_secure = true
)]
pub fn send_email(input: SendEmailInput) -> Result<SendEmailOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "MAILGUN_MISSING_CONNECTION",
            "MAILGUN capability invoked without a connection — add one in the step configuration",
        )
        .with_attr("integration", "MAILGUN")
    })?;

    // `domain` is a non-credential config param needed for path building and
    // the default sender address.
    let domain = connection.parameters["domain"].as_str().ok_or_else(|| {
        AgentError::permanent(
            "MAILGUN_MISSING_FIELD",
            "MAILGUN connection parameters missing required field: domain",
        )
        .with_attr("integration", "MAILGUN")
        .with_attr("field", "domain")
    })?;

    let from = input
        .from
        .clone()
        .unwrap_or_else(|| format!("noreply@{}", domain));

    // Build form-urlencoded body identical to the legacy implementation.
    let mut form_parts: Vec<(&str, String)> =
        vec![("from", from), ("to", input.to), ("subject", input.subject)];

    if let Some(text) = input.text {
        form_parts.push(("text", text));
    }
    if let Some(html) = input.html {
        form_parts.push(("html", html));
    }
    if let Some(cc) = input.cc {
        form_parts.push(("cc", cc));
    }
    if let Some(bcc) = input.bcc {
        form_parts.push(("bcc", bcc));
    }
    if let Some(reply_to) = input.reply_to {
        form_parts.push(("h:Reply-To", reply_to));
    }

    // Collect tag strings as owned values so their lifetimes outlive form_parts.
    let tag_strings: Vec<String> = input
        .tags
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().to_string())
        .collect();
    for tag in &tag_strings {
        form_parts.push(("o:tag", tag.clone()));
    }

    let encoded_body = url_encode_form(&form_parts);

    // Route through the proxy with the connection id so the proxy injects
    // Authorization: Basic <base64(api:<key>)> and resolves the base URL.
    let url = format!("/v3/{}/messages", domain);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let response = client
        .request("POST", &url)
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body_bytes(encoded_body.as_bytes())
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "MAILGUN_NETWORK_ERROR",
                format!("request to Mailgun failed: {e}"),
            )
            .with_attr("integration", "MAILGUN")
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let mut err = if status == 429 || (500..600).contains(&status) {
            AgentError::transient(
                "MAILGUN_UPSTREAM_ERROR",
                format!("Mailgun HTTP {status}: {}", truncate(&body_text, 512)),
            )
        } else if status == 401 || status == 403 {
            AgentError::permanent(
                "MAILGUN_UNAUTHORIZED",
                format!("Mailgun HTTP {status}: {}", truncate(&body_text, 512)),
            )
        } else {
            AgentError::permanent(
                "MAILGUN_REQUEST_FAILED",
                format!("Mailgun HTTP {status}: {}", truncate(&body_text, 512)),
            )
        };
        err = err
            .with_attr("integration", "MAILGUN")
            .with_attr("status_code", status.to_string())
            .with_attr("body", truncate(&body_text, 512));
        if status == 429 {
            let retry_after_ms = response
                .headers
                .get("retry-after-ms")
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    response
                        .headers
                        .get("retry-after")
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|s| s * 1000)
                });
            if let Some(ms) = retry_after_ms {
                err = err.with_retry_after_ms(ms);
            }
        }
        return Err(err);
    }

    let resp_json: Value = serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "MAILGUN_RESPONSE_PARSE_ERROR",
            format!("failed to parse Mailgun response: {e}"),
        )
        .with_attr("integration", "MAILGUN")
    })?;

    Ok(SendEmailOutput {
        id: resp_json["id"].as_str().unwrap_or("").to_string(),
        message: resp_json["message"]
            .as_str()
            .unwrap_or("Queued")
            .to_string(),
    })
}

// ============================================================================
// Helpers
// ============================================================================

/// Percent-encode a string for use in application/x-www-form-urlencoded bodies.
/// Space → `+`, everything else that isn't unreserved → `%XX`.
fn url_encode_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            ' ' => out.push('+'),
            _ => {
                for byte in c.to_string().as_bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

fn url_encode_form(parts: &[(&str, String)]) -> String {
    parts
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode_value(k), url_encode_value(v)))
        .collect::<Vec<_>>()
        .join("&")
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

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[&__CAPABILITY_META_SEND_EMAIL];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [(
        "SendEmailInput",
        &__INPUT_META_SendEmailInput as &InputTypeMeta,
    )]
    .into_iter()
    .collect();
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [(
        "SendEmailOutput",
        &__OUTPUT_META_SendEmailOutput as &OutputTypeMeta,
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
        id: "mailgun".into(),
        name: "Mailgun".into(),
        description: "Mailgun email service for sending transactional and marketing emails.".into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["mailgun".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_mailgun::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "send-email" => __executor_send_email(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("mailgun agent has no capability `{other}`"),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                });
            }
        };
        executor_result
            .map_err(error_string_to_error_info)
            .and_then(|out_value| serde_json::to_vec(&out_value).map_err(bad_json))
    }
}

#[cfg(target_arch = "wasm32")]
fn bad_json(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

/// The `#[capability]` macro packages each error as a JSON-string with
/// `{ code, message, category, severity, ... }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        let category = value
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("permanent")
            .to_string();
        let retryable = value
            .get("retryable")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| category == "transient");
        ErrorInfo {
            code: value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAPABILITY_ERROR")
                .into(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&s)
                .into(),
            category,
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable,
            retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
            attributes: value.get("attributes").map(|v| v.to_string()),
        }
    } else {
        ErrorInfo {
            code: "CAPABILITY_ERROR".into(),
            message: s,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);
