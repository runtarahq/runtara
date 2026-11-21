// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! LLM provider dispatch.
//!
//! Creates `CompletionModel` instances from connection parameters,
//! dispatching based on `integration_id`.

#[cfg(feature = "completion")]
use crate::providers::{bedrock, gemini, openai};
use serde_json::{Value, json};

pub const PROVIDER_OPENAI: &str = "openai";
pub const PROVIDER_BEDROCK: &str = "bedrock";
pub const PROVIDER_GEMINI: &str = "gemini";

const OPENAI_COMPATIBLE_INTEGRATIONS: &[&str] = &["openai_api_key"];
const BEDROCK_COMPATIBLE_INTEGRATIONS: &[&str] = &["aws_credentials"];
const GEMINI_COMPATIBLE_INTEGRATIONS: &[&str] = &["gemini_api_key"];

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
        PROVIDER_GEMINI => Some(GEMINI_COMPATIBLE_INTEGRATIONS),
        _ => None,
    }
}

/// True when `integration_id` is accepted for the explicit AI provider.
pub fn provider_supports_integration(provider: &str, integration_id: &str) -> bool {
    compatible_integration_ids_for_provider(provider)
        .map(|ids| ids.contains(&integration_id))
        .unwrap_or(false)
}

/// Infer the internal AI implementation from a connection integration id.
///
/// This mapping is intentionally AI-internal. Connection descriptors never
/// expose provider or driver strings, and non-AI resources do not participate.
pub fn provider_for_integration(integration_id: &str) -> Option<&'static str> {
    if OPENAI_COMPATIBLE_INTEGRATIONS.contains(&integration_id) {
        Some(PROVIDER_OPENAI)
    } else if BEDROCK_COMPATIBLE_INTEGRATIONS.contains(&integration_id) {
        Some(PROVIDER_BEDROCK)
    } else if GEMINI_COMPATIBLE_INTEGRATIONS.contains(&integration_id) {
        Some(PROVIDER_GEMINI)
    } else {
        None
    }
}

/// Create an OpenAI completion model from connection parameters.
///
/// Supports two modes:
/// - **Proxy** (preferred): if `connection_id` is provided, uses the proxy pattern
///   with relative paths and an explicit connection ID
/// - **Direct** (fallback): extracts `api_key` and optional `base_url` from parameters
///
/// # Arguments
/// * `parameters` - Connection parameters JSON (must contain `api_key` in direct mode)
/// * `model` - Model identifier (e.g., "gpt-4o"). Defaults to
///   [`crate::defaults::DEFAULT_OPENAI_MODEL`] if None.
/// * `connection_id` - Optional connection ID for proxy mode
#[cfg(feature = "completion")]
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

    let model_id = model.unwrap_or(crate::defaults::DEFAULT_OPENAI_MODEL);
    Ok(client.completion_model(model_id))
}

/// Whether OpenAI accepts `schema` in strict mode: every object lists its
/// properties, requires them all, and allows no others.
pub fn openai_strict_compatible(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return true;
    };
    let is_object = match object.get("type") {
        Some(Value::String(kind)) => kind == "object",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
        _ => object.contains_key("properties"),
    };
    if is_object {
        let Some(properties) = object.get("properties").and_then(Value::as_object) else {
            return false;
        };
        if object.get("additionalProperties") != Some(&Value::Bool(false)) {
            return false;
        }
        let required: Vec<&str> = object
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !properties
            .keys()
            .all(|name| required.contains(&name.as_str()))
        {
            return false;
        }
        if !properties.values().all(openai_strict_compatible) {
            return false;
        }
    }
    ["items", "anyOf", "oneOf", "allOf"]
        .iter()
        .all(|key| match object.get(*key) {
            Some(Value::Array(schemas)) => schemas.iter().all(openai_strict_compatible),
            Some(schema) => openai_strict_compatible(schema),
            None => true,
        })
}

/// Build provider-specific `additional_params` for structured output.
///
/// Takes a standard JSON Schema and wraps it in the format required by
/// each LLM provider's structured output feature:
/// - OpenAI: `response_format: { type: "json_schema", json_schema: { ... } }`
/// - Bedrock: `outputConfig: { textFormat: { type: "json_schema", ... } }`
/// - Gemini: `generationConfig: { responseMimeType, responseJsonSchema }`
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
                    // OpenAI refuses a strict schema with an open object (a
                    // field of type object with no properties): such a schema
                    // is sent as guidance, and the answer is checked here.
                    "strict": openai_strict_compatible(&json_schema),
                    "schema": json_schema
                }
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
        PROVIDER_GEMINI | "gemini_api_key" => Some(json!({
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseJsonSchema": json_schema
            }
        })),
        _ => None,
    }
}

