//! OpenAI LLM Operations
//!
//! OpenAI-specific LLM operations including chat completions, image generation, embeddings, and more.
//! Supports GPT-4, GPT-4o, DALL-E, and other OpenAI models.

use crate::connections::RawConnection;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::http::{self, HttpMethod, ResponseType};

use super::errors::{http_status_error, permanent_error};
pub use super::types::LlmUsage;

// ============================================================================
// Shared helpers
// ============================================================================

fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, String> {
    connection.as_ref().ok_or_else(|| {
        permanent_error(
            "OPENAI_MISSING_CONNECTION",
            "OpenAI connection is required",
            json!({}),
        )
    })
}

/// Build headers for OpenAI API calls via proxy.
fn openai_headers(connection: &RawConnection) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert(
        "X-Runtara-Connection-Id".to_string(),
        connection.connection_id.clone(),
    );
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers
}

// ============================================================================
// Operation 1: Text Completion
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Text Completion Input")]
pub struct TextCompletionInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The OpenAI model to use for generation",
        example = "gpt-4o",
        default = "gpt-4"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate in the response",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-2). Higher values make output more random, lower values more deterministic",
        example = "0.7"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[field(
        display_name = "Top P",
        description = "Nucleus sampling parameter. Only tokens with cumulative probability up to this value are considered",
        example = "0.9"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    #[field(
        display_name = "Stop Sequences",
        description = "Sequences where the model will stop generating further tokens",
        example = "[\"END\", \"STOP\"]"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    // Register the openai module with inventory
    module_display_name = "OpenAI",
    module_description = "OpenAI LLM integration for text completion, image generation, structured output, and vision capabilities",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "openai_api_key",
    module_secure = true
)]
pub fn text_completion(input: TextCompletionInput) -> Result<TextCompletionOutput, String> {
    let connection = require_connection(&input._connection)?;

    // Build messages array
    let mut messages = Vec::new();
    if let Some(system) = input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    // Build request body
    let model = input.model.unwrap_or_else(|| "gpt-4".to_string());
    let mut request_body = json!({
        "model": model.clone(),
        "messages": messages,
    });

    if let Some(max_tokens) = input.max_tokens {
        // Newer models (o1, o3, o4, etc.) require max_completion_tokens instead of max_tokens
        // https://platform.openai.com/docs/api-reference/chat/create
        if model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4") {
            request_body["max_completion_tokens"] = json!(max_tokens);
        } else {
            request_body["max_tokens"] = json!(max_tokens);
        }
    }
    // o-series models don't support temperature, top_p, or stop sequences
    let is_o_series = model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4");

    if let Some(temperature) = input.temperature
        && !is_o_series
    {
        request_body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = input.top_p
        && !is_o_series
    {
        request_body["top_p"] = json!(top_p);
    }
    if let Some(stop) = input.stop_sequences
        && !is_o_series
    {
        request_body["stop"] = json!(stop);
    }

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: "/v1/chat/completions".to_string(),
        headers: openai_headers(connection),
        query_parameters: HashMap::new(),
        body: http::HttpBody(request_body),
        response_type: ResponseType::Json,
        timeout_ms: 120000, // 2 minutes for LLM requests
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "OPENAI",
            response.status_code,
            &format!("OpenAI API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    // Parse response
    let response_json = match response.body {
        http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Expected JSON response from OpenAI",
                json!({}),
            ));
        }
    };

    let text = response_json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
                json!({"response": response_json}),
            )
        })?
        .to_string();

    let model = response_json["model"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    let usage = LlmUsage {
        prompt_tokens: response_json["usage"]["prompt_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        completion_tokens: response_json["usage"]["completion_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        total_tokens: response_json["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    };

    let finish_reason = response_json["choices"][0]["finish_reason"]
        .as_str()
        .unwrap_or("stop")
        .to_string();

    Ok(TextCompletionOutput {
        text,
        model,
        usage,
        finish_reason,
    })
}

// ============================================================================
// Operation 2: Image Generation
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Image Generation Input")]
pub struct ImageGenerationInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The DALL-E model to use for image generation",
        example = "dall-e-3",
        default = "dall-e-3"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Width of the generated image in pixels (DALL-E 3: 1024, 1792)",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the generated image in pixels (DALL-E 3: 1024, 1792)",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(
        display_name = "Quality",
        description = "Image quality setting (DALL-E 3 only: 'standard' or 'hd')",
        example = "hd"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    #[field(
        display_name = "Style",
        description = "Image style (DALL-E 3 only: 'vivid' or 'natural')",
        example = "vivid"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the generated image in pixels"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

#[capability(
    module = "openai",
    display_name = "Image Generation (OpenAI)",
    description = "Generate images using OpenAI DALL-E models"
)]
pub fn image_generation(input: ImageGenerationInput) -> Result<ImageGenerationOutput, String> {
    let connection = require_connection(&input._connection)?;

    let model = input.model.unwrap_or_else(|| "dall-e-3".to_string());

    // Build request body
    let mut request_body = json!({
        "model": model,
        "prompt": input.prompt,
        "response_format": "b64_json", // Get base64-encoded image
        "n": 1,
    });

    // DALL-E 3 supports size and quality
    if model == "dall-e-3" {
        if let (Some(width), Some(height)) = (input.width, input.height) {
            request_body["size"] = json!(format!("{}x{}", width, height));
        } else {
            request_body["size"] = json!("1024x1024");
        }
        if let Some(quality) = input.quality {
            request_body["quality"] = json!(quality);
        }
        if let Some(style) = input.style {
            request_body["style"] = json!(style);
        }
    } else {
        // DALL-E 2 only supports specific sizes
        request_body["size"] = json!("1024x1024");
    }

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: "/v1/images/generations".to_string(),
        headers: openai_headers(connection),
        query_parameters: HashMap::new(),
        body: http::HttpBody(request_body),
        response_type: ResponseType::Json,
        timeout_ms: 180000, // 3 minutes for image generation
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "OPENAI",
            response.status_code,
            &format!("OpenAI API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    let response_json = match response.body {
        http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Expected JSON response from OpenAI",
                json!({}),
            ));
        }
    };

    let image_data = response_json["data"][0]["b64_json"]
        .as_str()
        .ok_or_else(|| {
            permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Missing b64_json in OpenAI response",
                json!({"response": response_json}),
            )
        })?
        .to_string();

    let revised_prompt = response_json["data"][0]["revised_prompt"]
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

