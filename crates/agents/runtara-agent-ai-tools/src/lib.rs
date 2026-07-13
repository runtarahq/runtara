//! AI Tools integration agent — WebAssembly Component.
//!
//! Provider-router for deterministic AI capabilities (text completion, image
//! generation, vision, embeddings) across multiple LLM providers (OpenAI, AWS
//! Bedrock). Each capability routes from an explicit `provider` input; the
//! runtara HTTP proxy validates that provider against the selected connection,
//! then handles credential injection and base-URL rewriting per provider (OpenAI:
//! `https://api.openai.com`; Bedrock: `https://bedrock-runtime.{region}.amazonaws.com`).
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads the macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_ai_tools.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Capabilities:
//! - `text-completion`   — text generation with optional structured output
//! - `image-generation`  — image generation
//! - `vision-to-text`    — image analysis with optional structured output
//! - `vision-to-image`   — image editing/manipulation
//! - `embed-text`        — vector embedding for one or more strings
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// ============================================================================
// Local AgentError shim
// ============================================================================
//
// The host crate's `runtara_agents::types::AgentError` pulls in `tracing` and
// other host-only baggage. We only need the on-the-wire JSON shape that the
// `#[capability]` macro expects (`Into<String>` returning
// `{"code","message","category","severity",...}`), so we inline a minimal
// version here. Mirrors the shim in `runtara-agent-mailgun`.

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

/// Serialize into the canonical JSON envelope so the `#[capability]` macro
/// executor passes us straight through to `error_string_to_error_info` on the
/// wasm side (which parses the JSON back into a typed `ErrorInfo`).
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================
//
// The host crate's `RawConnection` lives in `runtara-agents` and isn't a
// wasm-compatible dependency. We mirror just the struct so the macro-derived
// executor can deserialize what the wasm Guest::invoke wrapper injects into
// the input JSON under the `_connection` key.

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
// Provider routing
// ============================================================================

const PROVIDER_OPENAI: &str = runtara_ai::provider::PROVIDER_OPENAI;
const PROVIDER_BEDROCK: &str = runtara_ai::provider::PROVIDER_BEDROCK;
const DEFAULT_OPENAI_MODEL: &str = runtara_ai::defaults::DEFAULT_OPENAI_MODEL;
const DEFAULT_OPENAI_MINI_MODEL: &str = runtara_ai::defaults::DEFAULT_OPENAI_MINI_MODEL;
const DEFAULT_BEDROCK_MODEL: &str = runtara_ai::defaults::DEFAULT_BEDROCK_MODEL;
// Only referenced from the host-only `agent_info()`; the wasm component
// resolves integrations at runtime via the connection proxy.
#[cfg(not(target_arch = "wasm32"))]
const INTEGRATION_OPENAI_API_KEY: &str = "openai_api_key";
#[cfg(not(target_arch = "wasm32"))]
const INTEGRATION_AWS_CREDENTIALS: &str = "aws_credentials";

fn require_connection(connection: Option<&RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.ok_or_else(|| {
        AgentError::permanent("AI_TOOLS_MISSING_CONNECTION", "LLM connection is required")
    })
}

fn require_provider(provider: &str) -> Result<&str, AgentError> {
    let provider = provider.trim();
    if provider.is_empty() {
        return Err(AgentError::permanent(
            "AI_TOOLS_MISSING_PROVIDER",
            "LLM provider is required",
        ));
    }
    if runtara_ai::provider::compatible_integration_ids_for_provider(provider).is_none() {
        return Err(unsupported_provider(provider));
    }
    Ok(provider)
}

fn unsupported_provider(provider: &str) -> AgentError {
    AgentError::permanent(
        "AI_TOOLS_UNSUPPORTED_PROVIDER",
        format!("LLM provider not supported: {}", provider),
    )
    .with_attr("provider", provider)
}

// ============================================================================
// Shared types
// ============================================================================

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

// ============================================================================
// OpenAI HTTP helper
// ============================================================================

/// POST `body` to `https://api.openai.com{path}` via the runtara proxy.
fn openai_post(
    connection: &RawConnection,
    path: &str,
    body: Value,
    timeout_ms: u64,
) -> Result<Value, AgentError> {
    let url = format!("https://api.openai.com{path}");
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| AgentError::permanent("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .header("X-Runtara-Ai-Provider", PROVIDER_OPENAI)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "NETWORK_ERROR",
                format!("OpenAI request to {path} failed: {e}"),
            )
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (category, code) = classify_http_status(status);
        let retry_after_ms = parse_retry_after(&response.headers);
        let mut err = if category == "transient" {
            AgentError::transient(
                code,
                format!("OpenAI HTTP {status}: {}", truncate(&body_text, 512)),
            )
        } else {
            AgentError::permanent(
                code,
                format!("OpenAI HTTP {status}: {}", truncate(&body_text, 512)),
            )
        };
        err = err
            .with_attr("status_code", status.to_string())
            .with_attr("path", path)
            .with_attr("body", truncate(&body_text, 512));
        if let Some(ms) = retry_after_ms {
            err = err.with_retry_after_ms(ms);
        }
        return Err(err);
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "RESPONSE_PARSE_ERROR",
            format!("OpenAI response parse error: {e}"),
        )
    })
}

// ============================================================================
// Bedrock HTTP helper
// ============================================================================

/// POST `body` to `https://bedrock-runtime.{region}.amazonaws.com{path}` via
/// the runtara proxy. The proxy injects SigV4 signing and resolves the regional
/// base URL from the aws_credentials connection parameters. We send a relative
/// path so the proxy constructs the regional endpoint (e.g.
/// `https://bedrock-runtime.us-east-1.amazonaws.com`).
fn bedrock_post(
    connection: &RawConnection,
    path: &str,
    body: Value,
    timeout_ms: u64,
) -> Result<Value, AgentError> {
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| AgentError::permanent("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("POST", path)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .header("X-Runtara-Ai-Provider", PROVIDER_BEDROCK)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "NETWORK_ERROR",
                format!("Bedrock request to {path} failed: {e}"),
            )
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (category, code) = classify_http_status(status);
        let retry_after_ms = parse_retry_after(&response.headers);
        let mut err = if category == "transient" {
            AgentError::transient(
                code,
                format!("Bedrock HTTP {status}: {}", truncate(&body_text, 512)),
            )
        } else {
            AgentError::permanent(
                code,
                format!("Bedrock HTTP {status}: {}", truncate(&body_text, 512)),
            )
        };
        err = err
            .with_attr("status_code", status.to_string())
            .with_attr("path", path)
            .with_attr("body", truncate(&body_text, 512));
        if let Some(ms) = retry_after_ms {
            err = err.with_retry_after_ms(ms);
        }
        return Err(err);
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "RESPONSE_PARSE_ERROR",
            format!("Bedrock response parse error: {e}"),
        )
    })
}

// ============================================================================
// Capability 1: Text Completion
// ============================================================================

#[derive(Debug, Default, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "AI Text Completion Input")]
pub struct TextCompletionInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (the UI/runtime fills it from the
    /// configured connection, not from user input).
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Provider",
        description = "LLM provider id (\"openai\" or \"bedrock\"); selects provider behavior explicitly"
    )]
    #[serde(default)]
    pub provider: String,

    #[field(
        display_name = "Prompt",
        description = "The user message or prompt to send to the LLM",
        example = "Explain quantum computing in simple terms"
    )]
    #[serde(default)]
    pub prompt: String,

    #[field(
        display_name = "System Prompt",
        description = "Optional system instructions to set the assistant's behavior",
        example = "You are a helpful assistant"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The model identifier to use (auto-selects based on provider if not specified)",
        example = "gpt-4o"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate in the response",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-2). Higher values increase randomness",
        example = "0.7"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[field(
        display_name = "Top P",
        description = "Nucleus sampling parameter for controlling diversity",
        example = "0.9"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    #[field(
        display_name = "Stop Sequences",
        description = "Sequences where the model will stop generating further tokens",
        example = "[\"END\", \"STOP\"]"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,

    #[field(
        display_name = "Output Schema",
        description = "Optional JSON schema for structured output. When provided, the model returns JSON conforming to this schema.",
        example = "{\"type\": \"object\", \"properties\": {\"name\": {\"type\": \"string\"}}}"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Text Completion Output")]
pub struct TextCompletionOutput {
    #[field(
        display_name = "Text",
        description = "The generated text response from the model"
    )]
    pub text: String,

    #[field(display_name = "Model", description = "The model used for generation")]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,

    #[field(
        display_name = "Finish Reason",
        description = "The reason generation stopped (e.g., 'stop', 'length')"
    )]
    pub finish_reason: String,

    #[field(
        display_name = "Structured Output",
        description = "Parsed JSON output when output_schema was provided"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
}

