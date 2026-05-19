//! AWS Bedrock integration agent — WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_bedrock.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to look up the
//! `aws_credentials` connection, compute SigV4 server-side, and rewrite the
//! base URL to the regional Bedrock endpoint
//! (`https://bedrock-runtime.{region}.amazonaws.com`). The component never sees
//! AWS credentials.
//!
//! For the control-plane `list-foundation-models` capability we pass a
//! `bedrock.amazonaws.com` host placeholder; the proxy rewrites the service
//! subdomain from `bedrock-runtime` to `bedrock` when the connection service
//! parameter routes it that way.
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
// version here. Mirrors the shim in `runtara-agent-mailgun` / `-openai`.

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
// Shared LlmUsage type — mirrors legacy `LlmUsage` (snake_case on the wire).
// ============================================================================
//
// Output struct shared by several capabilities. The `#[capability_output]`
// derive is required so the macro emits an `__OUTPUT_META_LlmUsage` static
// that the host-only `agent_info()` assembler can pick up.

#[derive(Debug, Default, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "LLM Usage",
    description = "Token count statistics from LLM API calls"
)]
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

/// Resolve the connection or return the legacy `BEDROCK_MISSING_CONNECTION`
/// error code (preserved for wire compatibility).
fn require_connection(connection: Option<&RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.ok_or_else(|| {
        AgentError::permanent(
            "BEDROCK_MISSING_CONNECTION",
            "Bedrock connection is required",
        )
        .with_attr("integration", "BEDROCK")
    })
}

/// POST `body` to a Bedrock runtime path via the runtara proxy. The proxy
/// resolves the `aws_credentials` connection, computes SigV4, and rewrites the
/// host to `https://bedrock-runtime.{region}.amazonaws.com`. We pass a
/// placeholder host so the URL is well-formed for `runtara-http`.
fn bedrock_post(
    connection: &RawConnection,
    path: &str,
    body: Value,
    timeout_ms: u64,
) -> Result<Value, AgentError> {
    let url = format!("https://bedrock-runtime.amazonaws.com{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| {
        AgentError::permanent("SERIALIZATION_ERROR", e.to_string())
            .with_attr("integration", "BEDROCK")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "NETWORK_ERROR",
                format!("Bedrock request to {path} failed: {e}"),
            )
            .with_attr("integration", "BEDROCK")
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (category, code) = if status == 429 {
            ("transient", "HTTP_429")
        } else if (500..600).contains(&status) {
            ("transient", "HTTP_5XX")
        } else {
            ("permanent", "HTTP_4XX")
        };
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
            .with_attr("integration", "BEDROCK")
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
            format!("Bedrock response parse error: {e}"),
        )
        .with_attr("integration", "BEDROCK")
    })
}

/// GET a Bedrock control-plane path (`bedrock.amazonaws.com`, distinct from
/// the runtime endpoint). Same proxy routing as `bedrock_post` — the proxy
/// resolves region/credentials from the connection.
fn bedrock_get(
    connection: &RawConnection,
    path: &str,
    timeout_ms: u64,
) -> Result<Value, AgentError> {
    let url = format!("https://bedrock.amazonaws.com{path}");

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("GET", &url)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "NETWORK_ERROR",
                format!("Bedrock GET request to {path} failed: {e}"),
            )
            .with_attr("integration", "BEDROCK")
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (category, code) = if status == 429 {
            ("transient", "HTTP_429")
        } else if (500..600).contains(&status) {
            ("transient", "HTTP_5XX")
        } else {
            ("permanent", "HTTP_4XX")
        };
        let err = if category == "transient" {
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
        return Err(err
            .with_attr("integration", "BEDROCK")
            .with_attr("status_code", status.to_string())
            .with_attr("path", path.to_string())
            .with_attr("body", truncate(&body_text, 512)));
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "RESPONSE_PARSE_ERROR",
            format!("Bedrock response parse error: {e}"),
        )
        .with_attr("integration", "BEDROCK")
    })
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
        description = "Optional system message to set the assistant's behavior",
        example = "You are a helpful assistant"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The Bedrock model ID to use (Claude or Titan)",
        example = "anthropic.claude-3-5-sonnet-20240620-v1:0",
        default = "anthropic.claude-3-5-sonnet-20240620-v1:0"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate",
        example = "1024",
        default = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-1). Higher values increase randomness",
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
        display_name = "Stop Sequences",
        description = "Sequences where generation stops",
        example = "[\"\\n\\nHuman:\"]"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Text Completion Output")]
