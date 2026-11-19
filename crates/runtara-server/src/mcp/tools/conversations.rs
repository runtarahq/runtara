//! Conversations with a workflow, for an MCP client (`docs/native-chat.md`,
//! `docs/native-chat-phase-4.md`).
//!
//! A client that is itself a model talks to a workflow one tool call at a
//! time: it sends a message and is answered with the message's turn. Each is
//! the conversation's own route, asked in this process.

use rmcp::model::{CallToolResult, ContentBlock};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, encode_path_param, validate_path_param};

/// How long a tool call waits for a turn or an event, unless told otherwise.
const WAIT_SECONDS: u32 = 60;
/// What was said, and what ended a turn: everything but a run's working.
const SAID: &str = "message,reply,waiting_for_input,message_delivered,message_failed,run_ended";

fn json_result(value: Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

fn conversation(workflow_id: &str, key: &str) -> Result<String, rmcp::ErrorData> {
    validate_path_param("workflow_id", workflow_id)?;
    if key.trim().is_empty() {
        return Err(rmcp::ErrorData::invalid_params("key is empty", None));
    }
    Ok(format!(
        "/api/runtime/workflows/{workflow_id}/conversations/{}",
        encode_path_param(key)
    ))
}

/// A turn without a run's working, unless it was asked for.
fn without_activity(mut answer: Value, include_activity: bool) -> Value {
    if !include_activity
        && let Some(events) = answer
            .pointer_mut("/turn/events")
            .and_then(Value::as_array_mut)
    {
        events.retain(|event| event["kind"] != "activity");
    }
    answer
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationSendParams {
    #[schemars(description = "The workflow to talk to")]
    pub workflow_id: String,
    #[schemars(
        description = "Your name for the conversation. The same key always reaches the same conversation; a new key opens a new one."
    )]
    pub key: String,
    #[schemars(description = "What to say")]
    pub text: Option<String>,
    #[schemars(
        description = "A structured message, for a run that waits for one: the answer to a waiting_for_input, conforming to its responseSchema."
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::optional_json_object_schema")]
    pub payload: Option<Value>,
    #[schemars(
        description = "The requestId of the waiting_for_input this message answers. With it, the message answers that request or fails; without it, it answers whatever the run waits for, or starts a run."
    )]
    pub in_reply_to: Option<String>,
    #[schemars(
        description = "Your id for this message. Send the same id again after an uncertain answer: it is the same message and is not sent twice. Left out, one is made."
    )]
    pub message_id: Option<String>,
    #[schemars(
        description = "How many seconds to wait for the turn to be over, up to 120 (60 unless given). 0 sends and does not wait."
    )]
    pub wait_seconds: Option<u32>,
    #[schemars(
        description = "Include what the run did (model and tool calls, steps) among the turn's events. Left out, only what was said."
    )]
    pub include_activity: Option<bool>,
}

/// Send a message into a conversation with a workflow and wait for its turn.
pub async fn conversation_send(
    server: &SmoMcpServer,
    params: ConversationSendParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let path = conversation(&params.workflow_id, &params.key)?;
    if params.text.is_none() && params.payload.is_none() {
        return Err(rmcp::ErrorData::invalid_params(
            "a message carries text or a payload",
            None,
        ));
    }
    let payload = params
        .payload
        .map(|payload| super::internal_api::normalize_json_arg(payload, "payload"))
        .transpose()?;
    let mut message = serde_json::Map::new();
    message.insert(
        "id".into(),
        json!(
            params
                .message_id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        ),
    );
    for (name, value) in [
        ("text", json!(params.text)),
        ("payload", json!(payload)),
        ("inReplyTo", json!(params.in_reply_to)),
    ] {
        if !value.is_null() {
            message.insert(name.into(), value);
        }
    }
    let wait = params.wait_seconds.unwrap_or(WAIT_SECONDS).min(120);
    let answered = api_post(
        server,
        &format!("{path}/messages?wait={wait}"),
        Some(Value::Object(message)),
    )
    .await?;
    let answer = answered.get("data").cloned().unwrap_or(answered);
    json_result(without_activity(
        answer,
        params.include_activity.unwrap_or(false),
    ))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationReadParams {
    #[schemars(description = "The workflow the conversation is with")]
    pub workflow_id: String,
    #[schemars(description = "The conversation's key")]
    pub key: String,
    #[schemars(
        description = "Read the events after this seq (0 or left out for the start). A turn's lastSeq is where to read on from."
    )]
    pub after: Option<i64>,
    #[schemars(
        description = "How many seconds to wait for an event when there is none, up to 25. Left out, answers at once."
    )]
    pub wait_seconds: Option<u32>,
    #[schemars(
        description = "The kinds to read, separated by commas. Left out: what was said and what ended each turn (no activity)."
    )]
    pub kinds: Option<String>,
    #[schemars(description = "At most this many, up to 500 (100 unless given)")]
    pub limit: Option<u32>,
    #[schemars(description = "Newest first, from the end: the last things said")]
    pub newest_first: Option<bool>,
}

