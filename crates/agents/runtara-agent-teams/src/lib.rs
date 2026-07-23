//! Microsoft Teams Bot agent — WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions the
//! wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_teams.meta.json` next to the
//! `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. Two control
//! headers steer it:
//!   * `X-Runtara-Connection-Id` — the proxy mints and injects the Bot
//!     Connector bearer token for the connection; the component never sees the
//!     app secret.
//!   * `X-Runtara-Endpoint-Ref` — an opaque, tenant+connection-bound signed
//!     token (produced by the Teams webhook after it authenticated the inbound
//!     activity) that supplies the conversation's `serviceUrl` as the request
//!     base. The component never sees the serviceUrl; it only relays the ref
//!     from its trigger data.
//!
//! The agent therefore sends a RELATIVE Bot Connector path (e.g.
//! `/v3/conversations/{id}/activities`); the proxy verifies the ref and joins
//! it under the validated serviceUrl base with path containment.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings {
    wit_bindgen::generate!({
        path: ["../../runtara-agent-wit/wit", "wit"],
        world: "runtara:agent-teams/agent",
        async: false,
        generate_all,
    });
}

// ============================================================================
// Local AgentError shim (mirrors runtara-agent-slack / -mailgun)
// ============================================================================

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

impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================

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

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Teams messages have a ~28 KB limit; split plain text well under it.
const TEXT_CHUNK_CHARS: usize = 4000;
/// Adaptive Card attachment content type in Teams.
const ADAPTIVE_CARD_CONTENT_TYPE: &str = "application/vnd.microsoft.card.adaptive";

// ============================================================================
// Bot Connector call helper
// ============================================================================

/// Result of one Bot Connector activity POST.
struct BotConnectorResponse {
    /// The `id` field of the returned `ResourceResponse` (the created/updated
    /// activity id), when present.
    activity_id: Option<String>,
}

/// POST an activity to the Bot Connector via the proxy. `path` is a RELATIVE
/// Bot Connector path — the proxy joins it under the conversation's serviceUrl
/// (bound by `endpoint_ref`) and injects the bearer token (by connection id).
fn bot_connector_post(
    path: &str,
    connection: &RawConnection,
    endpoint_ref: &str,
    body: &Value,
) -> Result<BotConnectorResponse, AgentError> {
    let body_bytes = serde_json::to_vec(body).map_err(|e| {
        AgentError::permanent(
            "TEAMS_SERIALIZATION_ERROR",
            format!("Failed to serialize activity: {e}"),
        )
        .with_attr("integration", "TEAMS")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS));
    let response = client
        .request("POST", path)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .header("X-Runtara-Endpoint-Ref", endpoint_ref)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "TEAMS_NETWORK_ERROR",
                format!("Network error calling the Bot Connector: {e}"),
            )
            .with_attr("integration", "TEAMS")
        })?;

    let status = response.status;
    let parsed: Option<Value> = serde_json::from_slice(&response.body).ok();

    if !(200..300).contains(&status) {
        return Err(map_bot_connector_error(
            status,
            &response.headers,
            &response.body,
            parsed,
        ));
    }

    let activity_id = parsed
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok(BotConnectorResponse { activity_id })
}

