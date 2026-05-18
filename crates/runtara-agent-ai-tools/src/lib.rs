//! AI Tools integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/ai_tools.rs`.
//!
//! This is a provider-router: at invoke time it inspects `connection.integration_id`
//! and dispatches to the correct provider (OpenAI or AWS Bedrock). The runtara HTTP
//! proxy handles credential injection and base-URL rewriting for each provider
//! (OpenAI: `https://api.openai.com`; Bedrock: `https://bedrock-runtime.{region}.amazonaws.com`).
//!
//! Capabilities:
//! - `text-completion`   — text generation with optional structured output
//! - `image-generation`  — image generation
//! - `vision-to-text`    — image analysis with optional structured output
//! - `vision-to-image`   — image editing/manipulation
//! - `embed-text`        — vector embedding for one or more strings

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::time::Duration;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// -----------------------------------------------------------------------------
// Provider routing
// -----------------------------------------------------------------------------

const PROVIDER_OPENAI: &str = "openai_api_key";
const PROVIDER_BEDROCK: &str = "aws_credentials";

fn provider_of(connection: &ConnectionInfo) -> &str {
    connection.integration_id.as_str()
}

// -----------------------------------------------------------------------------
// Shared types
// -----------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmUsage {
    prompt_tokens: i32,
    completion_tokens: i32,
    total_tokens: i32,
}

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "ai_tools".into(),
            display_name: "AI Tools".into(),
            description: "AI tools — deterministic AI capabilities for text completion, image \
                          generation, structured output, and vision across multiple LLM providers."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec![PROVIDER_OPENAI.into(), PROVIDER_BEDROCK.into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "text-completion",
                "ai_text_completion",
                "Text Completion",
                "Generate text completion using any LLM provider. Supports optional structured \
                 output via output_schema.",
                TEXT_COMPLETION_INPUT_SCHEMA,
                TEXT_COMPLETION_OUTPUT_SCHEMA,
            ),
            cap(
                "image-generation",
                "ai_image_generation",
                "Image Generation",
                "Generate images using AI image generation models",
                IMAGE_GENERATION_INPUT_SCHEMA,
                IMAGE_GENERATION_OUTPUT_SCHEMA,
            ),
            cap(
                "vision-to-text",
                "ai_vision_to_text",
                "Vision to Text",
                "Analyze images and generate text descriptions. Supports optional structured \
                 output via output_schema.",
                VISION_TO_TEXT_INPUT_SCHEMA,
                VISION_TO_TEXT_OUTPUT_SCHEMA,
            ),
            cap(
                "vision-to-image",
                "ai_vision_to_image",
                "Vision to Image",
                "Edit and manipulate images using AI models",
                VISION_TO_IMAGE_INPUT_SCHEMA,
                VISION_TO_IMAGE_OUTPUT_SCHEMA,
            ),
            cap(
                "embed-text",
                "ai_embed_text",
                "Embed Text",
                "Generate vector embeddings for one or more strings. Use the result to populate \
                 a Vector column for similarity search.",
                EMBED_TEXT_INPUT_SCHEMA,
                EMBED_TEXT_OUTPUT_SCHEMA,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "text-completion" => text_completion(&input, connection.as_ref()),
            "image-generation" => image_generation(&input, connection.as_ref()),
            "vision-to-text" => vision_to_text(&input, connection.as_ref()),
            "vision-to-image" => vision_to_image(&input, connection.as_ref()),
            "embed-text" => embed_text(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("ai_tools agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Helper: build CapabilityInfo with AI-tools-appropriate flags
// -----------------------------------------------------------------------------

fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects: true,
        is_idempotent: false,
        rate_limited: true,
        tags: vec!["ai".into(), "llm".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// -----------------------------------------------------------------------------
// Connection helpers
// -----------------------------------------------------------------------------

fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection
        .ok_or_else(|| permanent_err("AI_TOOLS_MISSING_CONNECTION", "LLM connection is required"))
}

// -----------------------------------------------------------------------------
// OpenAI HTTP helper
// -----------------------------------------------------------------------------

/// POST `body` to `https://api.openai.com{path}` via the runtara proxy.
fn openai_post(
    connection: &ConnectionInfo,
    path: &str,
    body: Value,
    timeout_ms: u64,
) -> Result<Value, ErrorInfo> {
    let url = format!("https://api.openai.com{path}");
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            transient_err(
                "NETWORK_ERROR",
                format!("OpenAI request to {path} failed: {e}"),
            )
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (category, code) = classify_http_status(status);
        let retry_after_ms = parse_retry_after(&response.headers);
        return Err(ErrorInfo {
            code: code.into(),
            message: format!("OpenAI HTTP {status}: {}", truncate(&body_text, 512)),
            category: category.into(),
            severity: "error".into(),
            retryable: category == "transient",
            retry_after_ms,
            attributes: serde_json::to_string(&json!({"status_code": status, "path": path})).ok(),
        });
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        permanent_err(
            "RESPONSE_PARSE_ERROR",
            format!("OpenAI response parse error: {e}"),
        )
    })
}

// -----------------------------------------------------------------------------
// Bedrock HTTP helper
// -----------------------------------------------------------------------------

/// POST `body` to `https://bedrock-runtime.{region}.amazonaws.com{path}` via the
/// runtara proxy. The proxy injects SigV4 signing and the regional base URL
/// from the aws_credentials connection parameters.
fn bedrock_post(
    connection: &ConnectionInfo,
    path: &str,
    body: Value,
    timeout_ms: u64,
) -> Result<Value, ErrorInfo> {
    // The proxy resolves the full URL from the connection's region parameter and
    // prepends the base URL. We send a relative path so the proxy can construct
    // the correct regional endpoint (e.g. https://bedrock-runtime.us-east-1.amazonaws.com).
    // Using the relative path form matches how the legacy ProxyHttpClient works.
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("POST", path)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .body_bytes(&body_bytes)
        .call_agent()
        .map_err(|e| {
            transient_err(
                "NETWORK_ERROR",
                format!("Bedrock request to {path} failed: {e}"),
            )
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        let (category, code) = classify_http_status(status);
        let retry_after_ms = parse_retry_after(&response.headers);
        return Err(ErrorInfo {
            code: code.into(),
            message: format!("Bedrock HTTP {status}: {}", truncate(&body_text, 512)),
            category: category.into(),
            severity: "error".into(),
            retryable: category == "transient",
            retry_after_ms,
            attributes: serde_json::to_string(&json!({"status_code": status, "path": path})).ok(),
        });
    }

    serde_json::from_slice(&response.body).map_err(|e| {
        permanent_err(
            "RESPONSE_PARSE_ERROR",
            format!("Bedrock response parse error: {e}"),
        )
    })
}

// -----------------------------------------------------------------------------
// Capability 1: Text Completion
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TextCompletionInput {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    max_tokens: Option<i32>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    top_p: Option<f64>,
    #[serde(default)]
    stop_sequences: Option<Vec<String>>,
    #[serde(default)]
    output_schema: Option<Value>,
}

#[derive(Debug, Serialize)]
struct TextCompletionOutput {
    text: String,
    model: String,
    usage: LlmUsage,
    finish_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_output: Option<Value>,
}

fn text_completion(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: TextCompletionInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    match provider_of(connection) {
        PROVIDER_OPENAI => text_completion_openai(&input, connection),
        PROVIDER_BEDROCK => text_completion_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn text_completion_openai(
    input: &TextCompletionInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
    // If output_schema is provided, use OpenAI structured output path.
    if let Some(ref schema) = input.output_schema {
        return text_completion_openai_structured(input, connection, schema);
    }

    let mut messages = Vec::new();
    if let Some(system) = &input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    let model = input.model.clone().unwrap_or_else(|| "gpt-4".to_string());
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
    if let Some(temperature) = input.temperature {
        if !is_o_series {
            body["temperature"] = json!(temperature);
        }
    }
    if let Some(top_p) = input.top_p {
        if !is_o_series {
            body["top_p"] = json!(top_p);
        }
    }
    if let Some(stop) = &input.stop_sequences {
        if !is_o_series {
            body["stop"] = json!(stop);
        }
    }

    let resp = openai_post(connection, "/v1/chat/completions", body, 120_000)?;
    let text = openai_extract_content(&resp)?;
    let model_used = resp["model"].as_str().unwrap_or("unknown").to_string();
    let finish_reason = resp["choices"][0]["finish_reason"]
        .as_str()
        .unwrap_or("stop")
        .to_string();
    let usage = extract_openai_usage(&resp);

    serde_json::to_string(&TextCompletionOutput {
        text,
        model: model_used,
        usage,
        finish_reason,
        structured_output: None,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn text_completion_openai_structured(
    input: &TextCompletionInput,
    connection: &ConnectionInfo,
    schema: &Value,
) -> Result<String, ErrorInfo> {
    let mut messages = Vec::new();
    if let Some(system) = &input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    let mut body = json!({
        "model": input.model.clone().unwrap_or_else(|| "gpt-4o-mini".to_string()),
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
        permanent_err(
            "OPENAI_INVALID_RESPONSE",
            format!("Failed to parse structured output: {e}"),
        )
    })?;
    let model_used = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_openai_usage(&resp);
    let text = serde_json::to_string(&structured_output).unwrap_or_default();

    serde_json::to_string(&TextCompletionOutput {
        text,
        model: model_used,
        usage,
        finish_reason: "stop".to_string(),
        structured_output: Some(structured_output),
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn text_completion_bedrock(
    input: &TextCompletionInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
    // If output_schema is provided, use prompt-engineering path for structured output.
    if let Some(ref schema) = input.output_schema {
        return text_completion_bedrock_structured(input, connection, schema);
    }

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string());

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

    serde_json::to_string(&TextCompletionOutput {
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
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn text_completion_bedrock_structured(
    input: &TextCompletionInput,
    connection: &ConnectionInfo,
    schema: &Value,
) -> Result<String, ErrorInfo> {
    let schema_str = serde_json::to_string_pretty(schema)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;
    let enhanced_prompt = format!(
        "{}\n\nRespond with valid JSON matching this schema:\n{}\n\nReturn ONLY the JSON, no other text.",
        input.prompt, schema_str
    );

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string());
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
        permanent_err(
            "BEDROCK_INVALID_RESPONSE",
            format!("Failed to parse structured output as JSON: {e}"),
        )
    })?;
    let serialized_text = serde_json::to_string(&structured_output).unwrap_or_default();

    serde_json::to_string(&TextCompletionOutput {
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
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 2: Image Generation
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ImageGenerationInput {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    negative_prompt: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    style: Option<String>,
}

#[derive(Debug, Serialize)]
struct ImageGenerationOutput {
    image_data: String,
    mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<i32>,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    revised_prompt: Option<String>,
}

fn image_generation(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: ImageGenerationInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    match provider_of(connection) {
        PROVIDER_OPENAI => image_generation_openai(&input, connection),
        PROVIDER_BEDROCK => image_generation_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn image_generation_openai(
    input: &ImageGenerationInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
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

    let resp = openai_post(connection, "/v1/images/generations", body, 180_000)?;

    let image_data = resp["data"][0]["b64_json"]
        .as_str()
        .ok_or_else(|| {
            permanent_err(
                "OPENAI_INVALID_RESPONSE",
                "Missing b64_json in OpenAI response",
            )
        })?
        .to_string();
    let revised_prompt = resp["data"][0]["revised_prompt"]
        .as_str()
        .map(|s| s.to_string());

    serde_json::to_string(&ImageGenerationOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
        revised_prompt,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn image_generation_bedrock(
    input: &ImageGenerationInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
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
    let resp = bedrock_post(connection, &path, request_body, 180_000)?;

    let image_data = resp["artifacts"][0]["base64"]
        .as_str()
        .ok_or_else(|| {
            permanent_err(
                "BEDROCK_INVALID_RESPONSE",
                "Missing base64 image in Bedrock response",
            )
        })?
        .to_string();

    serde_json::to_string(&ImageGenerationOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
        revised_prompt: None,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 3: Vision to Text
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VisionToTextInput {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    image_data: Option<String>,
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    max_tokens: Option<i32>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    output_schema: Option<Value>,
}

#[derive(Debug, Serialize)]
struct VisionToTextOutput {
    text: String,
    model: String,
    usage: LlmUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_output: Option<Value>,
}

fn vision_to_text(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: VisionToTextInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    match provider_of(connection) {
        PROVIDER_OPENAI => vision_to_text_openai(&input, connection),
        PROVIDER_BEDROCK => vision_to_text_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn vision_to_text_openai(
    input: &VisionToTextInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(permanent_err(
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

    let model = input.model.clone().unwrap_or_else(|| "gpt-4o".to_string());
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
    if let Some(temperature) = input.temperature {
        if !is_o_series {
            body["temperature"] = json!(temperature);
        }
    }

    let resp = openai_post(connection, "/v1/chat/completions", body, 120_000)?;
    let text = openai_extract_content(&resp)?;
    let model_used = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_openai_usage(&resp);
    let structured_output = parse_structured_output(&text, &input.output_schema);

    serde_json::to_string(&VisionToTextOutput {
        text,
        model: model_used,
        usage,
        structured_output,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn vision_to_text_bedrock(
    input: &VisionToTextInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(permanent_err(
            "AI_TOOLS_INVALID_INPUT",
            "Either image_data or image_url is required",
        ));
    }

    // Bedrock vision only supports base64 image data, not URLs.
    if input.image_url.is_some() && input.image_data.is_none() {
        return Err(permanent_err(
            "AI_TOOLS_INVALID_INPUT",
            "Bedrock vision requires base64-encoded image_data, not URLs",
        ));
    }

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string());

    // Only Claude 3/3.5 supports vision in Bedrock.
    if !model.starts_with("anthropic.claude-3") {
        return Err(permanent_err(
            "AI_TOOLS_UNSUPPORTED_MODEL",
            "Bedrock vision capabilities require Claude 3 or Claude 3.5 models",
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
            permanent_err(
                "BEDROCK_INVALID_RESPONSE",
                "Missing text in Bedrock vision response",
            )
        })?
        .to_string();

    let prompt_tokens = resp["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32;
    let completion_tokens = resp["usage"]["output_tokens"].as_i64().unwrap_or(0) as i32;
    let structured_output = parse_structured_output(&text, &input.output_schema);

    serde_json::to_string(&VisionToTextOutput {
        text,
        model,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
        structured_output,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 4: Vision to Image
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VisionToImageInput {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    image_data: String,
    #[serde(default)]
    mask_data: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
}

#[derive(Debug, Serialize)]
struct VisionToImageOutput {
    image_data: String,
    mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<i32>,
    model: String,
}

fn vision_to_image(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: VisionToImageInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    match provider_of(connection) {
        PROVIDER_OPENAI => vision_to_image_openai(&input, connection),
        PROVIDER_BEDROCK => vision_to_image_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn vision_to_image_openai(
    input: &VisionToImageInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
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

    let resp = openai_post(connection, &format!("/v1/{endpoint}"), body, 180_000)?;

    let image_data = resp["data"][0]["b64_json"]
        .as_str()
        .ok_or_else(|| {
            permanent_err(
                "OPENAI_INVALID_RESPONSE",
                "Missing b64_json in OpenAI response",
            )
        })?
        .to_string();
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "dall-e-2".to_string());

    serde_json::to_string(&VisionToImageOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn vision_to_image_bedrock(
    input: &VisionToImageInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
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
    let resp = bedrock_post(connection, &path, request_body, 180_000)?;

    let image_data = resp["artifacts"][0]["base64"]
        .as_str()
        .ok_or_else(|| {
            permanent_err(
                "BEDROCK_INVALID_RESPONSE",
                "Missing base64 image in Bedrock response",
            )
        })?
        .to_string();

    serde_json::to_string(&VisionToImageOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 5: Embed Text
// -----------------------------------------------------------------------------

const AI_EMBED_TEXT_BATCH_CAP: usize = 2048;
const AI_EMBED_TEXT_MAX_DIM: u32 = 4096;

#[derive(Debug, Deserialize)]
struct EmbedTextInput {
    #[serde(default)]
    texts: Vec<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    dimension: Option<u32>,
}

#[derive(Debug, Serialize)]
struct EmbedTextOutput {
    embeddings: Vec<Vec<f32>>,
    model: String,
    dimension: u32,
    usage: LlmUsage,
}

fn embed_text(input_json: &str, connection: Option<&ConnectionInfo>) -> Result<String, ErrorInfo> {
    let input: EmbedTextInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    // Validation (mirrors legacy ai_embed_text)
    if input.texts.is_empty() {
        return Err(permanent_err(
            "AI_TOOLS_INVALID_INPUT",
            "`texts` must contain at least one entry",
        ));
    }
    if input.texts.iter().any(|t| t.is_empty()) {
        return Err(permanent_err(
            "AI_TOOLS_INVALID_INPUT",
            "`texts` entries must be non-empty",
        ));
    }
    if input.texts.len() > AI_EMBED_TEXT_BATCH_CAP {
        return Err(ErrorInfo {
            code: "AI_TOOLS_BATCH_TOO_LARGE".into(),
            message: format!(
                "`texts` batch size {} exceeds cap {}",
                input.texts.len(),
                AI_EMBED_TEXT_BATCH_CAP
            ),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({
                "batch": input.texts.len(),
                "cap": AI_EMBED_TEXT_BATCH_CAP,
            }))
            .ok(),
        });
    }
    if let Some(d) = input.dimension {
        if d == 0 || d > AI_EMBED_TEXT_MAX_DIM {
            return Err(permanent_err(
                "AI_TOOLS_INVALID_INPUT",
                format!("`dimension` must be in 1..={}", AI_EMBED_TEXT_MAX_DIM),
            ));
        }
    }

    match provider_of(connection) {
        PROVIDER_OPENAI => embed_text_openai(&input, connection),
        PROVIDER_BEDROCK => embed_text_bedrock(&input, connection),
        other => Err(unsupported_provider(other)),
    }
}

fn embed_text_openai(
    input: &EmbedTextInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
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
        permanent_err(
            "OPENAI_INVALID_RESPONSE",
            "Missing data array in OpenAI embeddings response",
        )
    })?;

    let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(data.len());
    for item in data {
        let arr = item["embedding"].as_array().ok_or_else(|| {
            permanent_err(
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

    serde_json::to_string(&EmbedTextOutput {
        embeddings,
        model: model_used,
        dimension,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens: 0,
            total_tokens,
        },
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn embed_text_bedrock(
    input: &EmbedTextInput,
    connection: &ConnectionInfo,
) -> Result<String, ErrorInfo> {
    let model = input
        .model
        .clone()
        .unwrap_or_else(|| "amazon.titan-embed-text-v2:0".to_string());

    // Anthropic models do not support embeddings in Bedrock.
    if model.starts_with("anthropic") {
        return Err(permanent_err(
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
            permanent_err(
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
    if let Some(req_dim) = input.dimension {
        if dimension != req_dim {
            return Err(permanent_err(
                "BEDROCK_DIMENSION_MISMATCH",
                format!(
                    "Requested dimension {} but Bedrock returned {}",
                    req_dim, dimension
                ),
            ));
        }
    }

    serde_json::to_string(&EmbedTextOutput {
        embeddings,
        model,
        dimension,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens: 0,
            total_tokens: prompt_tokens,
        },
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Bedrock request builders
// -----------------------------------------------------------------------------

/// Build a Bedrock text-generation request body. Returns `(body, is_claude)`.
fn build_bedrock_text_request(
    prompt: &str,
    system_prompt: Option<&str>,
    model: &str,
    max_tokens: Option<i32>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    stop_sequences: Option<&[String]>,
) -> Result<(Value, bool), ErrorInfo> {
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
        Err(permanent_err(
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
) -> Result<(String, i32, i32, String), ErrorInfo> {
    if is_claude {
        let text = resp["content"][0]["text"]
            .as_str()
            .ok_or_else(|| {
                permanent_err(
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
                permanent_err(
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

// -----------------------------------------------------------------------------
// Shared utilities
// -----------------------------------------------------------------------------

fn extract_openai_usage(resp: &Value) -> LlmUsage {
    LlmUsage {
        prompt_tokens: resp["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32,
        completion_tokens: resp["usage"]["completion_tokens"].as_i64().unwrap_or(0) as i32,
        total_tokens: resp["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    }
}

fn openai_extract_content(resp: &Value) -> Result<String, ErrorInfo> {
    resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            permanent_err(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
            )
        })
        .map(|s| s.to_string())
}

fn is_openai_o_series(model: &str) -> bool {
    model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4")
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

fn parse_retry_after(headers: &std::collections::HashMap<String, String>) -> Option<u64> {
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

fn unsupported_provider(integration_id: &str) -> ErrorInfo {
    ErrorInfo {
        code: "AI_TOOLS_UNSUPPORTED_PROVIDER".into(),
        message: format!("LLM provider not supported: {}", integration_id),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: serde_json::to_string(&json!({"integration_id": integration_id})).ok(),
    }
}

fn permanent_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn transient_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "transient".into(),
        severity: "warning".into(),
        retryable: true,
        retry_after_ms: None,
        attributes: None,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push_str("\u{2026}");
        t
    }
}

// -----------------------------------------------------------------------------
// JSON Schemas — field names and defaults match the legacy file exactly
// -----------------------------------------------------------------------------

const TEXT_COMPLETION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":          { "type": "string", "description": "The user message or prompt to send to the LLM", "example": "Explain quantum computing in simple terms" },
        "system_prompt":   { "type": "string", "description": "Optional system instructions to set the assistant's behavior", "example": "You are a helpful assistant" },
        "model":           { "type": "string", "description": "The model identifier to use (auto-selects based on provider if not specified)", "example": "gpt-4o" },
        "max_tokens":      { "type": "integer", "description": "Maximum number of tokens to generate in the response", "example": 1024 },
        "temperature":     { "type": "number", "description": "Sampling temperature (0-2). Higher values increase randomness", "example": 0.7 },
        "top_p":           { "type": "number", "description": "Nucleus sampling parameter for controlling diversity", "example": 0.9 },
        "stop_sequences":  { "type": "array", "items": { "type": "string" }, "description": "Sequences where the model will stop generating further tokens", "example": ["END", "STOP"] },
        "output_schema":   { "description": "Optional JSON schema for structured output. When provided, the model returns JSON conforming to this schema.", "example": {"type": "object", "properties": {"name": {"type": "string"}}} }
    }
}"#;

const TEXT_COMPLETION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "text":             { "type": "string", "description": "The generated text response from the model" },
        "model":            { "type": "string", "description": "The model used for generation" },
        "usage":            { "type": "object", "description": "Token usage statistics", "properties": { "promptTokens": { "type": "integer" }, "completionTokens": { "type": "integer" }, "totalTokens": { "type": "integer" } } },
        "finish_reason":    { "type": "string", "description": "The reason generation stopped (e.g., 'stop', 'length')" },
        "structured_output":{ "description": "Parsed JSON output when output_schema was provided" }
    }
}"#;

const IMAGE_GENERATION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":          { "type": "string", "description": "Text description of the image to generate", "example": "A serene landscape with mountains at sunset" },
        "negative_prompt": { "type": "string", "description": "Elements to exclude from the generated image", "example": "blurry, low quality, distorted" },
        "model":           { "type": "string", "description": "Image generation model to use", "example": "dall-e-3" },
        "width":           { "type": "integer", "description": "Desired image width in pixels", "example": 1024 },
        "height":          { "type": "integer", "description": "Desired image height in pixels", "example": 1024 },
        "quality":         { "type": "string", "description": "Image quality setting (e.g., 'standard', 'hd')", "example": "hd" },
        "style":           { "type": "string", "description": "Image style preset (e.g., 'vivid', 'natural')", "example": "vivid" }
    }
}"#;

const IMAGE_GENERATION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "image_data":     { "type": "string", "description": "Base64-encoded image data" },
        "mime_type":      { "type": "string", "description": "Image format (e.g., 'image/png')" },
        "width":          { "type": "integer", "description": "Actual image width in pixels" },
        "height":         { "type": "integer", "description": "Actual image height in pixels" },
        "model":          { "type": "string", "description": "Model used for generation" },
        "revised_prompt": { "type": "string", "description": "AI-revised prompt if the model modified it" }
    }
}"#;

const VISION_TO_TEXT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":       { "type": "string", "description": "Question or instruction about the image", "example": "Describe what you see in this image" },
        "image_data":   { "type": "string", "description": "Base64-encoded image data (provide either image_data or image_url)", "example": "iVBORw0KGgoAAAANSUhEUg..." },
        "image_url":    { "type": "string", "description": "URL of the image to analyze (provide either image_data or image_url)", "example": "https://example.com/image.png" },
        "model":        { "type": "string", "description": "Vision model to use", "example": "gpt-4o" },
        "max_tokens":   { "type": "integer", "description": "Maximum number of tokens to generate", "example": 1024 },
        "temperature":  { "type": "number", "description": "Sampling temperature", "example": 0.7 },
        "output_schema":{ "description": "Optional JSON schema for structured output. When provided, the model returns JSON conforming to this schema.", "example": {"type": "object", "properties": {"objects": {"type": "array"}}} }
    }
}"#;

const VISION_TO_TEXT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "text":             { "type": "string", "description": "The generated text description or analysis" },
        "model":            { "type": "string", "description": "Model used for analysis" },
        "usage":            { "type": "object", "description": "Token usage statistics", "properties": { "promptTokens": { "type": "integer" }, "completionTokens": { "type": "integer" }, "totalTokens": { "type": "integer" } } },
        "structured_output":{ "description": "Parsed JSON output when output_schema was provided" }
    }
}"#;

const VISION_TO_IMAGE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt", "image_data"],
    "properties": {
        "prompt":     { "type": "string", "description": "Instructions for how to modify the image", "example": "Add dramatic lighting to the scene" },
        "image_data": { "type": "string", "description": "Base64-encoded source image to edit", "example": "iVBORw0KGgoAAAANSUhEUg..." },
        "mask_data":  { "type": "string", "description": "Optional base64-encoded mask for selective editing", "example": "iVBORw0KGgoAAAANSUhEUg..." },
        "model":      { "type": "string", "description": "Image editing model to use", "example": "dall-e-2" },
        "width":      { "type": "integer", "description": "Desired output width in pixels", "example": 1024 },
        "height":     { "type": "integer", "description": "Desired output height in pixels", "example": 1024 }
    }
}"#;

const VISION_TO_IMAGE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "image_data": { "type": "string", "description": "Base64-encoded modified image" },
        "mime_type":  { "type": "string", "description": "Image format (e.g., 'image/png')" },
        "width":      { "type": "integer", "description": "Actual output width in pixels" },
        "height":     { "type": "integer", "description": "Actual output height in pixels" },
        "model":      { "type": "string", "description": "Model used for editing" }
    }
}"#;

const EMBED_TEXT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["texts"],
    "properties": {
        "texts":     { "type": "array", "items": { "type": "string" }, "description": "Batch of input strings to embed. Provider-specific batch limits apply (OpenAI ≤2048; Bedrock Titan loops sequentially).", "example": ["hello", "world"] },
        "model":     { "type": "string", "description": "Embedding model override. Defaults: OpenAI = text-embedding-3-small, Bedrock = amazon.titan-embed-text-v2:0", "example": "text-embedding-3-small" },
        "dimension": { "type": "integer", "description": "Optional output dimension. Must match the target Vector column. Workflow author is responsible for alignment.", "example": 1536 }
    }
}"#;

const EMBED_TEXT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "embeddings": { "type": "array", "items": { "type": "array", "items": { "type": "number" } }, "description": "One f32 vector per input string, in the same order as the input." },
        "model":      { "type": "string", "description": "The model that produced the embeddings" },
        "dimension":  { "type": "integer", "description": "Dimensionality of each returned vector" },
        "usage":      { "type": "object", "description": "Token usage statistics", "properties": { "promptTokens": { "type": "integer" }, "completionTokens": { "type": "integer" }, "totalTokens": { "type": "integer" } } }
    }
}"#;

bindings::export!(Component with_types_in bindings);
