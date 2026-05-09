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
#[serde(deny_unknown_fields)]
pub struct ValidateGraphParams {
    #[schemars(description = "Execution graph JSON to validate")]
    pub execution_graph: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateMappingsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Version number")]
    pub version: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListConnectionsParams {
    #[schemars(
        description = "Filter by integration type (the connection's `integrationId`, e.g., 'shopify_access_token', 'openai_api_key', 'sftp', 'http_bearer'). Discover valid values from each agent's `integrationIds` field returned by list_agents — do not pass an agent id like 'shopify' here."
    )]
    pub integration_id: Option<String>,
}

// ===== Tool Implementations =====

pub async fn list_connections(
    server: &SmoMcpServer,
    params: ListConnectionsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let qs = match &params.integration_id {
        Some(id) => format!("?integrationId={}", id),
        None => String::new(),
    };
    let result = api_get(server, &format!("/api/runtime/connections{}", qs)).await?;
    json_result(result)
}

pub async fn validate_graph(
    server: &SmoMcpServer,
    params: ValidateGraphParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_post(
        server,
        "/api/runtime/workflows/graph/validate",
        Some(params.execution_graph),
    )
    .await?;
    json_result(result)
}

pub async fn validate_mappings(
    server: &SmoMcpServer,
    params: ValidateMappingsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    let result = api_post(
        server,
        &format!(
            "/api/runtime/workflows/{}/validate-mappings?versionNumber={}",
            params.workflow_id, params.version
        ),
        None,
    )
    .await?;
    json_result(result)
}
