//! AWS Bedrock integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/bedrock.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope, injecting `X-Runtara-Connection-Id` so the proxy can look up
//! the AWS connection and compute SigV4 server-side. The component never sees
//! AWS credentials.
//!
//! URL pattern used by the proxy:
//!   https://bedrock-runtime.{region}.amazonaws.com/model/{model_id}/invoke
//! The region is resolved from the connection's parameters by the proxy; the
//! component only sets the model-relative path `/model/{id}/invoke`.

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

/// Token usage statistics — mirrors `LlmUsage`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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
            id: "bedrock".into(),
            display_name: "AWS Bedrock".into(),
            description: "AWS Bedrock LLM integration for text completion, image generation, \
                          structured output, and vision capabilities using Claude and Titan models."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["aws_credentials".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "text-completion",
                "text_completion",
                "Text Completion (Bedrock)",
                "Generate text completion using AWS Bedrock models (Claude, Titan)",
                TEXT_COMPLETION_INPUT_SCHEMA,
                TEXT_COMPLETION_OUTPUT_SCHEMA,
            ),
            cap(
                "image-generation",
                "image_generation",
                "Image Generation (Bedrock)",
                "Generate images using AWS Bedrock models (Stable Diffusion)",
                IMAGE_GENERATION_INPUT_SCHEMA,
                IMAGE_GENERATION_OUTPUT_SCHEMA,
            ),
            cap(
                "structured-output",
                "structured_output",
                "Structured Output (Bedrock)",
                "Generate structured JSON output using AWS Bedrock models with prompt engineering",
                STRUCTURED_OUTPUT_INPUT_SCHEMA,
                STRUCTURED_OUTPUT_OUTPUT_SCHEMA,
            ),
            cap(
                "vision-to-text",
                "vision_to_text",
                "Vision to Text (Bedrock)",
                "Analyze images and generate text descriptions using AWS Bedrock Claude models",
                VISION_TO_TEXT_INPUT_SCHEMA,
                VISION_TO_TEXT_OUTPUT_SCHEMA,
            ),
            cap(
                "vision-to-image",
                "vision_to_image",
                "Vision to Image (Bedrock)",
                "Edit and manipulate images using AWS Bedrock Stable Diffusion models",
                VISION_TO_IMAGE_INPUT_SCHEMA,
                VISION_TO_IMAGE_OUTPUT_SCHEMA,
            ),
            cap(
                "invoke-model",
                "bedrock_invoke_model",
                "Invoke Model",
                "Directly invoke any AWS Bedrock model with custom request body",
                INVOKE_MODEL_INPUT_SCHEMA,
                INVOKE_MODEL_OUTPUT_SCHEMA,
            ),
            cap(
                "list-models",
                "bedrock_list_models",
                "List Models",
                "List available foundation models in AWS Bedrock",
                LIST_MODELS_INPUT_SCHEMA,
                LIST_MODELS_OUTPUT_SCHEMA,
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
            "invoke-model" => bedrock_invoke_model(&input, connection.as_ref()),
            "list-models" => bedrock_list_models(&input, connection.as_ref()),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("bedrock agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Helper: build a CapabilityInfo with Bedrock-appropriate flags
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
        tags: vec!["bedrock".into(), "llm".into(), "aws".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// -----------------------------------------------------------------------------
// Shared HTTP helper — POST JSON to AWS Bedrock via the runtara proxy.
//
// The proxy resolves the connection to get AWS credentials and the region,
// computes SigV4, and forwards to:
//   https://bedrock-runtime.{region}.amazonaws.com{path}
//
// `path` should be `/model/{model_id}/invoke` or `/foundation-models`.
// -----------------------------------------------------------------------------

fn bedrock_post(
    connection: &ConnectionInfo,
    path: &str,
    body: Value,
    timeout_ms: u64,
) -> Result<Value, ErrorInfo> {
    // The proxy uses the connection-id to resolve credentials and the regional
    // endpoint. We pass a placeholder base URL; the proxy replaces the host.
    let url = format!("https://bedrock-runtime.amazonaws.com{path}");
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("POST", &url)
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

fn bedrock_get(
    connection: &ConnectionInfo,
    path: &str,
    timeout_ms: u64,
) -> Result<Value, ErrorInfo> {
    let url = format!("https://bedrock.amazonaws.com{path}");

    let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(timeout_ms));
    let response = client
        .request("GET", &url)
        .header("Accept", "application/json")
        .header("X-Runtara-Connection-Id", &connection.connection_id)
        .call_agent()
        .map_err(|e| {
            transient_err(
                "NETWORK_ERROR",
                format!("Bedrock GET request to {path} failed: {e}"),
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
        return Err(ErrorInfo {
            code: code.into(),
            message: format!("Bedrock HTTP {status}: {}", truncate(&body_text, 512)),
            category: category.into(),
            severity: "error".into(),
            retryable: category == "transient",
            retry_after_ms: None,
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

/// Require a connection or return `BEDROCK_MISSING_CONNECTION` (wire-compatible
/// with the legacy error code).
fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection.ok_or_else(|| {
        permanent_err(
            "BEDROCK_MISSING_CONNECTION",
            "Bedrock connection is required",
        )
    })
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
        return Err(permanent_err(
            "BEDROCK_UNSUPPORTED_MODEL",
            format!("Unsupported Bedrock model: {}", model),
        ));
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
        (text, prompt_tokens, completion_tokens, finish_reason)
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
        (text, prompt_tokens, completion_tokens, finish_reason)
    };

    serde_json::to_string(&TextCompletionOutput {
        text,
        model,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
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
// Capability 3: Structured Output
//
// Bedrock lacks native JSON-schema enforcement; we use prompt engineering
// identical to the legacy implementation.
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

    let schema_str = serde_json::to_string_pretty(&input.json_schema).map_err(|e| {
        permanent_err(
            "BEDROCK_INVALID_INPUT",
            format!("Failed to serialize schema: {}", e),
        )
    })?;

    let enhanced_prompt = format!(
        "{}\n\nRespond with valid JSON matching this schema:\n{}\n\nReturn ONLY the JSON, no other text.",
        input.prompt, schema_str
    );

    // Delegate to text_completion capability.
    let tc_input = serde_json::to_string(&serde_json::json!({
        "prompt": enhanced_prompt,
        "system_prompt": input.system_prompt,
        "model": input.model,
        "max_tokens": 4096,
        "temperature": input.temperature,
    }))
    .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

    let tc_output_str = text_completion(&tc_input, connection)?;
    let tc_output: serde_json::Value = serde_json::from_str(&tc_output_str)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))?;

    let text = tc_output["text"].as_str().ok_or_else(|| {
        permanent_err(
            "BEDROCK_INVALID_RESPONSE",
            "Missing text in text_completion output",
        )
    })?;

    let output: Value = serde_json::from_str(text).map_err(|e| {
        permanent_err(
            "BEDROCK_INVALID_RESPONSE",
            format!("Failed to parse structured output as JSON: {}", e),
        )
    })?;

    let model = tc_output["model"].as_str().unwrap_or("unknown").to_string();
    let usage = LlmUsage {
        prompt_tokens: tc_output["usage"]["prompt_tokens"].as_i64().unwrap_or(0) as i32,
        completion_tokens: tc_output["usage"]["completion_tokens"]
            .as_i64()
            .unwrap_or(0) as i32,
        total_tokens: tc_output["usage"]["total_tokens"].as_i64().unwrap_or(0) as i32,
    };

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

    let model = input
        .model
        .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string());

    // Only Claude 3/3.5 models support vision in Bedrock.
    if !model.starts_with("anthropic.claude-3") {
        return Err(permanent_err(
            "BEDROCK_UNSUPPORTED_MODEL",
            "Vision capabilities require Claude 3 or Claude 3.5 models",
        ));
    }

    if input.image_data.is_none() && input.image_url.is_none() {
        return Err(permanent_err(
            "BEDROCK_MISSING_INPUT",
            "Either image_data or image_url is required",
        ));
    }

    // Bedrock does not support image URLs; only base64.
    if input.image_url.is_some() && input.image_data.is_none() {
        return Err(permanent_err(
            "BEDROCK_UNSUPPORTED_INPUT",
            "Bedrock vision requires base64-encoded image_data, not URLs",
        ));
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
            permanent_err(
                "BEDROCK_INVALID_RESPONSE",
                "Missing text in Bedrock response",
            )
        })?
        .to_string();

    let prompt_tokens = resp["usage"]["input_tokens"].as_i64().unwrap_or(0) as i32;
    let completion_tokens = resp["usage"]["output_tokens"].as_i64().unwrap_or(0) as i32;

    serde_json::to_string(&VisionToTextOutput {
        text,
        model,
        usage: LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
    })
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
// Capability 6: Invoke Model (raw)
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct InvokeModelInput {
    model_id: String,
    body: Value,
    #[serde(default)]
    accept: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct InvokeModelOutput {
    body: Value,
    content_type: String,
}

fn bedrock_invoke_model(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: InvokeModelInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    let content_type = input
        .content_type
        .unwrap_or_else(|| "application/json".to_string());
    let accept = input
        .accept
        .unwrap_or_else(|| "application/json".to_string());

    let body_bytes = serde_json::to_vec(&input.body)
        .map_err(|e| permanent_err("SERIALIZATION_ERROR", e.to_string()))?;

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
            transient_err(
                "NETWORK_ERROR",
                format!("Bedrock invoke-model request failed: {e}"),
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
        return Err(ErrorInfo {
            code: code.into(),
            message: format!("Bedrock HTTP {status}: {}", truncate(&body_text, 512)),
            category: category.into(),
            severity: "error".into(),
            retryable: category == "transient",
            retry_after_ms: None,
            attributes: serde_json::to_string(&json!({"status_code": status})).ok(),
        });
    }

    let response_content_type = response
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "application/json".to_string());

    let body: Value = serde_json::from_slice(&response.body).map_err(|e| {
        permanent_err(
            "BEDROCK_INVALID_RESPONSE",
            format!("Expected JSON response from Bedrock: {e}"),
        )
    })?;

    serde_json::to_string(&InvokeModelOutput {
        body,
        content_type: response_content_type,
    })
    .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 7: List Models
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListModelsInput {
    // No user-visible fields — connection required only.
}

#[derive(Debug, Serialize)]
struct ListModelsOutput {
    model_summaries: Vec<Value>,
}

fn bedrock_list_models(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let _input: ListModelsInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let connection = require_connection(connection)?;

    // List-foundation-models is on the control-plane endpoint (bedrock.region.amazonaws.com),
    // not the runtime endpoint. The proxy resolves the region from the connection.
    let resp = bedrock_get(connection, "/foundation-models", 30_000)?;

    let model_summaries = resp["modelSummaries"]
        .as_array()
        .ok_or_else(|| {
            permanent_err(
                "BEDROCK_INVALID_RESPONSE",
                "Missing modelSummaries in Bedrock response",
            )
        })?
        .clone();

    serde_json::to_string(&ListModelsOutput { model_summaries })
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Shared utilities
// -----------------------------------------------------------------------------

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
// JSON Schemas — mirror legacy field names, descriptions, and defaults exactly
// -----------------------------------------------------------------------------

const TEXT_COMPLETION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":         { "type": "string", "description": "The user message or prompt to send to the model" },
        "system_prompt":  { "type": "string", "description": "Optional system message to set the assistant's behavior" },
        "model":          { "type": "string", "description": "The Bedrock model ID to use (Claude or Titan)", "default": "anthropic.claude-3-5-sonnet-20240620-v1:0" },
        "max_tokens":     { "type": "integer", "description": "Maximum number of tokens to generate", "default": 1024 },
        "temperature":    { "type": "number", "description": "Sampling temperature (0-1). Higher values increase randomness" },
        "top_p":          { "type": "number", "description": "Nucleus sampling parameter" },
        "stop_sequences": { "type": "array", "items": { "type": "string" }, "description": "Sequences where generation stops" }
    }
}"#;

const TEXT_COMPLETION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "text":          { "type": "string", "description": "The generated text response" },
        "model":         { "type": "string", "description": "The model used for generation" },
        "usage":         { "type": "object", "description": "Token usage statistics", "properties": { "prompt_tokens": { "type": "integer" }, "completion_tokens": { "type": "integer" }, "total_tokens": { "type": "integer" } } },
        "finish_reason": { "type": "string", "description": "The reason generation stopped" }
    }
}"#;

const IMAGE_GENERATION_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":          { "type": "string", "description": "Text description of the image to generate" },
        "negative_prompt": { "type": "string", "description": "Elements to exclude from the generated image" },
        "model":           { "type": "string", "description": "The Bedrock image model to use (e.g., Stable Diffusion)", "default": "stability.stable-diffusion-xl-v1" },
        "width":           { "type": "integer", "description": "Width of the generated image in pixels", "default": 1024 },
        "height":          { "type": "integer", "description": "Height of the generated image in pixels", "default": 1024 },
        "quality":         { "type": "string", "description": "Image quality setting (if supported by model)" },
        "style":           { "type": "string", "description": "Image style preset (if supported by model)" }
    }
}"#;

const IMAGE_GENERATION_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "image_data":     { "type": "string", "description": "Base64-encoded image data" },
        "mime_type":      { "type": "string", "description": "MIME type of the generated image" },
        "width":          { "type": "integer", "description": "Width of the generated image" },
        "height":         { "type": "integer", "description": "Height of the generated image" },
        "model":          { "type": "string", "description": "The model used for generation" },
        "revised_prompt": { "type": "string", "description": "The prompt as interpreted by the model (if available)" }
    }
}"#;

