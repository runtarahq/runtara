//! Mailgun email agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/mailgun.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope. The `X-Runtara-Connection-Id` header causes the proxy to
//! attach Basic auth (derived from `api_key`) and resolve the base URL
//! (`https://api.mailgun.net` or `https://api.eu.mailgun.net` depending on
//! the `region` parameter). The component never sees secrets.
//!
//! The `domain` connection parameter is a non-credential config value exposed
//! in `connection.parameters` (JSON string); the component reads it to build
//! the request path and the default sender address.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::time::Duration;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::Deserialize;
use serde_json::Value;

// -----------------------------------------------------------------------------
// Input schema (mirrors SendEmailInput in mailgun.rs)
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SendEmailInput {
    /// Recipient address(es), comma-separated for multiple.
    to: String,
    /// Email subject line.
    subject: String,
    /// Plain-text body.
    #[serde(default)]
    text: Option<String>,
    /// HTML body (takes precedence over text when both provided).
    #[serde(default)]
    html: Option<String>,
    /// Sender address. Defaults to `noreply@{domain}` when absent.
    #[serde(default)]
    from: Option<String>,
    /// CC recipients, comma-separated.
    #[serde(default)]
    cc: Option<String>,
    /// BCC recipients, comma-separated.
    #[serde(default)]
    bcc: Option<String>,
    /// Reply-To address.
    #[serde(default)]
    reply_to: Option<String>,
    /// Comma-separated tags for tracking (mapped to Mailgun `o:tag` params).
    #[serde(default)]
    tags: Option<String>,
}

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "mailgun".into(),
            display_name: "Mailgun".into(),
            description: "Mailgun email service for sending transactional and marketing emails."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["mailgun".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![CapabilityInfo {
            id: "send-email".into(),
            function_name: "send_email".into(),
            display_name: Some("Send Email (Mailgun)".into()),
            description: Some(
                "Send an email via Mailgun REST API. \
                 Credentials are injected server-side by the runtara HTTP proxy."
                    .into(),
            ),
            has_side_effects: true,
            is_idempotent: false,
            rate_limited: true,
            tags: vec!["email".into(), "mailgun".into()],
            input_schema: SEND_EMAIL_INPUT_SCHEMA.into(),
            output_schema: SEND_EMAIL_OUTPUT_SCHEMA.into(),
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
            "send-email" => send_email(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("mailgun agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Capability implementation
// -----------------------------------------------------------------------------

fn send_email(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: SendEmailInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let conn = connection.ok_or_else(|| {
        permanent_err(
            "MAILGUN_MISSING_CONNECTION",
            "MAILGUN capability requires a connection",
        )
    })?;

    // `domain` lives in the connection parameters (non-secret config).
    let params: Value = serde_json::from_str(&conn.parameters).map_err(|e| {
        permanent_err(
            "MAILGUN_INVALID_PARAMETERS",
            format!("failed to parse connection parameters: {e}"),
        )
    })?;

    let domain = params["domain"].as_str().ok_or_else(|| {
        permanent_err(
            "MAILGUN_MISSING_FIELD",
            "MAILGUN connection parameters missing required field: domain",
        )
    })?;

    let from = input.from.unwrap_or_else(|| format!("noreply@{}", domain));

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
        form_parts.push(("o:tag", tag.to_string()));
    }

    let encoded_body = url_encode_form(&form_parts);

    // Route through the proxy with the connection id so the proxy injects
    // Authorization: Basic <base64(api:<key>)> and resolves the base URL.
    let url = format!("/v3/{}/messages", domain);
    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(30_000));
    let response = client
        .request("POST", &url)
        .header("X-Runtara-Connection-Id", &conn.connection_id)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body_bytes(encoded_body.as_bytes())
        .call_agent()
        .map_err(|e| {
            transient_err(
                "MAILGUN_NETWORK_ERROR",
                format!("request to Mailgun failed: {e}"),
            )
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (code, category, retryable) = if status == 429 || (500..600).contains(&status) {
            ("MAILGUN_UPSTREAM_ERROR", "transient", true)
        } else if status == 401 || status == 403 {
            ("MAILGUN_UNAUTHORIZED", "permanent", false)
        } else {
            ("MAILGUN_REQUEST_FAILED", "permanent", false)
        };
        let retry_after_ms = if status == 429 {
            response
                .headers
                .get("retry-after-ms")
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    response
                        .headers
                        .get("retry-after")
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|s| s * 1000)
                })
        } else {
            None
        };
        return Err(ErrorInfo {
            code: code.into(),
            message: format!("Mailgun HTTP {status}: {}", truncate(&body_text, 512)),
            category: category.into(),
            severity: "error".into(),
            retryable,
            retry_after_ms,
            attributes: None,
        });
    }

    let resp_json: Value = serde_json::from_slice(&response.body).map_err(|e| {
        permanent_err(
            "MAILGUN_RESPONSE_PARSE_ERROR",
            format!("failed to parse Mailgun response: {e}"),
        )
    })?;

    let output = serde_json::json!({
        "id":      resp_json["id"].as_str().unwrap_or(""),
        "message": resp_json["message"].as_str().unwrap_or("Queued"),
    });

    serde_json::to_string(&output)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

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

// -----------------------------------------------------------------------------
// JSON Schemas
// -----------------------------------------------------------------------------

const SEND_EMAIL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["to", "subject"],
    "properties": {
        "to":       { "type": "string",  "description": "Recipient email address(es), comma-separated for multiple", "example": "user@example.com" },
        "subject":  { "type": "string",  "description": "Email subject line", "example": "Order Confirmation" },
        "text":     { "type": "string",  "description": "Plain text email body" },
        "html":     { "type": "string",  "description": "HTML email body (takes precedence over text when both provided)" },
        "from":     { "type": "string",  "description": "Sender email address (defaults to noreply@{domain})" },
        "cc":       { "type": "string",  "description": "CC recipients, comma-separated" },
        "bcc":      { "type": "string",  "description": "BCC recipients, comma-separated" },
        "reply_to": { "type": "string",  "description": "Reply-To email address" },
        "tags":     { "type": "string",  "description": "Comma-separated tags for tracking" }
    }
}"#;

const SEND_EMAIL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "id":      { "type": "string", "description": "Mailgun message ID for tracking" },
        "message": { "type": "string", "description": "Mailgun response message" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