pub struct TextCompletionOutput {
    #[field(display_name = "Text", description = "The generated text response")]
    pub text: String,

    #[field(display_name = "Model", description = "The model used for generation")]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,

    #[field(
        display_name = "Finish Reason",
        description = "The reason generation stopped"
    )]
    pub finish_reason: String,
}

#[capability(
    module = "bedrock",
    display_name = "Text Completion (Bedrock)",
    description = "Generate text completion using AWS Bedrock models (Claude, Titan)",
    module_display_name = "AWS Bedrock",
    module_description = "AWS Bedrock LLM integration for text completion, image generation, structured output, and vision capabilities using Claude and Titan models.",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "aws_credentials",
    module_secure = true
)]
pub fn text_completion(input: TextCompletionInput) -> Result<TextCompletionOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let model = input
        .model
        .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string());

    let (request_body, is_claude) = if model.starts_with("anthropic.claude") {
        let mut body = json!({
            "messages": [{"role": "user", "content": input.prompt}],
            "max_tokens": input.max_tokens.unwrap_or(1024),
            "anthropic_version": "bedrock-2023-05-31"
        });
        if let Some(system) = input.system_prompt {
            body["system"] = json!(system);
        }
        if let Some(temp) = input.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(top_p) = input.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(stop) = input.stop_sequences {
            body["stop_sequences"] = json!(stop);
        }
        (body, true)
    } else if model.starts_with("amazon.titan") {
        let mut text_config = json!({
            "maxTokenCount": input.max_tokens.unwrap_or(1024),
        });
        if let Some(temp) = input.temperature {
            text_config["temperature"] = json!(temp);
        }
        if let Some(top_p) = input.top_p {
            text_config["topP"] = json!(top_p);
        }
        if let Some(stop) = input.stop_sequences {
            text_config["stopSequences"] = json!(stop);
        }
        let full_prompt = match input.system_prompt {
            Some(system) => format!("{}\n\n{}", system, input.prompt),
            None => input.prompt.clone(),
        };
        let body = json!({
            "inputText": full_prompt,
            "textGenerationConfig": text_config
        });
        (body, false)
    } else {
        return Err(AgentError::permanent(
            "BEDROCK_UNSUPPORTED_MODEL",
            format!("Unsupported Bedrock model: {}", model),
        )
        .with_attr("integration", "BEDROCK")
        .with_attr("model", model.clone()));
    };

    let resp = bedrock_post(
        connection,
        &format!("/model/{}/invoke", model),
        request_body,
        120_000,
    )?;

    let (text, prompt_tokens, completion_tokens, finish_reason) = if is_claude {
        let text = resp["content"][0]["text"]
            .as_str()
            .ok_or_else(|| {
                AgentError::permanent(
                    "BEDROCK_INVALID_RESPONSE",
                    "Missing text in Bedrock response",
                )
                .with_attr("integration", "BEDROCK")
            })?
            .to_string();
        let prompt_tokens = resp["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32;
        let completion_tokens = resp["usage"]["output_tokens"].as_i64().unwrap_or(0) as i32;
        let finish_reason = resp["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();
        (text, prompt_tokens, completion_tokens, finish_reason)
    } else {
        let text = resp["results"][0]["outputText"]
            .as_str()
            .ok_or_else(|| {
                AgentError::permanent(
                    "BEDROCK_INVALID_RESPONSE",
                    "Missing outputText in Bedrock response",
                )
                .with_attr("integration", "BEDROCK")
            })?
            .to_string();
        let prompt_tokens = resp["inputTextTokenCount"].as_i64().unwrap_or(0) as i32;
        let completion_tokens = resp["results"][0]["tokenCount"].as_i64().unwrap_or(0) as i32;
        let finish_reason = resp["results"][0]["completionReason"]
            .as_str()
            .unwrap_or("FINISH")
            .to_string();
        (text, prompt_tokens, completion_tokens, finish_reason)
    };

    Ok(TextCompletionOutput {
        text,
        model,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
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
        example = "A futuristic city skyline at sunset"
    )]
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
        description = "The Bedrock image model to use (e.g., Stable Diffusion)",
        example = "stability.stable-diffusion-xl-v1",
        default = "stability.stable-diffusion-xl-v1"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Width of the generated image in pixels",
        example = "1024",
        default = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the generated image in pixels",
        example = "1024",
        default = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(
        display_name = "Quality",
        description = "Image quality setting (if supported by model)",
        example = "standard"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    #[field(
        display_name = "Style",
        description = "Image style preset (if supported by model)",
        example = "photographic"
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
        description = "MIME type of the generated image"
    )]
    pub mime_type: String,

    #[field(display_name = "Width", description = "Width of the generated image")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(display_name = "Height", description = "Height of the generated image")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(display_name = "Model", description = "The model used for generation")]
    pub model: String,

    #[field(
        display_name = "Revised Prompt",
        description = "The prompt as interpreted by the model (if available)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

#[capability(
    module = "bedrock",
    display_name = "Image Generation (Bedrock)",
    description = "Generate images using AWS Bedrock models (Stable Diffusion)"
)]
pub fn image_generation(input: ImageGenerationInput) -> Result<ImageGenerationOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let model = input
        .model
        .unwrap_or_else(|| "stability.stable-diffusion-xl-v1".to_string());

    let mut text_prompts = vec![json!({"text": input.prompt, "weight": 1.0})];
    if let Some(negative) = input.negative_prompt {
        text_prompts.push(json!({"text": negative, "weight": -1.0}));
    }

    let mut body = json!({
        "text_prompts": text_prompts,
        "cfg_scale": 7,
        "seed": 0,
        "steps": 30,
        "width": input.width.unwrap_or(1024),
        "height": input.height.unwrap_or(1024),
    });

    // quality and style are accepted as pass-through parameters for models that
    // support them; Stable Diffusion XL ignores unknown fields.
    if let Some(quality) = &input.quality {
        body["quality"] = json!(quality);
    }
    if let Some(style) = &input.style {
        body["style_preset"] = json!(style);
    }

    let resp = bedrock_post(
        connection,
        &format!("/model/{}/invoke", model),
        body,
        180_000,
    )?;

    let image_data = resp["artifacts"][0]["base64"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "BEDROCK_INVALID_RESPONSE",
                "Missing base64 image in Bedrock response",
            )
            .with_attr("integration", "BEDROCK")
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
// Capability 3: Structured Output
//
// Bedrock lacks native JSON-schema enforcement; we use prompt engineering
// identical to the legacy implementation, delegating to `text_completion`.
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Structured Output Input")]
pub struct StructuredOutputInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Prompt",
        description = "The prompt describing what data to extract or generate",
        example = "Extract the person's name and age"
    )]
    pub prompt: String,

    #[field(
        display_name = "System Prompt",
        description = "Optional system message for context",
        example = "You are a data extraction assistant"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[field(
        display_name = "JSON Schema",
        description = "The JSON schema defining the expected output structure",
        example = "{\"type\": \"object\", \"properties\": {\"name\": {\"type\": \"string\"}}}"
    )]
    pub json_schema: Value,

    #[field(
        display_name = "Model",
        description = "The Bedrock model to use",
        example = "anthropic.claude-3-5-sonnet-20240620-v1:0"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (lower recommended for structured output)",
        example = "0.3"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Structured Output Output")]
