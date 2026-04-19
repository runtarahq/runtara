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
pub struct ListPendingSignalsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSignalSchemaParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(description = "Signal ID to get the response schema for")]
    pub signal_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitSignalResponseParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(
        description = "Signal ID from the pending input request (returned by list_pending_signals)"
    )]
    pub signal_id: String,
    #[schemars(
        description = "Response payload as JSON. Should conform to the response_schema from the pending input."
    )]
    pub payload: serde_json::Value,
}

// ===== Tool Implementations =====

/// List pending signals (WaitForSignal / human-in-the-loop requests) for a running execution.
pub async fn list_pending_signals(
    server: &SmoMcpServer,
    params: ListPendingSignalsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;
    let result = api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}/instances/{}/pending-input",
            params.workflow_id, params.instance_id
        ),
    )
    .await?;
    json_result(result)
}

/// Get the response schema for a specific pending signal.
pub async fn get_signal_schema(
    server: &SmoMcpServer,
    params: GetSignalSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("workflow_id", &params.workflow_id)?;
    validate_path_param("instance_id", &params.instance_id)?;
    validate_path_param("signal_id", &params.signal_id)?;
    let result = api_get(
        server,
        &format!(
            "/api/runtime/workflows/{}/instances/{}/pending-input",
            params.workflow_id, params.instance_id
        ),
    )
    .await?;

    // Extract the specific signal's schema from the list
    let schema = result
        .pointer("/data/pendingInputs")
        .and_then(|v| v.as_array())
        .and_then(|inputs| {
            inputs
                .iter()
                .find(|i| i.get("signalId").and_then(|v| v.as_str()) == Some(&params.signal_id))
        })
        .cloned();

    match schema {
        Some(signal) => json_result(serde_json::json!({
            "success": true,
            "data": {
                "signalId": params.signal_id,
                "message": signal.get("message"),
                "responseSchema": signal.get("responseSchema"),
                "toolName": signal.get("toolName"),
            }
        })),
        None => json_result(serde_json::json!({
            "success": false,
            "message": format!("No pending signal found with ID '{}'", params.signal_id),
        })),
    }
}

/// Submit a response to a pending signal, resuming the waiting execution.
pub async fn submit_signal_response(
    server: &SmoMcpServer,
    params: SubmitSignalResponseParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("instance_id", &params.instance_id)?;
    validate_path_param("signal_id", &params.signal_id)?;
    let body = serde_json::json!({
        "signalId": params.signal_id,
        "payload": params.payload,
    });
    let result = api_post(
        server,
        &format!("/api/runtime/signals/{}", params.instance_id),
        Some(body),
    )
    .await?;
    json_result(result)
}
