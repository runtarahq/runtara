// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! LLM provider dispatch.
//!
//! Creates `CompletionModel` instances from connection parameters,
//! dispatching based on `integration_id`.

use crate::providers::openai;
use serde_json::{Value, json};

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
        "openai_api_key" => Some(json!({
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
        _ => None,
    }
}

/// Dispatch to the appropriate LLM provider based on `integration_id`.
///
/// Currently supports:
/// - `openai_api_key` → OpenAI
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
pub fn create_completion_model_with_connection(
    integration_id: &str,
    parameters: &Value,
    model: Option<&str>,
    connection_id: Option<&str>,
) -> Result<Box<dyn crate::CompletionModel>, ProviderError> {
    match integration_id {
        "openai_api_key" => {
            let m = create_openai_model_with_connection(parameters, model, connection_id)?;
            Ok(Box::new(m))
        }
        other => Err(ProviderError::UnsupportedProvider(other.to_string())),
    }
}
