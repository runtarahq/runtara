//! Slack Bot Operations
//!
//! Send messages and files to Slack channels and users via the Slack Web API.

use crate::connections::RawConnection;
use crate::http::{self, BodyType, HttpBody, HttpMethod, ResponseType};
use base64::Engine;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

use super::errors::{http_status_error, permanent_error};
use super::integration_utils::{self as iu, ProxyHttpClient};

// ============================================================================
// Shared helpers
// ============================================================================

/// Extract the connection reference, required for all Slack operations.
fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, String> {
    iu::require_connection("SLACK", connection).map_err(String::from)
}

/// Call a Slack Web API method (JSON POST) and return the parsed response.
/// Handles Slack's "200 OK with `ok: false`" error pattern — the per-Slack-
/// error-code mapping stays local to preserve the Slack-specific wire
/// contract (e.g. `SLACK_CHANNEL_NOT_FOUND`).
fn slack_api_call(method: &str, connection: &RawConnection, body: Value) -> Result<Value, String> {
    let response_json = ProxyHttpClient::new(connection, "SLACK")
        .post(format!("/api/{}", method))
        .header("Content-Type", "application/json; charset=utf-8")
        .json_body(body)
        .send_json()
        .map_err(String::from)?;

    if response_json["ok"].as_bool() != Some(true) {
        let error = response_json["error"].as_str().unwrap_or("unknown_error");
        let msg = format!("Slack API error ({}): {}", method, error);
        let attrs = json!({"error": error, "method": method, "response": response_json});

        return Err(match error {
            "ratelimited" => super::errors::transient_error("SLACK_RATE_LIMITED", &msg, attrs),
            "channel_not_found" => permanent_error("SLACK_CHANNEL_NOT_FOUND", &msg, attrs),
            "not_in_channel" => permanent_error("SLACK_NOT_IN_CHANNEL", &msg, attrs),
            "is_archived" => permanent_error("SLACK_CHANNEL_ARCHIVED", &msg, attrs),
            "invalid_auth" | "account_inactive" | "token_revoked" => {
                permanent_error("SLACK_AUTH_ERROR", &msg, attrs)
            }
            _ => permanent_error("SLACK_API_ERROR", &msg, attrs),
        });
    }

    Ok(response_json)
}

// ============================================================================
// Send Message
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Message Input")]
pub struct SendMessageInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Value>,

    #[field(
        display_name = "Thread Timestamp",
        description = "Timestamp of the parent message to reply in a thread (e.g. 1234567890.123456)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,

    #[field(
        display_name = "Unfurl Links",
        description = "Whether to enable link unfurling (default: true)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unfurl_links: Option<bool>,

    #[field(
        display_name = "Unfurl Media",
        description = "Whether to enable media unfurling (default: true)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unfurl_media: Option<bool>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
pub fn send_message(input: SendMessageInput) -> Result<SendMessageOutput, String> {
    let connection = require_connection(&input._connection)?;

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

    let resp = slack_api_call("chat.postMessage", connection, body)?;

    Ok(SendMessageOutput {
        ok: true,
        channel: resp["channel"].as_str().unwrap_or("").to_string(),
        ts: resp["ts"].as_str().unwrap_or("").to_string(),
    })
}

// ============================================================================
// Upload File
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload File Input")]
pub struct UploadFileInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[field(
        display_name = "Initial Comment",
        description = "Message text to post alongside the file"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_comment: Option<String>,

    #[field(
        display_name = "Thread Timestamp",
        description = "Timestamp of the parent message to upload the file in a thread"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,

    #[field(
        display_name = "Content Type",
        description = "MIME type of the file (e.g. application/pdf, text/csv). Auto-detected by Slack if omitted."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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

/// Upload a file to Slack using the V2 upload flow:
/// 1. `files.getUploadURLExternal` — obtain a presigned upload URL
/// 2. POST raw bytes to the presigned URL
/// 3. `files.completeUploadExternal` — finalize and share to channel
#[capability(
    module = "slack",
    display_name = "Upload File",
    description = "Upload a file to a Slack channel. Accepts base64-encoded content. Uses the Slack V2 upload API.",
    module_display_name = "Slack",
    module_description = "Slack messaging for sending messages, files, and reactions",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "slack_bot",
    module_secure = true
)]
pub fn upload_file(input: UploadFileInput) -> Result<UploadFileOutput, String> {
    let connection = require_connection(&input._connection)?;

    // Decode base64 content
    let file_bytes = base64::engine::general_purpose::STANDARD
        .decode(&input.content)
        .map_err(|e| {
            permanent_error(
                "SLACK_INVALID_CONTENT",
                &format!("Invalid base64 file content: {}", e),
                json!({}),
            )
        })?;

    let file_length = file_bytes.len();

    // Step 1: Get upload URL
    let mut get_url_body = json!({
        "filename": input.filename,
        "length": file_length,
    });
    if let Some(ref snippet_type) = input.content_type {
        get_url_body["snippet_type"] = json!(snippet_type);
    }

    let url_resp = slack_api_call("files.getUploadURLExternal", connection, get_url_body)?;

    let upload_url = url_resp["upload_url"].as_str().ok_or_else(|| {
        permanent_error(
            "SLACK_MISSING_UPLOAD_URL",
            "Slack did not return an upload_url",
            json!({"response": url_resp}),
        )
    })?;

    let file_id = url_resp["file_id"].as_str().ok_or_else(|| {
        permanent_error(
            "SLACK_MISSING_FILE_ID",
            "Slack did not return a file_id",
            json!({"response": url_resp}),
        )
    })?;

    // Step 2: Upload raw bytes to the presigned URL
    let content_type = input
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");

    let mut upload_headers = HashMap::new();
    upload_headers.insert("Content-Type".to_string(), content_type.to_string());

    let upload_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: upload_url.to_string(),
        headers: upload_headers,
        query_parameters: HashMap::new(),
        body: HttpBody(Value::String(input.content)),
        body_type: BodyType::Binary,
        response_type: ResponseType::Text,
        timeout_ms: 120000,
        ..Default::default()
    };

    let upload_resp = http::http_request(upload_input)?;

    if !upload_resp.success {
        let body_str = format!("{:?}", upload_resp.body);
        return Err(http_status_error(
            "SLACK_UPLOAD",
            upload_resp.status_code,
            &format!("File upload to presigned URL failed: {}", body_str),
            json!({"status_code": upload_resp.status_code, "body": body_str}),
        ));
    }

    // Step 3: Complete the upload and share to channel
    let title = input.title.unwrap_or_else(|| input.filename.clone());

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

    let complete_resp = slack_api_call("files.completeUploadExternal", connection, complete_body)?;

    // Extract file info from the completed upload
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
