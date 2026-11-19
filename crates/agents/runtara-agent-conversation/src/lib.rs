// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conversation agent: a workflow says something to whoever it is talking
//! with.
//!
//! `reply` leaves one event on the calling run (`conversation_reply`), over
//! the workflow API, and nothing else: the conversation that carries the
//! run reads it and tells its user. The agent knows no conversation, key or
//! chat platform. It holds no credential and reaches no network.
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
    let event = reply_event(input)?;
    host::say(event).await?;
    Ok(ReplyOutput { said: true })
}

#[cfg(target_arch = "wasm32")]
mod host {
    use serde_json::Value;

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

    pub(super) async fn say(_: Value) -> Result<(), String> {
        Err(super::failed(
            "CONVERSATION_REQUIRES_INSTANCE",
            "permanent",
            "a reply is said from a run",
        ))
    }
}

/// Canonical `AgentInfo` for the sidecar meta.json (host-only).
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{AgentInfo, capability_to_api_with_types};
    use std::collections::HashMap;

    let output_types = HashMap::from([("ReplyOutput", &__OUTPUT_META_ReplyOutput)]);
    AgentInfo {
        id: "conversation".into(),
        name: "Conversation".into(),
        description: "Say something to whoever a workflow is talking with.".into(),
        has_side_effects: true,
        supports_connections: false,
        integration_ids: vec![],
        capabilities: vec![capability_to_api_with_types(
            &__CAPABILITY_META_REPLY,
            Some(&__INPUT_META_ReplyInput),
            Some(&__OUTPUT_META_ReplyOutput),
            &output_types,
        )],
    }
}

runtara_agent_macro::agent_component!(agent = "conversation", capabilities = [reply],);

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
    fn the_agent_describes_its_one_capability() {
        let info = agent_info();
        assert_eq!(info.id, "conversation");
        assert_eq!(info.capabilities.len(), 1);
        assert_eq!(info.capabilities[0].id, "reply");
        assert!(!info.supports_connections);
    }
}
