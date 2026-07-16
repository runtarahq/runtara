//! OpenAI integration agent — WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_openai.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to attach
//! `Authorization: Bearer <api_key>` from the stored connection — the
//! component never sees secrets.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

// Shared per-provider defaults; the `#[field(default = ...)]` literals below
// must stay in sync with these (pinned by tests, since macro attributes
// cannot reference consts).
const DEFAULT_OPENAI_MODEL: &str = runtara_ai::defaults::DEFAULT_OPENAI_MODEL;
const DEFAULT_OPENAI_MINI_MODEL: &str = runtara_ai::defaults::DEFAULT_OPENAI_MINI_MODEL;

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
// Shared LlmUsage type — mirrors legacy `LlmUsage` (camelCase on the wire).
// ============================================================================
//
// Output struct shared by several capabilities. The `#[capability_output]`
// derive is required so the macro emits an `__OUTPUT_META_LlmUsage` static
// that the host-only `agent_info()` assembler can pick up. (The capability
// fns that return this don't reference it directly via `output_type =
// "LlmUsage"` — they wrap it in their own per-capability output struct.)

#[derive(Debug, Default, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "LLM Usage",
    description = "Token count statistics from LLM API calls"
)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    #[field(
        display_name = "Prompt Tokens",
        description = "Token count for input prompt",
        example = "150"
    )]
    pub prompt_tokens: i32,

    #[field(
        display_name = "Completion Tokens",
        description = "Token count for generated response",
        example = "50"
    )]
    pub completion_tokens: i32,

    #[field(
        display_name = "Total Tokens",
        description = "Combined token count",
        example = "200"
    )]
    pub total_tokens: i32,
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Resolve the connection or return the legacy `OPENAI_MISSING_CONNECTION`
/// error code for wire compatibility.
fn require_connection(connection: Option<&RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.ok_or_else(|| {
        AgentError::permanent("OPENAI_MISSING_CONNECTION", "OpenAI connection is required")
            .with_attr("integration", "OPENAI")
    })
}

/// POST `body` to `https://api.openai.com{path}` via the runtara proxy. The
/// proxy attaches `Authorization: Bearer <api_key>` based on the connection
/// id header so the component never sees the secret.
fn openai_post_json(
    connection: &RawConnection,
    path: &str,
    body: Value,
    timeout_ms: u64,
) -> Result<Value, AgentError> {
    let url = format!("https://api.openai.com{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| {
        AgentError::permanent("SERIALIZATION_ERROR", e.to_string())
            .with_attr("integration", "OPENAI")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "NETWORK_ERROR",
                format!("OpenAI request to {path} failed: {e}"),
            )
            .with_attr("integration", "OPENAI")
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let mut err = if status == 429 {
            AgentError::transient(
                "HTTP_429",
                format!("OpenAI HTTP {status}: {}", truncate(&body_text, 512)),
            )
        } else if (500..600).contains(&status) {
            AgentError::transient(
                "HTTP_5XX",
                format!("OpenAI HTTP {status}: {}", truncate(&body_text, 512)),
            )
        } else {
            AgentError::permanent(
                "HTTP_4XX",
                format!("OpenAI HTTP {status}: {}", truncate(&body_text, 512)),
            )
        };
        err = err
            .with_attr("integration", "OPENAI")
            .with_attr("status_code", status.to_string())
            .with_attr("path", path.to_string())
            .with_attr("body", truncate(&body_text, 512));
        if status == 429 {
            let retry_after_ms = response
                .headers
                .get("retry-after-ms")
                .and_then(|v| v.parse::<u64>().ok())
                .or_else(|| {
                    response
                        .headers
                        .get("retry-after")
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|s| s * 1000)
                });
            if let Some(ms) = retry_after_ms {
                err = err.with_retry_after_ms(ms);
            }
        }
        return Err(err);
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "RESPONSE_PARSE_ERROR",
            format!("OpenAI response parse error: {e}"),
        )
        .with_attr("integration", "OPENAI")
    })
}

