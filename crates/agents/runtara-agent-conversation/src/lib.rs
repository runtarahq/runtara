// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conversation agent: a workflow says something to whoever it is talking
//! with, reads what was said, and talks to another workflow.
//!
//! `reply` leaves one event on the calling run (`conversation_reply`), over
//! the workflow API, and nothing else: the conversation that carries the
//! run reads it and tells its user. `history` asks the host for what was
//! said in the conversation the calling run carries, which the host finds
//! from the run and not from anything the guest says. `send` asks the host
//! to send a message into a conversation with a workflow of the run's
//! tenant: how an adapter's inbound workflow hands a platform's message
//! over. The agent knows no chat platform. It holds no credential and
//! reaches no network.
//!
//! A step that ran is not run again when its run is resumed, so a reply is
//! said once; a step retried after it failed part-way may say it again.

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The kind of the event `reply` leaves, which the conversation reads.
pub const REPLY_EVENT: &str = "conversation_reply";

/// The most text one reply carries.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
/// The most structured data one reply carries.
pub const MAX_DATA_BYTES: usize = 64 * 1024;
/// The longest id a reply is given.
pub const MAX_ID_BYTES: usize = 128;
/// The most text one piece of a reply carries.
pub const MAX_PIECE_BYTES: usize = 8 * 1024;
/// The most attachments one reply carries.
pub const MAX_ATTACHMENTS: usize = 16;

fn failed(code: &str, category: &str, message: impl Into<String>) -> String {
    json!({
        "code": code,
        "message": message.into(),
        "category": category,
        "severity": if category == "transient" { "warning" } else { "error" },
    })
    .to_string()
}

/// Something sent with a reply: where to fetch it, not it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub url: String,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Reply Input")]
pub struct ReplyInput {
    #[field(
        display_name = "Text",
        description = "What to tell the user",
        example = "Your order has shipped."
    )]
    #[serde(default)]
    pub text: Option<String>,
    #[field(
        display_name = "Attachments",
        description = "Files sent with the reply, each {name, contentType?, url}: where to fetch it"
    )]
    #[serde(default)]
    pub attachments: Option<Vec<Value>>,
    #[field(
        display_name = "Data",
        description = "Structured data sent beside the text, for a client that acts on it and does not read prose"
    )]
    #[serde(default)]
    pub data: Option<Value>,
    #[field(
        display_name = "Format",
        description = "What the text is written in: markdown (the default) or plain",
        example = "markdown"
    )]
    #[serde(default)]
    pub format: Option<String>,
    #[field(
        display_name = "Reply ID",
        description = "Your own id for this reply. A reply with an id is said once in a run, however often the step that says it is run",
        example = "order-shipped"
    )]
    #[serde(default)]
    pub reply_id: Option<String>,
    #[field(
        display_name = "Partial",
        description = "Say this text as a piece of the reply Reply ID names, shown as it is written and not kept. Say the whole reply under the same id, without Partial, to finish it"
    )]
    #[serde(default)]
    pub partial: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Reply Output")]
pub struct ReplyOutput {
    #[field(
        display_name = "Said",
        description = "True once the reply is left for the conversation"
    )]
    pub said: bool,
}