#[capability(
    module = "ai_tools",
    display_name = "Text Completion",
    description = "Generate text completion using any LLM provider. Supports optional structured output via output_schema.",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    tags = "ai,llm",
    module_display_name = "AI Tools",
    module_description = "AI tools — deterministic AI capabilities for text completion, image generation, structured output, and vision across multiple LLM providers",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "openai_api_key,aws_credentials",
    module_secure = true
)]
pub fn text_completion(input: TextCompletionInput) -> Result<TextCompletionOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;
    match require_provider(&input.provider)? {
        PROVIDER_OPENAI => text_completion_openai(&input, connection),
        PROVIDER_BEDROCK => text_completion_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn text_completion_openai(
    input: &TextCompletionInput,
    connection: &RawConnection,
) -> Result<TextCompletionOutput, AgentError> {
    // If output_schema is provided, use OpenAI structured output path.
    if let Some(ref schema) = input.output_schema {
        return text_completion_openai_structured(input, connection, schema);
    }

    let mut messages = Vec::new();
    if let Some(system) = &input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
    let is_o_series = is_openai_o_series(&model);

    let mut body = json!({
        "model": model,
        "messages": messages,
    });

    if let Some(max_tokens) = input.max_tokens {
        if is_o_series {
            body["max_completion_tokens"] = json!(max_tokens);
        } else {
            body["max_tokens"] = json!(max_tokens);
        }
    }
    if let Some(temperature) = input.temperature
        && !is_o_series
    {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = input.top_p
        && !is_o_series
    {
        body["top_p"] = json!(top_p);
    }
    if let Some(stop) = &input.stop_sequences
        && !is_o_series
    {
        body["stop"] = json!(stop);
    }

    let resp = openai_post(connection, "/v1/chat/completions", body, 120_000)?;
    let text = openai_extract_content(&resp)?;
    let model_used = resp["model"].as_str().unwrap_or("unknown").to_string();
    let finish_reason = resp["choices"][0]["finish_reason"]
        .as_str()
        .unwrap_or("stop")
        .to_string();
    let usage = extract_openai_usage(&resp);

    Ok(TextCompletionOutput {
        text,
        model: model_used,
        usage,
        finish_reason,
        structured_output: None,
    })
}

fn text_completion_openai_structured(
    input: &TextCompletionInput,
    connection: &RawConnection,
    schema: &Value,
) -> Result<TextCompletionOutput, AgentError> {
    let mut messages = Vec::new();
    if let Some(system) = &input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    let mut body = json!({
        "model": input.model.clone().unwrap_or_else(|| DEFAULT_OPENAI_MINI_MODEL.to_string()),
        "messages": messages,
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "structured_response",
                "strict": true,
                "schema": schema
            }
        }
    });
    if let Some(temperature) = input.temperature {
        body["temperature"] = json!(temperature);
    }

    let resp = openai_post(connection, "/v1/chat/completions", body, 120_000)?;
    let content = openai_extract_content(&resp)?;
    let structured_output: Value = serde_json::from_str(&content).map_err(|e| {
        AgentError::permanent(
            "OPENAI_INVALID_RESPONSE",
            format!("Failed to parse structured output: {e}"),
        )
    })?;
    let model_used = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_openai_usage(&resp);
    let text = serde_json::to_string(&structured_output).unwrap_or_default();

    Ok(TextCompletionOutput {
        text,
        model: model_used,
        usage,
        finish_reason: "stop".to_string(),
        structured_output: Some(structured_output),
    })
}

fn text_completion_bedrock(
    input: &TextCompletionInput,
    connection: &RawConnection,
) -> Result<TextCompletionOutput, AgentError> {
    // If output_schema is provided, use prompt-engineering path for structured output.
    if let Some(ref schema) = input.output_schema {
        return text_completion_bedrock_structured(input, connection, schema);
    }

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_BEDROCK_MODEL.to_string());

    let (request_body, is_claude) = build_bedrock_text_request(
        &input.prompt,
        input.system_prompt.as_deref(),
        &model,
        input.max_tokens,
        input.temperature,
        input.top_p,
        input.stop_sequences.as_deref(),
    )?;

    let path = format!("/model/{}/invoke", model);
    let resp = bedrock_post(connection, &path, request_body, 120_000)?;

    let (text, prompt_tokens, completion_tokens, finish_reason) =
        extract_bedrock_text_response(&resp, is_claude)?;

    Ok(TextCompletionOutput {
        text,
        model,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
        finish_reason,
        structured_output: None,
    })
}

fn text_completion_bedrock_structured(
    input: &TextCompletionInput,
    connection: &RawConnection,
    schema: &Value,
) -> Result<TextCompletionOutput, AgentError> {
    let schema_str = serde_json::to_string_pretty(schema)
        .map_err(|e| AgentError::permanent("SERIALIZATION_ERROR", e.to_string()))?;
    let enhanced_prompt = format!(
        "{}\n\nRespond with valid JSON matching this schema:\n{}\n\nReturn ONLY the JSON, no other text.",
        input.prompt, schema_str
    );

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_BEDROCK_MODEL.to_string());
    let (request_body, is_claude) = build_bedrock_text_request(
        &enhanced_prompt,
        input.system_prompt.as_deref(),
        &model,
        Some(input.max_tokens.unwrap_or(4096)),
        input.temperature,
        input.top_p,
        input.stop_sequences.as_deref(),
    )?;

    let path = format!("/model/{}/invoke", model);
    let resp = bedrock_post(connection, &path, request_body, 120_000)?;
    let (text, prompt_tokens, completion_tokens, _finish_reason) =
        extract_bedrock_text_response(&resp, is_claude)?;

    let structured_output: Value = serde_json::from_str(&text).map_err(|e| {
        AgentError::permanent(
            "BEDROCK_INVALID_RESPONSE",
            format!("Failed to parse structured output as JSON: {e}"),
        )
    })?;
    let serialized_text = serde_json::to_string(&structured_output).unwrap_or_default();

    Ok(TextCompletionOutput {
        text: serialized_text,
        model,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
        finish_reason: "stop".to_string(),
        structured_output: Some(structured_output),
    })
}

// ============================================================================
// Capability: Chat Completion (Ai Agent loop primitive)
// ============================================================================
//
// One LLM chat completion turn, the building block of the Ai Agent orchestration
// loop. Unlike `text-completion` (which builds provider JSON directly and
// returns plain text), this capability uses `runtara_ai::run_completion` — the
// exact logic the generated `__ai_llm_durable` runs inline — and returns the
// raw assistant `choice` (which may contain tool calls). This lets the
// direct-WASM emitter run the Ai Agent loop without linking `runtara-ai` into
// every workflow.wasm, while staying byte-identical with the generated path.

#[derive(Debug, Default, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "AI Chat Completion Input")]
pub struct ChatCompletionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Provider",
        description = "LLM provider id (\"openai\" or \"bedrock\"); selects provider behavior explicitly"
    )]
    #[serde(default)]
    pub provider: String,

    #[field(
        display_name = "System Prompt",
        description = "System instructions / preamble for the model"
    )]
    #[serde(default)]
    pub system_prompt: String,

    #[field(
        display_name = "User Prompt",
        description = "The user message for this turn (empty after the first iteration)"
    )]
    #[serde(default)]
    pub user_prompt: String,

    #[field(
        display_name = "Model",
        description = "Model identifier (auto-selected by provider when absent)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (default 0.7)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum tokens to generate"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,

    #[field(
        display_name = "Chat History",
        description = "Prior conversation messages (rig Message JSON)"
    )]
    #[serde(default)]
    pub chat_history: Vec<Value>,

    #[field(
        display_name = "Tools",
        description = "Tool definitions advertised to the model"
    )]
    #[serde(default)]
    pub tools: Vec<Value>,

    #[field(
        display_name = "Output Schema",
        description = "Optional JSON Schema for structured output"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Chat Completion Output")]
pub struct ChatCompletionOutput {
    #[field(
        display_name = "Choice",
        description = "The assistant response content (serialized OneOrMany<AssistantContent>); may contain tool calls"
    )]
    pub choice: Value,

    #[field(
        display_name = "Usage",
        description = "Token usage statistics, when reported"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,

    #[field(
        display_name = "Structured Output",
        description = "Parsed JSON response when an output schema was requested and the model returned valid JSON"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
}

