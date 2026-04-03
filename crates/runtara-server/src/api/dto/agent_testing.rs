//! Agent Testing DTOs

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// Default empty object for input field
fn default_empty_object() -> Value {
    serde_json::json!({})
}

/// Deserialize empty strings as None
fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt.filter(|s| !s.is_empty()))
}

/// Request body for testing an agent
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "input": {},
    "connectionId": "e9af2f09-0666-43b2-9173-b1ce6ac0c739"
}))]
pub struct TestAgentRequest {
    /// Input data for the agent (structure depends on the specific agent).
    /// Most agents expect an object with specific fields, or an empty object {}.
    /// If omitted, defaults to an empty object {}.
    /// Example for random-double: {"input": {}}
    /// Example for calculate: {"input": {"expression": "2 + 2", "variables": {}}}
    #[schema(value_type = Object, example = json!({}))]
    #[serde(default = "default_empty_object")]
    pub input: Value,

    /// Optional connection ID for agents that require connections (e.g., HTTP, Shopify).
    /// If provided, the connection will be looked up and passed to the agent.
    /// The connection must belong to the authenticated tenant and be in ACTIVE status.
    #[serde(
        rename = "connectionId",
        default,
        deserialize_with = "empty_string_as_none",
        skip_serializing_if = "Option::is_none"
    )]
    pub connection_id: Option<String>,
}

/// Response from testing an agent
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TestAgentResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "executionTimeMs")]
    pub execution_time_ms: f64,
    #[serde(rename = "maxMemoryMb", skip_serializing_if = "Option::is_none")]
    pub max_memory_mb: Option<f64>,
}

/// Error response for agent testing
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TestAgentErrorResponse {
    pub success: bool,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