fn extract_usage(resp: &Value) -> LlmUsage {
    LlmUsage {
        prompt_tokens: resp["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32,
        completion_tokens: resp["usage"]["completion_tokens"].as_i64().unwrap_or(0) as i32,
        total_tokens: resp["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push('…');
        t
    }
}

fn is_o_series(model: &str) -> bool {
    model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4")
}

// ============================================================================
// Capability 1: Text Completion
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Text Completion Input")]
pub struct TextCompletionInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (the UI/runtime fills it from the
    /// configured connection, not from user input).
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Prompt",
        description = "The user message or prompt to send to the model",
        example = "Explain quantum computing in simple terms"
    )]
    pub prompt: String,

    #[field(
        display_name = "System Prompt",
        description = "Optional system message to set the assistant's behavior and context",
        example = "You are a helpful assistant that explains complex topics simply"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The OpenAI model to use for generation",
        example = "gpt-4o",
        default = "gpt-4o"
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
        description = "Sampling temperature (0-2). Higher values make output more random, lower values more deterministic",
        example = "0.7"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[field(
        display_name = "Top P",
        description = "Nucleus sampling parameter. Only tokens with cumulative probability up to this value are considered",
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
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Text Completion Output")]
pub struct TextCompletionOutput {
    #[field(
        display_name = "Text",
        description = "The generated text response from the model"
    )]
    pub text: String,

    #[field(display_name = "Model", description = "The model used for generation")]
    pub model: String,

    #[field(
        display_name = "Usage",
        description = "Token usage statistics including prompt, completion, and total tokens"
    )]
    pub usage: LlmUsage,

    #[field(
        display_name = "Finish Reason",
        description = "The reason the model stopped generating (e.g., 'stop', 'length', 'content_filter')"
    )]
    pub finish_reason: String,
}

#[capability(
    module = "openai",
    display_name = "Text Completion (OpenAI)",
    description = "Generate text completion using OpenAI models",
    module_display_name = "OpenAI",
    module_description = "OpenAI LLM integration for text completion, image generation, structured output, and vision capabilities.",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "openai_api_key",
    module_secure = true
)]
pub fn text_completion(input: TextCompletionInput) -> Result<TextCompletionOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let mut messages = Vec::new();
    if let Some(system) = &input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    let model = input
        .model
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
    let o_series = is_o_series(&model);

    let mut body = json!({
        "model": model,
        "messages": messages,
    });

    if let Some(max_tokens) = input.max_tokens {
        if o_series {
            body["max_completion_tokens"] = json!(max_tokens);
        } else {
            body["max_tokens"] = json!(max_tokens);
        }
    }
    if let Some(temperature) = input.temperature
        && !o_series
    {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = input.top_p
        && !o_series
    {
        body["top_p"] = json!(top_p);
    }
    if let Some(stop) = input.stop_sequences
        && !o_series
    {
        body["stop"] = json!(stop);
    }

    let resp = openai_post_json(connection, "/v1/chat/completions", body, 120_000)?;

    let text = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
            )
            .with_attr("integration", "OPENAI")
        })?
        .to_string();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    let finish_reason = resp["choices"][0]["finish_reason"]
        .as_str()
        .unwrap_or("stop")
        .to_string();
    let usage = extract_usage(&resp);

    Ok(TextCompletionOutput {
        text,
        model,
        usage,
        finish_reason,
    })
}

// ============================================================================
// Capability 2: Image Generation
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Image Generation Input")]
pub struct ImageGenerationInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Prompt",
        description = "Text description of the image to generate",
        example = "A serene landscape with mountains at sunset"
    )]
    pub prompt: String,

    #[field(
        display_name = "Negative Prompt",
        description = "Elements to avoid in the generated image (not supported by all models)",
        example = "blurry, low quality, distorted"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The DALL-E model to use for image generation",
        example = "dall-e-3",
        default = "dall-e-3"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Width of the generated image in pixels (DALL-E 3: 1024, 1792)",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the generated image in pixels (DALL-E 3: 1024, 1792)",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(
        display_name = "Quality",
        description = "Image quality setting (DALL-E 3 only: 'standard' or 'hd')",
        example = "hd"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    #[field(
        display_name = "Style",
        description = "Image style (DALL-E 3 only: 'vivid' or 'natural')",
        example = "vivid"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Image Generation Output")]
pub struct ImageGenerationOutput {
    #[field(display_name = "Image Data", description = "Base64-encoded image data")]
    pub image_data: String,

    #[field(
        display_name = "MIME Type",
        description = "MIME type of the generated image (e.g., 'image/png')"
    )]
    pub mime_type: String,

    #[field(
        display_name = "Width",
        description = "Width of the generated image in pixels"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the generated image in pixels"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(
        display_name = "Model",
        description = "The model used for image generation"
    )]
    pub model: String,

    #[field(
        display_name = "Revised Prompt",
        description = "The prompt as revised by the model (DALL-E 3 may modify prompts)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