const STRUCTURED_OUTPUT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt", "json_schema"],
    "properties": {
        "prompt":        { "type": "string", "description": "The prompt describing what data to extract or generate" },
        "system_prompt": { "type": "string", "description": "Optional system message for context" },
        "json_schema":   { "description": "The JSON schema defining the expected output structure" },
        "model":         { "type": "string", "description": "The Bedrock model to use" },
        "temperature":   { "type": "number", "description": "Sampling temperature (lower recommended for structured output)" }
    }
}"#;

const STRUCTURED_OUTPUT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "output": { "description": "The structured JSON output" },
        "model":  { "type": "string", "description": "The model used" },
        "usage":  { "type": "object", "description": "Token usage statistics", "properties": { "prompt_tokens": { "type": "integer" }, "completion_tokens": { "type": "integer" }, "total_tokens": { "type": "integer" } } }
    }
}"#;

const VISION_TO_TEXT_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt"],
    "properties": {
        "prompt":      { "type": "string", "description": "Instructions for analyzing the image" },
        "image_data":  { "type": "string", "description": "Base64-encoded image data (required for Bedrock, URLs not supported)" },
        "image_url":   { "type": "string", "description": "URL of the image (not supported by Bedrock - use image_data instead)" },
        "model":       { "type": "string", "description": "The Bedrock model to use (must be Claude 3 or 3.5 for vision)", "default": "anthropic.claude-3-5-sonnet-20240620-v1:0" },
        "max_tokens":  { "type": "integer", "description": "Maximum number of tokens to generate", "default": 1024 },
        "temperature": { "type": "number", "description": "Sampling temperature" }
    }
}"#;