/// What the event carries, once the input is one a reply may be.
fn reply_event(input: ReplyInput) -> Result<Value, String> {
    let text = input.text.filter(|text| !text.trim().is_empty());
    if text
        .as_ref()
        .is_some_and(|text| text.len() > MAX_TEXT_BYTES)
    {
        return Err(failed(
            "CONVERSATION_INVALID",
            "permanent",
            format!("text is over {MAX_TEXT_BYTES} bytes"),
        ));
    }
    let attachments: Vec<Attachment> = input
        .attachments
        .unwrap_or_default()
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|error| {
            failed(
                "CONVERSATION_INVALID",
                "permanent",
                format!("an attachment is {{name, contentType?, url}}: {error}"),
            )
        })?;
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(failed(
            "CONVERSATION_INVALID",
            "permanent",
            format!("a reply carries at most {MAX_ATTACHMENTS} attachments"),
        ));
    }
    let data = input.data.filter(|data| !data.is_null());
    if data
        .as_ref()
        .is_some_and(|data| data.to_string().len() > MAX_DATA_BYTES)
    {
        return Err(failed(
            "CONVERSATION_INVALID",
            "permanent",
            format!("data is over {MAX_DATA_BYTES} bytes"),
        ));
    }
    if text.is_none() && attachments.is_empty() && data.is_none() {
        return Err(failed(
            "CONVERSATION_INVALID",
            "permanent",
            "a reply carries text, data or an attachment",
        ));
    }
    let format = match input.format.as_deref().map(str::trim) {
        None | Some("") | Some("markdown") => "markdown",
        Some("plain") => "plain",
        Some(other) => {
            return Err(failed(
                "CONVERSATION_INVALID",
                "permanent",
                format!("format is markdown or plain, not {other}"),
            ));
        }
    };
    let reply_id = input.reply_id.filter(|id| !id.trim().is_empty());
    if reply_id.as_ref().is_some_and(|id| id.len() > MAX_ID_BYTES) {
        return Err(failed(
            "CONVERSATION_INVALID",
            "permanent",
            format!("a reply's id is at most {MAX_ID_BYTES} bytes"),
        ));
    }
    let mut event = serde_json::Map::new();
    event.insert("format".into(), json!(format));
    event.insert("attachments".into(), json!(attachments));
    for (name, value) in [
        ("text", json!(text)),
        ("data", json!(data)),
        ("replyId", json!(reply_id)),
    ] {
        if !value.is_null() {
            event.insert(name.into(), value);
        }
    }
    Ok(Value::Object(event))
}

#[capability(
    module = "conversation",
    id = "reply",
    display_name = "Reply",
    description = "Tell the user of this run's conversation something now, without waiting for the run to end. Text, and attachments by where to fetch them.",
    module_display_name = "Conversation",
    module_description = "Say something to whoever a workflow is talking with.",
    module_has_side_effects = true,
    module_supports_connections = false,
    side_effects = true,
    tags = "runtime:requires-run",
    errors(
        permanent("CONVERSATION_INVALID", "The reply carries nothing, or too much"),
        permanent("CONVERSATION_REQUIRES_INSTANCE", "Only works inside a run"),
        transient("CONVERSATION_UNAVAILABLE", "The reply could not be left now"),
    )
)]
pub async fn reply(input: ReplyInput) -> Result<ReplyOutput, String> {
    if input.partial.unwrap_or(false) {
        let piece = piece_request(input)?;
        host::control(Call::Piece(piece)).await?;
        return Ok(ReplyOutput { said: true });
    }
    let event = reply_event(input)?;
    host::say(event).await?;
    Ok(ReplyOutput { said: true })
}

/// What a piece of a reply asks of the host: its reply's id and its text.
fn piece_request(input: ReplyInput) -> Result<Value, String> {
    let invalid = |message: &str| failed("CONVERSATION_INVALID", "permanent", message);
    let reply_id = input
        .reply_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| invalid("a piece of a reply names the reply it is of (Reply ID)"))?;
    if reply_id.len() > MAX_ID_BYTES {
        return Err(invalid("a reply's id is too long"));
    }
    let text = input
        .text
        .filter(|text| !text.is_empty())
        .ok_or_else(|| invalid("a piece of a reply has text"))?;
    if text.len() > MAX_PIECE_BYTES {
        return Err(invalid("a piece of a reply is at most 8 KB"));
    }
    Ok(json!({"replyId": reply_id, "text": text}))
}

#[cfg(target_arch = "wasm32")]
mod host {
    use serde_json::Value;

    pub(super) async fn control(call: super::Call) -> Result<Value, String> {
        use runtara_workflow_api::ControlCall;
        let call = match call {
            super::Call::Send(body) => ControlCall::ConversationSend(body),
            super::Call::History(body) => ControlCall::ConversationHistory(body),
            super::Call::Piece(body) => ControlCall::ConversationDelta(body),
        };
        runtara_workflow_sdk::control(&call, None)
            .await
            .map_err(|error| match error {
                runtara_workflow_sdk::Error::Api(error) => {
                    super::refused(error.code.as_str(), &error.message)
                }
                runtara_workflow_sdk::Error::Transport(message) => {
                    super::refused("unavailable", &message)
                }
            })
    }

