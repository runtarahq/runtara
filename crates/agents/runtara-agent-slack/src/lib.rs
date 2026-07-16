//! Slack Bot agent — WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_slack.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to attach the bot token
//! and resolve `https://slack.com/api/...`. The component never sees secrets.
//!
//! The `upload-file` capability follows Slack's V2 upload flow:
//!   1. `files.getUploadURLExternal` — obtain a presigned upload URL (via proxy + auth).
//!   2. POST raw bytes to the presigned URL — no connection header; URL is pre-signed.
//!   3. `files.completeUploadExternal` — finalize and share to channel (via proxy + auth).
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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

    pub fn with_attr_value(mut self, key: impl Into<String>, value: Value) -> Self {
        self.attributes.insert(key.into(), value);
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
// Constants
// ============================================================================

const SLACK_API_BASE: &str = "https://slack.com/api";
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const UPLOAD_TIMEOUT_MS: u64 = 120_000;

// ============================================================================
// Shared Slack API helper
// ============================================================================

/// POST a JSON body to `https://slack.com/api/{slack_method}` through the
/// proxy (which injects the Bot token for the connection). Handles Slack's
/// "200 OK with ok:false" error pattern and maps well-known error codes to
/// structured `AgentError` values.
fn slack_api_call(
    slack_method: &str,
    connection: &RawConnection,
    body: &Value,
) -> Result<Value, AgentError> {
    let url = format!("{}/{}", SLACK_API_BASE, slack_method);
    let body_bytes = serde_json::to_vec(body).map_err(|e| {
        AgentError::permanent(
            "SLACK_SERIALIZATION_ERROR",
            format!("Failed to serialize request body: {e}"),
        )
        .with_attr("integration", "SLACK")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "SLACK_NETWORK_ERROR",
                format!("Network error calling Slack {slack_method}: {e}"),
            )
            .with_attr("integration", "SLACK")
            .with_attr("method", slack_method)
        })?;

    let status = response.status;

    // Try parsing the response body as JSON regardless of status; Slack returns
    // JSON for both success and error responses on the Web API.
    let parse_result: Result<Value, _> = serde_json::from_slice(&response.body);

    // Non-2xx HTTP errors
    if !(200..300).contains(&status) {
        let body_str = match &parse_result {
            Ok(v) => serde_json::to_string(v).unwrap_or_default(),
            Err(_) => String::from_utf8_lossy(&response.body).to_string(),
        };
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

        let mut err = if status == 429 {
            AgentError::transient(
                "SLACK_RATE_LIMITED",
                format!("HTTP {status}: {}", truncate(&body_str, 512)),
            )
        } else if (500..600).contains(&status) {
            AgentError::transient(
                "SLACK_SERVER_ERROR",
                format!("HTTP {status}: {}", truncate(&body_str, 512)),
            )
        } else {
            AgentError::permanent(
                format!("SLACK_HTTP_{status}"),
                format!("HTTP {status}: {}", truncate(&body_str, 512)),
            )
        };
        err = err
            .with_attr("integration", "SLACK")
            .with_attr("method", slack_method)
            .with_attr("status_code", status.to_string())
            .with_attr("body", truncate(&body_str, 512));
        if let Some(ms) = retry_after_ms {
            err = err.with_retry_after_ms(ms);
        }
        return Err(err);
    }

    let response_json = parse_result.map_err(|e| {
        let body_str = String::from_utf8_lossy(&response.body).to_string();
        AgentError::permanent(
            "SLACK_RESPONSE_PARSE_ERROR",
            format!("Failed to parse Slack response: {e}"),
        )
        .with_attr("integration", "SLACK")
        .with_attr("method", slack_method)
        .with_attr("body", truncate(&body_str, 512))
    })?;

    // Slack API errors — "200 OK" with ok: false
    if response_json["ok"].as_bool() != Some(true) {
        let slack_code = response_json["error"]
            .as_str()
            .unwrap_or("unknown_error")
            .to_string();
        let msg = format!("Slack API error ({slack_method}): {slack_code}");

        // (code, retryable) — `retryable` decides whether the error envelope is
        // built via `transient()` (warning severity, category=transient) or
        // `permanent()` (error severity, category=permanent), preserving the
        // legacy host classifier's per-Slack-error mapping.
        let (code, retryable) = match slack_code.as_str() {
            "ratelimited" => ("SLACK_RATE_LIMITED", true),
            "channel_not_found" => ("SLACK_CHANNEL_NOT_FOUND", false),
            "not_in_channel" => ("SLACK_NOT_IN_CHANNEL", false),
            "is_archived" => ("SLACK_CHANNEL_ARCHIVED", false),
            "invalid_auth" | "account_inactive" | "token_revoked" => ("SLACK_AUTH_ERROR", false),
            _ => ("SLACK_API_ERROR", false),
        };

        let err = if retryable {
            AgentError::transient(code, msg)
        } else {
            AgentError::permanent(code, msg)
        };

        return Err(err
            .with_attr("integration", "SLACK")
            .with_attr("method", slack_method)
            .with_attr("error", slack_code)
            .with_attr_value("response", response_json));
    }

    Ok(response_json)
}

