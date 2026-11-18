// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What a run is doing, read from the debug events it leaves: model and tool
//! calls, memory, steps. A conversation relays these to its client as
//! `activity` events.

use serde::Serialize;
use serde_json::Value;

/// What a run did, as a conversation relays it
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub(crate) enum Activity {
    /// Memory loaded from previous conversation
    MemoryLoaded {
        message_count: i64,
        messages: Option<Vec<Value>>,
    },
    /// AI Agent is calling an LLM
    LlmStart {
        iteration: u32,
        model: Option<String>,
    },
    /// AI Agent received LLM response
    LlmEnd {
        iteration: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        response_preview: Option<String>,
    },
    /// AI Agent is calling a tool
    ToolCall {
        tool_name: String,
        iteration: u32,
        call_number: u32,
        arguments: Option<Value>,
    },
    /// Tool returned a result
    ToolResult {
        tool_name: String,
        iteration: u32,
        call_number: u32,
        result: Option<Value>,
        duration_ms: Option<u64>,
    },
    /// Execution is waiting for user input (WaitForSignal)
    WaitingForInput {
        signal_id: String,
        tool_name: Option<String>,
        message: String,
        response_schema: Option<Value>,
    },
    /// AI Agent produced a final text response
    Message {
        content: String,
        iterations: u32,
        tool_calls: Option<Vec<Value>>,
    },
    /// Memory saved after conversation
    MemorySaved { message_count: i64, success: bool },
    /// Generic step event (for non-AI-agent steps in the workflow)
    StepStart {
        step_id: String,
        step_name: Option<String>,
        step_type: String,
    },
    /// Step completed
    StepEnd {
        step_id: String,
        step_name: Option<String>,
        step_type: String,
        outputs: Option<Value>,
        duration_ms: Option<u64>,
    },
}

/// Parse a step debug event into chat-friendly events
/// Pull `runtara_<toolset>` out of a synthetic MCP tool step id of the form
/// `<agent>.tool.<toolName>.<counter>`. Returns `None` for step ids that
/// don't match that shape (e.g. regular edge-tool dispatch).
fn extract_tool_name_from_step_id(step_id: &str) -> Option<String> {
    let after = step_id.split_once(".tool.")?.1;
    let tool = match after.rsplit_once('.') {
        Some((name, _counter)) => name,
        None => after,
    };
    if tool.is_empty() {
        None
    } else {
        Some(tool.to_string())
    }
}