#[capability(
    module = "ai_tools",
    display_name = "Chat Completion",
    description = "Run one LLM chat-completion turn and return the raw assistant choice (with tool calls). Primitive used by the Ai Agent loop.",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    tags = "ai,llm,internal",
    module_display_name = "AI Tools",
    module_description = "AI tools — deterministic AI capabilities for text completion, image generation, structured output, and vision across multiple LLM providers",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "openai_api_key,aws_credentials",
    module_secure = true
)]
pub fn chat_completion(input: ChatCompletionInput) -> Result<ChatCompletionOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;
    let provider = require_provider(&input.provider)?;

    let chat_history = serde_json::from_value::<Vec<runtara_ai::Message>>(Value::Array(
        input.chat_history.clone(),
    ))
    .map_err(|e| {
        AgentError::permanent("AI_CHAT_BAD_HISTORY", format!("invalid chatHistory: {e}"))
    })?;
    let tools = serde_json::from_value::<Vec<runtara_ai::types::ToolDefinition>>(Value::Array(
        input.tools.clone(),
    ))
    .map_err(|e| AgentError::permanent("AI_CHAT_BAD_TOOLS", format!("invalid tools: {e}")))?;

    let req = runtara_ai::CompletionInvokeRequest {
        integration_id: provider.to_string(),
        conn_params: connection.parameters.clone(),
        connection_id: connection.connection_id.clone(),
        model_id: input.model.clone(),
        system_prompt: input.system_prompt.clone(),
        user_prompt: input.user_prompt.clone(),
        chat_history,
        tools,
        temperature: input.temperature.unwrap_or(0.7),
        max_tokens: input.max_tokens.map(|v| v.max(0) as u64),
        output_schema_json: input
            .output_schema
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default()),
    };

    let response = runtara_ai::run_completion(req)
        .map_err(|e| AgentError::transient("AI_CHAT_COMPLETION_FAILED", e))?;

    let choice = serde_json::to_value(&response.choice).map_err(|e| {
        AgentError::permanent(
            "AI_CHAT_BAD_CHOICE",
            format!("choice serialization failed: {e}"),
        )
    })?;
    let usage = response
        .usage
        .as_ref()
        .and_then(|u| serde_json::to_value(u).ok());

    // When an output schema was requested, parse the final assistant text as
    // JSON — mirroring the generated loop's `serde_json::from_str(&text)` with a
    // string fallback. We surface it as `structured_output` so `ai-agent-output`
    // can use it as the response value.
    let structured_output = if input.output_schema.is_some() {
        let final_text = response
            .choice
            .iter()
            .find_map(|content| match content {
                runtara_ai::AssistantContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        serde_json::from_str::<Value>(&final_text).ok()
    } else {
        None
    };

    Ok(ChatCompletionOutput {
        choice,
        usage,
        structured_output,
    })
}

// ============================================================================
// Capability: Ai Agent Turn (tool-loop primitive)
// ============================================================================
//
// One turn of the Ai Agent tool loop. It appends the previous turn's tool
// results to the conversation, runs the LLM, and decides whether the agent is
// done (`complete`) or wants to call tools (`tools`). All conversation state
// management lives here (in Rust) so the direct-WASM emitter's core loop only
// has to: invoke this, dispatch returned tool calls back through the workflow's
// agent invokes, and pass the results into the next turn. Mirrors the generated
// agentic loop body.

#[derive(Debug, Default, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "AI Agent Turn Input")]
pub struct ChatTurnInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Provider",
        description = "LLM provider id (\"openai\" or \"bedrock\")"
    )]
    #[serde(default)]
    pub provider: String,
    #[field(display_name = "Model", description = "Model identifier")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[field(display_name = "Temperature", description = "Sampling temperature")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[field(
        display_name = "Max Tokens",
        description = "Maximum tokens to generate"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    #[field(
        display_name = "Output Schema",
        description = "Optional JSON Schema for structured output"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,

    #[field(display_name = "System Prompt", description = "System instructions")]
    #[serde(default)]
    pub system_prompt: String,
    #[field(
        display_name = "System Prompt Suffix",
        description = "Text appended to the system prompt (e.g. the MCP toolset usage guide)"
    )]
    #[serde(default)]
    pub system_prompt_suffix: String,
    #[field(
        display_name = "User Prompt",
        description = "Original user prompt (sent on the first turn only)"
    )]
    #[serde(default)]
    pub user_prompt: String,
    #[field(
        display_name = "Tools",
        description = "Tool definitions advertised to the model"
    )]
    #[serde(default)]
    pub tools: Vec<Value>,

    #[field(
        display_name = "Chat History",
        description = "Accumulated conversation (rig Message JSON)"
    )]
    #[serde(default)]
    pub chat_history: Vec<Value>,
    #[field(display_name = "Iterations", description = "Turns completed so far")]
    #[serde(default)]
    pub iterations: u32,
    #[field(
        display_name = "Tool Call Log",
        description = "Accumulated tool-call log entries"
    )]
    #[serde(default)]
    pub tool_call_log: Vec<Value>,
    #[field(
        display_name = "Pending Tool Results",
        description = "Results of tools dispatched after the previous turn: [{tool_call_id, content}]"
    )]
    #[serde(default)]
    pub pending_tool_results: Vec<Value>,
}

#[derive(Debug, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Agent Turn Output")]
pub struct ChatTurnOutput {
    #[field(
        display_name = "Action",
        description = "`tools` to dispatch tool calls, or `complete`"
    )]
    pub action: String,
    #[field(display_name = "Chat History", description = "Updated conversation")]
    pub chat_history: Vec<Value>,
    #[field(
        display_name = "Iterations",
        description = "Turns completed including this one"
    )]
    pub iterations: u32,
    #[field(display_name = "Tool Call Log", description = "Updated tool-call log")]
    pub tool_call_log: Vec<Value>,
    #[field(
        display_name = "Tool Calls",
        description = "Tool calls to dispatch when action is `tools`: [{tool_call_id, name, arguments}]"
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<Value>,
    #[field(
        display_name = "Response",
        description = "Final response when action is `complete` (parsed object for structured output, else string)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<Value>,
}