pub struct StructuredOutputOutput {
    #[field(display_name = "Output", description = "The structured JSON output")]
    pub output: Value,

    #[field(display_name = "Model", description = "The model used")]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,
}

#[capability(
    module = "bedrock",
    display_name = "Structured Output (Bedrock)",
    description = "Generate structured JSON output using AWS Bedrock models with prompt engineering"
)]
pub fn structured_output(
    input: StructuredOutputInput,
) -> Result<StructuredOutputOutput, AgentError> {
    let schema_str = serde_json::to_string_pretty(&input.json_schema).map_err(|e| {
        AgentError::permanent(
            "BEDROCK_INVALID_INPUT",
            format!("Failed to serialize schema: {}", e),
        )
        .with_attr("integration", "BEDROCK")
    })?;

    let enhanced_prompt = format!(
        "{}\n\nRespond with valid JSON matching this schema:\n{}\n\nReturn ONLY the JSON, no other text.",
        input.prompt, schema_str
    );

    // Delegate to text_completion capability for the prompt-engineered call.
    let tc_result = text_completion(TextCompletionInput {
        _connection: input._connection.clone(),
        prompt: enhanced_prompt,
        system_prompt: input.system_prompt,
        model: input.model.clone(),
        max_tokens: Some(4096),
        temperature: input.temperature,
        top_p: None,
        stop_sequences: None,
    })?;

    let output: Value = serde_json::from_str(&tc_result.text).map_err(|e| {
        AgentError::permanent(
            "BEDROCK_INVALID_RESPONSE",
            format!("Failed to parse structured output as JSON: {}", e),
        )
        .with_attr("integration", "BEDROCK")
        .with_attr("response", truncate(&tc_result.text, 256))
    })?;

    Ok(StructuredOutputOutput {
        output,
        model: tc_result.model,
        usage: tc_result.usage,
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
        description = "Base64-encoded image data (required for Bedrock, URLs not supported)",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_data: Option<String>,

    #[field(
        display_name = "Image URL",
        description = "URL of the image (not supported by Bedrock - use image_data instead)",
        example = "https://example.com/image.png"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,

    #[field(
        display_name = "Model",
        description = "The Bedrock model to use (must be Claude 3 or 3.5 for vision)",
        example = "anthropic.claude-3-5-sonnet-20240620-v1:0",
        default = "anthropic.claude-3-5-sonnet-20240620-v1:0"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate",
        example = "1024",
        default = "1024"
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
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Vision to Text Output")]
pub struct VisionToTextOutput {
    #[field(
        display_name = "Text",
        description = "The generated text analysis of the image"
    )]
    pub text: String,

    #[field(display_name = "Model", description = "The model used")]
    pub model: String,

    #[field(display_name = "Usage", description = "Token usage statistics")]
    pub usage: LlmUsage,
}

#[capability(
    module = "bedrock",
    display_name = "Vision to Text (Bedrock)",
    description = "Analyze images and generate text descriptions using AWS Bedrock Claude models"
)]
pub fn vision_to_text(input: VisionToTextInput) -> Result<VisionToTextOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let model = input
        .model
        .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string());

    // Only Claude 3/3.5 models support vision in Bedrock.
    if !model.starts_with("anthropic.claude-3") {
        return Err(AgentError::permanent(
            "BEDROCK_UNSUPPORTED_MODEL",
            "Vision capabilities require Claude 3 or Claude 3.5 models",
        )
        .with_attr("integration", "BEDROCK")
        .with_attr("model", model.clone()));
    }

    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(AgentError::permanent(
            "BEDROCK_MISSING_INPUT",
            "Either image_data or image_url is required",
        )
        .with_attr("integration", "BEDROCK"));
    }

    // Bedrock does not support image URLs; only base64.
    if input.image_url.is_some() && input.image_data.is_none() {
        return Err(AgentError::permanent(
            "BEDROCK_UNSUPPORTED_INPUT",
            "Bedrock vision requires base64-encoded image_data, not URLs",
        )
        .with_attr("integration", "BEDROCK"));
    }

    let mut content_blocks = Vec::new();

    if let Some(image_data) = input.image_data {
        content_blocks.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": image_data
            }
        }));
    }

    content_blocks.push(json!({"type": "text", "text": input.prompt}));

    let mut body = json!({
        "messages": [{"role": "user", "content": content_blocks}],
        "max_tokens": input.max_tokens.unwrap_or(1024),
        "anthropic_version": "bedrock-2023-05-31"
    });

    if let Some(temp) = input.temperature {
        body["temperature"] = json!(temp);
    }

    let resp = bedrock_post(
        connection,
        &format!("/model/{}/invoke", model),
        body,
        120_000,
    )?;

    let text = resp["content"][0]["text"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "BEDROCK_INVALID_RESPONSE",
                "Missing text in Bedrock response",
            )
            .with_attr("integration", "BEDROCK")
        })?
        .to_string();

    let prompt_tokens = resp["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32;
    let completion_tokens = resp["usage"]["output_tokens"].as_i64().unwrap_or(0) as i32;

    Ok(VisionToTextOutput {
        text,
        model,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
    })
}

