//! AWS Bedrock LLM Operations
//!
//! AWS Bedrock-specific LLM operations supporting Claude, Titan, Stable Diffusion, and other models.
//! Requires AWS credentials configured in the connection.

use crate::connections::RawConnection;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::errors::permanent_error;
use super::integration_utils::ProxyHttpClient;

pub use super::types::LlmUsage;

// ============================================================================
// Shared helpers
// ============================================================================

/// Bedrock uses the historical `BEDROCK_MISSING_CONNECTION` code rather
/// than the shared `*_NO_CONNECTION` taxonomy, preserved for wire
/// compatibility.
fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, String> {
    connection.as_ref().ok_or_else(|| {
        permanent_error(
            "BEDROCK_MISSING_CONNECTION",
            "Bedrock connection is required",
            json!({}),
        )
    })
}

/// Create a proxy client with Bedrock's `Accept: application/json` header
/// already attached. Proxy handles SigV4 signing and credential injection.
fn bedrock_client<'a>(connection: &'a RawConnection) -> ProxyHttpClient<'a> {
    ProxyHttpClient::new(connection, "BEDROCK").with_header("Accept", "application/json")
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
        description = "Optional system message to set the assistant's behavior",
        example = "You are a helpful assistant"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The Bedrock model ID to use (Claude or Titan)",
        example = "anthropic.claude-3-5-sonnet-20240620-v1:0",
        default = "anthropic.claude-3-5-sonnet-20240620-v1:0"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate",
        example = "1024",
        default = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (0-1). Higher values increase randomness",
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
        display_name = "Stop Sequences",
        description = "Sequences where generation stops",
        example = "[\"\\n\\nHuman:\"]"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    // Register the bedrock module with inventory
    module_display_name = "AWS Bedrock",
    module_description = "AWS Bedrock LLM integration for text completion, image generation, structured output, and vision capabilities using Claude and Titan models",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "aws_credentials",
    module_secure = true
)]
pub fn text_completion(input: TextCompletionInput) -> Result<TextCompletionOutput, String> {
    let connection = require_connection(&input._connection)?;

    let model = input
        .model
        .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string());

    // Build request based on model family
    let request_body = if model.starts_with("anthropic.claude") {
        // Anthropic Claude format
        let mut messages = Vec::new();
        messages.push(json!({
            "role": "user",
            "content": input.prompt
        }));

        let mut body = json!({
            "messages": messages,
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

        body
    } else if model.starts_with("amazon.titan") {
        // Amazon Titan format
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

        let full_prompt = if let Some(system) = input.system_prompt {
            format!("{}\n\n{}", system, input.prompt)
        } else {
            input.prompt.clone()
        };

        json!({
            "inputText": full_prompt,
            "textGenerationConfig": text_config
        })
    } else {
        return Err(permanent_error(
            "BEDROCK_UNSUPPORTED_MODEL",
            &format!("Unsupported Bedrock model: {}", model),
            json!({"model": model}),
        ));
    };

    // Proxy handles SigV4 signing and resolves the regional endpoint
    let response_json = bedrock_client(connection)
        .post(format!("/model/{}/invoke", model))
        .timeout_ms(120_000)
        .json_body(request_body)
        .send_json()
        .map_err(String::from)?;

    // Parse response based on model family
    let (text, prompt_tokens, completion_tokens, finish_reason) =
        if model.starts_with("anthropic.claude") {
            let text = response_json["content"][0]["text"]
                .as_str()
                .ok_or_else(|| {
                    permanent_error(
                        "BEDROCK_INVALID_RESPONSE",
                        "Missing text in Bedrock response",
                        json!({}),
                    )
                })?
                .to_string();

            let prompt_tokens = response_json["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32;
            let completion_tokens = response_json["usage"]["output_tokens"]
                .as_i64()
                .unwrap_or(0) as i32;
            let finish_reason = response_json["stop_reason"]
                .as_str()
                .unwrap_or("end_turn")
                .to_string();

            (text, prompt_tokens, completion_tokens, finish_reason)
        } else if model.starts_with("amazon.titan") {
            let text = response_json["results"][0]["outputText"]
                .as_str()
                .ok_or_else(|| {
                    permanent_error(
                        "BEDROCK_INVALID_RESPONSE",
                        "Missing outputText in Bedrock response",
                        json!({}),
                    )
                })?
                .to_string();

            let prompt_tokens = response_json["inputTextTokenCount"].as_i64().unwrap_or(0) as i32;
            let completion_tokens = response_json["results"][0]["tokenCount"]
                .as_i64()
                .unwrap_or(0) as i32;
            let finish_reason = response_json["results"][0]["completionReason"]
                .as_str()
                .unwrap_or("FINISH")
                .to_string();

            (text, prompt_tokens, completion_tokens, finish_reason)
        } else {
            return Err(permanent_error(
                "BEDROCK_UNSUPPORTED_MODEL",
                &format!("Unsupported Bedrock model: {}", model),
                json!({"model": model}),
            ));
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
        example = "A futuristic city skyline at sunset"
    )]
    pub prompt: String,

    #[field(
        display_name = "Negative Prompt",
        description = "Elements to exclude from the generated image",
        example = "blurry, low quality, distorted"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The Bedrock image model to use (e.g., Stable Diffusion)",
        example = "stability.stable-diffusion-xl-v1",
        default = "stability.stable-diffusion-xl-v1"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Width of the generated image in pixels",
        example = "1024",
        default = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the generated image in pixels",
        example = "1024",
        default = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(
        display_name = "Quality",
        description = "Image quality setting (if supported by model)",
        example = "standard"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    #[field(
        display_name = "Style",
        description = "Image style preset (if supported by model)",
        example = "photographic"
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
        description = "MIME type of the generated image"
    )]
    pub mime_type: String,

    #[field(display_name = "Width", description = "Width of the generated image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(display_name = "Height", description = "Height of the generated image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(display_name = "Model", description = "The model used for generation")]
    pub model: String,

    #[field(
        display_name = "Revised Prompt",
        description = "The prompt as interpreted by the model (if available)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,
}