const VISION_TO_TEXT_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "text":  { "type": "string", "description": "The generated text analysis of the image" },
        "model": { "type": "string", "description": "The model used" },
        "usage": { "type": "object", "description": "Token usage statistics", "properties": { "prompt_tokens": { "type": "integer" }, "completion_tokens": { "type": "integer" }, "total_tokens": { "type": "integer" } } }
    }
}"#;

const VISION_TO_IMAGE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["prompt", "image_data"],
    "properties": {
        "prompt":     { "type": "string", "description": "Instructions for how to modify the image" },
        "image_data": { "type": "string", "description": "Base64-encoded source image to edit" },
        "mask_data":  { "type": "string", "description": "Optional base64-encoded mask for inpainting" },
        "model":      { "type": "string", "description": "The Bedrock image model to use", "default": "stability.stable-diffusion-xl-v1" },
        "width":      { "type": "integer", "description": "Width of the output image", "default": 1024 },
        "height":     { "type": "integer", "description": "Height of the output image", "default": 1024 }
    }
}"#;

const VISION_TO_IMAGE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "image_data": { "type": "string", "description": "Base64-encoded edited image" },
        "mime_type":  { "type": "string", "description": "MIME type of the output image" },
        "width":      { "type": "integer", "description": "Width of the output image" },
        "height":     { "type": "integer", "description": "Height of the output image" },
        "model":      { "type": "string", "description": "The model used" }
    }
}"#;

const INVOKE_MODEL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["model_id", "body"],
    "properties": {
        "model_id":     { "type": "string", "description": "The Bedrock model ID to invoke" },
        "body":         { "description": "The request body to send to the model (format depends on model)" },
        "accept":       { "type": "string", "description": "The MIME type for the response", "default": "application/json" },
        "content_type": { "type": "string", "description": "The MIME type of the request body", "default": "application/json" }
    }
}"#;

const INVOKE_MODEL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "body":         { "description": "The response body from the model" },
        "content_type": { "type": "string", "description": "The MIME type of the response" }
    }
}"#;

const LIST_MODELS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {}
}"#;

const LIST_MODELS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "model_summaries": { "type": "array", "items": {}, "description": "Array of available foundation models with their details" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