// ============================================================================
// Capability 5: Vision to Image
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Vision to Image Input")]
pub struct VisionToImageInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Prompt",
        description = "Instructions for how to modify the image",
        example = "Add dramatic lighting to the scene"
    )]
    pub prompt: String,

    #[field(
        display_name = "Image Data",
        description = "Base64-encoded source image to edit",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    pub image_data: String,

    #[field(
        display_name = "Mask Data",
        description = "Optional base64-encoded mask for inpainting",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask_data: Option<String>,

    #[field(
        display_name = "Model",
        description = "The Bedrock image model to use",
        example = "stability.stable-diffusion-xl-v1",
        default = "stability.stable-diffusion-xl-v1"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Width of the output image",
        example = "1024",
        default = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the output image",
        example = "1024",
        default = "1024"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Vision to Image Output")]
pub struct VisionToImageOutput {
    #[field(
        display_name = "Image Data",
        description = "Base64-encoded edited image"
    )]
    pub image_data: String,

    #[field(
        display_name = "MIME Type",
        description = "MIME type of the output image"
    )]
    pub mime_type: String,

    #[field(display_name = "Width", description = "Width of the output image")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(display_name = "Height", description = "Height of the output image")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(display_name = "Model", description = "The model used")]
    pub model: String,
}

