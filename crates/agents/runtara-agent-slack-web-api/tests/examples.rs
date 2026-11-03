// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vendor's own response examples (`tests/fixtures`, written by
//! `connector_gen.py fetch`) must deserialize into the generated output types
//! and survive a round trip without losing a field. Slack's spec is Swagger
//! 2.0 with form-encoded parameters; the generated capabilities send them as a
//! JSON body (writes) or a query (reads), and never take the token as an input.

use runtara_agent_slack_web_api::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Paths present in `original` but missing after the typed round trip.
fn lost_paths(original: &Value, round_tripped: &Value, at: &str, lost: &mut Vec<String>) {
    match (original, round_tripped) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let path = format!("{at}.{key}");
                match b.get(key) {
                    Some(other) => lost_paths(value, other, &path, lost),
                    None if !value.is_null() => lost.push(path),
                    None => {}
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                lost_paths(x, y, &format!("{at}[{i}]"), lost);
            }
        }
        _ => {}
    }
}

fn check<T: DeserializeOwned + Serialize>(name: &str) {
    let original = fixture(name);
    let typed: T = serde_json::from_value(original.clone())
        .unwrap_or_else(|e| panic!("{name}: example does not deserialize: {e}"));
    let round_tripped = serde_json::to_value(&typed).unwrap();
    let mut lost = Vec::new();
    lost_paths(&original, &round_tripped, name, &mut lost);
    assert!(
        lost.is_empty(),
        "{name}: fields dropped by the typed output: {lost:?}"
    );
}

#[test]
fn message_examples() {
    check::<ChatPostMessageSuccessSchema>("chat_postMessage");
    check::<ChatUpdateSuccessSchema>("chat_update");
    check::<ChatDeleteSuccessSchema>("chat_delete");
    check::<ChatPostEphemeralSuccessSchema>("chat_postEphemeral");
    // Not the scheduled-message examples: they contradict their own schemas,
    // which match Slack's API (`post_at` is an integer, a scheduled message id a
    // string such as `Q1298393284`).
    check::<ChatDeleteScheduledMessageSchema>("chat_deleteScheduledMessage");
    check::<ChatGetPermalinkSuccessSchema>("chat_getPermalink");
}

#[test]
fn conversation_examples() {
    check::<ConversationsListSuccessSchema>("conversations_list");
    check::<ConversationsInfoSuccessSchema>("conversations_info");
    check::<ConversationsHistorySuccessSchema>("conversations_history");
    check::<ConversationsRepliesSuccessSchema>("conversations_replies");
    check::<ConversationsMembersSuccessSchema>("conversations_members");
    check::<ConversationsCreateSuccessSchema>("conversations_create");
    // Slack's spec titles this success response "error schema".
    check::<ConversationsInviteErrorSchema>("conversations_invite");
    check::<ConversationsKickSuccessSchema>("conversations_kick");
    check::<ConversationsJoinSuccessSchema>("conversations_join");
    check::<ConversationsOpenSuccessSchema>("conversations_open");
    check::<ConversationsRenameSuccessSchema>("conversations_rename");
    check::<ConversationsSetTopicSuccessSchema>("conversations_setTopic");
    check::<ConversationsSetPurposeSuccessSchema>("conversations_setPurpose");
    check::<ConversationsArchiveSuccessSchema>("conversations_archive");
}

#[test]
fn user_reaction_file_and_pin_examples() {
    check::<UsersListSchema>("users_list");
    check::<UsersInfoSuccessSchema>("users_info");
    check::<UsersLookupByEmailSuccessSchema>("users_lookupByEmail");
    check::<ReactionsAddSchema>("reactions_add");
    check::<ReactionsRemoveSchema>("reactions_remove");
    check::<ReactionsGetSuccessSchema>("reactions_get");
    check::<FilesInfoSchema>("files_info");
    check::<FilesListSchema>("files_list");
    check::<FilesDeleteSchema>("files_delete");
    check::<PinsAddSchema>("pins_add");
    check::<PinsRemoveSchema>("pins_remove");
    check::<AuthTestSuccessSchema>("auth_test");
}

#[test]
fn metadata_lists_every_operation() {
    // openapi-generator groups operations by tag, then orders them by operationId.
    let info = runtara_agent_slack_web_api::agent_info();
    assert_eq!(info.id, "slack-web-api");
    assert_eq!(info.integration_ids, ["slack_bot"]);
    let mut ids: Vec<&str> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "auth-test",
            "chat-delete",
            "chat-delete-scheduled-message",
            "chat-get-permalink",
            "chat-post-ephemeral",
            "chat-post-message",
            "chat-schedule-message",
            "chat-scheduled-messages-list",
            "chat-update",
            "conversations-archive",
            "conversations-create",
            "conversations-history",
            "conversations-info",
            "conversations-invite",
            "conversations-join",
            "conversations-kick",
            "conversations-list",
            "conversations-members",
            "conversations-open",
            "conversations-rename",
            "conversations-replies",
            "conversations-set-purpose",
            "conversations-set-topic",
            "files-delete",
            "files-info",
            "files-list",
            "pins-add",
            "pins-remove",
            "reactions-add",
            "reactions-get",
            "reactions-remove",
            "users-info",
            "users-list",
            "users-lookup-by-email",
        ]
    );
    // Reads are Slack's GET methods; everything else changes the workspace.
    for capability in &info.capabilities {
        let read = [
            "auth-test",
            "chat-get-permalink",
            "chat-scheduled-messages-list",
            "conversations-history",
            "conversations-info",
            "conversations-list",
            "conversations-members",
            "conversations-replies",
            "files-info",
            "files-list",
            "reactions-get",
            "users-info",
            "users-list",
            "users-lookup-by-email",
        ]
        .contains(&capability.id.as_str());
        assert_eq!(capability.has_side_effects, !read, "{}", capability.id);
    }
}