    pub(super) async fn say(payload: Value) -> Result<(), String> {
        runtara_workflow_sdk::emit_events(&[runtara_workflow_api::Event {
            kind: super::REPLY_EVENT.to_owned(),
            payload,
        }])
        .await
        .map_err(|error| {
            super::failed(
                "CONVERSATION_UNAVAILABLE",
                "transient",
                format!("the reply was not left: {error:?}"),
            )
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod host {
    use serde_json::Value;

    pub(super) async fn control(_: super::Call) -> Result<Value, String> {
        Err(super::failed(
            "CONVERSATION_REQUIRES_INSTANCE",
            "permanent",
            "a conversation is reached from a run",
        ))
    }

    pub(super) async fn say(_: Value) -> Result<(), String> {
        Err(super::failed(
            "CONVERSATION_REQUIRES_INSTANCE",
            "permanent",
            "a reply is said from a run",
        ))
    }
}

/// What a message into a conversation carries, as `send` is given it.
#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Send Input")]
pub struct SendInput {
    #[field(
        display_name = "Workflow ID",
        description = "The workflow to talk to, in this run's tenant"
    )]
    pub workflow_id: String,
    #[field(
        display_name = "Key",
        description = "The conversation's key: the same key always reaches the same conversation. An adapter uses the platform's own (telegram:<chat id>)",
        example = "telegram:42"
    )]
    pub key: String,
    #[field(
        display_name = "Message ID",
        description = "The sender's own id for the message, such as the platform's. The same id again is the same message, so a retried step sends it once. Left out, one is made, and a retry sends it again"
    )]
    #[serde(default)]
    pub message_id: Option<String>,
    #[field(display_name = "Text", description = "What is said")]
    #[serde(default)]
    pub text: Option<String>,
    #[field(
        display_name = "Payload",
        description = "A structured message, for a run that waits for one"
    )]
    #[serde(default)]
    pub payload: Option<Value>,
    #[field(
        display_name = "Sender",
        description = "Who is speaking: {id, name?}, as the platform says"
    )]
    #[serde(default)]
    pub sender: Option<Value>,
    #[field(
        display_name = "Attachments",
        description = "Files sent with the message, each {name, contentType?, url}"
    )]
    #[serde(default)]
    pub attachments: Option<Vec<Value>>,
    #[field(
        display_name = "In Reply To",
        description = "The requestId of the waiting_for_input this message answers"
    )]
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[field(
        display_name = "Reply Workflow ID",
        description = "Set the workflow started with each reply as the conversation opens: an adapter's outbound workflow"
    )]
    #[serde(default)]
    pub reply_workflow_id: Option<String>,
    #[field(
        display_name = "Metadata",
        description = "Set what is kept about the conversation, an object handed to the reply workflow with each reply: the chat's id on the platform, a thread"
    )]
    #[serde(default)]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Send Output")]
pub struct SendOutput {
    #[field(display_name = "Message ID", description = "The message's id")]
    pub message_id: String,
    #[field(
        display_name = "State",
        description = "queued, started, delivered or failed"
    )]
    pub state: String,
    #[field(
        display_name = "Instance ID",
        description = "The run it started or answered, if it did"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[field(
        display_name = "Duplicate",
        description = "True if this id was sent before, and nothing was done again"
    )]
    pub duplicate: bool,
}