#[capability(
    module = "ai_tools",
    display_name = "Ai Agent Turn",
    description = "Run one turn of the Ai Agent tool loop: append prior tool results, call the LLM, and return either a `complete` response or `tools` to dispatch. Used by the direct emitter's tool loop.",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    tags = "ai,llm,internal",
    module_display_name = "AI Tools",
    module_description = "AI tools — deterministic AI capabilities for text completion, image generation, structured output, and vision across multiple LLM providers",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "openai_api_key,aws_credentials",
    module_secure = true
)]
pub fn chat_turn(input: ChatTurnInput) -> Result<ChatTurnOutput, AgentError> {
    use runtara_ai::OneOrMany;
    use runtara_ai::message::{AssistantContent, Message, ToolResultContent, UserContent};

    let connection = require_connection(input._connection.as_ref())?;
    let provider = require_provider(&input.provider)?;

    let mut history =
        serde_json::from_value::<Vec<Message>>(Value::Array(input.chat_history.clone())).map_err(
            |e| AgentError::permanent("AI_TURN_BAD_HISTORY", format!("invalid chatHistory: {e}")),
        )?;
    let tools = serde_json::from_value::<Vec<runtara_ai::types::ToolDefinition>>(Value::Array(
        input.tools.clone(),
    ))
    .map_err(|e| AgentError::permanent("AI_TURN_BAD_TOOLS", format!("invalid tools: {e}")))?;
    // Tool names captured before `tools` is moved into the completion request,
    // so we can resolve each tool call's name to its index.
    let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

    // Append the previous turn's tool results to the conversation.
    for pending in &input.pending_tool_results {
        let id = pending
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let content = pending
            .get("content")
            .map(|c| match c {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        history.push(Message::User {
            content: OneOrMany::one(UserContent::tool_result(
                id,
                OneOrMany::one(ToolResultContent::text(content)),
            )),
        });
    }

    let iterations = input.iterations + 1;
    let is_first = iterations == 1;

    let req = runtara_ai::CompletionInvokeRequest {
        integration_id: provider.to_string(),
        conn_params: connection.parameters.clone(),
        connection_id: connection.connection_id.clone(),
        model_id: input.model.clone(),
        // The MCP toolset usage guide (if any) is appended to the system prompt,
        // mirroring the generated loop's `system_prompt + mcp_prompt_addition`.
        system_prompt: if input.system_prompt_suffix.is_empty() {
            input.system_prompt.clone()
        } else {
            format!("{}{}", input.system_prompt, input.system_prompt_suffix)
        },
        user_prompt: if is_first {
            input.user_prompt.clone()
        } else {
            String::new()
        },
        chat_history: history.clone(),
        tools,
        temperature: input.temperature.unwrap_or(0.7),
        max_tokens: input.max_tokens.map(|v| v.max(0) as u64),
        output_schema_json: input
            .output_schema
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default()),
    };
    let response = runtara_ai::run_completion(req)
        .map_err(|e| AgentError::transient("AI_TURN_COMPLETION_FAILED", e))?;

    // Record the user message (first turn only) then the assistant message.
    if is_first {
        history.push(Message::user(&input.user_prompt));
    }

    let mut tool_call_log = input.tool_call_log.clone();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut assistant_contents: Vec<AssistantContent> = Vec::new();
    let mut final_text: Option<String> = None;
    for content in response.choice.iter() {
        match content {
            AssistantContent::ToolCall(tc) => {
                assistant_contents.push(AssistantContent::ToolCall(tc.clone()));
                // Resolve the tool name to its index in the advertised tools so
                // the direct loop can dispatch by index (out of range when the
                // model names an unknown tool).
                let tool_index = tool_names
                    .iter()
                    .position(|name| name == &tc.function.name)
                    .unwrap_or(tool_names.len()) as u32;
                tool_calls.push(serde_json::json!({
                    "tool_call_id": tc.id,
                    "name": tc.function.name,
                    "tool_index": tool_index,
                    "arguments": tc.function.arguments,
                }));
                tool_call_log.push(serde_json::json!({
                    "tool_name": tc.function.name,
                    "arguments": tc.function.arguments,
                }));
            }
            AssistantContent::Text(text) => {
                final_text = Some(text.text.clone());
                assistant_contents.push(AssistantContent::Text(text.clone()));
            }
        }
    }
    if let Ok(contents) = OneOrMany::many(assistant_contents) {
        history.push(Message::Assistant { content: contents });
    }

    let chat_history = history
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect::<Vec<_>>();

    if tool_calls.is_empty() {
        // Complete: derive the response value (parsed JSON for structured output).
        let text = final_text.unwrap_or_default();
        let response_value = if input.output_schema.is_some() {
            serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))
        } else {
            Value::String(text)
        };
        Ok(ChatTurnOutput {
            action: "complete".to_string(),
            chat_history,
            iterations,
            tool_call_log,
            tool_calls: Vec::new(),
            response: Some(response_value),
        })
    } else {
        Ok(ChatTurnOutput {
            action: "tools".to_string(),
            chat_history,
            iterations,
            tool_call_log,
            tool_calls,
            response: None,
        })
    }
}

// ============================================================================
// Capability: Summarize Memory (compaction primitive)
// ============================================================================
//
// Summarize-strategy compaction for AiAgent conversation memory. Given the loop
// state (`chat_history` + bookkeeping) and a `max_messages` threshold, it
// summarizes the oldest excess messages via one LLM call and replaces them with
// a single `[Previous conversation summary]: …` user message — mirroring the
// generated path. The conditional ("only when over the threshold") lives here in
// Rust so the direct-WASM loop just invokes it unconditionally before saving.

#[derive(Debug, Default, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Summarize Memory Input")]
pub struct SummarizeMemoryInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Provider",
        description = "LLM provider id (\"openai\" or \"bedrock\")"
    )]
    #[serde(default)]
    pub provider: String,
    #[field(display_name = "Model", description = "Model identifier")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[field(
        display_name = "Max Messages",
        description = "Compaction threshold; messages beyond this are summarized"
    )]
    #[serde(default)]
    pub max_messages: u32,
    #[field(
        display_name = "State",
        description = "AiAgent loop state ({chat_history, iterations, tool_call_log})"
    )]
    #[serde(default)]
    pub state: Value,
}

#[derive(Debug, Default, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Summarize Memory Output")]
pub struct SummarizeMemoryOutput {
    #[field(
        display_name = "State",
        description = "The loop state with the conversation compacted (unchanged below the threshold)"
    )]
    pub state: Value,
}

#[capability(
    module = "ai_tools",
    display_name = "Summarize Memory",
    description = "Summarize-strategy memory compaction: replace the oldest messages beyond max_messages with a single LLM-generated summary. Used by the direct emitter's AiAgent memory path.",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    tags = "ai,llm,internal",
    module_display_name = "AI Tools",
    module_description = "AI tools — deterministic AI capabilities for text completion, image generation, structured output, and vision across multiple LLM providers",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "openai_api_key,aws_credentials",
    module_secure = true
)]
pub fn summarize_memory(input: SummarizeMemoryInput) -> Result<SummarizeMemoryOutput, AgentError> {
    use runtara_ai::message::Message;

    let connection = require_connection(input._connection.as_ref())?;
    let provider = require_provider(&input.provider)?;

    let mut state = input.state.clone();
    let history = state
        .get("chat_history")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let max = input.max_messages as usize;
    // Below the threshold: nothing to compact (and no LLM call), exactly as the
    // generated path skips the summarization branch.
    if history.len() <= max {
        return Ok(SummarizeMemoryOutput { state });
    }

    let excess = history.len() - max;
    let old_json = serde_json::to_string(&history[..excess]).unwrap_or_default();
    let summary_prompt = format!(
        "Summarize the following conversation history concisely, preserving key \
         facts, decisions, and context:\n{old_json}"
    );

    let req = runtara_ai::CompletionInvokeRequest {
        integration_id: provider.to_string(),
        conn_params: connection.parameters.clone(),
        connection_id: connection.connection_id.clone(),
        model_id: input.model.clone(),
        system_prompt: "You are a conversation summarizer. Produce a concise \
                        summary preserving key facts."
            .to_string(),
        user_prompt: summary_prompt,
        chat_history: Vec::new(),
        tools: Vec::new(),
        temperature: 0.3,
        max_tokens: None,
        output_schema_json: None,
    };

    // A summarization failure degrades like the generated path: keep the most
    // recent messages (sliding window) without an inserted summary.
    let summary_text = match runtara_ai::run_completion(req) {
        Ok(response) => response
            .choice
            .iter()
            .find_map(|content| match content {
                runtara_ai::AssistantContent::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "[Summary unavailable]".to_string()),
        Err(_) => "[Summary unavailable]".to_string(),
    };

    // Drop the summarized messages and prepend the summary as a user message,
    // serialized in the same rig Message form chat-turn uses for chat_history.
    let summary_message = serde_json::to_value(Message::user(format!(
        "[Previous conversation summary]: {summary_text}"
    )))
    .unwrap_or(Value::Null);
    let mut compacted: Vec<Value> = Vec::with_capacity(max + 1);
    compacted.push(summary_message);
    compacted.extend_from_slice(&history[excess..]);
    if let Some(obj) = state.as_object_mut() {
        obj.insert("chat_history".to_string(), Value::Array(compacted));
    }

    Ok(SummarizeMemoryOutput { state })
}

// ============================================================================
// Capability 2: Image Generation
// ============================================================================

#[derive(Debug, Default, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "AI Image Generation Input")]
pub struct ImageGenerationInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Provider",
        description = "LLM provider id (\"openai\" or \"bedrock\"); selects provider behavior explicitly"
    )]
    #[serde(default)]
    pub provider: String,

    #[field(
        display_name = "Prompt",
        description = "Text description of the image to generate",
        example = "A serene landscape with mountains at sunset"
    )]
    #[serde(default)]
    pub prompt: String,

    #[field(
        display_name = "Negative Prompt",
        description = "Elements to exclude from the generated image",
        example = "blurry, low quality, distorted"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "Image generation model to use",
        example = "dall-e-3"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Desired image width in pixels",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Desired image height in pixels",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(
        display_name = "Quality",
        description = "Image quality setting (e.g., 'standard', 'hd')",
        example = "hd"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    #[field(
        display_name = "Style",
        description = "Image style preset (e.g., 'vivid', 'natural')",
        example = "vivid"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Image Generation Output")]
pub struct ImageGenerationOutput {
    #[field(display_name = "Image Data", description = "Base64-encoded image data")]
    pub image_data: String,

    #[field(
        display_name = "MIME Type",
        description = "Image format (e.g., 'image/png')"
    )]
    pub mime_type: String,

    #[field(display_name = "Width", description = "Actual image width in pixels")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(display_name = "Height", description = "Actual image height in pixels")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(display_name = "Model", description = "Model used for generation")]
    pub model: String,

    #[field(
        display_name = "Revised Prompt",
        description = "AI-revised prompt if the model modified it"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

