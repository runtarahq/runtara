//! OpenAI integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/openai.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope, injecting `X-Runtara-Connection-Id` so the proxy can attach
//! the OpenAI API key server-side. The component never sees secrets.

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
// Shared types (mirror the legacy schema)
// -----------------------------------------------------------------------------

/// Token usage statistics — mirrors `LlmUsage` (serde renamed camelCase in the
/// legacy struct, but the wire format uses camelCase keys).
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
            id: "openai".into(),
            display_name: "OpenAI".into(),
            description: "OpenAI LLM integration for text completion, image generation, \
                          structured output, and vision capabilities."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["openai_api_key".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "text-completion",
                "text_completion",
                "Text Completion (OpenAI)",
                "Generate text completion using OpenAI models",
                TEXT_COMPLETION_INPUT_SCHEMA,
                TEXT_COMPLETION_OUTPUT_SCHEMA,
            ),
            cap(
                "image-generation",
                "image_generation",
                "Image Generation (OpenAI)",
                "Generate images using OpenAI DALL-E models",
                IMAGE_GENERATION_INPUT_SCHEMA,
                IMAGE_GENERATION_OUTPUT_SCHEMA,
            ),
            cap(
                "structured-output",
                "structured_output",
                "Structured Output (OpenAI)",
                "Generate structured JSON output using OpenAI models with schema validation",
                STRUCTURED_OUTPUT_INPUT_SCHEMA,
                STRUCTURED_OUTPUT_OUTPUT_SCHEMA,
            ),
            cap(
                "vision-to-text",
                "vision_to_text",
                "Vision to Text (OpenAI)",
                "Analyze images and generate text descriptions using OpenAI vision models",
                VISION_TO_TEXT_INPUT_SCHEMA,
                VISION_TO_TEXT_OUTPUT_SCHEMA,
            ),
            cap(
                "vision-to-image",
                "vision_to_image",
                "Vision to Image (OpenAI)",
                "Edit and manipulate images using OpenAI DALL-E models",
                VISION_TO_IMAGE_INPUT_SCHEMA,
                VISION_TO_IMAGE_OUTPUT_SCHEMA,
            ),
            cap(
                "chat-completion",
                "openai_chat_completion",
                "Chat Completion",
                "OpenAI chat completion with full control over messages, tools, and parameters",
                CHAT_COMPLETION_INPUT_SCHEMA,
                CHAT_COMPLETION_OUTPUT_SCHEMA,
            ),
            cap(
                "create-embedding",
                "openai_create_embedding",
                "Create Embedding",
                "Generate embeddings for text using OpenAI embedding models",
                CREATE_EMBEDDING_INPUT_SCHEMA,
                CREATE_EMBEDDING_OUTPUT_SCHEMA,
            ),
            cap(
                "moderate-content",
                "openai_moderate_content",
                "Moderate Content",
                "Check content for policy violations using OpenAI moderation API",
                MODERATE_CONTENT_INPUT_SCHEMA,
                MODERATE_CONTENT_OUTPUT_SCHEMA,
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
            "structured-output" => structured_output(&input, connection.as_ref()),
            "vision-to-text" => vision_to_text(&input, connection.as_ref()),
            "vision-to-image" => vision_to_image(&input, connection.as_ref()),
            "chat-completion" => chat_completion(&input, connection.as_ref()),
            "create-embedding" => create_embedding(&input, connection.as_ref()),
            "moderate-content" => moderate_content(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("openai agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Helper: build a CapabilityInfo with OpenAI-appropriate flags
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
        tags: vec!["openai".into(), "llm".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// -----------------------------------------------------------------------------
// Shared HTTP helper — post JSON to OpenAI via the proxy
// -----------------------------------------------------------------------------

/// POST `body` to `https://api.openai.com{path}` via the runtara proxy.
/// The proxy injects `Authorization: Bearer <key>` from the connection.
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
        let (category, code) = if status == 429 {
            ("transient", "HTTP_429")
        } else if (500..600).contains(&status) {
            ("transient", "HTTP_5XX")
        } else {
            ("permanent", "HTTP_4XX")
        };
        let retry_after_ms = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after-ms"))
            .and_then(|(_, v)| v.parse::<u64>().ok())
            .or_else(|| {
                response
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, v)| v.parse::<u64>().ok())
                    .map(|s| s * 1000)
            });
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

/// Require a connection or return `OPENAI_MISSING_CONNECTION` (wire-compatible
/// with the legacy error code).
fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection
        .ok_or_else(|| permanent_err("OPENAI_MISSING_CONNECTION", "OpenAI connection is required"))
}

// -----------------------------------------------------------------------------
// Capability 1: Text Completion
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TextCompletionInput {
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
}

#[derive(Debug, Serialize)]
struct TextCompletionOutput {
    text: String,
    model: String,
    usage: LlmUsage,
    finish_reason: String,
}

fn text_completion(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: TextCompletionInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    let mut messages = Vec::new();
    if let Some(system) = &input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    let model = input.model.unwrap_or_else(|| "gpt-4".to_string());
    let is_o_series = model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4");

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
    if let Some(stop) = input.stop_sequences {
        if !is_o_series {
            body["stop"] = json!(stop);
        }
    }

    let resp = openai_post(connection, "/v1/chat/completions", body, 120_000)?;

    let text = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            permanent_err(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
            )
        })?
        .to_string();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    let finish_reason = resp["choices"][0]["finish_reason"]
        .as_str()
        .unwrap_or("stop")
        .to_string();
    let usage = extract_usage(&resp);

    serde_json::to_string(&TextCompletionOutput {
        text,
        model,
        usage,
        finish_reason,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 2: Image Generation
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ImageGenerationInput {
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
    width: Option<i32>,
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
    let width = input.width.or(Some(1024));
    let height = input.height.or(Some(1024));

    serde_json::to_string(&ImageGenerationOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width,
        height,
        model,
        revised_prompt,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 3: Structured Output
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct StructuredOutputInput {
    prompt: String,
    #[serde(default)]
    system_prompt: Option<String>,
    json_schema: Value,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    temperature: Option<f64>,
}

#[derive(Debug, Serialize)]
struct StructuredOutputOutput {
    output: Value,
    model: String,
    usage: LlmUsage,
}

fn structured_output(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: StructuredOutputInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    let mut messages = Vec::new();
    if let Some(system) = &input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    let mut body = json!({
        "model": input.model.unwrap_or_else(|| "gpt-4o-mini".to_string()),
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

    let resp = openai_post(connection, "/v1/chat/completions", body, 120_000)?;

    let content = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            permanent_err(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
            )
        })?;
    let output: Value = serde_json::from_str(content).map_err(|e| {
        permanent_err(
            "OPENAI_INVALID_RESPONSE",
            format!("Failed to parse structured output: {e}"),
        )
    })?;
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_usage(&resp);

    serde_json::to_string(&StructuredOutputOutput {
        output,
        model,
        usage,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 4: Vision to Text
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VisionToTextInput {
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
}

#[derive(Debug, Serialize)]
struct VisionToTextOutput {
    text: String,
    model: String,
    usage: LlmUsage,
}

fn vision_to_text(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: VisionToTextInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(permanent_err(
            "OPENAI_INVALID_INPUT",
            "Either image_data or image_url is required",
        ));
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

    let model = input.model.unwrap_or_else(|| "gpt-4o".to_string());
    let is_o_series = model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4");

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

    let text = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            permanent_err(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
            )
        })?
        .to_string();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_usage(&resp);

    serde_json::to_string(&VisionToTextOutput { text, model, usage })
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 5: Vision to Image
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VisionToImageInput {
    prompt: String,
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
    width: Option<i32>,
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
    let model = input.model.unwrap_or_else(|| "dall-e-2".to_string());

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
// Capability 6: Chat Completion (raw)
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChatCompletionInput {
    messages: Vec<Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    max_tokens: Option<i32>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    top_p: Option<f64>,
    #[serde(default)]
    frequency_penalty: Option<f64>,
    #[serde(default)]
    presence_penalty: Option<f64>,
    #[serde(default)]
    stop: Option<Vec<String>>,
    #[serde(default)]
    tools: Option<Vec<Value>>,
    #[serde(default)]
    tool_choice: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionOutput {
    choices: Vec<Value>,
    model: String,
    usage: LlmUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

fn chat_completion(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: ChatCompletionInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    let model = input.model.unwrap_or_else(|| "gpt-4".to_string());
    let is_o_series = model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4");

    let mut body = json!({
        "model": model,
        "messages": input.messages,
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
    if let Some(freq) = input.frequency_penalty {
        if !is_o_series {
            body["frequency_penalty"] = json!(freq);
        }
    }
    if let Some(pres) = input.presence_penalty {
        if !is_o_series {
            body["presence_penalty"] = json!(pres);
        }
    }
    if let Some(stop) = input.stop {
        if !is_o_series {
            body["stop"] = json!(stop);
        }
    }
    if let Some(tools) = input.tools {
        body["tools"] = json!(tools);
    }
    if let Some(tool_choice) = input.tool_choice {
        body["tool_choice"] = json!(tool_choice);
    }

    let resp = openai_post(connection, "/v1/chat/completions", body, 120_000)?;

    let choices = resp["choices"]
        .as_array()
        .ok_or_else(|| {
            permanent_err(
                "OPENAI_INVALID_RESPONSE",
                "Missing choices in OpenAI response",
            )
        })?
        .clone();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    let usage = extract_usage(&resp);
    let id = resp["id"].as_str().map(|s| s.to_string());

    serde_json::to_string(&ChatCompletionOutput {
        choices,
        model,
        usage,
        id,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 7: Create Embedding
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateEmbeddingInput {
    input: Value,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateEmbeddingOutput {
    data: Vec<Value>,
    model: String,
    usage: LlmUsage,
}

fn create_embedding(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: CreateEmbeddingInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    let body = json!({
        "model": input.model.unwrap_or_else(|| "text-embedding-3-small".to_string()),
        "input": input.input,
    });

    let resp = openai_post(connection, "/v1/embeddings", body, 60_000)?;

    let data = resp["data"]
        .as_array()
        .ok_or_else(|| permanent_err("OPENAI_INVALID_RESPONSE", "Missing data in OpenAI response"))?
        .clone();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();
    // Embeddings endpoint returns prompt_tokens + total_tokens only.
    let usage = LlmUsage {
        prompt_tokens: resp["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32,
        completion_tokens: 0,
        total_tokens: resp["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    };

    serde_json::to_string(&CreateEmbeddingOutput { data, model, usage })
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 8: Moderate Content
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ModerateContentInput {
    input: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModerateContentOutput {
    results: Vec<Value>,
    model: String,
}

fn moderate_content(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: ModerateContentInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    let body = json!({
        "input": input.input,
        "model": input.model.unwrap_or_else(|| "text-moderation-latest".to_string()),
    });

    let resp = openai_post(connection, "/v1/moderations", body, 30_000)?;

    let results = resp["results"]
        .as_array()
        .ok_or_else(|| {
            permanent_err(
                "OPENAI_INVALID_RESPONSE",
                "Missing results in OpenAI response",
            )
        })?
        .clone();
    let model = resp["model"].as_str().unwrap_or("unknown").to_string();

    serde_json::to_string(&ModerateContentOutput { results, model })
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Shared utilities
// -----------------------------------------------------------------------------

fn extract_usage(resp: &Value) -> LlmUsage {
    LlmUsage {
        prompt_tokens: resp["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32,
        completion_tokens: resp["usage"]["completion_tokens"].as_i64().unwrap_or(0) as i32,
        total_tokens: resp["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
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
        t.push_str("…");
        t
    }
}

// -----------------------------------------------------------------------------
// JSON Schemas — mirror legacy field names and defaults exactly
// -----------------------------------------------------------------------------

const TEXT_COMPLETION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":          { "type": "string", "description": "The user message or prompt to send to the model" },
        "system_prompt":   { "type": "string", "description": "Optional system message to set the assistant's behavior and context" },
        "model":           { "type": "string", "description": "The OpenAI model to use", "default": "gpt-4" },
        "max_tokens":      { "type": "integer", "description": "Maximum number of tokens to generate" },
        "temperature":     { "type": "number", "description": "Sampling temperature (0-2)" },
        "top_p":           { "type": "number", "description": "Nucleus sampling parameter" },
        "stop_sequences":  { "type": "array", "items": { "type": "string" }, "description": "Sequences where the model will stop generating" }
    }
}"#;

const TEXT_COMPLETION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "text":          { "type": "string", "description": "The generated text response" },
        "model":         { "type": "string", "description": "The model used for generation" },
        "usage":         { "type": "object", "description": "Token usage statistics", "properties": { "promptTokens": { "type": "integer" }, "completionTokens": { "type": "integer" }, "totalTokens": { "type": "integer" } } },
        "finish_reason": { "type": "string", "description": "The reason the model stopped generating" }
    }
}"#;

const IMAGE_GENERATION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":          { "type": "string", "description": "Text description of the image to generate" },
        "negative_prompt": { "type": "string", "description": "Elements to avoid in the generated image" },
        "model":           { "type": "string", "description": "The DALL-E model to use", "default": "dall-e-3" },
        "width":           { "type": "integer", "description": "Width of the generated image in pixels" },
        "height":          { "type": "integer", "description": "Height of the generated image in pixels" },
        "quality":         { "type": "string", "description": "Image quality (DALL-E 3 only: 'standard' or 'hd')" },
        "style":           { "type": "string", "description": "Image style (DALL-E 3 only: 'vivid' or 'natural')" }
    }
}"#;

const IMAGE_GENERATION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "image_data":     { "type": "string", "description": "Base64-encoded image data" },
        "mime_type":      { "type": "string", "description": "MIME type of the generated image" },
        "width":          { "type": "integer", "description": "Width of the generated image" },
        "height":         { "type": "integer", "description": "Height of the generated image" },
        "model":          { "type": "string", "description": "The model used for image generation" },
        "revised_prompt": { "type": "string", "description": "The prompt as revised by the model" }
    }
}"#;

const STRUCTURED_OUTPUT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt", "json_schema"],
    "properties": {
        "prompt":        { "type": "string", "description": "The user message or prompt describing what structured data to generate" },
        "system_prompt": { "type": "string", "description": "Optional system message to set context" },
        "json_schema":   { "description": "The JSON schema defining the expected output structure" },
        "model":         { "type": "string", "description": "The OpenAI model to use (must support structured outputs)", "default": "gpt-4o-mini" },
        "temperature":   { "type": "number", "description": "Sampling temperature (0-2)" }
    }
}"#;

const STRUCTURED_OUTPUT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "output": { "description": "The structured JSON output conforming to the provided schema" },
        "model":  { "type": "string", "description": "The model used for generation" },
        "usage":  { "type": "object", "description": "Token usage statistics", "properties": { "promptTokens": { "type": "integer" }, "completionTokens": { "type": "integer" }, "totalTokens": { "type": "integer" } } }
    }
}"#;

const VISION_TO_TEXT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":      { "type": "string", "description": "Instructions for analyzing the image" },
        "image_data":  { "type": "string", "description": "Base64-encoded image data (provide either image_data or image_url)" },
        "image_url":   { "type": "string", "description": "URL of the image to analyze (provide either image_data or image_url)" },
        "model":       { "type": "string", "description": "The OpenAI model to use (must support vision)", "default": "gpt-4o" },
        "max_tokens":  { "type": "integer", "description": "Maximum number of tokens to generate" },
        "temperature": { "type": "number", "description": "Sampling temperature (0-2)" }
    }
}"#;

const VISION_TO_TEXT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "text":  { "type": "string", "description": "The generated text description or analysis of the image" },
        "model": { "type": "string", "description": "The model used for analysis" },
        "usage": { "type": "object", "description": "Token usage statistics", "properties": { "promptTokens": { "type": "integer" }, "completionTokens": { "type": "integer" }, "totalTokens": { "type": "integer" } } }
    }
}"#;

const VISION_TO_IMAGE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt", "image_data"],
    "properties": {
        "prompt":     { "type": "string", "description": "Instructions for how to edit or transform the image" },
        "image_data": { "type": "string", "description": "Base64-encoded source image data to edit" },
        "mask_data":  { "type": "string", "description": "Optional base64-encoded mask image indicating areas to edit" },
        "model":      { "type": "string", "description": "The DALL-E model to use for image editing", "default": "dall-e-2" },
        "width":      { "type": "integer", "description": "Width of the output image in pixels" },
        "height":     { "type": "integer", "description": "Height of the output image in pixels" }
    }
}"#;

const VISION_TO_IMAGE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "image_data": { "type": "string", "description": "Base64-encoded edited image data" },
        "mime_type":  { "type": "string", "description": "MIME type of the output image" },
        "width":      { "type": "integer", "description": "Width of the output image in pixels" },
        "height":     { "type": "integer", "description": "Height of the output image in pixels" },
        "model":      { "type": "string", "description": "The model used for image editing" }
    }
}"#;

const CHAT_COMPLETION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["messages"],
    "properties": {
        "messages":          { "type": "array", "items": {}, "description": "Array of messages in the conversation" },
        "model":             { "type": "string", "description": "The OpenAI model to use", "default": "gpt-4" },
        "max_tokens":        { "type": "integer", "description": "Maximum number of tokens to generate" },
        "temperature":       { "type": "number", "description": "Sampling temperature (0-2)" },
        "top_p":             { "type": "number", "description": "Nucleus sampling parameter" },
        "frequency_penalty": { "type": "number", "description": "Penalty for token frequency (-2.0 to 2.0)" },
        "presence_penalty":  { "type": "number", "description": "Penalty for token presence (-2.0 to 2.0)" },
        "stop":              { "type": "array", "items": { "type": "string" }, "description": "Sequences where generation stops" },
        "tools":             { "type": "array", "items": {}, "description": "Array of tool/function definitions for function calling" },
        "tool_choice":       { "description": "Controls which tool is called" }
    }
}"#;

const CHAT_COMPLETION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "choices": { "type": "array", "items": {}, "description": "Array of completion choices" },
        "model":   { "type": "string", "description": "The model used for completion" },
        "usage":   { "type": "object", "description": "Token usage statistics", "properties": { "promptTokens": { "type": "integer" }, "completionTokens": { "type": "integer" }, "totalTokens": { "type": "integer" } } },
        "id":      { "type": "string", "description": "Unique identifier for the completion" }
    }
}"#;

const CREATE_EMBEDDING_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["input"],
    "properties": {
        "input": { "description": "Text to generate embeddings for (string or array of strings)" },
        "model": { "type": "string", "description": "The embedding model to use", "default": "text-embedding-3-small" }
    }
}"#;

const CREATE_EMBEDDING_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "data":  { "type": "array", "items": {}, "description": "Array of embedding objects with vectors" },
        "model": { "type": "string", "description": "The model used to generate embeddings" },
        "usage": { "type": "object", "description": "Token usage statistics", "properties": { "promptTokens": { "type": "integer" }, "completionTokens": { "type": "integer" }, "totalTokens": { "type": "integer" } } }
    }
}"#;

const MODERATE_CONTENT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["input"],
    "properties": {
        "input": { "type": "string", "description": "Text content to check for policy violations" },
        "model": { "type": "string", "description": "The moderation model to use", "default": "text-moderation-latest" }
    }
}"#;

const MODERATE_CONTENT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "results": { "type": "array", "items": {}, "description": "Array of moderation results with category flags and scores" },
        "model":   { "type": "string", "description": "The moderation model used" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
