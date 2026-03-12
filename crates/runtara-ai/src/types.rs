// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared types for AI Agent execution.

use serde::{Deserialize, Serialize};

/// A message in the AI Agent conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    /// Role: "system", "user", "assistant", or "tool"
    pub role: String,
    /// Text content of the message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool call ID (for tool results)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool name (for tool calls/results)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool call arguments (for tool calls)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_arguments: Option<serde_json::Value>,
}

/// A log entry for a single tool call within an AI Agent loop iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallLog {
    /// Iteration number (1-based)
    pub iteration: u32,
    /// Tool name that was called
    pub tool_name: String,
    /// Arguments passed to the tool
    pub arguments: serde_json::Value,
    /// Result returned by the tool
    pub result: serde_json::Value,
    /// Whether the tool call succeeded
    pub success: bool,
}

/// Aggregated token usage across all LLM calls in an AI Agent loop.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregatedUsage {
    /// Total prompt/input tokens
    pub prompt_tokens: u64,
    /// Total completion/output tokens
    pub completion_tokens: u64,
    /// Total tokens (prompt + completion)
    pub total_tokens: u64,
    /// Number of LLM calls made
    pub llm_calls: u32,
}

/// Tool definition for the LLM, derived from edge labels and target step metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name (matches the edge label)
    pub name: String,
    /// Human-readable description of the tool
    pub description: String,
    /// JSON Schema for the tool's parameters
    pub parameters: serde_json::Value,
}