#[test]
fn the_token_is_the_connections_not_an_input() {
    let info = runtara_agent_slack_web_api::agent_info();
    for capability in &info.capabilities {
        assert!(
            !capability.inputs.iter().any(|f| f.name == "token"),
            "{} takes a token",
            capability.id
        );
    }
}

#[test]
fn post_message_sends_its_fields_as_json() {
    let input: ChatPostMessageInput = serde_json::from_value(json!({
        "channel": "C0123456789",
        "text": "Deploy finished",
        "thread_ts": "1712345678.000100",
        "unfurl_links": false,
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/api/chat.postMessage");
    assert_eq!(request.query_pairs(), []);
    assert_eq!(
        request.body_json(),
        Some(json!({
            "channel": "C0123456789",
            "text": "Deploy finished",
            "thread_ts": "1712345678.000100",
            "unfurl_links": false,
        }))
    );
    assert!(serde_json::from_value::<ChatPostMessageInput>(json!({ "text": "x" })).is_err());
}

#[test]
fn json_inputs_take_json_types() {
    // Slack's spec keeps its form-encoded API's strings; as JSON, Block Kit is
    // an array, flags are booleans and times are numbers.
    let blocks = json!([{ "type": "section", "text": { "type": "mrkdwn", "text": "*Done*" } }]);
    let input: ChatScheduleMessageInput = serde_json::from_value(json!({
        "channel": "C0123456789",
        "text": "Done",
        "blocks": blocks,
        "link_names": true,
        "post_at": 1793697708,
    }))
    .unwrap();
    assert_eq!(
        input.request().unwrap().body_json(),
        Some(json!({
            "channel": "C0123456789",
            "text": "Done",
            "blocks": blocks,
            "link_names": true,
            "post_at": 1793697708,
        }))
    );
    let input: ChatUpdateInput = serde_json::from_value(json!({
        "channel": "C0123456789", "ts": "1712345678.000100", "as_user": true, "link_names": false,
    }))
    .unwrap();
    assert_eq!(
        input.request().unwrap().body_json().unwrap()["as_user"],
        json!(true)
    );
}

#[test]
fn message_timestamps_are_sent_as_written() {
    // Slack's spec types these as numbers; as one, `…000100` would become `…0001`.
    let input: ChatDeleteInput = serde_json::from_value(json!({
        "channel": "C0123456789",
        "ts": "1712345678.000100",
    }))
    .unwrap();
    assert_eq!(
        input.request().unwrap().body_json(),
        Some(json!({ "channel": "C0123456789", "ts": "1712345678.000100" }))
    );

    let input: ConversationsRepliesInput = serde_json::from_value(json!({
        "channel": "C0123456789",
        "ts": "1712345678.000100",
        "oldest": "1712345600.000000",
    }))
    .unwrap();
    let request = input.request().unwrap();
    assert_eq!(request.method(), "GET");
    assert_eq!(request.path(), "/api/conversations.replies");
    assert_eq!(request.body_json(), None);
    assert_eq!(
        request.query_pairs(),
        [
            ("channel".to_string(), "C0123456789".to_string()),
            ("ts".to_string(), "1712345678.000100".to_string()),
            ("oldest".to_string(), "1712345600.000000".to_string()),
        ]
    );
}

#[test]
fn history_pages_with_a_cursor() {
    let input: ConversationsHistoryInput = serde_json::from_value(json!({
        "channel": "C0123456789",
        "limit": 200,
        "cursor": "dXNlcjpVMDYxTkZUVDI=",
    }))
    .unwrap();
    assert_eq!(
        input.request().unwrap().query_pairs(),
        [
            ("channel".to_string(), "C0123456789".to_string()),
            ("limit".to_string(), "200".to_string()),
            ("cursor".to_string(), "dXNlcjpVMDYxTkZUVDI=".to_string()),
        ]
    );
}

#[test]
fn passthrough_map_is_not_an_output_field() {
    let info = runtara_agent_slack_web_api::agent_info();
    let post = info
        .capabilities
        .iter()
        .find(|c| c.id == "chat-post-message")
        .unwrap();
    let output = serde_json::to_value(&post.output).unwrap();
    assert!(
        !output.to_string().contains("__extra"),
        "__extra leaked into the output metadata: {output}"
    );
    assert!(output.to_string().contains("\"ts\""), "{output}");
}