// ============================================================================
// Send Message
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Message Input")]
pub struct SendMessageInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (the UI/runtime fills it from the
    /// configured connection, not from user input).
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Channel",
        description = "Slack channel ID (e.g. C01234ABCDE) or user ID for direct messages",
        example = "C01234ABCDE"
    )]
    pub channel: String,

    #[field(
        display_name = "Text",
        description = "Message text (supports Slack mrkdwn formatting). Used as fallback when blocks are provided.",
        example = "Hello, world!"
    )]
    pub text: String,

    #[field(
        display_name = "Blocks",
        description = "Block Kit blocks as JSON array for rich message formatting. When provided, text becomes the fallback for notifications."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Value>,

    #[field(
        display_name = "Thread Timestamp",
        description = "Timestamp of the parent message to reply in a thread (e.g. 1234567890.123456)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,

    #[field(
        display_name = "Unfurl Links",
        description = "Whether to enable link unfurling (default: true)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfurl_links: Option<bool>,

    #[field(
        display_name = "Unfurl Media",
        description = "Whether to enable media unfurling (default: true)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfurl_media: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Send Message Output")]
pub struct SendMessageOutput {
    #[field(
        display_name = "OK",
        description = "Whether the API call was successful"
    )]
    pub ok: bool,

    #[field(
        display_name = "Channel",
        description = "Channel ID where the message was posted"
    )]
    pub channel: String,

    #[field(
        display_name = "Timestamp",
        description = "Message timestamp (unique message identifier within the channel)"
    )]
    pub ts: String,
}

#[capability(
    module = "slack",
    display_name = "Send Message",
    description = "Send a message to a Slack channel or user. Supports plain text with mrkdwn formatting and Block Kit for rich layouts.",
    module_display_name = "Slack",
    module_description = "Slack messaging for sending messages, files, and reactions",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "slack_bot",
    module_secure = true
)]
pub fn send_message(input: SendMessageInput) -> Result<SendMessageOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "SLACK_MISSING_CONNECTION",
            "SLACK capability invoked without a connection — add one in the step configuration",
        )
        .with_attr("integration", "SLACK")
    })?;

    let mut body = json!({
        "channel": input.channel,
        "text": input.text,
    });

    if let Some(blocks) = &input.blocks {
        body["blocks"] = blocks.clone();
    }
    if let Some(thread_ts) = &input.thread_ts {
        body["thread_ts"] = json!(thread_ts);
    }
    if let Some(unfurl_links) = input.unfurl_links {
        body["unfurl_links"] = json!(unfurl_links);
    }
    if let Some(unfurl_media) = input.unfurl_media {
        body["unfurl_media"] = json!(unfurl_media);
    }

    let resp = slack_api_call("chat.postMessage", connection, &body)?;

    Ok(SendMessageOutput {
        ok: true,
        channel: resp["channel"].as_str().unwrap_or("").to_string(),
        ts: resp["ts"].as_str().unwrap_or("").to_string(),
    })
}