#[capability(
    module = "ai_tools",
    display_name = "Image Generation",
    description = "Generate images using AI image generation models",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    tags = "ai,llm"
)]
pub fn image_generation(input: ImageGenerationInput) -> Result<ImageGenerationOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;
    match require_provider(&input.provider)? {
        PROVIDER_OPENAI => image_generation_openai(&input, connection),
        PROVIDER_BEDROCK => image_generation_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn image_generation_openai(
    input: &ImageGenerationInput,
    connection: &RawConnection,
) -> Result<ImageGenerationOutput, AgentError> {
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "dall-e-3".to_string());
    let mut body = json!({
        "model": model,
        "prompt": input.prompt,
        "response_format": "b64_json",
        "n": 1,
    });

    if model == "dall-e-3" {
        if let (Some(w), Some(h)) = (input.width, input.height) {
            body["size"] = json!(format!("{w}x{h}"));
        } else {
            body["size"] = json!("1024x1024");
        }
        if let Some(ref quality) = input.quality {
            body["quality"] = json!(quality);
        }
        if let Some(ref style) = input.style {
            body["style"] = json!(style);
        }
    } else {
        body["size"] = json!("1024x1024");
    }

    if let Some(ref neg) = input.negative_prompt {
        let existing = body["prompt"].as_str().unwrap_or("").to_string();
        body["prompt"] = json!(format!("{existing}. Avoid: {neg}"));
    }

    let resp = openai_post(
        connection,
        "/v1/images/generations",
        body,
        runtara_dsl::DEFAULT_STEP_TIMEOUT_MS,
    )?;

    let image_data = resp["data"][0]["b64_json"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing b64_json in OpenAI response",
            )
        })?
        .to_string();
    let revised_prompt = resp["data"][0]["revised_prompt"]
        .as_str()
        .map(|s| s.to_string());

    Ok(ImageGenerationOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
        revised_prompt,
    })
}

fn image_generation_bedrock(
    input: &ImageGenerationInput,
    connection: &RawConnection,
) -> Result<ImageGenerationOutput, AgentError> {
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "stability.stable-diffusion-xl-v1".to_string());

    let mut text_prompts = vec![json!({"text": input.prompt, "weight": 1.0})];
    if let Some(ref neg) = input.negative_prompt {
        text_prompts.push(json!({"text": neg, "weight": -1.0}));
    }

    let request_body = json!({
        "text_prompts": text_prompts,
        "cfg_scale": 7,
        "seed": 0,
        "steps": 30,
        "width": input.width.unwrap_or(1024),
        "height": input.height.unwrap_or(1024),
    });

    let path = format!("/model/{}/invoke", model);
    let resp = bedrock_post(
        connection,
        &path,
        request_body,
        runtara_dsl::DEFAULT_STEP_TIMEOUT_MS,
    )?;

    let image_data = resp["artifacts"][0]["base64"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "BEDROCK_INVALID_RESPONSE",
                "Missing base64 image in Bedrock response",
            )
        })?
        .to_string();

    Ok(ImageGenerationOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
        revised_prompt: None,
    })
}

// ============================================================================
// Capability 3: Vision to Text
// ============================================================================

#[derive(Debug, Default, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "AI Vision to Text Input")]
pub struct VisionToTextInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Provider",
        description = "LLM provider id (\"openai\" or \"bedrock\"); selects provider behavior explicitly"
    )]
    #[serde(default)]
    pub provider: String,

    #[field(
        display_name = "Prompt",
        description = "Question or instruction about the image",
        example = "Describe what you see in this image"
    )]
    #[serde(default)]
    pub prompt: String,

    #[field(
        display_name = "Image Data",
        description = "Base64-encoded image data (provide either image_data or image_url)",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_data: Option<String>,

    #[field(
        display_name = "Image URL",
        description = "URL of the image to analyze (provide either image_data or image_url)",
        example = "https://example.com/image.png"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,

    #[field(
        display_name = "Model",
        description = "Vision model to use",
        example = "gpt-4o"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature",
        example = "0.7"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[field(
        display_name = "Output Schema",
        description = "Optional JSON schema for structured output. When provided, the model returns JSON conforming to this schema.",
        example = "{\"type\": \"object\", \"properties\": {\"objects\": {\"type\": \"array\"}}}"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Vision to Text Output")]
pub struct VisionToTextOutput {
    #[field(
        display_name = "Text",
        description = "The generated text description or analysis"
    )]
    pub text: String,

    #[field(display_name = "Model", description = "Model used for analysis")]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,

    #[field(
        display_name = "Structured Output",
        description = "Parsed JSON output when output_schema was provided"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
}

#[capability(
    module = "ai_tools",
    display_name = "Vision to Text",
    description = "Analyze images and generate text descriptions. Supports optional structured output via output_schema.",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    tags = "ai,llm"
)]
pub fn vision_to_text(input: VisionToTextInput) -> Result<VisionToTextOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;
    match require_provider(&input.provider)? {
        PROVIDER_OPENAI => vision_to_text_openai(&input, connection),
        PROVIDER_BEDROCK => vision_to_text_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn vision_to_text_openai(
    input: &VisionToTextInput,
    connection: &RawConnection,
) -> Result<VisionToTextOutput, AgentError> {
    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            "Either image_data or image_url is required",
        ));
    }

    let mut content = vec![json!({"type": "text", "text": input.prompt})];
    if let Some(ref url) = input.image_url {
        content.push(json!({"type": "image_url", "image_url": {"url": url}}));
    } else if let Some(ref data) = input.image_data {
        content.push(json!({
            "type": "image_url",
            "image_url": {"url": format!("data:image/png;base64,{data}")}
        }));
    }

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
    let is_o_series = is_openai_o_series(&model);

    let mut body = json!({
        "model": model,
        "messages": [{"role": "user", "content": content}],
    });
    if let Some(max_tokens) = input.max_tokens {
        if is_o_series {
            body["max_completion_tokens"] = json!(max_tokens);
        } else {
            body["max_tokens"] = json!(max_tokens);
        }
    }
    if let Some(temperature) = input.temperature
        && !is_o_series
    {
        body["temperature"] = json!(temperature);
    }

    let resp = openai_post(connection, "/v1/chat/completions", body, 120_000)?;
    let text = openai_extract_content(&resp)?;
    let model_used = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_openai_usage(&resp);
    let structured_output = parse_structured_output(&text, &input.output_schema);

    Ok(VisionToTextOutput {
        text,
        model: model_used,
        usage,
        structured_output,
    })
}

fn vision_to_text_bedrock(
    input: &VisionToTextInput,
    connection: &RawConnection,
) -> Result<VisionToTextOutput, AgentError> {
    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            "Either image_data or image_url is required",
        ));
    }

    // Bedrock vision only supports base64 image data, not URLs.
    if input.image_url.is_some() && input.image_data.is_none() {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            "Bedrock vision requires base64-encoded image_data, not URLs",
        ));
    }

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_BEDROCK_MODEL.to_string());

    // Only Anthropic Claude models support vision in Bedrock.
    if !bedrock_vision_model_supported(&model) {
        return Err(AgentError::permanent(
            "AI_TOOLS_UNSUPPORTED_MODEL",
            "Bedrock vision capabilities require an Anthropic Claude model",
        ));
    }

    let mut content_blocks = Vec::new();
    if let Some(ref data) = input.image_data {
        content_blocks.push(json!({
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": data}
        }));
    }
    content_blocks.push(json!({"type": "text", "text": input.prompt}));

    let mut request_body = json!({
        "messages": [{"role": "user", "content": content_blocks}],
        "max_tokens": input.max_tokens.unwrap_or(1024),
        "anthropic_version": "bedrock-2023-05-31"
    });
    if let Some(temp) = input.temperature {
        request_body["temperature"] = json!(temp);
    }

    let path = format!("/model/{}/invoke", model);
    let resp = bedrock_post(connection, &path, request_body, 120_000)?;

    let text = resp["content"][0]["text"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "BEDROCK_INVALID_RESPONSE",
                "Missing text in Bedrock vision response",
            )
        })?
        .to_string();

    let prompt_tokens = resp["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32;
    let completion_tokens = resp["usage"]["output_tokens"].as_i64().unwrap_or(0) as i32;
    let structured_output = parse_structured_output(&text, &input.output_schema);

    Ok(VisionToTextOutput {
        text,
        model,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
        structured_output,
    })
}

