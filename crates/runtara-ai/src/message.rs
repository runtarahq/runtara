// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Message types for AI Agent conversations.
//!
//! These types mirror the rig message API and serialize to/from
//! a format compatible with OpenAI's chat completion messages.

use crate::OneOrMany;
use serde::{Deserialize, Serialize};

// ================================================================
// Top-level Message enum
// ================================================================

/// A single message in a conversation (user or assistant).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    /// User message (text or tool results).
    User { content: OneOrMany<UserContent> },
    /// Assistant message (text or tool calls).
    Assistant {
        content: OneOrMany<AssistantContent>,
    },
}

impl Message {
    /// Create a user message from a text string.
    pub fn user(text: impl Into<String>) -> Self {
        Message::User {
            content: OneOrMany::one(UserContent::text(text)),
        }
    }

    /// Create an assistant message from a text string.
    pub fn assistant(text: impl Into<String>) -> Self {
        Message::Assistant {
            content: OneOrMany::one(AssistantContent::text(text)),
        }
    }
}

impl From<&str> for Message {
    fn from(text: &str) -> Self {
        Message::user(text)
    }
}

impl From<String> for Message {
    fn from(text: String) -> Self {
        Message::user(text)
    }
}

// ================================================================
// User content
// ================================================================

/// Content inside a user message.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum UserContent {
    /// Plain text.
    Text(Text),
    /// Result from a tool call (sent back to the model).
    #[serde(rename = "tool_result")]
    ToolResult(ToolResult),
}

impl UserContent {
    /// Create text content.
    pub fn text(text: impl Into<String>) -> Self {
        UserContent::Text(text.into().into())
    }

    /// Create tool-result content.
    pub fn tool_result(id: impl Into<String>, content: OneOrMany<ToolResultContent>) -> Self {
        UserContent::ToolResult(ToolResult {
            id: id.into(),
            content,
        })
    }
}

impl From<String> for UserContent {
    fn from(text: String) -> Self {
        UserContent::text(text)
    }
}

// ================================================================
// Assistant content
// ================================================================

/// Content inside an assistant message.
///
/// Uses `#[serde(untagged)]` so that text objects and tool-call objects
/// both deserialize correctly from the serialized format.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AssistantContent {
    /// Plain text response.
    Text(Text),
    /// A tool/function call requested by the model.
    ToolCall(ToolCall),
}

impl AssistantContent {
    /// Create text content.
    pub fn text(text: impl Into<String>) -> Self {
        AssistantContent::Text(text.into().into())
    }

    /// Create text content carrying the provider's signature for it.
    pub fn signed_text(text: impl Into<String>, signature: Option<String>) -> Self {
        AssistantContent::Text(Text {
            text: text.into(),
            signature,
        })
    }

    /// Create a tool-call content.
    pub fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        AssistantContent::ToolCall(ToolCall {
            id: id.into(),
            function: ToolFunction {
                name: name.into(),
                arguments,
            },
            signature: None,
        })
    }

    /// Create a tool-call content carrying the provider's signature for it.
    pub fn signed_tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        signature: Option<String>,
    ) -> Self {
        AssistantContent::ToolCall(ToolCall {
            id: id.into(),
            function: ToolFunction {
                name: name.into(),
                arguments,
            },
            signature,
        })
    }
}

impl From<String> for AssistantContent {
    fn from(text: String) -> Self {
        AssistantContent::text(text)
    }
}

// ================================================================
// Leaf types
// ================================================================

/// Basic text content.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Text {
    pub text: String,
    /// Opaque value the provider wants back when this text is replayed in later
    /// turns (Gemini's `thoughtSignature`). Absent for other providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl From<String> for Text {
    fn from(text: String) -> Self {
        Text {
            text,
            signature: None,
        }
    }
}

impl From<&str> for Text {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

/// The assistant's whole answer in `content`, or `None` when it has no text.
/// A provider can split one answer over several text items.
pub fn assistant_text<'a>(
    content: impl IntoIterator<Item = &'a AssistantContent>,
) -> Option<String> {
    let mut answer = None;
    for item in content {
        if let AssistantContent::Text(text) = item {
            answer.get_or_insert_with(String::new).push_str(&text.text);
        }
    }
    answer
}