#[capability(
    module = "openai",
    display_name = "Image Generation (OpenAI)",
    description = "Generate images using OpenAI DALL-E models"
)]
pub fn image_generation(input: ImageGenerationInput) -> Result<ImageGenerationOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let model = input.model.unwrap_or_else(|| "dall-e-3".to_string());
    let mut body = json!({
        "model": model,
        "prompt": input.prompt,
        "response_format": "b64_json",
        "n": 1,
    });

    if model == "dall-e-3" {
        if let (Some(w), Some(h)) = (input.width, input.height) {
            body["size"] = json!(format!("{}x{}", w, h));
        } else {
            body["size"] = json!("1024x1024");
        }
        if let Some(quality) = input.quality {
            body["quality"] = json!(quality);
        }
        if let Some(style) = input.style {
            body["style"] = json!(style);
        }
    } else {
        body["size"] = json!("1024x1024");
    }

    // negative_prompt is not a first-class OpenAI param; appended to prompt
    // for models that support it via prompt engineering (preserved from legacy).
    if let Some(neg) = &input.negative_prompt {
        let existing = body["prompt"].as_str().unwrap_or("").to_string();
        body["prompt"] = json!(format!("{existing}. Avoid: {neg}"));
    }

    let resp = openai_post_json(
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
            .with_attr("integration", "OPENAI")
        })?
        .to_string();
    let revised_prompt = resp["data"][0]["revised_prompt"]
        .as_str()
        .map(|s| s.to_string());
    let width = input.width.or(Some(1024));
    let height = input.height.or(Some(1024));

    Ok(ImageGenerationOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width,
        height,
        model,
        revised_prompt,
    })
}

// ============================================================================
// Capability 3: Structured Output
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Structured Output Input")]
pub struct StructuredOutputInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Prompt",
        description = "The user message or prompt describing what structured data to generate",
        example = "Extract the person's name and age from: John is 30 years old"
    )]
    pub prompt: String,

    #[field(
        display_name = "System Prompt",
        description = "Optional system message to set context for structured output generation",
        example = "You are a data extraction assistant"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[field(
        display_name = "JSON Schema",
        description = "The JSON schema that defines the structure of the expected output",
        example = "{\"type\": \"object\", \"properties\": {\"name\": {\"type\": \"string\"}, \"age\": {\"type\": \"integer\"}}}"
    )]
    pub json_schema: Value,

    #[field(
        display_name = "Model",
        description = "The OpenAI model to use (must support structured outputs)",
        example = "gpt-4o-mini",
        default = "gpt-4o-mini"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-2). Lower values recommended for structured output",
        example = "0.3"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Structured Output Output")]
pub struct StructuredOutputOutput {
    #[field(
        display_name = "Output",
        description = "The structured JSON output conforming to the provided schema"
    )]
    pub output: Value,

    #[field(display_name = "Model", description = "The model used for generation")]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,
}

#[capability(
    module = "openai",
    display_name = "Structured Output (OpenAI)",
    description = "Generate structured JSON output using OpenAI models with schema validation"
)]
pub fn structured_output(
    input: StructuredOutputInput,
) -> Result<StructuredOutputOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let mut messages = Vec::new();
    if let Some(system) = &input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    let mut body = json!({
        "model": input.model.unwrap_or_else(|| DEFAULT_OPENAI_MINI_MODEL.to_string()),
        "messages": messages,
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "structured_response",
                "strict": true,
                "schema": input.json_schema
            }
        }
    });
    if let Some(temperature) = input.temperature {
        body["temperature"] = json!(temperature);
    }

    let resp = openai_post_json(connection, "/v1/chat/completions", body, 120_000)?;

    let content = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
            )
            .with_attr("integration", "OPENAI")
        })?;
    let output: Value = serde_json::from_str(content).map_err(|e| {
        AgentError::permanent(
            "OPENAI_INVALID_RESPONSE",
            format!("Failed to parse structured output: {e}"),
        )
        .with_attr("integration", "OPENAI")
    })?;
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_usage(&resp);

    Ok(StructuredOutputOutput {
        output,
        model,
        usage,
    })
}