/// Map a non-2xx Bot Connector response to a structured `AgentError`.
///
/// Retryable per Teams docs: 429 plus 412/502/504. 401 is an auth failure;
/// 403 with `errorCode 209 / MessageWritesBlocked` means the bot was
/// uninstalled/blocked for the conversation (the ref's target is dead).
fn map_bot_connector_error(
    status: u16,
    headers: &HashMap<String, String>,
    raw_body: &[u8],
    parsed: Option<Value>,
) -> AgentError {
    let body_str = parsed
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| String::from_utf8_lossy(raw_body).to_string());
    let error_code = parsed
        .as_ref()
        .and_then(|v| v.pointer("/error/code"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let error_subcode = parsed
        .as_ref()
        .and_then(|v| v.pointer("/error/innerHttpError/statusCode"))
        .and_then(|v| v.as_i64());
    let _ = error_subcode;

    let retry_after_ms = headers
        .get("retry-after-ms")
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| {
            headers
                .get("retry-after")
                .and_then(|v| v.parse::<u64>().ok())
                .map(|s| s * 1000)
        });

    // (code, retryable)
    let (code, retryable): (String, bool) = match status {
        401 => ("TEAMS_AUTH_ERROR".into(), false),
        403 if error_code.as_deref() == Some("MessageWritesBlocked")
            || body_str.contains("MessageWritesBlocked") =>
        {
            ("TEAMS_TARGET_BLOCKED".into(), false)
        }
        403 => ("TEAMS_PERMISSION_ERROR".into(), false),
        404 => ("TEAMS_TARGET_NOT_FOUND".into(), false),
        429 => ("TEAMS_RATE_LIMITED".into(), true),
        412 | 502 | 504 => (format!("TEAMS_HTTP_{status}"), true),
        500..=599 => ("TEAMS_SERVER_ERROR".into(), true),
        _ => (format!("TEAMS_HTTP_{status}"), false),
    };

    let msg = format!("Bot Connector HTTP {status}: {}", truncate(&body_str, 512));
    let mut err = if retryable {
        AgentError::transient(code, msg)
    } else {
        AgentError::permanent(code, msg)
    };
    err = err
        .with_attr("integration", "TEAMS")
        .with_attr("status_code", status.to_string());
    if let Some(code) = error_code {
        err = err.with_attr("error_code", code);
    }
    if let Some(ms) = retry_after_ms {
        err = err.with_retry_after_ms(ms);
    }
    if let Some(v) = parsed {
        err = err.with_attr_value("response", v);
    }
    err
}

// ============================================================================
// Send Message
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Message Input")]
pub struct SendMessageInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this out
    /// of the capability metadata.
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Target",
        description = "Opaque conversation target reference from the Teams trigger (data.target.ref). Binds this send to the originating conversation's serviceUrl.",
        example = "1.eyJ2IjoxLC..."
    )]
    pub target: String,

    #[field(
        display_name = "Conversation ID",
        description = "Teams conversation id from the trigger (data.target.conversationId), including any ;messageid= thread suffix.",
        example = "19:abc@thread.tacv2"
    )]
    pub conversation_id: String,

    #[field(
        display_name = "Text",
        description = "Message text. Optional when an Adaptive Card is provided; used as the accessible fallback.",
        example = "Hello from Runtara!"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[field(
        display_name = "Adaptive Card",
        description = "Optional Adaptive Card JSON (v1.5 desktop / v1.2 mobile). Sent as an attachment; card interactivity (Action.Execute) is not handled."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Value>,

    #[field(
        display_name = "Reply To Activity ID",
        description = "Optional inbound activity id to reply to (data.target.replyToActivityId). Threads the reply where the channel supports it."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_activity_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Send Message Output")]
pub struct SendMessageOutput {
    #[field(display_name = "OK", description = "Whether the send succeeded")]
    pub ok: bool,

    #[field(
        display_name = "Conversation ID",
        description = "The conversation the activity was sent to"
    )]
    pub conversation_id: String,

    #[field(
        display_name = "Activity ID",
        description = "The Bot Connector activity id of the sent message (from ResourceResponse.id), when returned"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_id: Option<String>,
}