// ============================================================================
// Capability 4: Vision to Image
// ============================================================================

#[derive(Debug, Default, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "AI Vision to Image Input")]
pub struct VisionToImageInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Provider",
        description = "LLM provider id (\"openai\" or \"bedrock\"); selects provider behavior explicitly"
    )]
    #[serde(default)]
    pub provider: String,

    #[field(
        display_name = "Prompt",
        description = "Instructions for how to modify the image",
        example = "Add dramatic lighting to the scene"
    )]
    #[serde(default)]
    pub prompt: String,

    #[field(
        display_name = "Image Data",
        description = "Base64-encoded source image to edit",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    #[serde(default)]
    pub image_data: String,

    #[field(
        display_name = "Mask Data",
        description = "Optional base64-encoded mask for selective editing",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_data: Option<String>,

    #[field(
        display_name = "Model",
        description = "Image editing model to use",
        example = "dall-e-2"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Desired output width in pixels",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Desired output height in pixels",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Vision to Image Output")]
pub struct VisionToImageOutput {
    #[field(
        display_name = "Image Data",
        description = "Base64-encoded modified image"
    )]
    pub image_data: String,

    #[field(
        display_name = "MIME Type",
        description = "Image format (e.g., 'image/png')"
    )]
    pub mime_type: String,

    #[field(display_name = "Width", description = "Actual output width in pixels")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Actual output height in pixels"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(display_name = "Model", description = "Model used for editing")]
    pub model: String,
}

#[capability(
    module = "ai_tools",
    display_name = "Vision to Image",
    description = "Edit and manipulate images using AI models",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    tags = "ai,llm"
)]
pub fn vision_to_image(input: VisionToImageInput) -> Result<VisionToImageOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;
    match require_provider(&input.provider)? {
        PROVIDER_OPENAI => vision_to_image_openai(&input, connection),
        PROVIDER_BEDROCK => vision_to_image_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn vision_to_image_openai(
    input: &VisionToImageInput,
    connection: &RawConnection,
) -> Result<VisionToImageOutput, AgentError> {
    // NOTE: OpenAI images/edits and images/variations endpoints require
    // multipart/form-data with binary PNG payloads. The proxy currently only
    // supports JSON bodies. This sends a JSON body as best-effort; it will
    // likely return 415/400 from OpenAI until proxy-side multipart support lands.
    // TODO: add multipart support to the runtara proxy and update this handler.
    let endpoint = if input.mask_data.is_some() {
        "images/edits"
    } else {
        "images/variations"
    };

    let body = json!({
        "prompt": input.prompt,
        "n": 1,
        "response_format": "b64_json",
        "size": format!("{}x{}", input.width.unwrap_or(1024), input.height.unwrap_or(1024)),
    });

    let resp = openai_post(
        connection,
        &format!("/v1/{endpoint}"),
        body,
        runtara_dsl::DEFAULT_STEP_TIMEOUT_MS,
    )?;

    let image_data = resp["data"][0]["b64_json"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing b64_json in OpenAI response",
            )
        })?
        .to_string();
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "dall-e-2".to_string());

    Ok(VisionToImageOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
    })
}

fn vision_to_image_bedrock(
    input: &VisionToImageInput,
    connection: &RawConnection,
) -> Result<VisionToImageOutput, AgentError> {
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "stability.stable-diffusion-xl-v1".to_string());

    let request_body = json!({
        "text_prompts": [{"text": input.prompt, "weight": 1.0}],
        "init_image": input.image_data,
        "cfg_scale": 7,
        "image_strength": 0.5,
        "steps": 30,
        "width": input.width.unwrap_or(1024),
        "height": input.height.unwrap_or(1024),
    });

    let path = format!("/model/{}/invoke", model);
    let resp = bedrock_post(
        connection,
        &path,
        request_body,
        runtara_dsl::DEFAULT_STEP_TIMEOUT_MS,
    )?;

    let image_data = resp["artifacts"][0]["base64"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "BEDROCK_INVALID_RESPONSE",
                "Missing base64 image in Bedrock response",
            )
        })?
        .to_string();

    Ok(VisionToImageOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
    })
}

// ============================================================================
// Capability 5: Embed Text
// ============================================================================

const AI_EMBED_TEXT_BATCH_CAP: usize = 2048;
const AI_EMBED_TEXT_MAX_DIM: u32 = 4096;

#[derive(Debug, Default, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "AI Embed Text Input")]
pub struct EmbedTextInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Provider",
        description = "LLM provider id (\"openai\" or \"bedrock\"); selects provider behavior explicitly"
    )]
    #[serde(default)]
    pub provider: String,

    #[field(
        display_name = "Texts",
        description = "Batch of input strings to embed. Provider-specific batch limits apply (OpenAI ≤2048; Bedrock Titan loops sequentially).",
        example = "[\"hello\", \"world\"]"
    )]
    #[serde(default)]
    pub texts: Vec<String>,

    #[field(
        display_name = "Model",
        description = "Embedding model override. Defaults: OpenAI = text-embedding-3-small, Bedrock = amazon.titan-embed-text-v2:0",
        example = "text-embedding-3-small"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Dimension",
        description = "Optional output dimension. Must match the target Vector column. Workflow author is responsible for alignment.",
        example = "1536"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Embed Text Output")]
pub struct EmbedTextOutput {
    #[field(
        display_name = "Embeddings",
        description = "One f32 vector per input string, in the same order as the input. Cast to f32 to match pgvector storage."
    )]
    pub embeddings: Vec<Vec<f32>>,

    #[field(
        display_name = "Model",
        description = "The model that produced the embeddings"
    )]
    pub model: String,

    #[field(
        display_name = "Dimension",
        description = "Dimensionality of each returned vector"
    )]
    pub dimension: u32,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,
}

#[capability(
    module = "ai_tools",
    display_name = "Embed Text",
    description = "Generate vector embeddings for one or more strings. Use the result to populate a Vector column for similarity search.",
    side_effects = true,
    idempotent = false,
    rate_limited = true,
    tags = "ai,llm"
)]
pub fn embed_text(input: EmbedTextInput) -> Result<EmbedTextOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;
    let provider = require_provider(&input.provider)?;

    // Validation (mirrors legacy ai_embed_text)
    if input.texts.is_empty() {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            "`texts` must contain at least one entry",
        ));
    }
    if input.texts.iter().any(|t| t.is_empty()) {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            "`texts` entries must be non-empty",
        ));
    }
    if input.texts.len() > AI_EMBED_TEXT_BATCH_CAP {
        return Err(AgentError::permanent(
            "AI_TOOLS_BATCH_TOO_LARGE",
            format!(
                "`texts` batch size {} exceeds cap {}",
                input.texts.len(),
                AI_EMBED_TEXT_BATCH_CAP
            ),
        )
        .with_attr_value("batch", json!(input.texts.len()))
        .with_attr_value("cap", json!(AI_EMBED_TEXT_BATCH_CAP)));
    }
    if let Some(d) = input.dimension
        && (d == 0 || d > AI_EMBED_TEXT_MAX_DIM)
    {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            format!("`dimension` must be in 1..={}", AI_EMBED_TEXT_MAX_DIM),
        ));
    }

    match provider {
        PROVIDER_OPENAI => embed_text_openai(&input, connection),
        PROVIDER_BEDROCK => embed_text_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn embed_text_openai(
    input: &EmbedTextInput,
    connection: &RawConnection,
) -> Result<EmbedTextOutput, AgentError> {
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "text-embedding-3-small".to_string());

    let mut body = json!({
        "model": model,
        "input": input.texts,
    });
    if let Some(dim) = input.dimension {
        body["dimensions"] = json!(dim);
    }

    let resp = openai_post(connection, "/v1/embeddings", body, 60_000)?;

    let data = resp["data"].as_array().ok_or_else(|| {
        AgentError::permanent(
            "OPENAI_INVALID_RESPONSE",
            "Missing data array in OpenAI embeddings response",
        )
    })?;

    let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(data.len());
    for item in data {
        let arr = item["embedding"].as_array().ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing embedding array in OpenAI response item",
            )
        })?;
        let vec: Vec<f32> = arr
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();
        embeddings.push(vec);
    }

    let dimension = embeddings.first().map(|v| v.len() as u32).unwrap_or(0);
    let prompt_tokens = resp["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32;
    let total_tokens = resp["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32;
    let model_used = resp["model"].as_str().unwrap_or(&model).to_string();

    Ok(EmbedTextOutput {
        embeddings,
        model: model_used,
        dimension,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens: 0,
            total_tokens,
        },
    })
}