/// What `send` asks of the host, once its input is one a message may be.
fn send_request(input: SendInput) -> Result<Value, String> {
    let invalid = |message: String| failed("CONVERSATION_INVALID", "permanent", message);
    if input.workflow_id.trim().is_empty() || input.key.trim().is_empty() {
        return Err(invalid("a message is sent to a workflow and a key".into()));
    }
    let text = input.text.filter(|text| !text.trim().is_empty());
    let payload = input.payload.filter(|payload| !payload.is_null());
    let attachments = input.attachments.unwrap_or_default();
    if text.is_none() && payload.is_none() && attachments.is_empty() {
        return Err(invalid(
            "a message carries text, a payload or an attachment".into(),
        ));
    }
    let mut message = serde_json::Map::new();
    message.insert(
        "id".into(),
        json!(
            input
                .message_id
                .filter(|id| !id.trim().is_empty())
                .unwrap_or_else(made_id)
        ),
    );
    for (name, value) in [
        ("text", json!(text)),
        ("payload", json!(payload)),
        ("sender", json!(input.sender)),
        ("inReplyTo", json!(input.in_reply_to)),
    ] {
        if !value.is_null() {
            message.insert(name.into(), value);
        }
    }
    if !attachments.is_empty() {
        message.insert("attachments".into(), json!(attachments));
    }
    let mut request = serde_json::Map::new();
    request.insert("workflowId".into(), json!(input.workflow_id));
    request.insert("key".into(), json!(input.key));
    request.insert("message".into(), Value::Object(message));
    if let Some(route) = input.reply_workflow_id.filter(|route| !route.is_empty()) {
        request.insert("replyWorkflowId".into(), json!(route));
    }
    if let Some(metadata) = input.metadata.filter(|metadata| !metadata.is_null()) {
        if !metadata.is_object() {
            return Err(invalid("metadata is an object".into()));
        }
        request.insert("metadata".into(), metadata);
    }
    Ok(Value::Object(request))
}

/// An id for a message its sender gave none: unique, and so not the same
/// message when the step is retried.
fn made_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("sent-{now:x}-{}", COUNT.fetch_add(1, Ordering::Relaxed))
}

/// Send a message into a conversation with a workflow, at any time.
#[capability(
    module = "conversation",
    id = "send",
    display_name = "Send",
    description = "Send a message into a conversation with a workflow of this tenant, opening it if it is new: how an adapter hands a platform's message to the workflow that answers it. Answers what became of the message; it does not wait for the reply, which goes to the conversation's reply workflow.",
    module_display_name = "Conversation",
    module_description = "Say something to whoever a workflow is talking with.",
    module_has_side_effects = true,
    module_supports_connections = false,
    side_effects = true,
    tags = "runtime:requires-run",
    errors(
        permanent("CONVERSATION_INVALID", "Not a message, or no such workflow"),
        transient(
            "CONVERSATION_QUEUE_FULL",
            "The conversation holds as many messages as it may"
        ),
        transient("CONVERSATION_UNAVAILABLE", "The message could not be sent now"),
    )
)]
pub async fn send(input: SendInput) -> Result<SendOutput, String> {
    let request = send_request(input)?;
    let answer = host::control(Call::Send(request)).await?;
    serde_json::from_value(answer).map_err(|error| {
        failed(
            "CONVERSATION_UNAVAILABLE",
            "transient",
            format!("an unexpected answer: {error}"),
        )
    })
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "History Input")]
pub struct HistoryInput {
    #[field(
        display_name = "Limit",
        description = "The last this many things said, up to 100 (20 unless given)",
        example = "20"
    )]
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "History Output")]
pub struct HistoryOutput {
    #[field(
        display_name = "Said",
        description = "Oldest first, each {role: user or assistant, text?, data?, sender?, at}: a transcript to hand a model as it is"
    )]
    pub said: Vec<Value>,
}

/// What was said in this run's conversation.
#[capability(
    module = "conversation",
    id = "history",
    display_name = "History",
    description = "What was said in the conversation this run carries: the last messages and replies, oldest first, each with a role (user or assistant), for a step that answers in context.",
    module_display_name = "Conversation",
    module_description = "Say something to whoever a workflow is talking with.",
    module_has_side_effects = true,
    module_supports_connections = false,
    side_effects = false,
    tags = "runtime:requires-run",
    errors(
        permanent("CONVERSATION_NOT_FOUND", "This run carries no conversation"),
        permanent("CONVERSATION_REQUIRES_INSTANCE", "Only works inside a run"),
        transient("CONVERSATION_UNAVAILABLE", "What was said could not be read now"),
    )
)]
pub async fn history(input: HistoryInput) -> Result<HistoryOutput, String> {
    let request = json!({"limit": input.limit.unwrap_or(20).clamp(1, 100)});
    let answer = host::control(Call::History(request)).await?;
    serde_json::from_value(answer).map_err(|error| {
        failed(
            "CONVERSATION_UNAVAILABLE",
            "transient",
            format!("an unexpected answer: {error}"),
        )
    })
}