#[capability(
    module = "bedrock",
    display_name = "Image Generation (Bedrock)",
    description = "Generate images using AWS Bedrock models (Stable Diffusion)"
)]
pub fn image_generation(input: ImageGenerationInput) -> Result<ImageGenerationOutput, String> {
    let connection = require_connection(&input._connection)?;

    let model = input
        .model
        .unwrap_or_else(|| "stability.stable-diffusion-xl-v1".to_string());

    // Build request for Stable Diffusion
    let mut text_prompts = vec![json!({
        "text": input.prompt,
        "weight": 1.0
    })];

    if let Some(negative) = input.negative_prompt {
        text_prompts.push(json!({
            "text": negative,
            "weight": -1.0
        }));
    }

    let request_body = json!({
        "text_prompts": text_prompts,
        "cfg_scale": 7,
        "seed": 0,
        "steps": 30,
        "width": input.width.unwrap_or(1024),
        "height": input.height.unwrap_or(1024),
    });

    let response_json = bedrock_client(connection)
        .post(format!("/model/{}/invoke", model))
        .timeout_ms(180_000)
        .json_body(request_body)
        .send_json()
        .map_err(String::from)?;

    let image_data = response_json["artifacts"][0]["base64"]
        .as_str()
        .ok_or_else(|| {
            permanent_error(
                "BEDROCK_INVALID_RESPONSE",
                "Missing base64 image in Bedrock response",
                json!({}),
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
        description = "The prompt describing what data to extract or generate",
        example = "Extract the person's name and age"
    )]
    pub prompt: String,

    #[field(
        display_name = "System Prompt",
        description = "Optional system message for context",
        example = "You are a data extraction assistant"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature (lower recommended for structured output)",
        example = "0.3"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
pub fn structured_output(input: StructuredOutputInput) -> Result<StructuredOutputOutput, String> {
    // For Bedrock, we'll use prompt engineering to get structured output
    // since native structured output support is limited

    let schema_str = serde_json::to_string_pretty(&input.json_schema).map_err(|e| {
        permanent_error(
            "BEDROCK_INVALID_INPUT",
            &format!("Failed to serialize schema: {}", e),
            json!({}),
        )
    })?;

    let enhanced_prompt = format!(
        "{}\n\nRespond with valid JSON matching this schema:\n{}\n\nReturn ONLY the JSON, no other text.",
        input.prompt, schema_str
    );

    let text_completion = text_completion(TextCompletionInput {
        _connection: input._connection.clone(),
        prompt: enhanced_prompt,
        system_prompt: input.system_prompt,
        model: input.model.clone(),
        max_tokens: Some(4096),
        temperature: input.temperature,
        top_p: None,
        stop_sequences: None,
    })?;

    // Parse the JSON response
    let output: Value = serde_json::from_str(&text_completion.text).map_err(|e| {
        permanent_error(
            "BEDROCK_INVALID_RESPONSE",
            &format!("Failed to parse structured output as JSON: {}", e),
            json!({"response": text_completion.text}),
        )
    })?;

    Ok(StructuredOutputOutput {
        output,
        model: text_completion.model,
        usage: text_completion.usage,
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
        description = "Base64-encoded image data (required for Bedrock, URLs not supported)",
        example = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk..."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_data: Option<String>,

    #[field(
        display_name = "Image URL",
        description = "URL of the image (not supported by Bedrock - use image_data instead)",
        example = "https://example.com/image.png"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,

    #[field(
        display_name = "Model",
        description = "The Bedrock model to use (must be Claude 3 or 3.5 for vision)",
        example = "anthropic.claude-3-5-sonnet-20240620-v1:0",
        default = "anthropic.claude-3-5-sonnet-20240620-v1:0"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate",
        example = "1024",
        default = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    #[field(
        display_name = "Temperature",
        description = "Sampling temperature",
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
pub fn vision_to_text(input: VisionToTextInput) -> Result<VisionToTextOutput, String> {
    let connection = require_connection(&input._connection)?;

    let model = input
        .model
        .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string());

    // Only Claude 3 models support vision in Bedrock
    if !model.starts_with("anthropic.claude-3") && !model.starts_with("anthropic.claude-3-5") {
        return Err(permanent_error(
            "BEDROCK_UNSUPPORTED_MODEL",
            "Vision capabilities require Claude 3 or Claude 3.5 models",
            json!({"model": model}),
        ));
    }

    // Ensure we have image data
    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(permanent_error(
            "BEDROCK_MISSING_INPUT",
            "Either image_data or image_url is required",
            json!({}),
        ));
    }

    // Build content blocks
    let mut content_blocks = Vec::new();

    // Add image first
    if let Some(image_data) = input.image_data {
        content_blocks.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": image_data
            }
        }));
    } else if let Some(_image_url) = input.image_url {
        // Bedrock doesn't support URLs directly, would need to fetch and encode
        return Err(permanent_error(
            "BEDROCK_UNSUPPORTED_INPUT",
            "Bedrock vision requires base64-encoded image_data, not URLs",
            json!({}),
        ));
    }

    // Add text prompt
    content_blocks.push(json!({
        "type": "text",
        "text": input.prompt
    }));

    let messages = vec![json!({
        "role": "user",
        "content": content_blocks
    })];

    let mut request_body = json!({
        "messages": messages,
        "max_tokens": input.max_tokens.unwrap_or(1024),
        "anthropic_version": "bedrock-2023-05-31"
    });

    if let Some(temp) = input.temperature {
        request_body["temperature"] = json!(temp);
    }

    let response_json = bedrock_client(connection)
        .post(format!("/model/{}/invoke", model))
        .timeout_ms(120_000)
        .json_body(request_body)
        .send_json()
        .map_err(String::from)?;

    let text = response_json["content"][0]["text"]
        .as_str()
        .ok_or_else(|| {
            permanent_error(
                "BEDROCK_INVALID_RESPONSE",
                "Missing text in Bedrock response",
                json!({}),
            )
        })?
        .to_string();

    let usage = LlmUsage {
        prompt_tokens: response_json["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32,
        completion_tokens: response_json["usage"]["output_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        total_tokens: 0, // Will be computed below
    };

    Ok(VisionToTextOutput {
        text,
        model,
        usage: LlmUsage {
            total_tokens: usage.prompt_tokens + usage.completion_tokens,
            ..usage
        },
    })
}

// ============================================================================
// Operation 5: Vision to Image
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_data: Option<String>,

    #[field(
        display_name = "Model",
        description = "The Bedrock image model to use",
        example = "stability.stable-diffusion-xl-v1",
        default = "stability.stable-diffusion-xl-v1"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Width of the output image",
        example = "1024",
        default = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Height of the output image",
        example = "1024",
        default = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(display_name = "Height", description = "Height of the output image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(display_name = "Model", description = "The model used")]
    pub model: String,
}

#[capability(
    module = "bedrock",
    display_name = "Vision to Image (Bedrock)",
    description = "Edit and manipulate images using AWS Bedrock Stable Diffusion models"
)]
pub fn vision_to_image(input: VisionToImageInput) -> Result<VisionToImageOutput, String> {
    let connection = require_connection(&input._connection)?;

    let model = input
        .model
        .unwrap_or_else(|| "stability.stable-diffusion-xl-v1".to_string());

    // Use image-to-image or inpainting endpoint
    let request_body = json!({
        "text_prompts": [{
            "text": input.prompt,
            "weight": 1.0
        }],
        "init_image": input.image_data,
        "cfg_scale": 7,
        "image_strength": 0.5,
        "steps": 30,
        "width": input.width.unwrap_or(1024),
        "height": input.height.unwrap_or(1024),
    });

    let response_json = bedrock_client(connection)
        .post(format!("/model/{}/invoke", model))
        .timeout_ms(180_000)
        .json_body(request_body)
        .send_json()
        .map_err(String::from)?;

    let image_data = response_json["artifacts"][0]["base64"]
        .as_str()
        .ok_or_else(|| {
            permanent_error(
                "BEDROCK_INVALID_RESPONSE",
                "Missing base64 image in Bedrock response",
                json!({}),
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
// Direct Bedrock Operations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bedrock Invoke Model Input")]
pub struct BedrockInvokeModelInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept: Option<String>,

    #[field(
        display_name = "Content Type",
        description = "The MIME type of the request body",
        example = "application/json",
        default = "application/json"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<BedrockInvokeModelOutput, String> {
    let connection = require_connection(&input._connection)?;

    let content_type = input
        .content_type
        .unwrap_or_else(|| "application/json".to_string());

    let response = bedrock_client(connection)
        .post(format!("/model/{}/invoke", input.model_id))
        .header("Content-Type", &content_type)
        .timeout_ms(180_000)
        .json_body(input.body)
        .send_raw()
        .map_err(String::from)?;

    let body = match response.body {
        crate::http::HttpResponseBody::Json(v) => v,
        _ => {
            return Err(permanent_error(
                "BEDROCK_INVALID_RESPONSE",
                "Expected JSON response from Bedrock",
                json!({}),
            ));
        }
    };

    let response_content_type = response
        .headers
        .get("content-type")
        .cloned()
        .unwrap_or_else(|| "application/json".to_string());

    Ok(BedrockInvokeModelOutput {
        body,
        content_type: response_content_type,
    })
}

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Bedrock List Models Input")]
pub struct BedrockListModelsInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
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
) -> Result<BedrockListModelsOutput, String> {
    let connection = require_connection(&input._connection)?;

    let response_json = bedrock_client(connection)
        .get("/foundation-models")
        .timeout_ms(30_000)
        .send_json()
        .map_err(String::from)?;

    let model_summaries = response_json["modelSummaries"]
        .as_array()
        .ok_or_else(|| {
            permanent_error(
                "BEDROCK_INVALID_RESPONSE",
                "Missing modelSummaries in Bedrock response",
                json!({}),
            )
        })?
        .clone();

    Ok(BedrockListModelsOutput { model_summaries })
}
