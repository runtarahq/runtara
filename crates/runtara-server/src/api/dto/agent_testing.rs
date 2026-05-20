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
    /// Which engine actually executed this call ("components" or "legacy").
    /// Omitted on legacy paths that don't surface it yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<crate::api::services::agent_testing::ActiveEngine>,
}

/// Error response for agent testing
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TestAgentErrorResponse {
    pub success: bool,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Which execution engine to use for the test call. Forms the value of the
/// `?engine=` query string on `POST /api/runtime/agents/{name}/capabilities/{cap}/test`.
///
/// The legacy rustc-compiled dispatcher image was removed in Phase 3 step 10
/// once every agent shipped as its own WASM component. All variants now
/// route through the embedded wasmtime component host; the enum stays as a
/// stable API surface so existing `?engine=...` query strings still parse.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TestEngine {
    /// Route through the embedded wasmtime component host. Returns 404 if no
    /// component is loaded for the agent.
    #[default]
    Auto,
    /// Alias for `Auto` — kept for callers that send `?engine=components`
    /// explicitly. Behaves identically.
    Components,
    /// Alias for `Auto` — preserves backward compatibility with callers
    /// that explicitly send `?engine=legacy`. Before the components-only
    /// migration this selected the rustc-compiled dispatcher; that image
    /// is gone, so the request now silently routes through the component
    /// host. Kept as a variant rather than rejected so old SDK pins and
    /// scripted callers don't break with 400 on query deserialization.
    Legacy,
}

/// Query string parameters for `test_agent_handler`.
#[derive(Debug, Default, Clone, Deserialize, ToSchema)]
pub struct TestAgentQuery {
    #[serde(default)]
    pub engine: TestEngine,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `?engine=legacy` predates the components-only migration. Callers
    /// (older SDK pins, hand-rolled scripts) still send it; the server
    /// must keep parsing the value rather than failing query
    /// deserialization with a 400. axum's `Query` extractor calls into
    /// the type's `Deserialize` impl, same impl serde_json drives — so
    /// covering the string-to-enum path here is sufficient.
    #[test]
    fn test_engine_parses_legacy_alias() {
        let parsed: TestEngine =
            serde_json::from_str("\"legacy\"").expect("legacy variant must parse");
        assert_eq!(parsed, TestEngine::Legacy);
    }

    #[test]
    fn test_engine_parses_auto_and_components() {
        let auto: TestEngine = serde_json::from_str("\"auto\"").unwrap();
        let components: TestEngine = serde_json::from_str("\"components\"").unwrap();
        assert_eq!(auto, TestEngine::Auto);
        assert_eq!(components, TestEngine::Components);
    }

    #[test]
    fn test_engine_default_is_auto() {
        let q: TestAgentQuery = serde_json::from_value(serde_json::json!({}))
            .expect("empty object must parse with defaults");
        assert_eq!(q.engine, TestEngine::Auto);
    }
}