#[capability(
    module = "teams",
    display_name = "Send Message",
    description = "Send a message to a Microsoft Teams conversation. Supports plain text and an optional Adaptive Card, and can reply to a specific activity.",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    module_display_name = "Microsoft Teams",
    module_description = "Microsoft Teams messaging via the Bot Framework Connector",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "teams_bot",
    module_secure = true
)]
pub fn send_message(input: SendMessageInput) -> Result<SendMessageOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "TEAMS_MISSING_CONNECTION",
            "TEAMS capability invoked without a connection — add one in the step configuration",
        )
        .with_attr("integration", "TEAMS")
    })?;

    if input.target.trim().is_empty() {
        return Err(AgentError::permanent(
            "TEAMS_MISSING_TARGET",
            "A conversation target reference is required (map data.target.ref from the trigger)",
        )
        .with_attr("integration", "TEAMS"));
    }
    if input.conversation_id.trim().is_empty() {
        return Err(AgentError::permanent(
            "TEAMS_MISSING_CONVERSATION",
            "A conversation id is required (map data.target.conversationId from the trigger)",
        )
        .with_attr("integration", "TEAMS"));
    }
    if input
        .text
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
        && input.card.is_none()
    {
        return Err(AgentError::permanent(
            "TEAMS_EMPTY_MESSAGE",
            "Provide message text, an Adaptive Card, or both",
        )
        .with_attr("integration", "TEAMS"));
    }

    let encoded_conv = percent_encode_path_segment(&input.conversation_id);
    let base_path = match &input.reply_to_activity_id {
        Some(activity_id) if !activity_id.trim().is_empty() => format!(
            "/v3/conversations/{encoded_conv}/activities/{}",
            percent_encode_path_segment(activity_id)
        ),
        _ => format!("/v3/conversations/{encoded_conv}/activities"),
    };

    // A card rides as a single attachment; when both text and card are present
    // the text becomes the message's accessible content.
    let card_attachment = input
        .card
        .as_ref()
        .map(|card| json!([{ "contentType": ADAPTIVE_CARD_CONTENT_TYPE, "content": card }]));

    // Split only the plain-text case; a card is sent once.
    let text = input.text.clone().unwrap_or_default();
    let chunks: Vec<String> = if card_attachment.is_some() || text.trim().is_empty() {
        vec![text.clone()]
    } else {
        split_message(&text, TEXT_CHUNK_CHARS)
    };

    let mut last_activity_id = None;
    for (i, chunk) in chunks.iter().enumerate() {
        let mut activity = json!({ "type": "message" });
        // Signal that the bot accepts further input, so Teams keeps the
        // conversation's compose box enabled after the reply. Without this,
        // Teams can lock input with "You can't send messages to this bot".
        activity["inputHint"] = json!("acceptingInput");
        if !chunk.is_empty() {
            activity["text"] = json!(chunk);
        }
        if let Some(reply_to) = &input.reply_to_activity_id
            && !reply_to.trim().is_empty()
        {
            activity["replyToId"] = json!(reply_to);
        }
        // Attach the card only on the first activity.
        if i == 0
            && let Some(attachments) = &card_attachment
        {
            activity["attachments"] = attachments.clone();
        }

        let resp = bot_connector_post(&base_path, connection, &input.target, &activity)?;
        last_activity_id = resp.activity_id.or(last_activity_id);
    }

    Ok(SendMessageOutput {
        ok: true,
        conversation_id: input.conversation_id.clone(),
        activity_id: last_activity_id,
    })
}

// ============================================================================
// Helpers
// ============================================================================

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Respect char boundaries.
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut t = s[..end].to_string();
        t.push('…');
        t
    }
}

/// Percent-encode a single path segment. Teams conversation ids contain `;`,
/// `@`, `:`, `=` — none of which are safe unencoded in a path segment. The
/// proxy percent-decodes for its conversation-id containment check, so this
/// must be a straightforward RFC 3986 encoding of everything but unreserved.
fn percent_encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len() * 3);
    for &byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