fn embed_text_bedrock(
    input: &EmbedTextInput,
    connection: &RawConnection,
) -> Result<EmbedTextOutput, AgentError> {
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "amazon.titan-embed-text-v2:0".to_string());

    // Anthropic models do not support embeddings in Bedrock.
    if model.starts_with("anthropic") {
        return Err(AgentError::permanent(
            "AI_TOOLS_UNSUPPORTED_MODEL",
            format!("Bedrock model '{}' does not support embeddings", model),
        ));
    }

    // Titan has no batch endpoint — issue one call per input and accumulate.
    let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(input.texts.len());
    let mut prompt_tokens = 0i32;

    for text in &input.texts {
        let mut body = json!({"inputText": text});
        if let Some(dim) = input.dimension {
            body["dimensions"] = json!(dim);
        }

        let path = format!("/model/{}/invoke", model);
        let resp = bedrock_post(connection, &path, body, 60_000)?;

        let arr = resp["embedding"].as_array().ok_or_else(|| {
            AgentError::permanent(
                "BEDROCK_INVALID_RESPONSE",
                "Bedrock embedding response missing `embedding` array",
            )
        })?;
        let vec: Vec<f32> = arr
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();
        embeddings.push(vec);

        prompt_tokens += resp["inputTextTokenCount"].as_i64().unwrap_or(0) as i32;
    }

    let dimension = embeddings.first().map(|v| v.len() as u32).unwrap_or(0);

    // Verify dimension matches caller's requested dimension.
    if let Some(req_dim) = input.dimension
        && dimension != req_dim
    {
        return Err(AgentError::permanent(
            "BEDROCK_DIMENSION_MISMATCH",
            format!(
                "Requested dimension {} but Bedrock returned {}",
                req_dim, dimension
            ),
        ));
    }

    Ok(EmbedTextOutput {
        embeddings,
        model,
        dimension,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens: 0,
            total_tokens: prompt_tokens,
        },
    })
}

// ============================================================================
// Bedrock request builders
// ============================================================================

/// Build a Bedrock text-generation request body. Returns `(body, is_claude)`.
fn build_bedrock_text_request(
    prompt: &str,
    system_prompt: Option<&str>,
    model: &str,
    max_tokens: Option<i32>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    stop_sequences: Option<&[String]>,
) -> Result<(Value, bool), AgentError> {
    if model.starts_with("anthropic.claude") {
        let messages = vec![json!({"role": "user", "content": prompt})];
        let mut body = json!({
            "messages": messages,
            "max_tokens": max_tokens.unwrap_or(1024),
            "anthropic_version": "bedrock-2023-05-31"
        });
        if let Some(system) = system_prompt {
            body["system"] = json!(system);
        }
        if let Some(temp) = temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(tp) = top_p {
            body["top_p"] = json!(tp);
        }
        if let Some(stop) = stop_sequences {
            body["stop_sequences"] = json!(stop);
        }
        Ok((body, true))
    } else if model.starts_with("amazon.titan") {
        let mut text_config = json!({
            "maxTokenCount": max_tokens.unwrap_or(1024),
        });
        if let Some(temp) = temperature {
            text_config["temperature"] = json!(temp);
        }
        if let Some(tp) = top_p {
            text_config["topP"] = json!(tp);
        }
        if let Some(stop) = stop_sequences {
            text_config["stopSequences"] = json!(stop);
        }
        let full_prompt = match system_prompt {
            Some(sys) => format!("{}\n\n{}", sys, prompt),
            None => prompt.to_string(),
        };
        let body = json!({
            "inputText": full_prompt,
            "textGenerationConfig": text_config
        });
        Ok((body, false))
    } else {
        Err(AgentError::permanent(
            "AI_TOOLS_UNSUPPORTED_MODEL",
            format!("Unsupported Bedrock model: {}", model),
        ))
    }
}

