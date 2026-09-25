use rmcp::model::{CallToolResult, ContentBlock};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{
    api_get, api_post, encode_path_param, normalize_json_arg, validate_identifier_param,
    validate_path_param,
};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
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
    #[schemars(description = "Opaque request ID to get the response schema for")]
    pub request_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitSignalResponseParams {
    #[schemars(description = "Execution instance UUID")]
    pub instance_id: String,
    #[schemars(description = "Opaque requestId returned by list_pending_signals")]
    pub request_id: String,
    #[schemars(
        description = "Caller-generated operation ID. Reuse the same ID, request and payload after an uncertain acknowledgement."
    )]
    pub operation_id: String,
    #[schemars(
        description = "Response payload as JSON. Should conform to the response_schema from the pending input."
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub payload: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitActionResponseParams {
    #[schemars(description = "Opaque requestId/actionId from a managed workflow action row.")]
    pub action_id: String,
    #[schemars(
        description = "Caller-generated operation ID. Reuse with the identical target and payload after a timeout or uncertain acknowledgement."
    )]
    pub operation_id: String,
    #[schemars(
        description = "Response payload as JSON. Validated against the action input schema."
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub payload: serde_json::Value,
    #[schemars(
        description = "Workflow ID for direct workflow action submission. Provide with instance_id, or omit when using report_id + block_id."
    )]
    pub workflow_id: Option<String>,
    #[schemars(
        description = "Execution instance UUID from the action row. Required for both direct and report-scoped submissions."
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
    #[schemars(schema_with = "crate::mcp::tools::internal_api::optional_json_object_schema")]
    pub filters: Option<serde_json::Value>,
    #[schemars(description = "Per-block filter values keyed by filter id. Report context only.")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::optional_json_object_schema")]
    pub block_filters: Option<serde_json::Value>,
}

// ===== Tool Implementations =====

/// List pending signals (WaitForSignal / human-in-the-loop requests) for an eligible running or suspended execution.
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
    validate_identifier_param("request_id", &params.request_id)?;
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
                .find(|i| i.get("requestId").and_then(|v| v.as_str()) == Some(&params.request_id))
        })
        .cloned();

    match schema {
        Some(signal) => json_result(serde_json::json!({
            "success": true,
            "data": {
                "requestId": params.request_id,
                "message": signal.get("message"),
                "responseSchema": signal.get("responseSchema"),
                "toolName": signal.get("toolName"),
            }
        })),
        None => json_result(serde_json::json!({
            "success": false,
            "message": format!("No pending request found with ID '{}'", params.request_id),
        })),
    }
}

/// Accept a managed response or replay its receipt. Explicit pauses are preserved.
pub async fn submit_signal_response(
    server: &SmoMcpServer,
    params: SubmitSignalResponseParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("instance_id", &params.instance_id)?;
    validate_identifier_param("request_id", &params.request_id)?;
    // Recover a client-stringified payload object so the waiting step receives an
    // object, not a JSON string.
    let payload = normalize_json_arg(params.payload, "payload")?;
    let body = serde_json::json!({
        "requestId": params.request_id,
        "operationId": params.operation_id,
        "payload": payload,
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

    // Recover a client-stringified payload object before it reaches the action.
    let payload = normalize_json_arg(params.payload, "payload")?;

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
                Some(serde_json::json!({ "requestId": params.action_id, "operationId": params.operation_id, "payload": payload })),
            )
            .await?;
            json_result(result)
        }
        (None, Some(instance_id), Some(report_id), Some(block_id)) => {
            validate_path_param("instance_id", &instance_id)?;
            validate_path_param("report_id", &report_id)?;
            validate_path_param("block_id", &block_id)?;

            // Recover client-stringified filter objects (keyed-by-id maps).
            let filters = match params.filters {
                Some(filters) => normalize_json_arg(filters, "filters")?,
                None => serde_json::json!({}),
            };
            let block_filters = match params.block_filters {
                Some(block_filters) => normalize_json_arg(block_filters, "block_filters")?,
                None => serde_json::json!({}),
            };

            let result = api_post(
                server,
                &format!(
                    "/api/runtime/reports/{}/blocks/{}/actions/{}/submit",
                    report_id,
                    block_id,
                    encode_path_param(&params.action_id)
                ),
                Some(serde_json::json!({
                    "instanceId": instance_id,
                    "requestId": params.action_id,
                    "operationId": params.operation_id,
                    "payload": payload,
                    "filters": filters,
                    "blockFilters": block_filters,
                })),
            )
            .await?;
            json_result(result)
        }
        _ => Err(rmcp::ErrorData::invalid_params(
            "Provide instance_id and exactly one action context: workflow_id, or report_id + block_id.",
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn managed_signal_parameters_require_request_and_operation_identity() {
        let valid = json!({"instance_id":"instance", "request_id":"request", "operation_id":"operation", "payload":{"answer":true}});
        let parsed: SubmitSignalResponseParams = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(parsed.request_id, "request");
        assert_eq!(parsed.operation_id, "operation");
        for field in ["request_id", "operation_id"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<SubmitSignalResponseParams>(missing).is_err());
        }
        assert!(
            serde_json::from_value::<SubmitSignalResponseParams>(
                json!({"instance_id":"instance","signal_id":"legacy","payload":{}})
            )
            .is_err()
        );
        let schema =
            serde_json::to_value(schemars::schema_for!(SubmitSignalResponseParams)).unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("operation_id")));
        assert!(required.contains(&json!("request_id")));
    }
}