// ============================================================================
// Capability 4: Vision to Text
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Vision to Text Input")]
pub struct VisionToTextInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Prompt",
        description = "Instructions for analyzing the image",
        example = "Describe what you see in this image"
    )]
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
        description = "The OpenAI model to use (must support vision)",
        example = "gpt-4o",
        default = "gpt-4o"
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
        description = "Sampling temperature (0-2)",
        example = "0.7"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Vision to Text Output")]
pub struct VisionToTextOutput {
    #[field(
        display_name = "Text",
        description = "The generated text description or analysis of the image"
    )]
    pub text: String,

    #[field(display_name = "Model", description = "The model used for analysis")]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,
}

#[capability(
    module = "openai",
    display_name = "Vision to Text (OpenAI)",
    description = "Analyze images and generate text descriptions using OpenAI vision models"
)]
pub fn vision_to_text(input: VisionToTextInput) -> Result<VisionToTextOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(AgentError::permanent(
            "OPENAI_INVALID_INPUT",
            "Either image_data or image_url is required",
        )
        .with_attr("integration", "OPENAI"));
    }

    let mut content = vec![json!({"type": "text", "text": input.prompt})];
    if let Some(url) = &input.image_url {
        content.push(json!({"type": "image_url", "image_url": {"url": url}}));
    } else if let Some(data) = &input.image_data {
        content.push(json!({
            "type": "image_url",
            "image_url": {"url": format!("data:image/png;base64,{data}")}
        }));
    }

    let model = input
        .model
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
    let o_series = is_o_series(&model);

    let mut body = json!({
        "model": model,
        "messages": [{"role": "user", "content": content}],
    });
    if let Some(max_tokens) = input.max_tokens {
        if o_series {
            body["max_completion_tokens"] = json!(max_tokens);
        } else {
            body["max_tokens"] = json!(max_tokens);
        }
    }
    if let Some(temperature) = input.temperature
        && !o_series
    {
        body["temperature"] = json!(temperature);
    }

    let resp = openai_post_json(connection, "/v1/chat/completions", body, 120_000)?;

    let text = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
            )
            .with_attr("integration", "OPENAI")
        })?
        .to_string();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_usage(&resp);

    Ok(VisionToTextOutput { text, model, usage })
}

// ============================================================================
// Capability 5: Vision to Image (Image Editing)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Vision to Image Input")]
pub struct VisionToImageInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Prompt",
        description = "Instructions for how to edit or transform the image",
        example = "Add a sunset sky in the background"
    )]
    pub prompt: String,

    #[field(
        display_name = "Image Data",
        description = "Base64-encoded source image data to edit",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    pub image_data: String,

    #[field(
        display_name = "Mask Data",
        description = "Optional base64-encoded mask image indicating areas to edit (transparent = edit)",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_data: Option<String>,

    #[field(
        display_name = "Model",
        description = "The DALL-E model to use for image editing",
        example = "dall-e-2",
        default = "dall-e-2"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Width of the output image in pixels",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the output image in pixels",
        example = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Vision to Image Output")]
pub struct VisionToImageOutput {
    #[field(
        display_name = "Image Data",
        description = "Base64-encoded edited image data"
    )]
    pub image_data: String,

    #[field(
        display_name = "MIME Type",
        description = "MIME type of the output image"
    )]
    pub mime_type: String,

    #[field(
        display_name = "Width",
        description = "Width of the output image in pixels"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the output image in pixels"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(
        display_name = "Model",
        description = "The model used for image editing"
    )]
    pub model: String,
}

