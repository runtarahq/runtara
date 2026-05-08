use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{
    api_get, api_post, encode_path_param, validate_identifier_param, validate_path_param,
};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListPendingSignalsParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSignalSchemaParams {
    #[schemars(description = "Workflow ID")]
    pub workflow_id: String,
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(description = "Signal ID to get the response schema for")]
    pub signal_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitActionResponseParams {
    #[schemars(
        description = "Action ID from a workflow_runtime action row. Canonical IDs may contain '/', '::', and step suffixes."
    )]
    pub action_id: String,
    #[schemars(
        description = "Response payload as JSON. Validated against the action input schema."
    )]
    pub payload: serde_json::Value,
    #[schemars(
        description = "Workflow ID for direct workflow action submission. Provide with instance_id, or omit when using report_id + block_id."
    )]
    pub workflow_id: Option<String>,
    #[schemars(
        description = "Execution instance UUID for direct workflow action submission. Provide with workflow_id, or omit when using report_id + block_id."
    )]
    pub instance_id: Option<String>,
    #[schemars(
        description = "Report id or slug for report-scoped action submission. Provide with block_id to re-fetch the filtered action row and apply report implicitPayload."
    )]
    pub report_id: Option<String>,
    #[schemars(
        description = "Report actions block id for report-scoped action submission. Provide with report_id."
    )]
    pub block_id: Option<String>,
    #[schemars(
        description = "Global report filter values keyed by filter id. Report context only."
    )]
    pub filters: Option<serde_json::Value>,
    #[schemars(description = "Per-block filter values keyed by filter id. Report context only.")]
    pub block_filters: Option<serde_json::Value>,
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
    validate_identifier_param("signal_id", &params.signal_id)?;
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
    validate_identifier_param("signal_id", &params.signal_id)?;
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

/// Submit a response to an open workflow action.
pub async fn submit_action_response(
    server: &SmoMcpServer,
    params: SubmitActionResponseParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_identifier_param("action_id", &params.action_id)?;

    match (
        params.workflow_id,
        params.instance_id,
        params.report_id,
        params.block_id,
    ) {
        (Some(workflow_id), Some(instance_id), None, None) => {
            validate_path_param("workflow_id", &workflow_id)?;
            validate_path_param("instance_id", &instance_id)?;
            if params.filters.is_some() || params.block_filters.is_some() {
                return Err(rmcp::ErrorData::invalid_params(
                    "filters and block_filters are only supported with report_id + block_id.",
                    None,
                ));
            }

            let result = api_post(
                server,
                &format!(
                    "/api/runtime/workflows/{}/instances/{}/actions/{}/submit",
                    workflow_id,
                    instance_id,
                    encode_path_param(&params.action_id)
                ),
                Some(serde_json::json!({ "payload": params.payload })),
            )
            .await?;
            json_result(result)
        }
        (None, None, Some(report_id), Some(block_id)) => {
            validate_path_param("report_id", &report_id)?;
            validate_path_param("block_id", &block_id)?;

            let result = api_post(
                server,
                &format!(
                    "/api/runtime/reports/{}/blocks/{}/actions/{}/submit",
                    report_id,
                    block_id,
                    encode_path_param(&params.action_id)
                ),
                Some(serde_json::json!({
                    "payload": params.payload,
                    "filters": params.filters.unwrap_or_else(|| serde_json::json!({})),
                    "blockFilters": params
                        .block_filters
                        .unwrap_or_else(|| serde_json::json!({})),
                })),
            )
            .await?;
            json_result(result)
        }
        _ => Err(rmcp::ErrorData::invalid_params(
            "Provide exactly one action context: workflow_id + instance_id, or report_id + block_id.",
            None,
        )),
    }
}
