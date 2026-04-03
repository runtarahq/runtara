use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, validate_path_param};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStepTypeSchemaParams {
    #[schemars(description = "Step type name (e.g., 'Agent', 'Conditional', 'Split', 'Switch')")]
    pub step_type: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetAgentParams {
    #[schemars(
        description = "Agent ID — same value used in agentId field of Agent steps (e.g., 'utils', 'transform', 'shopify', 'http', 'openai')"
    )]
    pub agent_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCapabilityParams {
    #[schemars(description = "Agent ID (e.g., 'http', 'shopify', 'utils')")]
    pub agent_id: String,
    #[schemars(
        description = "Capability ID — the hyphenated id (e.g., 'http-request', 'random-double', 'graphql'), NOT the underscored name"
    )]
    pub capability_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TestCapabilityParams {
    #[schemars(description = "Agent ID (e.g., 'http', 'shopify', 'utils')")]
    pub agent_id: String,
    #[schemars(
        description = "Capability ID — the hyphenated id (e.g., 'http-request', 'random-double'), NOT the underscored name"
    )]
    pub capability_id: String,
    #[schemars(description = "Test input data as JSON")]
    pub inputs: serde_json::Value,
    #[schemars(
        description = "Connection ID (required for agents that need credentials, e.g. shopify, openai). Use list_connections to find available connections."
    )]
    pub connection_id: Option<String>,
}

// ===== Tool Implementations =====

pub async fn list_step_types(server: &SmoMcpServer) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut result = api_get(server, "/api/runtime/specs/dsl/steps").await?;

    // Strip full JSON schemas — they make the response 300K+.
    // Use get_step_type_schema for individual step schemas.
    if let Some(step_types) = result
        .pointer_mut("/stepTypes")
        .and_then(|v| v.as_array_mut())
    {
        for step in step_types {
            if let Some(obj) = step.as_object_mut() {
                obj.remove("schema");
            }
        }
    }

    json_result(result)
}

pub async fn get_step_type_schema(
    server: &SmoMcpServer,
    params: GetStepTypeSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("step_type", &params.step_type)?;
    let result = api_get(
        server,
        &format!("/api/runtime/specs/dsl/steps/{}", params.step_type),
    )
    .await?;
    json_result(result)
}

pub async fn list_agents(server: &SmoMcpServer) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_get(server, "/api/runtime/agents").await?;
    json_result(result)
}

pub async fn get_agent(
    server: &SmoMcpServer,
    params: GetAgentParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("agent_id", &params.agent_id)?;
    let mut result = api_get(server, &format!("/api/runtime/agents/{}", params.agent_id)).await?;

    // Slim down capabilities and reduce id/name confusion.
    // The `id` field (hyphenated, e.g., "http-request") is what capabilityId expects.
    // Remove `name` (underscored, e.g., "http_request") to avoid confusion.
    // Use get_capability for full input/output schemas.
    if let Some(capabilities) = result
        .pointer_mut("/data/capabilities")
        .and_then(|v| v.as_array_mut())
    {
        for cap in capabilities {
            if let Some(obj) = cap.as_object_mut() {
                obj.remove("inputs");
                obj.remove("output");
                obj.remove("knownErrors");
                obj.remove("inputType");
                obj.remove("name"); // Remove underscored name — id is the canonical identifier
            }
        }
    }

    json_result(result)
}

pub async fn get_capability(
    server: &SmoMcpServer,
    params: GetCapabilityParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("agent_id", &params.agent_id)?;
    validate_path_param("capability_id", &params.capability_id)?;
    let result = api_get(
        server,
        &format!(
            "/api/runtime/agents/{}/capabilities/{}",
            params.agent_id, params.capability_id
        ),
    )
    .await?;
    json_result(result)
}

pub async fn test_capability(
    server: &SmoMcpServer,
    params: TestCapabilityParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("agent_id", &params.agent_id)?;
    validate_path_param("capability_id", &params.capability_id)?;
    let mut body = serde_json::json!({
        "input": params.inputs,
    });
    if let Some(conn_id) = &params.connection_id {
        body["connectionId"] = serde_json::json!(conn_id);
    }
    let result = api_post(
        server,
        &format!(
            "/api/runtime/agents/{}/capabilities/{}/test",
            params.agent_id, params.capability_id
        ),
        Some(body),
    )
    .await?;
    json_result(result)
}