#[capability(
    module = "openai",
    display_name = "Vision to Image (OpenAI)",
    description = "Edit and manipulate images using OpenAI DALL-E models"
)]
pub fn vision_to_image(input: VisionToImageInput) -> Result<VisionToImageOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let endpoint = if input.mask_data.is_some() {
        "images/edits"
    } else {
        "images/variations"
    };

    // NOTE: The OpenAI images/edits and images/variations endpoints require
    // multipart/form-data with binary PNG payloads, which cannot be satisfied
    // by a simple JSON POST. The proxy currently only supports JSON bodies.
    // We call the JSON-compatible path here; full multipart support requires
    // proxy-side changes. The body below is best-effort and will likely return
    // a 415/400 from OpenAI until multipart proxy support lands.
    // TODO: add multipart support to the runtara proxy and update this handler.
    let body = json!({
        "prompt": input.prompt,
        "n": 1,
        "response_format": "b64_json",
        "size": format!("{}x{}", input.width.unwrap_or(1024), input.height.unwrap_or(1024)),
    });

    let resp = openai_post_json(
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
            .with_attr("integration", "OPENAI")
        })?
        .to_string();
    let model = input.model.unwrap_or_else(|| "dall-e-2".to_string());

    Ok(VisionToImageOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
    })
}

// ============================================================================
// Capability 6: Chat Completion (raw)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "OpenAI Chat Completion Input")]
pub struct OpenaiChatCompletionInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Messages",
        description = "Array of messages in the conversation (each with 'role' and 'content')",
        example = "[{\"role\": \"user\", \"content\": \"Hello!\"}]"
    )]
    pub messages: Vec<Value>,

    #[field(
        display_name = "Model",
        description = "The OpenAI model to use",
        example = "gpt-4o",
        default = "gpt-4o"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate",
        example = "2048"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-2)",
        example = "0.7"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[field(
        display_name = "Top P",
        description = "Nucleus sampling parameter",
        example = "0.9"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    #[field(
        display_name = "Frequency Penalty",
        description = "Penalty for token frequency (-2.0 to 2.0). Positive values decrease repetition",
        example = "0.5"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,

    #[field(
        display_name = "Presence Penalty",
        description = "Penalty for token presence (-2.0 to 2.0). Positive values encourage new topics",
        example = "0.5"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,

    #[field(
        display_name = "Stop Sequences",
        description = "Sequences where generation stops",
        example = "[\"END\"]"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,

    #[field(
        display_name = "Tools",
        description = "Array of tool/function definitions for function calling",
        example = "[{\"type\": \"function\", \"function\": {\"name\": \"get_weather\"}}]"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,

    #[field(
        display_name = "Tool Choice",
        description = "Controls which tool is called ('auto', 'none', or specific tool)",
        example = "auto"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "OpenAI Chat Completion Output")]
pub struct OpenaiChatCompletionOutput {
    #[field(
        display_name = "Choices",
        description = "Array of completion choices with messages and finish reasons"
    )]
    pub choices: Vec<Value>,

    #[field(display_name = "Model", description = "The model used for completion")]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,

    #[field(
        display_name = "ID",
        description = "Unique identifier for the completion"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[capability(
    module = "openai",
    display_name = "Chat Completion",
    description = "OpenAI chat completion with full control over messages, tools, and parameters"
)]
pub fn openai_chat_completion(
    input: OpenaiChatCompletionInput,
) -> Result<OpenaiChatCompletionOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let model = input
        .model
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
    let o_series = is_o_series(&model);

    let mut body = json!({
        "model": model,
        "messages": input.messages,
    });

    if let Some(max_tokens) = input.max_tokens {
        if o_series {
            body["max_completion_tokens"] = json!(max_tokens);
        } else {
            body["max_tokens"] = json!(max_tokens);
        }
    }
    if let Some(temperature) = input.temperature
        && !o_series
    {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = input.top_p
        && !o_series
    {
        body["top_p"] = json!(top_p);
    }
    if let Some(freq) = input.frequency_penalty
        && !o_series
    {
        body["frequency_penalty"] = json!(freq);
    }
    if let Some(pres) = input.presence_penalty
        && !o_series
    {
        body["presence_penalty"] = json!(pres);
    }
    if let Some(stop) = input.stop
        && !o_series
    {
        body["stop"] = json!(stop);
    }
    if let Some(tools) = input.tools {
        body["tools"] = json!(tools);
    }
    if let Some(tool_choice) = input.tool_choice {
        body["tool_choice"] = json!(tool_choice);
    }

    let resp = openai_post_json(connection, "/v1/chat/completions", body, 120_000)?;

    let choices = resp["choices"]
        .as_array()
        .ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing choices in OpenAI response",
            )
            .with_attr("integration", "OPENAI")
        })?
        .clone();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_usage(&resp);
    let id = resp["id"].as_str().map(|s| s.to_string());

    Ok(OpenaiChatCompletionOutput {
        choices,
        model,
        usage,
        id,
    })
}