#[capability(
    module = "bedrock",
    display_name = "Vision to Image (Bedrock)",
    description = "Edit and manipulate images using AWS Bedrock Stable Diffusion models"
)]
pub fn vision_to_image(input: VisionToImageInput) -> Result<VisionToImageOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let model = input
        .model
        .unwrap_or_else(|| "stability.stable-diffusion-xl-v1".to_string());

    // Stable Diffusion image-to-image mode: pass init_image with image_strength.
    // mask_data is forwarded for inpainting workflows.
    let mut body = json!({
        "text_prompts": [{"text": input.prompt, "weight": 1.0}],
        "init_image": input.image_data,
        "cfg_scale": 7,
        "image_strength": 0.5,
        "steps": 30,
        "width": input.width.unwrap_or(1024),
        "height": input.height.unwrap_or(1024),
    });

    if let Some(mask) = input.mask_data {
        body["mask_image"] = json!(mask);
    }

    let resp = bedrock_post(
        connection,
        &format!("/model/{}/invoke", model),
        body,
        180_000,
    )?;

    let image_data = resp["artifacts"][0]["base64"]
        .as_str()
        .ok_or_else(|| {
            AgentError::permanent(
                "BEDROCK_INVALID_RESPONSE",
                "Missing base64 image in Bedrock response",
            )
            .with_attr("integration", "BEDROCK")
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
// Capability 6: Invoke Model (raw passthrough)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bedrock Invoke Model Input")]
pub struct BedrockInvokeModelInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Model ID",
        description = "The Bedrock model ID to invoke",
        example = "anthropic.claude-3-5-sonnet-20240620-v1:0"
    )]
    pub model_id: String,

    #[field(
        display_name = "Body",
        description = "The request body to send to the model (format depends on model)",
        example = "{\"messages\": [{\"role\": \"user\", \"content\": \"Hello\"}], \"max_tokens\": 1024}"
    )]
    pub body: Value,

    #[field(
        display_name = "Accept",
        description = "The MIME type for the response",
        example = "application/json",
        default = "application/json"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<String>,

    #[field(
        display_name = "Content Type",
        description = "The MIME type of the request body",
        example = "application/json",
        default = "application/json"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bedrock Invoke Model Output")]
pub struct BedrockInvokeModelOutput {
    #[field(
        display_name = "Body",
        description = "The response body from the model"
    )]
    pub body: Value,

    #[field(
        display_name = "Content Type",
        description = "The MIME type of the response"
    )]
    pub content_type: String,
}