// ============================================================================
// Operation 3: Structured Output
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Structured Output Input")]
pub struct StructuredOutputInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-2). Lower values recommended for structured output",
        example = "0.3"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
pub fn structured_output(input: StructuredOutputInput) -> Result<StructuredOutputOutput, String> {
    let connection = require_connection(&input._connection)?;

    // Build messages array
    let mut messages = Vec::new();
    if let Some(system) = input.system_prompt {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": input.prompt}));

    // Build request body with structured output
    let mut request_body = json!({
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
        request_body["temperature"] = json!(temperature);
    }

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: "/v1/chat/completions".to_string(),
        headers: openai_headers(connection),
        query_parameters: HashMap::new(),
        body: http::HttpBody(request_body),
        response_type: ResponseType::Json,
        timeout_ms: 120000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "OPENAI",
            response.status_code,
            &format!("OpenAI API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    let response_json = match response.body {
        http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Expected JSON response from OpenAI",
                json!({}),
            ));
        }
    };

    let content = response_json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
                json!({"response": response_json}),
            )
        })?;

    let output: Value = serde_json::from_str(content).map_err(|e| {
        permanent_error(
            "OPENAI_INVALID_RESPONSE",
            &format!("Failed to parse structured output: {}", e),
            json!({"content": content, "error": e.to_string()}),
        )
    })?;

    let model = response_json["model"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    let usage = LlmUsage {
        prompt_tokens: response_json["usage"]["prompt_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        completion_tokens: response_json["usage"]["completion_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        total_tokens: response_json["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    };

    Ok(StructuredOutputOutput {
        output,
        model,
        usage,
    })
}

// ============================================================================
// Operation 4: Vision to Text
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Vision to Text Input")]
pub struct VisionToTextInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_data: Option<String>,

    #[field(
        display_name = "Image URL",
        description = "URL of the image to analyze (provide either image_data or image_url)",
        example = "https://example.com/image.png"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,

    #[field(
        display_name = "Model",
        description = "The OpenAI model to use (must support vision)",
        example = "gpt-4o",
        default = "gpt-4o"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate in the response",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-2)",
        example = "0.7"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
pub fn vision_to_text(input: VisionToTextInput) -> Result<VisionToTextOutput, String> {
    let connection = require_connection(&input._connection)?;

    // Ensure we have either image_data or image_url
    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(permanent_error(
            "OPENAI_INVALID_INPUT",
            "Either image_data or image_url is required",
            json!({}),
        ));
    }

    // Build content array with text and image
    let mut content = Vec::new();
    content.push(json!({"type": "text", "text": input.prompt}));

    if let Some(image_url) = input.image_url {
        content.push(json!({"type": "image_url", "image_url": {"url": image_url}}));
    } else if let Some(image_data) = input.image_data {
        content.push(json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{}", image_data)
            }
        }));
    }

    let messages = vec![json!({"role": "user", "content": content})];

    let model = input.model.unwrap_or_else(|| "gpt-4o".to_string());
    let mut request_body = json!({
        "model": model.clone(),
        "messages": messages,
    });

    // o-series models have different parameter requirements
    let is_o_series = model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4");

    if let Some(max_tokens) = input.max_tokens {
        // o-series models require max_completion_tokens instead of max_tokens
        if is_o_series {
            request_body["max_completion_tokens"] = json!(max_tokens);
        } else {
            request_body["max_tokens"] = json!(max_tokens);
        }
    }
    if let Some(temperature) = input.temperature
        && !is_o_series
    {
        request_body["temperature"] = json!(temperature);
    }

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: "/v1/chat/completions".to_string(),
        headers: openai_headers(connection),
        query_parameters: HashMap::new(),
        body: http::HttpBody(request_body),
        response_type: ResponseType::Json,
        timeout_ms: 120000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "OPENAI",
            response.status_code,
            &format!("OpenAI API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    let response_json = match response.body {
        http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Expected JSON response from OpenAI",
                json!({}),
            ));
        }
    };

    let text = response_json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Missing content in OpenAI response",
                json!({"response": response_json}),
            )
        })?
        .to_string();

    let model = response_json["model"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    let usage = LlmUsage {
        prompt_tokens: response_json["usage"]["prompt_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        completion_tokens: response_json["usage"]["completion_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        total_tokens: response_json["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    };

    Ok(VisionToTextOutput { text, model, usage })
}

// ============================================================================
// Operation 5: Vision to Image (Image Editing)
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Vision to Image Input")]
pub struct VisionToImageInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_data: Option<String>,

    #[field(
        display_name = "Model",
        description = "The DALL-E model to use for image editing",
        example = "dall-e-2",
        default = "dall-e-2"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Width of the output image in pixels",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the output image in pixels",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the output image in pixels"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
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
pub fn vision_to_image(input: VisionToImageInput) -> Result<VisionToImageOutput, String> {
    let connection = require_connection(&input._connection)?;

    // OpenAI image editing requires multipart/form-data
    // For now, we'll use the simpler approach of calling DALL-E with variation
    // Note: Full implementation would require multipart form data support

    let endpoint = if input.mask_data.is_some() {
        "images/edits"
    } else {
        "images/variations"
    };

    // Build request body
    let request_body = json!({
        "prompt": input.prompt,
        "n": 1,
        "response_format": "b64_json",
        "size": format!("{}x{}",
            input.width.unwrap_or(1024),
            input.height.unwrap_or(1024)
        ),
    });

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: format!("/v1/{}", endpoint),
        headers: openai_headers(connection),
        query_parameters: HashMap::new(),
        body: http::HttpBody(request_body),
        response_type: ResponseType::Json,
        timeout_ms: 180000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "OPENAI",
            response.status_code,
            &format!("OpenAI API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    let response_json = match response.body {
        http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Expected JSON response from OpenAI",
                json!({}),
            ));
        }
    };

    let image_data = response_json["data"][0]["b64_json"]
        .as_str()
        .ok_or_else(|| {
            permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Missing b64_json in OpenAI response",
                json!({"response": response_json}),
            )
        })?
        .to_string();

    Ok(VisionToImageOutput {
        image_data,
        mime_type: "image/png".to_string(),
        width: input.width.or(Some(1024)),
        height: input.height.or(Some(1024)),
        model: input.model.unwrap_or_else(|| "dall-e-2".to_string()),
    })
}

