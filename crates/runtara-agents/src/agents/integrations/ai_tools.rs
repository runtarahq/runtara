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
//! These are agent capabilities that can be used standalone in workflow steps
//! or invoked as tools by an AI Agent step.

use crate::connections::RawConnection;
use crate::types::AgentError;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{bedrock, openai};

/// Resolve the integration_id for a connection.
/// In compiled workflow binaries, the connection stub has an empty integration_id.
fn resolve_integration_id(connection: &RawConnection) -> Result<String, AgentError> {
    let integration_id = &connection.integration_id;
    if !integration_id.is_empty() {
        return Ok(integration_id.clone());
    }

    use crate::integrations::integration_utils::env;
    let url = format!(
        "{}/{}/{}",
        env::connection_service_url(),
        env::tenant_id(),
        connection.connection_id
    );

    let client = runtara_http::HttpClient::new();
    let resp = client.request("GET", &url).call().map_err(|e| {
        AgentError::permanent(
            "AI_TOOLS_CONNECTION_FETCH_ERROR",
            format!("Failed to fetch connection: {}", e),
        )
        .with_attrs(json!({"connection_id": connection.connection_id}))
    })?;

    let body: Value = resp.into_json().map_err(|e| {
        AgentError::permanent(
            "AI_TOOLS_CONNECTION_PARSE_ERROR",
            format!("Failed to parse connection response: {}", e),
        )
        .with_attrs(json!({}))
    })?;

    body["integration_id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| {
            AgentError::permanent(
                "AI_TOOLS_MISSING_INTEGRATION_ID",
                "Connection has no integration_id",
            )
            .with_attrs(json!({"connection_id": connection.connection_id}))
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
pub fn ai_text_completion(
    input: AiTextCompletionInput,
) -> Result<AiTextCompletionOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent("AI_TOOLS_MISSING_CONNECTION", "LLM connection is required")
            .with_attrs(json!({}))
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
        _ => Err(AgentError::permanent(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            format!("LLM provider not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

/// Internal: dispatch to structured output when output_schema is provided
fn ai_text_completion_structured(
    conn: Option<RawConnection>,
    connection: &RawConnection,
    input: &AiTextCompletionInput,
    schema: &Value,
) -> Result<AiTextCompletionOutput, AgentError> {
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
        _ => Err(AgentError::permanent(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            format!("LLM provider not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
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
) -> Result<AiImageGenerationOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent("AI_TOOLS_MISSING_CONNECTION", "LLM connection is required")
            .with_attrs(json!({}))
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
        _ => Err(AgentError::permanent(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            format!("LLM provider not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
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
pub fn ai_vision_to_text(input: AiVisionToTextInput) -> Result<AiVisionToTextOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent("AI_TOOLS_MISSING_CONNECTION", "LLM connection is required")
            .with_attrs(json!({}))
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
        _ => Err(AgentError::permanent(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            format!("LLM provider not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
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
pub fn ai_vision_to_image(
    input: AiVisionToImageInput,
) -> Result<AiVisionToImageOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent("AI_TOOLS_MISSING_CONNECTION", "LLM connection is required")
            .with_attrs(json!({}))
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
        _ => Err(AgentError::permanent(
            "AI_TOOLS_UNSUPPORTED_PROVIDER",
            format!("LLM provider not supported: {}", connection.integration_id),
        )
        .with_attrs(json!({"integration_id": connection.integration_id}))),
    }
}

// ============================================================================
// Operation 5: Text Embedding
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "AI Embed Text Input")]
pub struct AiEmbedTextInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[field(
        display_name = "Dimension",
        description = "Optional output dimension. Must match the target Vector column. Workflow author is responsible for alignment.",
        example = "1536"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "AI Embed Text Output")]
pub struct AiEmbedTextOutput {
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

const AI_EMBED_TEXT_BATCH_CAP: usize = 2048;
const AI_EMBED_TEXT_MAX_DIM: u32 = 4096;

#[capability(
    module = "ai_tools",
    display_name = "Embed Text",
    description = "Generate vector embeddings for one or more strings. Use the result to populate a Vector column for similarity search.",
    module_supports_connections = true,
    module_integration_ids = "openai_api_key,aws_credentials",
    module_secure = true
)]
pub fn ai_embed_text(input: AiEmbedTextInput) -> Result<AiEmbedTextOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(|| {
        AgentError::permanent("AI_TOOLS_MISSING_CONNECTION", "LLM connection is required")
            .with_attrs(json!({}))
    })?;

    if input.texts.is_empty() {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            "`texts` must contain at least one entry",
        )
        .with_attrs(json!({})));
    }
    if input.texts.iter().any(|t| t.is_empty()) {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            "`texts` entries must be non-empty",
        )
        .with_attrs(json!({})));
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
        .with_attrs(json!({"batch": input.texts.len(), "cap": AI_EMBED_TEXT_BATCH_CAP})));
    }
    if let Some(d) = input.dimension
        && (d == 0 || d > AI_EMBED_TEXT_MAX_DIM)
    {
        return Err(AgentError::permanent(
            "AI_TOOLS_INVALID_INPUT",
            format!("`dimension` must be in 1..={}", AI_EMBED_TEXT_MAX_DIM),
        )
        .with_attrs(json!({"dimension": d, "max": AI_EMBED_TEXT_MAX_DIM})));
    }

    let integration_id = resolve_integration_id(connection)?;
    let (embeddings, model, dimension, usage) = match integration_id.as_str() {
        "openai_api_key" => {
            let r = openai::embed_text(
                connection,
                openai::EmbedTextRequest {
                    texts: input.texts,
                    model: input.model,
                    dimensions: input.dimension,
                },
            )?;
            (r.embeddings, r.model, r.dimension, r.usage)
        }
        "aws_credentials" => {
            let r = bedrock::embed_text(
                connection,
                bedrock::EmbedTextRequest {
                    texts: input.texts,
                    model: input.model,
                    dimensions: input.dimension,
                },
            )?;
            (r.embeddings, r.model, r.dimension, r.usage)
        }
        _ => {
            return Err(AgentError::permanent(
                "AI_TOOLS_UNSUPPORTED_PROVIDER",
                format!(
                    "Embedding provider not supported: {}",
                    connection.integration_id
                ),
            )
            .with_attrs(json!({"integration_id": connection.integration_id})));
        }
    };

    Ok(AiEmbedTextOutput {
        embeddings,
        model,
        dimension,
        usage: LlmUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        },
    })
}

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

    #[test]
    fn embed_text_rejects_missing_connection() {
        let input = AiEmbedTextInput {
            _connection: None,
            texts: vec!["hi".into()],
            model: None,
            dimension: None,
        };
        let err = ai_embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_MISSING_CONNECTION");
    }

    #[test]
    fn embed_text_rejects_empty_batch() {
        let input = AiEmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            texts: vec![],
            model: None,
            dimension: None,
        };
        let err = ai_embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_INVALID_INPUT");
        assert!(err.message.contains("at least one"), "{}", err.message);
    }

    #[test]
    fn embed_text_rejects_empty_text_entry() {
        let input = AiEmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            texts: vec!["ok".into(), String::new()],
            model: None,
            dimension: None,
        };
        let err = ai_embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_INVALID_INPUT");
        assert!(err.message.contains("non-empty"), "{}", err.message);
    }

    #[test]
    fn embed_text_rejects_oversize_dimension() {
        let input = AiEmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            texts: vec!["x".into()],
            model: None,
            dimension: Some(99_999),
        };
        let err = ai_embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_INVALID_INPUT");
        assert!(err.message.contains("dimension"), "{}", err.message);
    }

    #[test]
    fn embed_text_rejects_zero_dimension() {
        let input = AiEmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            texts: vec!["x".into()],
            model: None,
            dimension: Some(0),
        };
        let err = ai_embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_INVALID_INPUT");
    }

    #[test]
    fn embed_text_rejects_oversize_batch() {
        let texts = (0..AI_EMBED_TEXT_BATCH_CAP + 1)
            .map(|i| format!("t-{}", i))
            .collect();
        let input = AiEmbedTextInput {
            _connection: Some(fake_connection("openai_api_key")),
            texts,
            model: None,
            dimension: None,
        };
        let err = ai_embed_text(input).unwrap_err();
        assert_eq!(err.code, "AI_TOOLS_BATCH_TOO_LARGE");
    }
}