/// A tool/function call from the assistant.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub function: ToolFunction,
    /// Opaque value the provider requires back when this call is replayed in
    /// later turns (Gemini's `thoughtSignature`). Absent for other providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// The function part of a tool call.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolFunction {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool result sent back to the model.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub id: String,
    pub content: OneOrMany<ToolResultContent>,
}

/// Individual piece of content inside a tool result.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolResultContent {
    Text(Text),
}

impl ToolResultContent {
    /// Create text tool-result content.
    pub fn text(text: impl Into<String>) -> Self {
        ToolResultContent::Text(text.into().into())
    }
}

impl From<String> for ToolResultContent {
    fn from(text: String) -> Self {
        ToolResultContent::text(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OneOrMany;

    #[test]
    fn user_text_roundtrip() {
        let msg = Message::user("hello");
        let json = serde_json::to_value(&msg).unwrap();
        let msg2: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg, msg2);
    }

    #[test]
    fn assistant_text_roundtrip() {
        let msg = Message::assistant("world");
        let json = serde_json::to_value(&msg).unwrap();
        let msg2: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg, msg2);
    }

    #[test]
    fn assistant_tool_call_roundtrip() {
        let msg = Message::Assistant {
            content: OneOrMany::one(AssistantContent::tool_call(
                "call_123",
                "my_func",
                serde_json::json!({"x": 1}),
            )),
        };
        let json = serde_json::to_value(&msg).unwrap();
        let msg2: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg, msg2);
    }

    #[test]
    fn signed_tool_call_keeps_its_signature_across_a_roundtrip() {
        let msg = Message::Assistant {
            content: OneOrMany::one(AssistantContent::signed_tool_call(
                "call_1",
                "search",
                serde_json::json!({"q": "test"}),
                Some("c2lnbmF0dXJl".to_string()),
            )),
        };
        let json = serde_json::to_value(&msg).unwrap();
        let msg2: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg, msg2);
    }

    #[test]
    fn signed_text_keeps_its_signature_across_a_roundtrip() {
        let msg = Message::Assistant {
            content: OneOrMany::one(AssistantContent::signed_text(
                "answer",
                Some("c2lnbmF0dXJl".to_string()),
            )),
        };
        let json = serde_json::to_value(&msg).unwrap();
        let msg2: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg, msg2);
    }

    #[test]
    fn unsigned_text_serializes_without_a_signature_field() {
        let json = serde_json::to_value(Message::assistant("hello")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"role": "assistant", "content": [{"text": "hello"}]})
        );
    }

    #[test]
    fn assistant_text_joins_every_text_item_in_order() {
        let content = [
            AssistantContent::text("Hello"),
            AssistantContent::tool_call("a", "f", serde_json::json!({})),
            AssistantContent::signed_text(" world", Some("sig".to_string())),
        ];
        assert_eq!(assistant_text(&content).as_deref(), Some("Hello world"));
        assert_eq!(assistant_text(&content[1..2]), None);
    }

    #[test]
    fn history_saved_before_signatures_existed_still_loads() {
        let saved = serde_json::json!({
            "role": "assistant",
            "content": [{"id": "call_1", "function": {"name": "f", "arguments": {}}}]
        });
        let msg: Message = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(
            msg,
            Message::Assistant {
                content: OneOrMany::one(AssistantContent::tool_call(
                    "call_1",
                    "f",
                    serde_json::json!({}),
                )),
            }
        );
        assert_eq!(serde_json::to_value(&msg).unwrap(), saved);
    }

    #[test]
    fn tool_result_roundtrip() {
        let msg = Message::User {
            content: OneOrMany::one(UserContent::tool_result(
                "call_123",
                OneOrMany::one(ToolResultContent::text("result data")),
            )),
        };
        let json = serde_json::to_value(&msg).unwrap();
        let msg2: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg, msg2);
    }

    #[test]
    fn mixed_assistant_content() {
        let msg = Message::Assistant {
            content: OneOrMany::many(vec![
                AssistantContent::text("thinking..."),
                AssistantContent::tool_call("call_1", "search", serde_json::json!({"q": "test"})),
            ])
            .unwrap(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        let msg2: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg, msg2);
    }
}