// ============================================================================
// Upload File
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload File Input")]
pub struct UploadFileInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (the UI/runtime fills it from the
    /// configured connection, not from user input).
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Channel",
        description = "Slack channel ID to share the file in",
        example = "C01234ABCDE"
    )]
    pub channel: String,

    #[field(
        display_name = "Content",
        description = "Base64-encoded file content",
        example = "SGVsbG8gV29ybGQ="
    )]
    pub content: String,

    #[field(
        display_name = "Filename",
        description = "Filename with extension (e.g. report.pdf, data.csv)",
        example = "report.pdf"
    )]
    pub filename: String,

    #[field(
        display_name = "Title",
        description = "Display title for the file in Slack (defaults to filename)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[field(
        display_name = "Initial Comment",
        description = "Message text to post alongside the file"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_comment: Option<String>,

    #[field(
        display_name = "Thread Timestamp",
        description = "Timestamp of the parent message to upload the file in a thread"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,

    #[field(
        display_name = "Content Type",
        description = "MIME type of the file (e.g. application/pdf, text/csv). Auto-detected by Slack if omitted."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Upload File Output")]
pub struct UploadFileOutput {
    #[field(display_name = "OK", description = "Whether the upload was successful")]
    pub ok: bool,

    #[field(display_name = "File ID", description = "Slack file ID")]
    pub file_id: String,

    #[field(
        display_name = "Title",
        description = "File title as displayed in Slack"
    )]
    pub title: String,
}