/// Read what a conversation has said, and its state.
pub async fn conversation_read(
    server: &SmoMcpServer,
    params: ConversationReadParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let path = conversation(&params.workflow_id, &params.key)?;
    let kinds = params.kinds.unwrap_or_else(|| SAID.to_owned());
    if !kinds
        .chars()
        .all(|c| c.is_ascii_lowercase() || c == '_' || c == ',')
    {
        return Err(rmcp::ErrorData::invalid_params(
            "kinds are names separated by commas",
            None,
        ));
    }
    let mut query = format!(
        "after={}&limit={}&kinds={kinds}",
        params.after.unwrap_or(0).max(0),
        params.limit.unwrap_or(100).clamp(1, 500)
    );
    if params.newest_first.unwrap_or(false) {
        query.push_str("&order=desc");
    } else if let Some(wait) = params.wait_seconds {
        query.push_str(&format!("&wait={}", wait.min(25)));
    }
    let events = api_get(server, &format!("{path}/events?{query}")).await?;
    let state = api_get(server, &path).await?;
    json_result(json!({
        "conversation": state.get("data").cloned().unwrap_or(state),
        "events": events.get("data").cloned().unwrap_or(events),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationListParams {
    #[schemars(description = "The workflow whose conversations to list")]
    pub workflow_id: String,
    #[schemars(description = "Only conversations whose key begins with this")]
    pub prefix: Option<String>,
    #[schemars(description = "A page's `next`, for the page after it")]
    pub cursor: Option<String>,
    #[schemars(description = "At most this many, up to 200 (50 unless given)")]
    pub limit: Option<u32>,
}

/// List a workflow's conversations, most recently active first.
pub async fn conversation_list(
    server: &SmoMcpServer,
    params: ConversationListParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let mut query = format!("limit={}", params.limit.unwrap_or(50).clamp(1, 200));
    for (name, value) in [("prefix", params.prefix), ("cursor", params.cursor)] {
        if let Some(value) = value {
            query.push_str(&format!("&{name}={}", encode_path_param(&value)));
        }
    }
    let listed = api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}/conversations?{query}",
            params.workflow_id
        ),
    )
    .await?;
    json_result(listed.get("data").cloned().unwrap_or(listed))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationStopParams {
    #[schemars(description = "The workflow the conversation is with")]
    pub workflow_id: String,
    #[schemars(description = "The conversation's key")]
    pub key: String,
    #[schemars(
        description = "Also fail the messages waiting their turn. Left out, they keep it and the next one starts the next run."
    )]
    pub clear_queue: Option<bool>,
}

/// Stop the run that carries a conversation.
pub async fn conversation_stop(
    server: &SmoMcpServer,
    params: ConversationStopParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let path = conversation(&params.workflow_id, &params.key)?;
    let stopped = api_post(
        server,
        &format!("{path}/stop"),
        Some(json!({"clearQueue": params.clear_queue.unwrap_or(false)})),
    )
    .await?;
    json_result(stopped.get("data").cloned().unwrap_or(stopped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_what_was_said_unless_the_runs_working_is_asked_for() {
        let answer = json!({
            "state": "started",
            "turn": {"settled": true, "events": [
                {"kind": "message"}, {"kind": "activity"}, {"kind": "reply"}, {"kind": "run_ended"}
            ]},
        });
        let kinds = |answer: Value| -> Vec<String> {
            answer["turn"]["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|event| event["kind"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(
            kinds(without_activity(answer.clone(), false)),
            ["message", "reply", "run_ended"]
        );
        assert_eq!(kinds(without_activity(answer, true)).len(), 4);
        // An answer with no turn in it is left as it is.
        assert_eq!(
            without_activity(json!({"state": "queued"}), false),
            json!({"state": "queued"})
        );
    }

    #[test]
    fn a_conversation_is_named_by_a_workflow_and_a_key_of_any_letters() {
        assert_eq!(
            conversation("wf-1", "telegram:42/a b").unwrap(),
            "/api/runtime/workflows/wf-1/conversations/telegram%3A42%2Fa%20b"
        );
        assert!(conversation("wf/../x", "k").is_err());
        assert!(conversation("wf-1", "  ").is_err());
    }
}
