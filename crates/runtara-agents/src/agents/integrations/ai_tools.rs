//! AI Tools
//!
//! Deterministic AI capabilities across multiple providers (OpenAI, AWS Bedrock, etc.).
//! These capabilities dispatch to the appropriate provider based on the connection type.
//!
//! Capabilities:
//! - Text Completion (with optional structured output via output_schema)
//! - Image Generation
//! - Vision to Text (with optional structured output via output_schema)
//! - Vision to Image (image editing)
//!
//! These are agent capabilities that can be used standalone in scenario steps
//! or invoked as tools by an AI Agent step.

use crate::connections::RawConnection;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{bedrock, openai};

use super::errors::permanent_error;

/// Resolve the integration_id for a connection.
/// In compiled scenario binaries, the connection stub has an empty integration_id.
fn resolve_integration_id(connection: &RawConnection) -> Result<String, String> {
    let integration_id = &connection.integration_id;
    if !integration_id.is_empty() {
        return Ok(integration_id.clone());
    }

    let base_url = std::env::var("CONNECTION_SERVICE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7001/api/connections".to_string());
    let tenant_id = std::env::var("RUNTARA_TENANT_ID").unwrap_or_default();
    let url = format!("{}/{}/{}", base_url, tenant_id, connection.connection_id);

    let client = runtara_http::HttpClient::new();
    let resp = client.request("GET", &url).call().map_err(|e| {
        permanent_error(
            "AI_TOOLS_CONNECTION_FETCH_ERROR",
            &format!("Failed to fetch connection: {}", e),
            json!({"connection_id": connection.connection_id}),
        )
    })?;

    let body: Value = resp.into_json().map_err(|e| {
        permanent_error(
            "AI_TOOLS_CONNECTION_PARSE_ERROR",
            &format!("Failed to parse connection response: {}", e),
            json!({}),
        )
    })?;

    body["integration_id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| {
            permanent_error(
                "AI_TOOLS_MISSING_INTEGRATION_ID",
                "Connection has no integration_id",
                json!({"connection_id": connection.connection_id}),
            )
        })
}

pub use super::types::LlmUsage;

// ============================================================================
// Common Data Models
// ============================================================================

/// Image data structure for vision operations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmImageData {
    /// Base64-encoded image data
    pub image_data: String,

    /// MIME type of the image
    pub mime_type: String,

    /// Image width in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    /// Image height in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

// ============================================================================
// Operation 1: Text Completion (with optional structured output)
// ============================================================================

#[derive(Serialize, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "AI Text Completion Input")]
pub struct AiTextCompletionInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "The model identifier to use (auto-selects based on provider if not specified)",
        example = "gpt-4o"
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
        description = "Sampling temperature (0-2). Higher values increase randomness",
        example = "0.7"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[field(
        display_name = "Top P",
        description = "Nucleus sampling parameter for controlling diversity",
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

    #[field(
        display_name = "Output Schema",
        description = "Optional JSON schema for structured output. When provided, the model returns JSON conforming to this schema.",
        example = "{\"type\": \"object\", \"properties\": {\"name\": {\"type\": \"string\"}}}"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Text Completion Output")]
pub struct AiTextCompletionOutput {
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
    module_display_name = "AI Tools",
    module_description = "AI tools — deterministic AI capabilities for text completion, image generation, structured output, and vision across multiple LLM providers",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "openai_api_key,aws_credentials",
    module_secure = true
)]
pub fn ai_text_completion(input: AiTextCompletionInput) -> Result<AiTextCompletionOutput, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "AI_TOOLS_MISSING_CONNECTION",
            "LLM connection is required",
            json!({}),
        )
    })?;

    // If output_schema is provided, use structured output path
    if let Some(ref schema) = input.output_schema {
        return ai_text_completion_structured(
            input._connection.clone(),
            connection,
            &input,
            schema,
        );
    }

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "openai_api_key" => {
            let openai_input = openai::TextCompletionInput {
                _connection: input._connection.clone(),
                prompt: input.prompt,
                system_prompt: input.system_prompt,
                model: input.model,
                max_tokens: input.max_tokens,
                temperature: input.temperature,
                top_p: input.top_p,
                stop_sequences: input.stop_sequences,
            };
            let output = openai::text_completion(openai_input)?;
            Ok(AiTextCompletionOutput {
                text: output.text,
                model: output.model,
                usage: LlmUsage {
                    prompt_tokens: output.usage.prompt_tokens,
                    completion_tokens: output.usage.completion_tokens,
                    total_tokens: output.usage.total_tokens,
                },
                finish_reason: output.finish_reason,
                structured_output: None,
            })
        }
        "aws_credentials" => {
            let bedrock_input = bedrock::TextCompletionInput {
                _connection: input._connection.clone(),
                prompt: input.prompt,
                system_prompt: input.system_prompt,
                model: input.model,
                max_tokens: input.max_tokens,
                temperature: input.temperature,
                top_p: input.top_p,
                stop_sequences: input.stop_sequences,
            };
            let output = bedrock::text_completion(bedrock_input)?;
            Ok(AiTextCompletionOutput {
                text: output.text,
                model: output.model,
                usage: LlmUsage {
                    prompt_tokens: output.usage.prompt_tokens,
                    completion_tokens: output.usage.completion_tokens,
                    total_tokens: output.usage.total_tokens,
                },
                finish_reason: output.finish_reason,
                structured_output: None,
            })
        }
        _ => Err(permanent_error(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            &format!("LLM provider not supported: {}", connection.integration_id),
            json!({"integration_id": connection.integration_id}),
        )),
    }
}