/// Split `text` into chunks of at most `max_chars` characters, never splitting
/// a char. Mirrors the server-side channel adapter's behavior.
fn split_message(text: &str, max_chars: usize) -> Vec<String> {
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for ch in text.chars() {
        current.push(ch);
        count += 1;
        if count >= max_chars {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

// ============================================================================
// AgentInfo assembler (host-only)
// ============================================================================

#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[&__CAPABILITY_META_SEND_MESSAGE];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [(
        "SendMessageInput",
        &__INPUT_META_SendMessageInput as &InputTypeMeta,
    )]
    .into_iter()
    .collect();
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [(
        "SendMessageOutput",
        &__OUTPUT_META_SendMessageOutput as &OutputTypeMeta,
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
        id: "teams".into(),
        name: "Microsoft Teams".into(),
        description: "Microsoft Teams messaging via the Bot Framework Connector".into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["teams_bot".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_teams::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "send-message" => __executor_send_message(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("teams agent has no capability `{other}`"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_info_exposes_send_message() {
        let info = agent_info();
        assert_eq!(info.id, "teams");
        assert_eq!(info.integration_ids, vec!["teams_bot".to_string()]);
        assert_eq!(info.capabilities.len(), 1);
        let cap = &info.capabilities[0];
        assert_eq!(cap.id, "send-message");
        assert!(cap.has_side_effects);
        // _connection is skipped from metadata; user-facing inputs only.
        let names: Vec<&str> = cap.inputs.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "target",
                "conversation_id",
                "text",
                "card",
                "reply_to_activity_id"
            ]
        );
    }

    #[test]
    fn send_message_requires_connection() {
        let err = send_message(SendMessageInput {
            _connection: None,
            target: "ref".into(),
            conversation_id: "19:abc@thread.tacv2".into(),
            text: Some("hi".into()),
            card: None,
            reply_to_activity_id: None,
        })
        .expect_err("connection required");
        assert_eq!(err.code, "TEAMS_MISSING_CONNECTION");
    }

    fn conn() -> RawConnection {
        RawConnection {
            connection_id: "conn-1".into(),
            connection_subtype: None,
            integration_id: "teams_bot".into(),
            parameters: json!({}),
            rate_limit_config: None,
        }
    }

    #[test]
    fn send_message_requires_target_and_conversation() {
        let missing_target = send_message(SendMessageInput {
            _connection: Some(conn()),
            target: "  ".into(),
            conversation_id: "19:abc".into(),
            text: Some("hi".into()),
            card: None,
            reply_to_activity_id: None,
        })
        .expect_err("target required");
        assert_eq!(missing_target.code, "TEAMS_MISSING_TARGET");

        let missing_conv = send_message(SendMessageInput {
            _connection: Some(conn()),
            target: "ref".into(),
            conversation_id: "".into(),
            text: Some("hi".into()),
            card: None,
            reply_to_activity_id: None,
        })
        .expect_err("conversation required");
        assert_eq!(missing_conv.code, "TEAMS_MISSING_CONVERSATION");
    }

    #[test]
    fn send_message_requires_text_or_card() {
        let err = send_message(SendMessageInput {
            _connection: Some(conn()),
            target: "ref".into(),
            conversation_id: "19:abc".into(),
            text: None,
            card: None,
            reply_to_activity_id: None,
        })
        .expect_err("text or card required");
        assert_eq!(err.code, "TEAMS_EMPTY_MESSAGE");
    }

    #[test]
    fn percent_encodes_conversation_id_specials() {
        // ; @ : = must all be encoded so the path segment is unambiguous.
        let encoded = percent_encode_path_segment("19:abc@thread.tacv2;messageid=1");
        assert_eq!(encoded, "19%3Aabc%40thread.tacv2%3Bmessageid%3D1");
        // Round-trips through a standard decoder back to the original.
        let decoded = urlencoding_decode(&encoded);
        assert_eq!(decoded, "19:abc@thread.tacv2;messageid=1");
    }

    /// Minimal percent-decoder for the round-trip assertion above.
    fn urlencoding_decode(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hi = (bytes[i + 1] as char).to_digit(16).unwrap();
                let lo = (bytes[i + 2] as char).to_digit(16).unwrap();
                out.push((hi * 16 + lo) as u8);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn split_message_respects_char_count() {
        let text = "x".repeat(9001);
        let chunks = split_message(&text, 4000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chars().count(), 4000);
        assert_eq!(chunks[2].chars().count(), 1001);
    }

    #[test]
    fn maps_message_writes_blocked_to_target_blocked() {
        let body = json!({ "error": { "code": "MessageWritesBlocked", "message": "blocked" } });
        let raw = serde_json::to_vec(&body).unwrap();
        let err = map_bot_connector_error(403, &HashMap::new(), &raw, Some(body));
        assert_eq!(err.code, "TEAMS_TARGET_BLOCKED");
        assert_eq!(err.category, "permanent");
    }

    #[test]
    fn maps_retryable_status_codes() {
        for status in [429u16, 412, 502, 504, 500] {
            let err = map_bot_connector_error(status, &HashMap::new(), b"{}", Some(json!({})));
            assert_eq!(err.category, "transient", "status {status} should retry");
        }
        for status in [401u16, 403, 404, 400] {
            let err = map_bot_connector_error(status, &HashMap::new(), b"{}", Some(json!({})));
            assert_eq!(err.category, "permanent", "status {status} is permanent");
        }
    }

    #[test]
    fn rate_limited_carries_retry_after() {
        let mut headers = HashMap::new();
        headers.insert("retry-after".to_string(), "3".to_string());
        let err = map_bot_connector_error(429, &headers, b"{}", Some(json!({})));
        assert_eq!(err.code, "TEAMS_RATE_LIMITED");
        assert_eq!(err.retry_after_ms, Some(3000));
    }
}