// ============================================================================
// Capability 7: Create Embedding
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "OpenAI Create Embedding Input")]
pub struct OpenaiCreateEmbeddingInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Input",
        description = "Text to generate embeddings for (string or array of strings)",
        example = "The quick brown fox jumps over the lazy dog"
    )]
    pub input: Value,

    #[field(
        display_name = "Model",
        description = "The embedding model to use",
        example = "text-embedding-3-small",
        default = "text-embedding-3-small"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "OpenAI Create Embedding Output")]
pub struct OpenaiCreateEmbeddingOutput {
    #[field(
        display_name = "Data",
        description = "Array of embedding objects with vectors"
    )]
    pub data: Vec<Value>,

    #[field(
        display_name = "Model",
        description = "The model used to generate embeddings"
    )]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,
}

#[capability(
    module = "openai",
    display_name = "Create Embedding",
    description = "Generate embeddings for text using OpenAI embedding models"
)]
pub fn openai_create_embedding(
    input: OpenaiCreateEmbeddingInput,
) -> Result<OpenaiCreateEmbeddingOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let body = json!({
        "model": input.model.unwrap_or_else(|| "text-embedding-3-small".to_string()),
        "input": input.input,
    });

    let resp = openai_post_json(connection, "/v1/embeddings", body, 60_000)?;

    let data = resp["data"]
        .as_array()
        .ok_or_else(|| {
            AgentError::permanent("OPENAI_INVALID_RESPONSE", "Missing data in OpenAI response")
                .with_attr("integration", "OPENAI")
        })?
        .clone();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    // Embeddings endpoint returns prompt_tokens + total_tokens only.
    let usage = LlmUsage {
        prompt_tokens: resp["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32,
        completion_tokens: 0,
        total_tokens: resp["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    };

    Ok(OpenaiCreateEmbeddingOutput { data, model, usage })
}

// ============================================================================
// Capability 8: Moderate Content
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "OpenAI Moderate Content Input")]
pub struct OpenaiModerateContentInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Input",
        description = "Text content to check for policy violations",
        example = "Hello, how are you today?"
    )]
    pub input: String,

    #[field(
        display_name = "Model",
        description = "The moderation model to use",
        example = "text-moderation-latest",
        default = "text-moderation-latest"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "OpenAI Moderate Content Output")]
pub struct OpenaiModerateContentOutput {
    #[field(
        display_name = "Results",
        description = "Array of moderation results with category flags and scores"
    )]
    pub results: Vec<Value>,

    #[field(display_name = "Model", description = "The moderation model used")]
    pub model: String,
}