/// Internal: dispatch to structured output when output_schema is provided
fn ai_text_completion_structured(
    conn: Option<RawConnection>,
    connection: &RawConnection,
    input: &AiTextCompletionInput,
    schema: &Value,
) -> Result<AiTextCompletionOutput, String> {
    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "openai_api_key" => {
            let openai_input = openai::StructuredOutputInput {
                _connection: conn,
                prompt: input.prompt.clone(),
                system_prompt: input.system_prompt.clone(),
                json_schema: schema.clone(),
                model: input.model.clone(),
                temperature: input.temperature,
            };
            let output = openai::structured_output(openai_input)?;
            let text = serde_json::to_string(&output.output).unwrap_or_default();
            Ok(AiTextCompletionOutput {
                text,
                model: output.model,
                usage: LlmUsage {
                    prompt_tokens: output.usage.prompt_tokens,
                    completion_tokens: output.usage.completion_tokens,
                    total_tokens: output.usage.total_tokens,
                },
                finish_reason: "stop".to_string(),
                structured_output: Some(output.output),
            })
        }
        "aws_credentials" => {
            let bedrock_input = bedrock::StructuredOutputInput {
                _connection: conn,
                prompt: input.prompt.clone(),
                system_prompt: input.system_prompt.clone(),
                json_schema: schema.clone(),
                model: input.model.clone(),
                temperature: input.temperature,
            };
            let output = bedrock::structured_output(bedrock_input)?;
            let text = serde_json::to_string(&output.output).unwrap_or_default();
            Ok(AiTextCompletionOutput {
                text,
                model: output.model,
                usage: LlmUsage {
                    prompt_tokens: output.usage.prompt_tokens,
                    completion_tokens: output.usage.completion_tokens,
                    total_tokens: output.usage.total_tokens,
                },
                finish_reason: "stop".to_string(),
                structured_output: Some(output.output),
            })
        }
        _ => Err(permanent_error(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            &format!("LLM provider not supported: {}", connection.integration_id),
            json!({"integration_id": connection.integration_id}),
        )),
    }
}

// ============================================================================
// Operation 2: Image Generation
// ============================================================================

#[derive(Serialize, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "AI Image Generation Input")]
pub struct AiImageGenerationInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,

    #[field(
        display_name = "Model",
        description = "Image generation model to use",
        example = "dall-e-3"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Desired image width in pixels",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Desired image height in pixels",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,

    #[field(
        display_name = "Quality",
        description = "Image quality setting (e.g., 'standard', 'hd')",
        example = "hd"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    #[field(
        display_name = "Style",
        description = "Image style preset (e.g., 'vivid', 'natural')",
        example = "vivid"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Image Generation Output")]
pub struct AiImageGenerationOutput {
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
    description = "Generate images using AI image generation models"
)]
pub fn ai_image_generation(
    input: AiImageGenerationInput,
) -> Result<AiImageGenerationOutput, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "AI_TOOLS_MISSING_CONNECTION",
            "LLM connection is required",
            json!({}),
        )
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "openai_api_key" => {
            let openai_input = openai::ImageGenerationInput {
                _connection: input._connection.clone(),
                prompt: input.prompt,
                negative_prompt: input.negative_prompt,
                model: input.model,
                width: input.width,
                height: input.height,
                quality: input.quality,
                style: input.style,
            };
            let output = openai::image_generation(openai_input)?;
            Ok(AiImageGenerationOutput {
                image_data: output.image_data,
                mime_type: output.mime_type,
                width: output.width,
                height: output.height,
                model: output.model,
                revised_prompt: output.revised_prompt,
            })
        }
        "aws_credentials" => {
            let bedrock_input = bedrock::ImageGenerationInput {
                _connection: input._connection.clone(),
                prompt: input.prompt,
                negative_prompt: input.negative_prompt,
                model: input.model,
                width: input.width,
                height: input.height,
                quality: input.quality,
                style: input.style,
            };
            let output = bedrock::image_generation(bedrock_input)?;
            Ok(AiImageGenerationOutput {
                image_data: output.image_data,
                mime_type: output.mime_type,
                width: output.width,
                height: output.height,
                model: output.model,
                revised_prompt: output.revised_prompt,
            })
        }
        _ => Err(permanent_error(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            &format!("LLM provider not supported: {}", connection.integration_id),
            json!({"integration_id": connection.integration_id}),
        )),
    }
}

// ============================================================================
// Operation 3: Vision to Text (with optional structured output)
// ============================================================================