pub(crate) fn parse_debug_event(subtype: Option<&str>, payload: &Value) -> Vec<Activity> {
    let step_type = payload
        .get("step_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let step_id = payload
        .get("step_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let step_name = payload
        .get("step_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match subtype {
        Some("step_debug_start") => parse_debug_start(step_type, step_id, step_name, payload),
        Some("step_debug_end") => parse_debug_end(step_type, step_id, step_name, payload),
        Some("external_input_requested") => {
            let signal_id = payload
                .get("signal_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tool_name = payload
                .get("tool_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let message = payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Input required")
                .to_string();
            let response_schema = payload.get("response_schema").cloned();

            vec![Activity::WaitingForInput {
                signal_id,
                tool_name,
                message,
                response_schema,
            }]
        }
        _ => vec![],
    }
}

fn parse_debug_start(
    step_type: &str,
    step_id: &str,
    step_name: Option<String>,
    payload: &Value,
) -> Vec<Activity> {
    match step_type {
        "AiAgentMemoryLoad" => vec![], // Will emit on end
        "AiAgentMemorySave" => {
            // Memory save start has message_count in inputs
            let inputs = payload.get("inputs");
            let message_count = inputs
                .and_then(|i| i.get("message_count"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            vec![Activity::MemorySaved {
                message_count,
                success: true,
            }]
        }
        "AiAgentMemoryCompaction" => vec![], // Internal, skip
        "AiAgentLlmCall" => {
            let inputs = payload.get("inputs");
            let iteration = inputs
                .and_then(|i| i.get("iteration"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let model = inputs
                .and_then(|i| i.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            vec![Activity::LlmStart { iteration, model }]
        }
        "AiAgentToolCall" => {
            let inputs = payload.get("inputs");
            let tool_name = inputs
                .and_then(|i| i.get("tool_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let iteration = inputs
                .and_then(|i| i.get("iteration"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let call_number = inputs
                .and_then(|i| i.get("call_number"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let arguments = inputs.and_then(|i| i.get("arguments")).cloned();

            vec![Activity::ToolCall {
                tool_name,
                iteration,
                call_number,
                arguments,
            }]
        }
        "AiAgent" => vec![], // The overall AI Agent step — skip start, emit on end
        _ => {
            // Generic step start
            vec![Activity::StepStart {
                step_id: step_id.to_string(),
                step_name,
                step_type: step_type.to_string(),
            }]
        }
    }
}

fn parse_debug_end(
    step_type: &str,
    step_id: &str,
    step_name: Option<String>,
    payload: &Value,
) -> Vec<Activity> {
    // Debug events have structure: payload.outputs = { outputs: {actual data}, stepId, stepType }
    // The inner "outputs" contains the step's actual output data
    let outer_outputs = payload.get("outputs");
    let inner_outputs = outer_outputs.and_then(|o| o.get("outputs"));
    let duration_ms = payload.get("duration_ms").and_then(|v| v.as_u64());

    match step_type {
        "AiAgentMemoryLoad" => {
            let message_count = inner_outputs
                .and_then(|o| o.get("message_count"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let messages = inner_outputs
                .and_then(|o| o.get("messages"))
                .and_then(|v| v.as_array())
                .cloned();
            vec![Activity::MemoryLoaded {
                message_count,
                messages,
            }]
        }
        "AiAgentMemorySave" => vec![], // Emitted from start event (has message_count)
        "AiAgentMemoryCompaction" => vec![], // Internal, skip
        "AiAgentLlmCall" => {
            let iteration = inner_outputs
                .and_then(|o| o.get("iteration"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let response_preview = inner_outputs
                .and_then(|o| o.get("response_preview"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            vec![Activity::LlmEnd {
                iteration,
                response_preview,
            }]
        }
        "AiAgentToolCall" => {
            // Step events truncate large outputs into a {_truncated, _preview,
            // _original_size} stub, so `inner_outputs` is often missing. Fall
            // back to data we always have: tool_name lives in the step id
            // (`<agent>.tool.<toolName>.<counter>`) and in step_name as
            // "Tool: <toolName>".
            let tool_name = inner_outputs
                .and_then(|o| o.get("tool_name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| extract_tool_name_from_step_id(step_id))
                .or_else(|| {
                    step_name
                        .as_deref()
                        .and_then(|n| n.strip_prefix("Tool: "))
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "unknown".to_string());
            let iteration = inner_outputs
                .and_then(|o| o.get("iteration"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let call_number = inner_outputs
                .and_then(|o| o.get("call_number"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let result = inner_outputs.and_then(|o| o.get("result")).cloned();

            vec![Activity::ToolResult {
                tool_name,
                iteration,
                call_number,
                result,
                duration_ms,
            }]
        }
        "AiAgent" => {
            // The AI Agent step completed — extract the final response
            // Structure: payload.outputs.outputs = { response, iterations, toolCalls }
            let response = inner_outputs
                .and_then(|o| o.get("response"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let iterations = inner_outputs
                .and_then(|o| o.get("iterations"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let tool_calls = inner_outputs
                .and_then(|o| o.get("toolCalls"))
                .and_then(|v| v.as_array())
                .cloned();

            if !response.is_empty() {
                vec![Activity::Message {
                    content: response,
                    iterations,
                    tool_calls,
                }]
            } else {
                vec![]
            }
        }
        _ => {
            // Generic step end
            vec![Activity::StepEnd {
                step_id: step_id.to_string(),
                step_name,
                step_type: step_type.to_string(),
                outputs: outer_outputs.cloned(),
                duration_ms,
            }]
        }
    }
}
