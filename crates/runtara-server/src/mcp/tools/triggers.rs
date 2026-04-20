use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_delete, api_get, api_post, api_put, validate_path_param};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTriggersParams {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetTriggerParams {
    #[schemars(description = "Trigger UUID")]
    pub trigger_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTriggerParams {
    #[schemars(description = "Workflow ID to invoke when the trigger fires")]
    pub workflow_id: String,
    #[schemars(
        description = "Trigger type (uppercase): CRON, HTTP, EMAIL, APPLICATION, or CHANNEL"
    )]
    pub trigger_type: String,
    #[schemars(description = "Whether the trigger is active on creation (default: true)")]
    pub active: Option<bool>,
    #[schemars(description = "Trigger-specific JSON configuration. CRON: \
                       {expression: \"0 0 * * *\", timezone?: \"UTC\", inputs?: {...}, debug?: false}. \
                       CHANNEL: {connection_id: \"...\"}. Field `inputs` carries the workflow input \
                       payload for CRON triggers.")]
    pub configuration: Option<serde_json::Value>,
    #[schemars(description = "Only allow one concurrent run (default: false)")]
    pub single_instance: Option<bool>,
    #[schemars(description = "Remote tenant identifier for external system triggers")]
    pub remote_tenant_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTriggerParams {
    #[schemars(description = "Trigger UUID")]
    pub trigger_id: String,
    #[schemars(description = "Workflow ID to invoke when the trigger fires")]
    pub workflow_id: String,
    #[schemars(
        description = "Trigger type (uppercase): CRON, HTTP, EMAIL, APPLICATION, or CHANNEL"
    )]
    pub trigger_type: String,
    #[schemars(description = "Whether the trigger is active")]
    pub active: bool,
    #[schemars(
        description = "Full trigger configuration (replaces existing). See create_trigger for shape."
    )]
    pub configuration: Option<serde_json::Value>,
    #[schemars(description = "Only allow one concurrent run")]
    pub single_instance: bool,
    #[schemars(description = "Remote tenant identifier for external system triggers")]
    pub remote_tenant_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteTriggerParams {
    #[schemars(description = "Trigger UUID")]
    pub trigger_id: String,
}

// ===== Tool Implementations =====

pub async fn list_triggers(
    server: &SmoMcpServer,
    _params: ListTriggersParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_get(server, "/api/runtime/triggers").await?;
    json_result(result)
}

pub async fn get_trigger(
    server: &SmoMcpServer,
    params: GetTriggerParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("trigger_id", &params.trigger_id)?;
    let result = api_get(
        server,
        &format!("/api/runtime/triggers/{}", params.trigger_id),
    )
    .await?;
    json_result(result)
}

pub async fn create_trigger(
    server: &SmoMcpServer,
    params: CreateTriggerParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut body = serde_json::json!({
        "workflow_id": params.workflow_id,
        "trigger_type": params.trigger_type,
    });
    if let Some(active) = params.active {
        body["active"] = serde_json::json!(active);
    }
    if let Some(config) = params.configuration {
        body["configuration"] = config;
    }
    if let Some(si) = params.single_instance {
        body["single_instance"] = serde_json::json!(si);
    }
    if let Some(rt) = params.remote_tenant_id {
        body["remote_tenant_id"] = serde_json::json!(rt);
    }
    let result = api_post(server, "/api/runtime/triggers", Some(body)).await?;
    json_result(result)
}

pub async fn update_trigger(
    server: &SmoMcpServer,
    params: UpdateTriggerParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("trigger_id", &params.trigger_id)?;
    let body = serde_json::json!({
        "workflow_id": params.workflow_id,
        "trigger_type": params.trigger_type,
        "active": params.active,
        "configuration": params.configuration,
        "single_instance": params.single_instance,
        "remote_tenant_id": params.remote_tenant_id,
    });
    let result = api_put(
        server,
        &format!("/api/runtime/triggers/{}", params.trigger_id),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn delete_trigger(
    server: &SmoMcpServer,
    params: DeleteTriggerParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("trigger_id", &params.trigger_id)?;
    let result = api_delete(
        server,
        &format!("/api/runtime/triggers/{}", params.trigger_id),
    )
    .await?;
    json_result(result)
}
