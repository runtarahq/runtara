// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! LLM provider dispatch.
//!
//! Creates rig `CompletionModel` instances from connection parameters,
//! dispatching based on `integration_id`.

use rig::providers::openai;
use serde_json::Value;

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
/// Extracts `api_key` and optional `base_url` from the connection parameters,
/// creates a rig OpenAI client, and returns the specified model.
///
/// # Arguments
/// * `parameters` - Connection parameters JSON (must contain `api_key`)
/// * `model` - Model identifier (e.g., "gpt-4o"). Defaults to "gpt-4o" if None.
pub fn create_openai_model(
    parameters: &Value,
    model: Option<&str>,
) -> Result<openai::CompletionModel, ProviderError> {
    let api_key = parameters
        .get("api_key")
        .and_then(|v| v.as_str())
        .ok_or(ProviderError::MissingApiKey)?;

    let base_url = parameters.get("base_url").and_then(|v| v.as_str());

    let client = if let Some(base_url) = base_url {
        openai::Client::from_url(api_key, base_url)
    } else {
        openai::Client::new(api_key)
    };

    let model_id = model.unwrap_or("gpt-4o");
    Ok(client.completion_model(model_id))
}

/// Dispatch to the appropriate LLM provider based on `integration_id`.
///
/// Currently supports:
/// - `openai_api_key` → OpenAI
///
/// Future: `anthropic_api_key`, `aws_credentials` (Bedrock)
pub fn create_completion_model(
    integration_id: &str,
    parameters: &Value,
    model: Option<&str>,
) -> Result<openai::CompletionModel, ProviderError> {
    match integration_id {
        "openai_api_key" => create_openai_model(parameters, model),
        other => Err(ProviderError::UnsupportedProvider(other.to_string())),
    }
}