/// Dispatch to the appropriate LLM provider based on provider or integration id,
/// optionally using the proxy pattern.
///
/// Currently supports:
/// - `openai` / `openai_api_key` → OpenAI-compatible chat completions
/// - `bedrock` / `aws_credentials` → Amazon Bedrock Converse
/// - `gemini` / `gemini_api_key` → Google Gemini `generateContent`
///
/// Returns a boxed `CompletionModel` so the caller doesn't need to know
/// which concrete provider type is in use.
///
/// The first argument is the explicit provider id for AI Agent calls, but this
/// function also accepts legacy connection integration ids for direct callers.
#[cfg(feature = "completion")]
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
        PROVIDER_GEMINI | "gemini_api_key" => {
            let conn_id = connection_id
                .filter(|id| !id.is_empty())
                .ok_or(ProviderError::MissingConnection)?;
            let m = gemini::Client::from_connection_id(conn_id).completion_model(model);
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

    /// An open object (no properties) cannot be strict on OpenAI, which
    /// refuses the request: it is sent as guidance instead.
    #[test]
    fn a_schema_with_an_open_object_is_not_sent_strict() {
        let closed = json!({"type": "object", "additionalProperties": false, "required": ["a"],
            "properties": {"a": {"type": "string"}}});
        assert!(openai_strict_compatible(&closed));
        let params = structured_output_params("openai_api_key", closed).unwrap();
        assert_eq!(params["response_format"]["json_schema"]["strict"], true);

        let open = json!({"type": "object", "additionalProperties": false, "required": ["a", "sample"],
            "properties": {"a": {"type": "string"}, "sample": {"type": "object"}}});
        assert!(!openai_strict_compatible(&open));
        let params = structured_output_params("openai_api_key", open).unwrap();
        assert_eq!(params["response_format"]["json_schema"]["strict"], false);

        let optional = json!({"type": "object", "additionalProperties": false, "required": [],
            "properties": {"a": {"type": "string"}}});
        assert!(!openai_strict_compatible(&optional));
        let nested = json!({"type": "object", "additionalProperties": false, "required": ["list"],
            "properties": {"list": {"type": "array", "items": {"type": "object"}}}});
        assert!(!openai_strict_compatible(&nested));
    }

    use super::*;

    #[test]
    fn bedrock_structured_output_uses_converse_shape() {
        let params = structured_output_params(PROVIDER_BEDROCK, json!({"type": "object"}))
            .expect("bedrock params");
        assert_eq!(params["outputConfig"]["textFormat"]["type"], "json_schema");
    }

    #[test]
    fn gemini_structured_output_sits_in_generation_config() {
        let params = structured_output_params(PROVIDER_GEMINI, json!({"type": "object"}))
            .expect("gemini params");
        assert_eq!(
            params["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(
            params["generationConfig"]["responseJsonSchema"],
            json!({"type": "object"})
        );
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
        assert!(provider_supports_integration(
            PROVIDER_GEMINI,
            "gemini_api_key"
        ));
        assert!(!provider_supports_integration(
            PROVIDER_OPENAI,
            "aws_credentials"
        ));
        assert!(!provider_supports_integration(
            PROVIDER_GEMINI,
            "openai_api_key"
        ));
        assert!(compatible_integration_ids_for_provider("unknown").is_none());
    }

    #[test]
    #[cfg(feature = "completion")]
    fn structured_output_is_offered_only_for_dispatchable_providers() {
        // An id may carry a structured-output envelope only if the dispatcher
        // can actually build a model for it. Anything else is dead shape: the
        // request fails at model creation before structured output is reached.
        let params = json!({ "api_key": "test-key" });
        for id in [
            PROVIDER_OPENAI,
            PROVIDER_BEDROCK,
            PROVIDER_GEMINI,
            "openai_api_key",
            "aws_credentials",
            "gemini_api_key",
            "anthropic_api_key",
            "sqs_credentials",
            "",
        ] {
            let has_envelope = structured_output_params(id, json!({"type": "object"})).is_some();
            let dispatchable = !matches!(
                create_completion_model_with_connection(id, &params, None, Some("conn-1")),
                Err(ProviderError::UnsupportedProvider(_)),
            );
            assert_eq!(
                has_envelope, dispatchable,
                "structured-output support and provider dispatch disagree for '{id}'"
            );
        }
    }

    #[test]
    fn provider_is_inferred_only_from_registered_ai_integrations() {
        assert_eq!(
            provider_for_integration("openai_api_key"),
            Some(PROVIDER_OPENAI)
        );
        assert_eq!(
            provider_for_integration("aws_credentials"),
            Some(PROVIDER_BEDROCK)
        );
        assert_eq!(
            provider_for_integration("gemini_api_key"),
            Some(PROVIDER_GEMINI)
        );
        assert_eq!(provider_for_integration("sqs_credentials"), None);
    }
}
