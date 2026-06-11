// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! LLM provider dispatch.
//!
//! Creates `CompletionModel` instances from connection parameters,
//! dispatching based on `integration_id`.

use crate::providers::{bedrock, openai};
use serde_json::{Value, json};

pub const PROVIDER_OPENAI: &str = "openai";
pub const PROVIDER_BEDROCK: &str = "bedrock";

const OPENAI_COMPATIBLE_INTEGRATIONS: &[&str] = &["openai_api_key"];
const BEDROCK_COMPATIBLE_INTEGRATIONS: &[&str] = &["aws_credentials"];

/// Errors that can occur during provider creation.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Missing connection parameters")]
    MissingConnection,
    #[error("Missing API key in connection parameters")]
    MissingApiKey,
    #[error("Unsupported LLM provider: {0}")]
    UnsupportedProvider(String),
}

/// Return the connection integration ids compatible with an explicit AI provider.
pub fn compatible_integration_ids_for_provider(provider: &str) -> Option<&'static [&'static str]> {
    match provider {
        PROVIDER_OPENAI => Some(OPENAI_COMPATIBLE_INTEGRATIONS),
        PROVIDER_BEDROCK => Some(BEDROCK_COMPATIBLE_INTEGRATIONS),
        _ => None,
    }
}

/// True when `integration_id` is accepted for the explicit AI provider.
pub fn provider_supports_integration(provider: &str, integration_id: &str) -> bool {
    compatible_integration_ids_for_provider(provider)
        .map(|ids| ids.contains(&integration_id))
        .unwrap_or(false)
}

/// Create an OpenAI completion model from connection parameters.
///
/// Supports two modes:
/// - **Proxy** (preferred): if `connection_id` is provided, uses the proxy pattern
///   with relative paths and `X-Runtara-Connection-Id` header
/// - **Direct** (fallback): extracts `api_key` and optional `base_url` from parameters
///
/// # Arguments
/// * `parameters` - Connection parameters JSON (must contain `api_key` in direct mode)
/// * `model` - Model identifier (e.g., "gpt-4o"). Defaults to "gpt-4o" if None.
/// * `connection_id` - Optional connection ID for proxy mode
pub fn create_openai_model(
    parameters: &Value,
    model: Option<&str>,
) -> Result<openai::OpenAICompletionModel, ProviderError> {
    create_openai_model_with_connection(parameters, model, None)
}

/// Create an OpenAI completion model, optionally using the proxy pattern.
pub fn create_openai_model_with_connection(
    parameters: &Value,
    model: Option<&str>,
    connection_id: Option<&str>,
) -> Result<openai::OpenAICompletionModel, ProviderError> {
    let client = if let Some(conn_id) = connection_id
        && !conn_id.is_empty()
    {
        // Proxy mode: connection_id header + relative paths
        openai::Client::from_connection_id(conn_id)
    } else {
        // Direct mode: api_key + base_url
        let api_key = parameters
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or(ProviderError::MissingApiKey)?;

        let base_url = parameters.get("base_url").and_then(|v| v.as_str());

        if let Some(base_url) = base_url {
            openai::Client::from_url(api_key, base_url)
        } else {
            openai::Client::new(api_key)
        }
    };

    let model_id = model.unwrap_or("gpt-4o");
    Ok(client.completion_model(model_id))
}

/// Build provider-specific `additional_params` for structured output.
///
/// Takes a standard JSON Schema and wraps it in the format required by
/// each LLM provider's structured output feature:
/// - OpenAI: `response_format: { type: "json_schema", json_schema: { ... } }`
/// - Anthropic: `response_format: { type: "json", schema: { ... } }`
///
/// Returns `None` for unsupported providers (structured output will be
/// best-effort via prompt instructions).
pub fn structured_output_params(integration_id: &str, json_schema: Value) -> Option<Value> {
    match integration_id {
        PROVIDER_OPENAI | "openai_api_key" => Some(json!({
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "structured_response",
                    "strict": true,
                    "schema": json_schema
                }
            }
        })),
        "anthropic_api_key" => Some(json!({
            "response_format": {
                "type": "json",
                "schema": json_schema
            }
        })),
        PROVIDER_BEDROCK | "aws_credentials" => Some(json!({
            "outputConfig": {
                "textFormat": {
                    "type": "json_schema",
                    "structure": {
                        "jsonSchema": {
                            "name": "structured_response",
                            "description": "Structured response for the AI Agent step",
                            "schema": serde_json::to_string(&json_schema).unwrap_or_else(|_| "{}".to_string())
                        }
                    }
                }
            }
        })),
        _ => None,
    }
}

/// Dispatch to the appropriate LLM provider based on provider or integration id.
///
/// Currently supports:
/// - `openai` / `openai_api_key` → OpenAI-compatible chat completions
/// - `bedrock` / `aws_credentials` → Amazon Bedrock Converse
///
/// Returns a boxed `CompletionModel` so the caller doesn't need to know
/// which concrete provider type is in use.
pub fn create_completion_model(
    integration_id: &str,
    parameters: &Value,
    model: Option<&str>,
) -> Result<Box<dyn crate::CompletionModel>, ProviderError> {
    create_completion_model_with_connection(integration_id, parameters, model, None)
}

/// Dispatch to the appropriate LLM provider, optionally using the proxy pattern.
///
/// The first argument is the explicit provider id for AI Agent calls, but this
/// function also accepts legacy connection integration ids for direct callers.
pub fn create_completion_model_with_connection(
    integration_id: &str,
    parameters: &Value,
    model: Option<&str>,
    connection_id: Option<&str>,
) -> Result<Box<dyn crate::CompletionModel>, ProviderError> {
    match integration_id {
        PROVIDER_BEDROCK | "aws_credentials" => {
            let conn_id = connection_id
                .filter(|id| !id.is_empty())
                .ok_or(ProviderError::MissingConnection)?;
            let m = bedrock::Client::from_connection_id(conn_id).completion_model(model);
            Ok(Box::new(m))
        }
        PROVIDER_OPENAI | "openai_api_key" => {
            let m = create_openai_model_with_connection(parameters, model, connection_id)?;
            Ok(Box::new(m))
        }
        other => Err(ProviderError::UnsupportedProvider(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_structured_output_uses_converse_shape() {
        let params = structured_output_params(PROVIDER_BEDROCK, json!({"type": "object"}))
            .expect("bedrock params");
        assert_eq!(params["outputConfig"]["textFormat"]["type"], "json_schema");
    }

    #[test]
    fn provider_compatibility_maps_provider_to_connection_integrations() {
        assert!(provider_supports_integration(
            PROVIDER_OPENAI,
            "openai_api_key"
        ));
        assert!(provider_supports_integration(
            PROVIDER_BEDROCK,
            "aws_credentials"
        ));
        assert!(!provider_supports_integration(
            PROVIDER_OPENAI,
            "aws_credentials"
        ));
        assert!(compatible_integration_ids_for_provider("unknown").is_none());
    }
}