#[capability(
    module = "bedrock",
    display_name = "Invoke Model",
    description = "Directly invoke any AWS Bedrock model with custom request body"
)]
pub fn bedrock_invoke_model(
    input: BedrockInvokeModelInput,
) -> Result<BedrockInvokeModelOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    let content_type = input
        .content_type
        .unwrap_or_else(|| "application/json".to_string());
    let accept = input
        .accept
        .unwrap_or_else(|| "application/json".to_string());

    let body_bytes = serde_json::to_vec(&input.body).map_err(|e| {
        AgentError::permanent("SERIALIZATION_ERROR", e.to_string())
            .with_attr("integration", "BEDROCK")
    })?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(180_000));
    let url = format!(
        "https://bedrock-runtime.amazonaws.com/model/{}/invoke",
        input.model_id
    );
    let response = client
        .request("POST", &url)
        .header("Content-Type", &content_type)
        .header("Accept", &accept)
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            AgentError::transient(
                "NETWORK_ERROR",
                format!("Bedrock invoke-model request failed: {e}"),
            )
            .with_attr("integration", "BEDROCK")
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (category, code) = if status == 429 {
            ("transient", "HTTP_429")
        } else if (500..600).contains(&status) {
            ("transient", "HTTP_5XX")
        } else {
            ("permanent", "HTTP_4XX")
        };
        let err = if category == "transient" {
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
        return Err(err
            .with_attr("integration", "BEDROCK")
            .with_attr("status_code", status.to_string())
            .with_attr("body", truncate(&body_text, 512)));
    }

    let response_content_type = response
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "application/json".to_string());

    let body: Value = serde_json::from_slice(&response.body).map_err(|e| {
        AgentError::permanent(
            "BEDROCK_INVALID_RESPONSE",
            format!("Expected JSON response from Bedrock: {e}"),
        )
        .with_attr("integration", "BEDROCK")
    })?;

    Ok(BedrockInvokeModelOutput {
        body,
        content_type: response_content_type,
    })
}

// ============================================================================
// Capability 7: List Models
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bedrock List Models Input")]
pub struct BedrockListModelsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Bedrock List Models Output")]
pub struct BedrockListModelsOutput {
    #[field(
        display_name = "Model Summaries",
        description = "Array of available foundation models with their details"
    )]
    pub model_summaries: Vec<Value>,
}

#[capability(
    module = "bedrock",
    display_name = "List Models",
    description = "List available foundation models in AWS Bedrock"
)]
pub fn bedrock_list_models(
    input: BedrockListModelsInput,
) -> Result<BedrockListModelsOutput, AgentError> {
    let connection = require_connection(input._connection.as_ref())?;

    // list-foundation-models is on the control-plane endpoint
    // (bedrock.region.amazonaws.com), not the runtime endpoint. The proxy
    // resolves the region from the connection.
    let resp = bedrock_get(connection, "/foundation-models", 30_000)?;

    let model_summaries = resp["modelSummaries"]
        .as_array()
        .ok_or_else(|| {
            AgentError::permanent(
                "BEDROCK_INVALID_RESPONSE",
                "Missing modelSummaries in Bedrock response",
            )
            .with_attr("integration", "BEDROCK")
        })?
        .clone();

    Ok(BedrockListModelsOutput { model_summaries })
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
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_TEXT_COMPLETION,
        &__CAPABILITY_META_IMAGE_GENERATION,
        &__CAPABILITY_META_STRUCTURED_OUTPUT,
        &__CAPABILITY_META_VISION_TO_TEXT,
        &__CAPABILITY_META_VISION_TO_IMAGE,
        &__CAPABILITY_META_BEDROCK_INVOKE_MODEL,
        &__CAPABILITY_META_BEDROCK_LIST_MODELS,
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
            "BedrockInvokeModelInput",
            &__INPUT_META_BedrockInvokeModelInput as &InputTypeMeta,
        ),
        (
            "BedrockListModelsInput",
            &__INPUT_META_BedrockListModelsInput as &InputTypeMeta,
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
            "BedrockInvokeModelOutput",
            &__OUTPUT_META_BedrockInvokeModelOutput as &OutputTypeMeta,
        ),
        (
            "BedrockListModelsOutput",
            &__OUTPUT_META_BedrockListModelsOutput as &OutputTypeMeta,
        ),
        ("LlmUsage", &__OUTPUT_META_LlmUsage as &OutputTypeMeta),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
            )
        })
        .collect();

    AgentInfo {
        id: "bedrock".into(),
        name: "AWS Bedrock".into(),
        description:
            "AWS Bedrock LLM integration for text completion, image generation, structured output, and vision capabilities using Claude and Titan models."
                .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["aws_credentials".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent::capabilities::{ConnectionInfo, ErrorInfo, Guest};

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
            "image-generation" => __executor_image_generation(value),
            "structured-output" => __executor_structured_output(value),
            "vision-to-text" => __executor_vision_to_text(value),
            "vision-to-image" => __executor_vision_to_image(value),
            "bedrock-invoke-model" => __executor_bedrock_invoke_model(value),
            "bedrock-list-models" => __executor_bedrock_list_models(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("bedrock agent has no capability `{other}`"),
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