#[capability(
    module = "openai",
    display_name = "Moderate Content",
    description = "Check content for policy violations using OpenAI moderation API"
)]
pub fn openai_moderate_content(
    input: OpenaiModerateContentInput,
) -> Result<OpenaiModerateContentOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let body = json!({
        "input": input.input,
        "model": input.model.unwrap_or_else(|| "text-moderation-latest".to_string()),
    });

    let resp = openai_post_json(connection, "/v1/moderations", body, 30_000)?;

    let results = resp["results"]
        .as_array()
        .ok_or_else(|| {
            AgentError::permanent(
                "OPENAI_INVALID_RESPONSE",
                "Missing results in OpenAI response",
            )
            .with_attr("integration", "OPENAI")
        })?
        .clone();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();

    Ok(OpenaiModerateContentOutput { results, model })
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
        &__CAPABILITY_META_IMAGE_GENERATION,
        &__CAPABILITY_META_STRUCTURED_OUTPUT,
        &__CAPABILITY_META_VISION_TO_TEXT,
        &__CAPABILITY_META_VISION_TO_IMAGE,
        &__CAPABILITY_META_OPENAI_CHAT_COMPLETION,
        &__CAPABILITY_META_OPENAI_CREATE_EMBEDDING,
        &__CAPABILITY_META_OPENAI_MODERATE_CONTENT,
    ];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "TextCompletionInput",
            &__INPUT_META_TextCompletionInput as &InputTypeMeta,
        ),
        (
            "ImageGenerationInput",
            &__INPUT_META_ImageGenerationInput as &InputTypeMeta,
        ),
        (
            "StructuredOutputInput",
            &__INPUT_META_StructuredOutputInput as &InputTypeMeta,
        ),
        (
            "VisionToTextInput",
            &__INPUT_META_VisionToTextInput as &InputTypeMeta,
        ),
        (
            "VisionToImageInput",
            &__INPUT_META_VisionToImageInput as &InputTypeMeta,
        ),
        (
            "OpenaiChatCompletionInput",
            &__INPUT_META_OpenaiChatCompletionInput as &InputTypeMeta,
        ),
        (
            "OpenaiCreateEmbeddingInput",
            &__INPUT_META_OpenaiCreateEmbeddingInput as &InputTypeMeta,
        ),
        (
            "OpenaiModerateContentInput",
            &__INPUT_META_OpenaiModerateContentInput as &InputTypeMeta,
        ),
    ]
    .into_iter()
    .collect();
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "TextCompletionOutput",
            &__OUTPUT_META_TextCompletionOutput as &OutputTypeMeta,
        ),
        (
            "ImageGenerationOutput",
            &__OUTPUT_META_ImageGenerationOutput as &OutputTypeMeta,
        ),
        (
            "StructuredOutputOutput",
            &__OUTPUT_META_StructuredOutputOutput as &OutputTypeMeta,
        ),
        (
            "VisionToTextOutput",
            &__OUTPUT_META_VisionToTextOutput as &OutputTypeMeta,
        ),
        (
            "VisionToImageOutput",
            &__OUTPUT_META_VisionToImageOutput as &OutputTypeMeta,
        ),
        (
            "OpenaiChatCompletionOutput",
            &__OUTPUT_META_OpenaiChatCompletionOutput as &OutputTypeMeta,
        ),
        (
            "OpenaiCreateEmbeddingOutput",
            &__OUTPUT_META_OpenaiCreateEmbeddingOutput as &OutputTypeMeta,
        ),
        (
            "OpenaiModerateContentOutput",
            &__OUTPUT_META_OpenaiModerateContentOutput as &OutputTypeMeta,
        ),
        ("LlmUsage", &__OUTPUT_META_LlmUsage as &OutputTypeMeta),
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
        id: "openai".into(),
        name: "OpenAI".into(),
        description:
            "OpenAI LLM integration for text completion, image generation, structured output, and vision capabilities."
                .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["openai_api_key".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_openai::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "text-completion" => __executor_text_completion(value),
            "image-generation" => __executor_image_generation(value),
            "structured-output" => __executor_structured_output(value),
            "vision-to-text" => __executor_vision_to_text(value),
            "vision-to-image" => __executor_vision_to_image(value),
            "openai-chat-completion" => __executor_openai_chat_completion(value),
            "openai-create-embedding" => __executor_openai_create_embedding(value),
            "openai-moderate-content" => __executor_openai_moderate_content(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("openai agent has no capability `{other}`"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_dsl::agent_meta::{InputFieldMeta, InputTypeMeta};

    fn model_field(meta: &'static InputTypeMeta) -> &'static InputFieldMeta {
        meta.fields
            .iter()
            .find(|f| f.name == "model")
            .unwrap_or_else(|| panic!("{} has no model field", meta.type_name))
    }

    // `#[field(default/example = ...)]` are attribute literals and cannot
    // reference consts, so pin them to the shared defaults here — otherwise
    // the authoring metadata can silently drift from the runtime fallbacks.
    #[test]
    fn gpt_model_field_metadata_matches_shared_defaults() {
        for meta in [
            &__INPUT_META_TextCompletionInput,
            &__INPUT_META_VisionToTextInput,
            &__INPUT_META_OpenaiChatCompletionInput,
        ] {
            let field = model_field(meta);
            assert_eq!(
                field.default_value,
                Some(DEFAULT_OPENAI_MODEL),
                "{} model default",
                meta.type_name
            );
            assert_eq!(
                field.example,
                Some(DEFAULT_OPENAI_MODEL),
                "{} model example",
                meta.type_name
            );
        }

        let structured = model_field(&__INPUT_META_StructuredOutputInput);
        assert_eq!(structured.default_value, Some(DEFAULT_OPENAI_MINI_MODEL));
        assert_eq!(structured.example, Some(DEFAULT_OPENAI_MINI_MODEL));
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);