#[capability(
    module = "slack",
    display_name = "Upload File",
    description = "Upload a file to a Slack channel. Accepts base64-encoded content. Uses the Slack V2 upload API (files.getUploadURLExternal → raw POST → files.completeUploadExternal).",
    module_display_name = "Slack",
    module_description = "Slack messaging for sending messages, files, and reactions",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "slack_bot",
    module_secure = true
)]
pub fn upload_file(input: UploadFileInput) -> Result<UploadFileOutput, AgentError> {
    use base64::Engine as _;

    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "SLACK_MISSING_CONNECTION",
            "SLACK capability invoked without a connection — add one in the step configuration",
        )
        .with_attr("integration", "SLACK")
    })?;

    // Decode base64 file content
    let file_bytes = base64::engine::general_purpose::STANDARD
        .decode(&input.content)
        .map_err(|e| {
            AgentError::permanent(
                "SLACK_INVALID_CONTENT",
                format!("Invalid base64 file content: {e}"),
            )
            .with_attr("integration", "SLACK")
        })?;

    let file_length = file_bytes.len();

    // Step 1: Get a presigned upload URL from Slack
    let mut get_url_body = json!({
        "filename": input.filename,
        "length": file_length,
    });
    if let Some(ref snippet_type) = input.content_type {
        get_url_body["snippet_type"] = json!(snippet_type);
    }

    let url_resp = slack_api_call("files.getUploadURLExternal", connection, &get_url_body)?;

    let upload_url = url_resp["upload_url"].as_str().ok_or_else(|| {
        AgentError::permanent(
            "SLACK_MISSING_UPLOAD_URL",
            "Slack did not return an upload_url",
        )
        .with_attr("integration", "SLACK")
        .with_attr_value("response", url_resp.clone())
    })?;

    let file_id = url_resp["file_id"].as_str().ok_or_else(|| {
        AgentError::permanent("SLACK_MISSING_FILE_ID", "Slack did not return a file_id")
            .with_attr("integration", "SLACK")
            .with_attr_value("response", url_resp.clone())
    })?;

    // Step 2: Upload raw bytes to the presigned URL — no connection header,
    // no auth injection; the URL is already authenticated by Slack.
    let content_type = input
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");

    let upload_client =
        runtara_http::HttpClient::with_timeout(Duration::from_millis(UPLOAD_TIMEOUT_MS));
    let upload_response = upload_client
        .request("POST", upload_url)
        .header("Content-Type", content_type)
        .body_bytes(&file_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "SLACK_UPLOAD_NETWORK_ERROR",
                format!("Network error uploading file bytes: {e}"),
            )
            .with_attr("integration", "SLACK")
        })?;

    let upload_status = upload_response.status;
    if !(200..300).contains(&upload_status) {
        let body_str = String::from_utf8_lossy(&upload_response.body).to_string();
        let retry_after_ms = upload_response
            .headers
            .get("retry-after-ms")
            .and_then(|v| v.parse::<u64>().ok())
            .or_else(|| {
                upload_response
                    .headers
                    .get("retry-after")
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|s| s * 1000)
            });

        let mut err = if upload_status == 429 {
            AgentError::transient(
                "SLACK_UPLOAD_RATE_LIMITED",
                format!("HTTP {upload_status}: {}", truncate(&body_str, 512)),
            )
        } else if (500..600).contains(&upload_status) {
            AgentError::transient(
                "SLACK_UPLOAD_SERVER_ERROR",
                format!("HTTP {upload_status}: {}", truncate(&body_str, 512)),
            )
        } else {
            AgentError::permanent(
                format!("SLACK_UPLOAD_HTTP_{upload_status}"),
                format!("HTTP {upload_status}: {}", truncate(&body_str, 512)),
            )
        };
        err = err
            .with_attr("integration", "SLACK")
            .with_attr("status_code", upload_status.to_string())
            .with_attr("body", truncate(&body_str, 512));
        if let Some(ms) = retry_after_ms {
            err = err.with_retry_after_ms(ms);
        }
        return Err(err);
    }

    // Step 3: Complete the upload and share to channel
    let title = input
        .title
        .clone()
        .unwrap_or_else(|| input.filename.clone());

    let mut complete_body = json!({
        "files": [{"id": file_id, "title": title}],
        "channel_id": input.channel,
    });
    if let Some(ref comment) = input.initial_comment {
        complete_body["initial_comment"] = json!(comment);
    }
    if let Some(ref thread_ts) = input.thread_ts {
        complete_body["thread_ts"] = json!(thread_ts);
    }

    let complete_resp = slack_api_call("files.completeUploadExternal", connection, &complete_body)?;

    // Extract file info from the completed upload response
    let files = complete_resp["files"].as_array();
    let returned_id = files
        .and_then(|f| f.first())
        .and_then(|f| f["id"].as_str())
        .unwrap_or(file_id)
        .to_string();
    let returned_title = files
        .and_then(|f| f.first())
        .and_then(|f| f["title"].as_str())
        .unwrap_or(&title)
        .to_string();

    Ok(UploadFileOutput {
        ok: true,
        file_id: returned_id,
        title: returned_title,
    })
}

// ============================================================================
// Helpers
// ============================================================================

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

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_SEND_MESSAGE,
        &__CAPABILITY_META_UPLOAD_FILE,
    ];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "SendMessageInput",
            &__INPUT_META_SendMessageInput as &InputTypeMeta,
        ),
        ("UploadFileInput", &__INPUT_META_UploadFileInput),
    ]
    .into_iter()
    .collect();
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "SendMessageOutput",
            &__OUTPUT_META_SendMessageOutput as &OutputTypeMeta,
        ),
        ("UploadFileOutput", &__OUTPUT_META_UploadFileOutput),
    ]
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
        id: "slack".into(),
        name: "Slack".into(),
        description: "Slack messaging for sending messages, files, and reactions".into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["slack_bot".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_slack::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "send-message" => __executor_send_message(value),
            "upload-file" => __executor_upload_file(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("slack agent has no capability `{other}`"),
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