#[derive(Serialize, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "AI Vision to Text Input")]
pub struct AiVisionToTextInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

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
        description = "Vision model to use",
        example = "gpt-4o"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Max Tokens",
        description = "Maximum number of tokens to generate",
        example = "1024"
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

    #[field(
        display_name = "Output Schema",
        description = "Optional JSON schema for structured output. When provided, the model returns JSON conforming to this schema.",
        example = "{\"type\": \"object\", \"properties\": {\"objects\": {\"type\": \"array\"}}}"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Vision to Text Output")]
pub struct AiVisionToTextOutput {
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
    description = "Analyze images and generate text descriptions. Supports optional structured output via output_schema."
)]
pub fn ai_vision_to_text(input: AiVisionToTextInput) -> Result<AiVisionToTextOutput, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "AI_TOOLS_MISSING_CONNECTION",
            "LLM connection is required",
            json!({}),
        )
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "openai_api_key" => {
            let openai_input = openai::VisionToTextInput {
                _connection: input._connection.clone(),
                prompt: input.prompt,
                image_data: input.image_data,
                image_url: input.image_url,
                model: input.model,
                max_tokens: input.max_tokens,
                temperature: input.temperature,
            };
            let output = openai::vision_to_text(openai_input)?;
            let structured_output = parse_structured_output(&output.text, &input.output_schema);
            Ok(AiVisionToTextOutput {
                text: output.text,
                model: output.model,
                usage: LlmUsage {
                    prompt_tokens: output.usage.prompt_tokens,
                    completion_tokens: output.usage.completion_tokens,
                    total_tokens: output.usage.total_tokens,
                },
                structured_output,
            })
        }
        "aws_credentials" => {
            let bedrock_input = bedrock::VisionToTextInput {
                _connection: input._connection.clone(),
                prompt: input.prompt,
                image_data: input.image_data,
                image_url: input.image_url,
                model: input.model,
                max_tokens: input.max_tokens,
                temperature: input.temperature,
            };
            let output = bedrock::vision_to_text(bedrock_input)?;
            let structured_output = parse_structured_output(&output.text, &input.output_schema);
            Ok(AiVisionToTextOutput {
                text: output.text,
                model: output.model,
                usage: LlmUsage {
                    prompt_tokens: output.usage.prompt_tokens,
                    completion_tokens: output.usage.completion_tokens,
                    total_tokens: output.usage.total_tokens,
                },
                structured_output,
            })
        }
        _ => Err(permanent_error(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            &format!("LLM provider not supported: {}", connection.integration_id),
            json!({"integration_id": connection.integration_id}),
        )),
    }
}

/// Try to parse text as JSON when output_schema was requested.
/// Returns None if no schema was provided or if parsing fails.
fn parse_structured_output(text: &str, schema: &Option<Value>) -> Option<Value> {
    schema.as_ref()?;
    serde_json::from_str(text).ok()
}

// ============================================================================
// Operation 4: Vision to Image
// ============================================================================

#[derive(Serialize, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "AI Vision to Image Input")]
pub struct AiVisionToImageInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_data: Option<String>,

    #[field(
        display_name = "Model",
        description = "Image editing model to use",
        example = "dall-e-2"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Width",
        description = "Desired output width in pixels",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,

    #[field(
        display_name = "Height",
        description = "Desired output height in pixels",
        example = "1024"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Vision to Image Output")]
pub struct AiVisionToImageOutput {
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
    description = "Edit and manipulate images using AI models"
)]
pub fn ai_vision_to_image(input: AiVisionToImageInput) -> Result<AiVisionToImageOutput, String> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        permanent_error(
            "AI_TOOLS_MISSING_CONNECTION",
            "LLM connection is required",
            json!({}),
        )
    })?;

    let integration_id = resolve_integration_id(connection)?;
    match integration_id.as_str() {
        "openai_api_key" => {
            let openai_input = openai::VisionToImageInput {
                _connection: input._connection.clone(),
                prompt: input.prompt,
                image_data: input.image_data,
                mask_data: input.mask_data,
                model: input.model,
                width: input.width,
                height: input.height,
            };
            let output = openai::vision_to_image(openai_input)?;
            Ok(AiVisionToImageOutput {
                image_data: output.image_data,
                mime_type: output.mime_type,
                width: output.width,
                height: output.height,
                model: output.model,
            })
        }
        "aws_credentials" => {
            let bedrock_input = bedrock::VisionToImageInput {
                _connection: input._connection.clone(),
                prompt: input.prompt,
                image_data: input.image_data,
                mask_data: input.mask_data,
                model: input.model,
                width: input.width,
                height: input.height,
            };
            let output = bedrock::vision_to_image(bedrock_input)?;
            Ok(AiVisionToImageOutput {
                image_data: output.image_data,
                mime_type: output.mime_type,
                width: output.width,
                height: output.height,
                model: output.model,
            })
        }
        _ => Err(permanent_error(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            &format!("LLM provider not supported: {}", connection.integration_id),
            json!({"integration_id": connection.integration_id}),
        )),
    }
}
