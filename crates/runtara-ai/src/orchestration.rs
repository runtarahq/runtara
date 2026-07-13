// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared AI-agent orchestration primitives.
//!
//! [`run_completion`] is the single source of truth for issuing one LLM chat
//! completion in the Ai Agent loop. It backs the `chat-completion` agent
//! capability that the direct-WASM emitter wires into the Ai Agent loop (see
//! `runtara-workflows/src/direct_wasm/compile/ai_agent_loop.rs`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::completion::CompletionResponse;
use crate::message::Message;
use crate::types::ToolDefinition;

/// Parameters for a single LLM chat completion.
///
/// Field set and semantics match the generated `__ai_llm_durable` parameters so
/// the two call sites stay in lockstep.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionInvokeRequest {
    /// Provider integration id (e.g. `"openai"`, `"bedrock"`).
    pub integration_id: String,
    /// Connection parameters JSON (provider-specific).
    #[serde(default)]
    pub conn_params: Value,
    /// Connection id; empty string means "no connection".
    #[serde(default)]
    pub connection_id: String,
    /// Optional model override.
    #[serde(default)]
    pub model_id: Option<String>,
    /// System prompt / preamble.
    #[serde(default)]
    pub system_prompt: String,
    /// User prompt for this turn (empty after the first iteration).
    #[serde(default)]
    pub user_prompt: String,
    /// Prior conversation messages.
    #[serde(default)]
    pub chat_history: Vec<Message>,
    /// Tool definitions advertised to the model.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: f64,
    /// Optional max tokens.
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// Optional structured-output JSON Schema, serialized as a string.
    #[serde(default)]
    pub output_schema_json: Option<String>,
    /// Per-attempt outbound-HTTP timeout for this LLM call, in milliseconds.
    /// `None` resolves to [`runtara_dsl::DEFAULT_STEP_TIMEOUT_MS`].
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Issue one LLM chat completion. Returns the model's response (choice + usage)
/// or a human-readable error string.
///
/// This is intentionally identical in behavior to the generated
/// `__ai_llm_durable` body; keep the two in sync.
pub fn run_completion(req: CompletionInvokeRequest) -> Result<CompletionResponse, String> {
    let conn_id_opt = if req.connection_id.is_empty() {
        None
    } else {
        Some(req.connection_id.as_str())
    };
    let mut model = crate::provider::create_completion_model_with_connection(
        &req.integration_id,
        &req.conn_params,
        req.model_id.as_deref(),
        conn_id_opt,
    )
    .map_err(|e| format!("LLM model creation failed: {e}"))?;

    // Bound the LLM HTTP call at the configured per-attempt timeout (or the
    // shared default). Enforced at the proxy via the serialized `timeout_ms`.
    model.set_timeout(
        req.timeout_ms
            .unwrap_or(runtara_dsl::DEFAULT_STEP_TIMEOUT_MS),
    );

    let mut builder = model
        .completion_request(Message::user(&req.user_prompt))
        .preamble(req.system_prompt.clone())
        .temperature(req.temperature);
    if let Some(mt) = req.max_tokens {
        builder = builder.max_tokens(mt);
    }
    // Inject structured output via additional_params when output_schema is set.
    if let Some(ref schema_str) = req.output_schema_json
        && let Ok(schema_val) = serde_json::from_str::<Value>(schema_str)
        && let Some(params) =
            crate::provider::structured_output_params(&req.integration_id, schema_val)
    {
        builder = builder.additional_params(params);
    }
    for tool in &req.tools {
        builder = builder.tool(tool.clone());
    }
    for msg in &req.chat_history {
        builder = builder.message(msg.clone());
    }

    model
        .completion(builder.build())
        .map_err(|e| format!("LLM call failed: {e}"))
}