/// What the agent asks of the host's conversations. Read only in a run:
/// outside one the host is not asked.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum Call {
    Send(Value),
    History(Value),
    Piece(Value),
}

/// A host refusal, as the agent's error: what the host's code means here.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn refused(code: &str, message: &str) -> String {
    let (code, category) = match code {
        "not-found" => ("CONVERSATION_NOT_FOUND", "permanent"),
        "invalid" | "not-runnable" | "denied" => ("CONVERSATION_INVALID", "permanent"),
        "requires-instance" => ("CONVERSATION_REQUIRES_INSTANCE", "permanent"),
        "capacity" => ("CONVERSATION_QUEUE_FULL", "transient"),
        _ => ("CONVERSATION_UNAVAILABLE", "transient"),
    };
    failed(code, category, message)
}

/// Canonical `AgentInfo` for the sidecar meta.json (host-only).
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{AgentInfo, capability_to_api_with_types};
    use std::collections::HashMap;

    let output_types = HashMap::from([
        ("ReplyOutput", &__OUTPUT_META_ReplyOutput),
        ("SendOutput", &__OUTPUT_META_SendOutput),
        ("HistoryOutput", &__OUTPUT_META_HistoryOutput),
    ]);
    AgentInfo {
        id: "conversation".into(),
        name: "Conversation".into(),
        description: "Say something to whoever a workflow is talking with.".into(),
        has_side_effects: true,
        supports_connections: false,
        integration_ids: vec![],
        capabilities: vec![
            capability_to_api_with_types(
                &__CAPABILITY_META_REPLY,
                Some(&__INPUT_META_ReplyInput),
                Some(&__OUTPUT_META_ReplyOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_SEND,
                Some(&__INPUT_META_SendInput),
                Some(&__OUTPUT_META_SendOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_HISTORY,
                Some(&__INPUT_META_HistoryInput),
                Some(&__OUTPUT_META_HistoryOutput),
                &output_types,
            ),
        ],
    }
}

runtara_agent_macro::agent_component!(
    agent = "conversation",
    capabilities = [reply, send, history],
);

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: Option<&str>, attachments: Option<Vec<Value>>) -> ReplyInput {
        ReplyInput {
            text: text.map(str::to_owned),
            attachments,
            data: None,
            format: None,
            reply_id: None,
            partial: None,
        }
    }

    #[test]
    fn a_reply_is_text_attachments_or_both() {
        assert_eq!(
            reply_event(input(Some("hello"), None)).unwrap(),
            json!({"text": "hello", "format": "markdown", "attachments": []})
        );
        let file = json!({"name": "a.pdf", "contentType": "application/pdf", "url": "https://files.example/a.pdf"});
        let event = reply_event(input(None, Some(vec![file.clone()]))).unwrap();
        assert_eq!(event["text"], Value::Null);
        assert_eq!(event["attachments"], json!([file]));
    }

    #[test]
    fn a_reply_with_nothing_in_it_or_too_much_is_refused() {
        for refused in [
            input(None, None),
            input(Some("   "), Some(vec![])),
            input(Some(&"x".repeat(MAX_TEXT_BYTES + 1)), None),
            input(Some("hi"), Some(vec![json!({"name": "no url"})])),
            input(
                Some("hi"),
                Some(vec![
                    json!({"name": "f", "url": "https://files.example/f"});
                    MAX_ATTACHMENTS + 1
                ]),
            ),
        ] {
            let error: Value = serde_json::from_str(&reply_event(refused).unwrap_err()).unwrap();
            assert_eq!(error["code"], "CONVERSATION_INVALID");
            assert_eq!(error["category"], "permanent");
        }
    }

    #[test]
    fn a_reply_carries_its_id_its_data_and_what_its_text_is_written_in() {
        let event = reply_event(ReplyInput {
            data: Some(json!({"order": 7})),
            format: Some("plain".into()),
            reply_id: Some("shipped".into()),
            ..input(Some("Shipped."), None)
        })
        .unwrap();
        assert_eq!(
            event,
            json!({
                "text": "Shipped.",
                "format": "plain",
                "data": {"order": 7},
                "replyId": "shipped",
                "attachments": [],
            })
        );
        // Data alone is a reply: for a client that reads no prose.
        let data_only = reply_event(ReplyInput {
            data: Some(json!([1, 2])),
            ..input(None, None)
        })
        .unwrap();
        assert_eq!(data_only.get("text"), None);
        assert_eq!(data_only["data"], json!([1, 2]));
        for refused in [
            ReplyInput {
                format: Some("html".into()),
                ..input(Some("hi"), None)
            },
            ReplyInput {
                reply_id: Some("x".repeat(MAX_ID_BYTES + 1)),
                ..input(Some("hi"), None)
            },
            ReplyInput {
                data: Some(json!("x".repeat(MAX_DATA_BYTES))),
                ..input(Some("hi"), None)
            },
        ] {
            let error: Value = serde_json::from_str(&reply_event(refused).unwrap_err()).unwrap();
            assert_eq!(error["code"], "CONVERSATION_INVALID");
        }
    }

    #[test]
    fn a_message_sent_is_what_the_host_takes_and_nothing_more() {
        let input = |value: Value| -> SendInput { serde_json::from_value(value).unwrap() };
        let request = send_request(input(json!({
            "workflowId": "answer", "key": "telegram:42", "messageId": "update-7",
            "text": "hi", "sender": {"id": "u1", "name": "Ann"},
            "replyWorkflowId": "out", "metadata": {"chat": 42},
        })))
        .unwrap();
        assert_eq!(
            request,
            json!({
                "workflowId": "answer", "key": "telegram:42",
                "message": {"id": "update-7", "text": "hi", "sender": {"id": "u1", "name": "Ann"}},
                "replyWorkflowId": "out", "metadata": {"chat": 42},
            })
        );
        // Without an id one is made, and two sends are two messages.
        let made = |_| {
            send_request(input(json!({"workflowId": "w", "key": "k", "text": "x"}))).unwrap()
                ["message"]["id"]
                .clone()
        };
        assert_ne!(made(1), made(2));
        for refused in [
            json!({"workflowId": "w", "key": "k"}),
            json!({"workflowId": " ", "key": "k", "text": "x"}),
            json!({"workflowId": "w", "key": "k", "text": "x", "metadata": [1]}),
        ] {
            let error: Value =
                serde_json::from_str(&send_request(input(refused)).unwrap_err()).unwrap();
            assert_eq!(error["code"], "CONVERSATION_INVALID");
        }
    }

    #[test]
    fn a_piece_of_a_reply_names_its_reply_and_carries_text() {
        let piece = |reply_id: Option<&str>, text: Option<&str>| {
            piece_request(ReplyInput {
                reply_id: reply_id.map(str::to_owned),
                partial: Some(true),
                ..input(text, None)
            })
        };
        assert_eq!(
            piece(Some("answer"), Some("Hel")).unwrap(),
            json!({"replyId": "answer", "text": "Hel"})
        );
        // A leading space is part of the text: pieces are joined as they are.
        assert_eq!(
            piece(Some("answer"), Some(" world")).unwrap()["text"],
            " world"
        );
        for refused in [
            piece(None, Some("x")),
            piece(Some("answer"), None),
            piece(Some("answer"), Some(&"x".repeat(MAX_PIECE_BYTES + 1))),
        ] {
            let error: Value = serde_json::from_str(&refused.unwrap_err()).unwrap();
            assert_eq!(error["code"], "CONVERSATION_INVALID");
        }
    }

    #[test]
    fn a_host_refusal_is_the_agents_own_error() {
        for (host, code, category) in [
            ("not-found", "CONVERSATION_NOT_FOUND", "permanent"),
            ("capacity", "CONVERSATION_QUEUE_FULL", "transient"),
            (
                "requires-instance",
                "CONVERSATION_REQUIRES_INSTANCE",
                "permanent",
            ),
            ("timeout", "CONVERSATION_UNAVAILABLE", "transient"),
        ] {
            let error: Value = serde_json::from_str(&refused(host, "m")).unwrap();
            assert_eq!(
                (error["code"].as_str(), error["category"].as_str()),
                (Some(code), Some(category))
            );
        }
    }

    #[test]
    fn the_agent_describes_its_capabilities() {
        let info = agent_info();
        assert_eq!(info.id, "conversation");
        let ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["reply", "send", "history"]);
        assert!(!info.supports_connections);
    }
}