// ============================================================================
// Direct OpenAI Operations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "OpenAI Chat Completion Input")]
pub struct OpenaiChatCompletionInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
        default = "gpt-4"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate",
        example = "2048"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-2)",
        example = "0.7"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[field(
        display_name = "Top P",
        description = "Nucleus sampling parameter",
        example = "0.9"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    #[field(
        display_name = "Frequency Penalty",
        description = "Penalty for token frequency (-2.0 to 2.0). Positive values decrease repetition",
        example = "0.5"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,

    #[field(
        display_name = "Presence Penalty",
        description = "Penalty for token presence (-2.0 to 2.0). Positive values encourage new topics",
        example = "0.5"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,

    #[field(
        display_name = "Stop Sequences",
        description = "Sequences where generation stops",
        example = "[\"END\"]"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,

    #[field(
        display_name = "Tools",
        description = "Array of tool/function definitions for function calling",
        example = "[{\"type\": \"function\", \"function\": {\"name\": \"get_weather\"}}]"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,

    #[field(
        display_name = "Tool Choice",
        description = "Controls which tool is called ('auto', 'none', or specific tool)",
        example = "auto"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[capability(
    module = "openai",
    display_name = "Chat Completion",
    description = "OpenAI chat completion with full control over messages, tools, and parameters"
)]
pub fn openai_chat_completion(
    input: OpenaiChatCompletionInput,
) -> Result<OpenaiChatCompletionOutput, String> {
    let connection = require_connection(&input._connection)?;

    let model = input.model.unwrap_or_else(|| "gpt-4".to_string());
    let mut request_body = json!({
        "model": model.clone(),
        "messages": input.messages,
    });

    // o-series models have different parameter requirements
    let is_o_series = model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4");

    if let Some(max_tokens) = input.max_tokens {
        // o-series models require max_completion_tokens instead of max_tokens
        if is_o_series {
            request_body["max_completion_tokens"] = json!(max_tokens);
        } else {
            request_body["max_tokens"] = json!(max_tokens);
        }
    }
    if let Some(temperature) = input.temperature
        && !is_o_series
    {
        request_body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = input.top_p
        && !is_o_series
    {
        request_body["top_p"] = json!(top_p);
    }
    if let Some(freq) = input.frequency_penalty
        && !is_o_series
    {
        request_body["frequency_penalty"] = json!(freq);
    }
    if let Some(pres) = input.presence_penalty
        && !is_o_series
    {
        request_body["presence_penalty"] = json!(pres);
    }
    if let Some(stop) = input.stop
        && !is_o_series
    {
        request_body["stop"] = json!(stop);
    }
    if let Some(tools) = input.tools {
        request_body["tools"] = json!(tools);
    }
    if let Some(tool_choice) = input.tool_choice {
        request_body["tool_choice"] = json!(tool_choice);
    }

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: "/v1/chat/completions".to_string(),
        headers: openai_headers(connection),
        query_parameters: HashMap::new(),
        body: http::HttpBody(request_body),
        response_type: ResponseType::Json,
        timeout_ms: 120000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "OPENAI",
            response.status_code,
            &format!("OpenAI API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    let response_json = match response.body {
        http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Expected JSON response from OpenAI",
                json!({}),
            ));
        }
    };

    let choices = response_json["choices"]
        .as_array()
        .ok_or_else(|| {
            permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Missing choices in OpenAI response",
                json!({"response": response_json}),
            )
        })?
        .clone();

    let model = response_json["model"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    let usage = LlmUsage {
        prompt_tokens: response_json["usage"]["prompt_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        completion_tokens: response_json["usage"]["completion_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        total_tokens: response_json["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    };

    let id = response_json["id"].as_str().map(|s| s.to_string());

    Ok(OpenaiChatCompletionOutput {
        choices,
        model,
        usage,
        id,
    })
}

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "OpenAI Create Embedding Input")]
pub struct OpenaiCreateEmbeddingInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<OpenaiCreateEmbeddingOutput, String> {
    let connection = require_connection(&input._connection)?;

    let request_body = json!({
        "model": input.model.unwrap_or_else(|| "text-embedding-3-small".to_string()),
        "input": input.input,
    });

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: "/v1/embeddings".to_string(),
        headers: openai_headers(connection),
        query_parameters: HashMap::new(),
        body: http::HttpBody(request_body),
        response_type: ResponseType::Json,
        timeout_ms: 60000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "OPENAI",
            response.status_code,
            &format!("OpenAI API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    let response_json = match response.body {
        http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Expected JSON response from OpenAI",
                json!({}),
            ));
        }
    };

    let data = response_json["data"]
        .as_array()
        .ok_or_else(|| {
            permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Missing data in OpenAI response",
                json!({"response": response_json}),
            )
        })?
        .clone();

    let model = response_json["model"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    let usage = LlmUsage {
        prompt_tokens: response_json["usage"]["prompt_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        completion_tokens: 0,
        total_tokens: response_json["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    };

    Ok(OpenaiCreateEmbeddingOutput { data, model, usage })
}

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "OpenAI Moderate Content Input")]
pub struct OpenaiModerateContentInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<OpenaiModerateContentOutput, String> {
    let connection = require_connection(&input._connection)?;

    let request_body = json!({
        "input": input.input,
        "model": input.model.unwrap_or_else(|| "text-moderation-latest".to_string()),
    });

    let http_input = http::HttpRequestInput {
        method: HttpMethod::Post,
        url: "/v1/moderations".to_string(),
        headers: openai_headers(connection),
        query_parameters: HashMap::new(),
        body: http::HttpBody(request_body),
        response_type: ResponseType::Json,
        timeout_ms: 30000,
        ..Default::default()
    };

    let response = http::http_request(http_input)?;

    if !response.success {
        let body_str = format!("{:?}", response.body);
        return Err(http_status_error(
            "OPENAI",
            response.status_code,
            &format!("OpenAI API error: {}", body_str),
            json!({"status_code": response.status_code, "body": body_str}),
        ));
    }

    let response_json = match response.body {
        http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Expected JSON response from OpenAI",
                json!({}),
            ));
        }
    };

    let results = response_json["results"]
        .as_array()
        .ok_or_else(|| {
            permanent_error(
                "OPENAI_INVALID_RESPONSE",
                "Missing results in OpenAI response",
                json!({"response": response_json}),
            )
        })?
        .clone();

    let model = response_json["model"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    Ok(OpenaiModerateContentOutput { results, model })
}
