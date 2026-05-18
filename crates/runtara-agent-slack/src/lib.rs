//! Slack Bot agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/slack.rs`.
//!
//! Routing model: `runtara_http::HttpClient` reads `RUNTARA_HTTP_PROXY_URL` and
//! routes every request through the proxy. For Slack API calls, we set
//! `X-Runtara-Connection-Id` so the proxy can attach the Bot token server-side.
//! The component never handles secrets directly.
//!
//! The upload-file capability uses the Slack V2 upload flow:
//!   1. `files.getUploadURLExternal` — obtain a presigned upload URL (via proxy + auth).
//!   2. POST raw bytes to the presigned URL — no connection header; URL is pre-signed.
//!   3. `files.completeUploadExternal` — finalize and share to channel (via proxy + auth).

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::time::Duration;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

const SLACK_API_BASE: &str = "https://slack.com/api";
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const UPLOAD_TIMEOUT_MS: u64 = 120_000;

// -----------------------------------------------------------------------------
// Helpers — error constructors
// -----------------------------------------------------------------------------

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

/// Classify an HTTP status code into a transient or permanent error.
fn classify_http_err(
    prefix: &str,
    status: u16,
    body: &str,
    retry_after_ms: Option<u64>,
) -> ErrorInfo {
    let (code, category, retryable) = if status == 429 {
        (format!("{prefix}_RATE_LIMITED"), "transient", true)
    } else if (500..600).contains(&status) {
        (format!("{prefix}_SERVER_ERROR"), "transient", true)
    } else {
        (format!("{prefix}_HTTP_{status}"), "permanent", false)
    };
    ErrorInfo {
        code,
        message: format!("HTTP {status}: {}", truncate(body, 512)),
        category: category.into(),
        severity: if retryable { "warning" } else { "error" }.into(),
        retryable,
        retry_after_ms,
        attributes: None,
    }
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

// -----------------------------------------------------------------------------
// Core Slack API helper
// -----------------------------------------------------------------------------

/// POST a JSON body to `https://slack.com/api/{slack_method}` through the
/// proxy (which injects the Bot token for the connection). Handles Slack's
/// "200 OK with ok:false" error pattern and maps well-known error codes to
/// structured `ErrorInfo` values.
fn slack_api_call(
    slack_method: &str,
    connection: &ConnectionInfo,
    body: &Value,
) -> Result<Value, ErrorInfo> {
    let url = format!("{}/{}", SLACK_API_BASE, slack_method);
    let body_bytes = serde_json::to_vec(body).map_err(|e| {
        permanent_err(
            "SLACK_SERIALIZATION_ERROR",
            format!("Failed to serialize request body: {e}"),
        )
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            transient_err(
                "SLACK_NETWORK_ERROR",
                format!("Network error calling Slack {slack_method}: {e}"),
            )
        })?;

    let status = response.status;

    // Parse the response body as JSON
    let resp_json: Value = serde_json::from_slice(&response.body).map_err(|_| {
        let body_str = String::from_utf8_lossy(&response.body).to_string();
        classify_http_err("SLACK", status, &body_str, None)
    })?;

    // Non-2xx HTTP errors
    if !(200..300).contains(&status) {
        let body_str = serde_json::to_string(&resp_json).unwrap_or_default();
        let retry_after = response
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
        return Err(classify_http_err("SLACK", status, &body_str, retry_after));
    }

    // Slack API errors — "200 OK" with ok: false
    if resp_json["ok"].as_bool() != Some(true) {
        let slack_code = resp_json["error"]
            .as_str()
            .unwrap_or("unknown_error")
            .to_string();
        let msg = format!("Slack API error ({slack_method}): {slack_code}");
        let attrs = serde_json::to_string(&json!({
            "error": slack_code,
            "method": slack_method,
        }))
        .ok();

        let (code, category, retryable) = match slack_code.as_str() {
            "ratelimited" => ("SLACK_RATE_LIMITED", "transient", true),
            "channel_not_found" => ("SLACK_CHANNEL_NOT_FOUND", "permanent", false),
            "not_in_channel" => ("SLACK_NOT_IN_CHANNEL", "permanent", false),
            "is_archived" => ("SLACK_CHANNEL_ARCHIVED", "permanent", false),
            "invalid_auth" | "account_inactive" | "token_revoked" => {
                ("SLACK_AUTH_ERROR", "permanent", false)
            }
            _ => ("SLACK_API_ERROR", "permanent", false),
        };

        return Err(ErrorInfo {
            code: code.into(),
            message: msg,
            category: category.into(),
            severity: if retryable { "warning" } else { "error" }.into(),
            retryable,
            retry_after_ms: None,
            attributes: attrs,
        });
    }

    Ok(resp_json)
}

// -----------------------------------------------------------------------------
// Capability: send-message
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SendMessageInput {
    channel: String,
    text: String,
    #[serde(default)]
    blocks: Option<Value>,
    #[serde(default)]
    thread_ts: Option<String>,
    #[serde(default)]
    unfurl_links: Option<bool>,
    #[serde(default)]
    unfurl_media: Option<bool>,
}

#[derive(Debug, Serialize)]
struct SendMessageOutput {
    ok: bool,
    channel: String,
    ts: String,
}

fn send_message(input_json: &str, connection: &ConnectionInfo) -> Result<String, ErrorInfo> {
    let input: SendMessageInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

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

    let output = SendMessageOutput {
        ok: true,
        channel: resp["channel"].as_str().unwrap_or("").to_string(),
        ts: resp["ts"].as_str().unwrap_or("").to_string(),
    };

    serde_json::to_string(&output)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability: upload-file
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UploadFileInput {
    channel: String,
    /// Base64-encoded file content.
    content: String,
    filename: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    initial_comment: Option<String>,
    #[serde(default)]
    thread_ts: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct UploadFileOutput {
    ok: bool,
    file_id: String,
    title: String,
}

fn upload_file(input_json: &str, connection: &ConnectionInfo) -> Result<String, ErrorInfo> {
    use base64::Engine as _;

    let input: UploadFileInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    // Decode base64 file content
    let file_bytes = base64::engine::general_purpose::STANDARD
        .decode(&input.content)
        .map_err(|e| {
            permanent_err(
                "SLACK_INVALID_CONTENT",
                format!("Invalid base64 file content: {e}"),
            )
        })?;

    let file_length = file_bytes.len();

    // Step 1: Get a presigned upload URL from Slack
    let mut get_url_body = json!({
        "filename": input.filename,
        "length": file_length,
    });
    if let Some(ref ct) = input.content_type {
        get_url_body["snippet_type"] = json!(ct);
    }

    let url_resp = slack_api_call("files.getUploadURLExternal", connection, &get_url_body)?;

    let upload_url = url_resp["upload_url"].as_str().ok_or_else(|| {
        permanent_err(
            "SLACK_MISSING_UPLOAD_URL",
            "Slack did not return an upload_url",
        )
    })?;

    let file_id = url_resp["file_id"]
        .as_str()
        .ok_or_else(|| permanent_err("SLACK_MISSING_FILE_ID", "Slack did not return a file_id"))?;

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
            transient_err(
                "SLACK_UPLOAD_NETWORK_ERROR",
                format!("Network error uploading file bytes: {e}"),
            )
        })?;

    let upload_status = upload_response.status;
    if !(200..300).contains(&upload_status) {
        let body_str = String::from_utf8_lossy(&upload_response.body).to_string();
        let retry_after = upload_response
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
        return Err(classify_http_err(
            "SLACK_UPLOAD",
            upload_status,
            &body_str,
            retry_after,
        ));
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

    let output = UploadFileOutput {
        ok: true,
        file_id: returned_id,
        title: returned_title,
    };

    serde_json::to_string(&output)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// WIT component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "slack".into(),
            display_name: "Slack".into(),
            description: "Slack messaging: send messages and upload files via the Slack Web API."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["slack_bot".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            CapabilityInfo {
                id: "send-message".into(),
                function_name: "send_message".into(),
                display_name: Some("Send Message".into()),
                description: Some(
                    "Send a message to a Slack channel or user. Supports plain text with \
                     mrkdwn formatting and Block Kit for rich layouts."
                        .into(),
                ),
                has_side_effects: true,
                is_idempotent: false,
                rate_limited: true,
                tags: vec!["slack".into(), "messaging".into()],
                input_schema: SEND_MESSAGE_INPUT_SCHEMA.into(),
                output_schema: SEND_MESSAGE_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "upload-file".into(),
                function_name: "upload_file".into(),
                display_name: Some("Upload File".into()),
                description: Some(
                    "Upload a file to a Slack channel. Accepts base64-encoded content. \
                     Uses the Slack V2 upload API (files.getUploadURLExternal → raw POST \
                     → files.completeUploadExternal)."
                        .into(),
                ),
                has_side_effects: true,
                is_idempotent: false,
                rate_limited: true,
                tags: vec!["slack".into(), "files".into()],
                input_schema: UPLOAD_FILE_INPUT_SCHEMA.into(),
                output_schema: UPLOAD_FILE_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        let conn = connection.ok_or_else(|| {
            permanent_err(
                "SLACK_CONNECTION_REQUIRED",
                "Slack capabilities require a connection (slack_bot)",
            )
        })?;

        match capability_id.as_str() {
            "send-message" => send_message(&input, &conn),
            "upload-file" => upload_file(&input, &conn),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("slack agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// JSON Schemas (mirrors legacy CapabilityInput/CapabilityOutput structs)
// -----------------------------------------------------------------------------

const SEND_MESSAGE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["channel", "text"],
    "properties": {
        "channel": {
            "type": "string",
            "description": "Slack channel ID (e.g. C01234ABCDE) or user ID for direct messages",
            "example": "C01234ABCDE"
        },
        "text": {
            "type": "string",
            "description": "Message text (supports Slack mrkdwn formatting). Used as fallback when blocks are provided.",
            "example": "Hello, world!"
        },
        "blocks": {
            "description": "Block Kit blocks as JSON array for rich message formatting. When provided, text becomes the fallback for notifications."
        },
        "thread_ts": {
            "type": "string",
            "description": "Timestamp of the parent message to reply in a thread (e.g. 1234567890.123456)"
        },
        "unfurl_links": {
            "type": "boolean",
            "description": "Whether to enable link unfurling (default: true)"
        },
        "unfurl_media": {
            "type": "boolean",
            "description": "Whether to enable media unfurling (default: true)"
        }
    }
}"#;

const SEND_MESSAGE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "ok":      { "type": "boolean", "description": "Whether the API call was successful" },
        "channel": { "type": "string",  "description": "Channel ID where the message was posted" },
        "ts":      { "type": "string",  "description": "Message timestamp (unique message identifier within the channel)" }
    }
}"#;

const UPLOAD_FILE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["channel", "content", "filename"],
    "properties": {
        "channel": {
            "type": "string",
            "description": "Slack channel ID to share the file in",
            "example": "C01234ABCDE"
        },
        "content": {
            "type": "string",
            "description": "Base64-encoded file content",
            "example": "SGVsbG8gV29ybGQ="
        },
        "filename": {
            "type": "string",
            "description": "Filename with extension (e.g. report.pdf, data.csv)",
            "example": "report.pdf"
        },
        "title": {
            "type": "string",
            "description": "Display title for the file in Slack (defaults to filename)"
        },
        "initial_comment": {
            "type": "string",
            "description": "Message text to post alongside the file"
        },
        "thread_ts": {
            "type": "string",
            "description": "Timestamp of the parent message to upload the file in a thread"
        },
        "content_type": {
            "type": "string",
            "description": "MIME type of the file (e.g. application/pdf, text/csv). Auto-detected by Slack if omitted."
        }
    }
}"#;

const UPLOAD_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "ok":      { "type": "boolean", "description": "Whether the upload was successful" },
        "file_id": { "type": "string",  "description": "Slack file ID" },
        "title":   { "type": "string",  "description": "File title as displayed in Slack" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