/// Extract (text, prompt_tokens, completion_tokens, finish_reason) from a
/// Bedrock text-generation response.
fn extract_bedrock_text_response(
    resp: &Value,
    is_claude: bool,
) -> Result<(String, i32, i32, String), AgentError> {
    if is_claude {
        let text = resp["content"][0]["text"]
            .as_str()
            .ok_or_else(|| {
                AgentError::permanent(
                    "BEDROCK_INVALID_RESPONSE",
                    "Missing text in Bedrock response",
                )
            })?
            .to_string();
        let prompt_tokens = resp["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32;
        let completion_tokens = resp["usage"]["output_tokens"].as_i64().unwrap_or(0) as i32;
        let finish_reason = resp["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();
        Ok((text, prompt_tokens, completion_tokens, finish_reason))
    } else {
        let text = resp["results"][0]["outputText"]
            .as_str()
            .ok_or_else(|| {
                AgentError::permanent(
                    "BEDROCK_INVALID_RESPONSE",
                    "Missing outputText in Bedrock response",
                )
            })?
            .to_string();
        let prompt_tokens = resp["inputTextTokenCount"].as_i64().unwrap_or(0) as i32;
        let completion_tokens = resp["results"][0]["tokenCount"].as_i64().unwrap_or(0) as i32;
        let finish_reason = resp["results"][0]["completionReason"]
            .as_str()
            .unwrap_or("FINISH")
            .to_string();
        Ok((text, prompt_tokens, completion_tokens, finish_reason))
    }
}

// ============================================================================
// Shared utilities
// ============================================================================

fn extract_openai_usage(resp: &Value) -> LlmUsage {
    LlmUsage {
        prompt_tokens: resp["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32,
        completion_tokens: resp["usage"]["completion_tokens"].as_i64().unwrap_or(0) as i32,
        total_tokens: resp["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    }
}

fn openai_extract_content(resp: &Value) -> Result<String, AgentError> {
    resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
            )
        })
        .map(|s| s.to_string())
}

fn is_openai_o_series(model: &str) -> bool {
    model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4")
}

/// Whether `model` is an Anthropic Claude id — the only family Bedrock vision
/// supports. Broad on purpose: the shipped catalog's Claude 4.x entries aren't
/// tagged with an IMAGE `inputModality`, so gating on modality would reject
/// them too.
fn bedrock_vision_model_supported(model: &str) -> bool {
    model.starts_with("anthropic.claude")
}

/// Try to parse text as JSON when output_schema was provided. Returns None if
/// no schema was provided or if parsing fails.
fn parse_structured_output(text: &str, schema: &Option<Value>) -> Option<Value> {
    schema.as_ref()?;
    serde_json::from_str(text).ok()
}

fn classify_http_status(status: u16) -> (&'static str, &'static str) {
    if status == 429 {
        ("transient", "HTTP_429")
    } else if (500..600).contains(&status) {
        ("transient", "HTTP_5XX")
    } else {
        ("permanent", "HTTP_4XX")
    }
}

fn parse_retry_after(headers: &HashMap<String, String>) -> Option<u64> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after-ms"))
        .and_then(|(_, v)| v.parse::<u64>().ok())
        .or_else(|| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                .and_then(|(_, v)| v.parse::<u64>().ok())
                .map(|s| s * 1000)
        })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push('\u{2026}');
        t
    }
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_TEXT_COMPLETION,
        &__CAPABILITY_META_CHAT_COMPLETION,
        &__CAPABILITY_META_CHAT_TURN,
        &__CAPABILITY_META_SUMMARIZE_MEMORY,
        &__CAPABILITY_META_IMAGE_GENERATION,
        &__CAPABILITY_META_VISION_TO_TEXT,
        &__CAPABILITY_META_VISION_TO_IMAGE,
        &__CAPABILITY_META_EMBED_TEXT,
    ];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "TextCompletionInput",
            &__INPUT_META_TextCompletionInput as &InputTypeMeta,
        ),
        ("ChatCompletionInput", &__INPUT_META_ChatCompletionInput),
        ("ChatTurnInput", &__INPUT_META_ChatTurnInput),
        ("SummarizeMemoryInput", &__INPUT_META_SummarizeMemoryInput),
        ("ImageGenerationInput", &__INPUT_META_ImageGenerationInput),
        ("VisionToTextInput", &__INPUT_META_VisionToTextInput),
        ("VisionToImageInput", &__INPUT_META_VisionToImageInput),
        ("EmbedTextInput", &__INPUT_META_EmbedTextInput),
    ]
    .into_iter()
    .collect();
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "TextCompletionOutput",
            &__OUTPUT_META_TextCompletionOutput as &OutputTypeMeta,
        ),
        ("ChatCompletionOutput", &__OUTPUT_META_ChatCompletionOutput),
        ("ChatTurnOutput", &__OUTPUT_META_ChatTurnOutput),
        (
            "SummarizeMemoryOutput",
            &__OUTPUT_META_SummarizeMemoryOutput,
        ),
        (
            "ImageGenerationOutput",
            &__OUTPUT_META_ImageGenerationOutput,
        ),
        ("VisionToTextOutput", &__OUTPUT_META_VisionToTextOutput),
        ("VisionToImageOutput", &__OUTPUT_META_VisionToImageOutput),
        ("EmbedTextOutput", &__OUTPUT_META_EmbedTextOutput),
    ]
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
        id: "ai-tools".into(),
        name: "AI Tools".into(),
        description: "AI tools — deterministic AI capabilities for text completion, image \
                      generation, structured output, and vision across multiple LLM providers."
            .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec![
            INTEGRATION_OPENAI_API_KEY.to_string(),
            INTEGRATION_AWS_CREDENTIALS.to_string(),
        ],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_ai_tools::capabilities::{ConnectionInfo, ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(
        capability_id: String,
        input: Vec<u8>,
        connection: Option<ConnectionInfo>,
    ) -> Result<Vec<u8>, ErrorInfo> {
        let mut value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        // Inject the WIT `connection` arg into the input JSON under `_connection`
        // so the macro-generated executor can deserialize it into the
        // capability input struct's `_connection: Option<RawConnection>` field.
        if let Some(c) = connection.as_ref() {
            if let serde_json::Value::Object(ref mut obj) = value {
                let parameters = serde_json::from_str::<serde_json::Value>(&c.parameters)
                    .unwrap_or(serde_json::Value::Null);
                let rate_limit_config = c
                    .rate_limit_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
                obj.insert(
                    "_connection".into(),
                    serde_json::json!({
                        "connection_id": c.connection_id,
                        "integration_id": c.integration_id,
                        "connection_subtype": c.connection_subtype,
                        "parameters": parameters,
                        "rate_limit_config": rate_limit_config,
                    }),
                );
            }
        }

        let executor_result = match capability_id.as_str() {
            "text-completion" => __executor_text_completion(value),
            "chat-completion" => __executor_chat_completion(value),
            "chat-turn" => __executor_chat_turn(value),
            "summarize-memory" => __executor_summarize_memory(value),
            "image-generation" => __executor_image_generation(value),
            "vision-to-text" => __executor_vision_to_text(value),
            "vision-to-image" => __executor_vision_to_image(value),
            "embed-text" => __executor_embed_text(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("ai-tools agent has no capability `{other}`"),
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

/// The `#[capability]` macro packages each error as a JSON-string with
/// `{ code, message, category, severity, ... }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
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

    fn fake_connection(integration_id: &str) -> RawConnection {
        RawConnection {
            connection_id: "test-conn".into(),
            connection_subtype: None,
            integration_id: integration_id.into(),
            parameters: serde_json::Value::Null,
            rate_limit_config: None,
        }
    }

    // `#[field(example = ...)]` values are attribute literals and cannot
    // reference consts, so pin the model examples to the shared defaults here
    // (the runtime fallbacks already reference the consts directly).
    #[test]
    fn model_field_examples_match_shared_defaults() {
        use runtara_dsl::agent_meta::InputTypeMeta;

        fn model_example(meta: &'static InputTypeMeta) -> Option<&'static str> {
            meta.fields
                .iter()
                .find(|f| f.name == "model")
                .and_then(|f| f.example)
        }

        assert_eq!(
            model_example(&__INPUT_META_TextCompletionInput),
            Some(DEFAULT_OPENAI_MODEL)
        );
        assert_eq!(
            model_example(&__INPUT_META_VisionToTextInput),
            Some(DEFAULT_OPENAI_MODEL)
        );
    }

    #[test]
    fn embed_text_rejects_missing_connection() {
        let input = EmbedTextInput {
            _connection: None,
            provider: PROVIDER_OPENAI.into(),
            texts: vec!["hi".into()],
            model: None,
            dimension: None,
        };
        let err = embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_MISSING_CONNECTION");
    }

    #[test]
    fn embed_text_rejects_empty_batch() {
        let input = EmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            provider: PROVIDER_OPENAI.into(),
            texts: vec![],
            model: None,
            dimension: None,
        };
        let err = embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_INVALID_INPUT");
        assert!(err.message.contains("at least one"), "{}", err.message);
    }

    #[test]
    fn embed_text_rejects_empty_text_entry() {
        let input = EmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            provider: PROVIDER_OPENAI.into(),
            texts: vec!["ok".into(), String::new()],
            model: None,
            dimension: None,
        };
        let err = embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_INVALID_INPUT");
        assert!(err.message.contains("non-empty"), "{}", err.message);
    }

    #[test]
    fn embed_text_rejects_oversize_dimension() {
        let input = EmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            provider: PROVIDER_OPENAI.into(),
            texts: vec!["x".into()],
            model: None,
            dimension: Some(99_999),
        };
        let err = embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_INVALID_INPUT");
        assert!(err.message.contains("dimension"), "{}", err.message);
    }

    #[test]
    fn embed_text_rejects_zero_dimension() {
        let input = EmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            provider: PROVIDER_OPENAI.into(),
            texts: vec!["x".into()],
            model: None,
            dimension: Some(0),
        };
        let err = embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_INVALID_INPUT");
    }

    #[test]
    fn embed_text_rejects_oversize_batch() {
        let texts = (0..AI_EMBED_TEXT_BATCH_CAP + 1)
            .map(|i| format!("t-{}", i))
            .collect();
        let input = EmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            provider: PROVIDER_OPENAI.into(),
            texts,
            model: None,
            dimension: None,
        };
        let err = embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_BATCH_TOO_LARGE");
    }

    fn vision_input(model: Option<String>) -> VisionToTextInput {
        VisionToTextInput {
            _connection: Some(fake_connection("aws_credentials")),
            provider: PROVIDER_BEDROCK.into(),
            prompt: "describe this".into(),
            image_data: Some("aGVsbG8=".into()),
            image_url: None,
            model,
            max_tokens: None,
            temperature: None,
            output_schema: None,
        }
    }

    #[test]
    fn vision_to_text_bedrock_rejects_non_anthropic_model() {
        let err = vision_to_text(vision_input(Some("qwen.qwen3-32b-v1:0".into()))).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_UNSUPPORTED_MODEL");
    }

    #[test]
    fn bedrock_vision_model_supported_accepts_catalog_claude_4_ids() {
        // Every Anthropic id actually shipped in bedrock_models.generated.json.
        assert!(bedrock_vision_model_supported(
            "anthropic.claude-sonnet-4-6"
        ));
        assert!(bedrock_vision_model_supported(
            "anthropic.claude-opus-4-6-v1"
        ));
        assert!(bedrock_vision_model_supported(
            "anthropic.claude-opus-4-5-20251101-v1:0"
        ));
        assert!(bedrock_vision_model_supported(
            "anthropic.claude-sonnet-4-5-20250929-v1:0"
        ));
        assert!(bedrock_vision_model_supported(
            "anthropic.claude-haiku-4-5-20251001-v1:0"
        ));
        // Old Claude 3.x ids remain supported too.
        assert!(bedrock_vision_model_supported(
            "anthropic.claude-3-5-sonnet-20240620-v1:0"
        ));
    }

    #[test]
    fn bedrock_vision_model_supported_rejects_non_anthropic_ids() {
        assert!(!bedrock_vision_model_supported("qwen.qwen3-32b-v1:0"));
        assert!(!bedrock_vision_model_supported(
            "amazon.titan-embed-text-v2:0"
        ));
    }

    #[test]
    fn vision_to_text_bedrock_default_model_clears_gate() {
        // The fallback default itself must be a catalog-present, gate-passing id.
        assert!(bedrock_vision_model_supported(
            "anthropic.claude-sonnet-4-6"
        ));
    }
}
