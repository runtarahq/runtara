use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use runtara_management_sdk::{EventSortOrder, ListEventsOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::api::services::workflow_runtime::{
    SubmitWorkflowActionRequest, WorkflowRuntimeError, list_instance_actions,
    list_workflow_actions, submit_workflow_action as submit_runtime_workflow_action,
};
use crate::runtime_client::RuntimeClient;
use crate::workers::execution_engine::ExecutionEngine;

/// Query parameters for step events endpoint
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct StepEventsQuery {
    /// Filter by event type (e.g., "custom", "started", "completed")
    pub event_type: Option<String>,
    /// Filter by subtype (e.g., "step_debug_start", "step_debug_end", "workflow_log")
    pub subtype: Option<String>,
    /// Limit number of results (default: 100, max: 1000)
    pub limit: Option<u32>,
    /// Pagination offset
    pub offset: Option<u32>,
    /// Filter events created after this timestamp (ISO 8601 format)
    pub created_after: Option<DateTime<Utc>>,
    /// Filter events created before this timestamp (ISO 8601 format)
    pub created_before: Option<DateTime<Utc>>,
    /// Full-text search in event payload JSON
    pub payload_contains: Option<String>,
    /// Filter events by scope ID (for hierarchical step events in Split/While/EmbedWorkflow)
    pub scope_id: Option<String>,
    /// Filter events by parent scope ID (use "null" for root-level events)
    pub parent_scope_id: Option<String>,
    /// When true, only return events from root scopes (no parent scope)
    pub root_scopes_only: Option<bool>,
    /// Sort order for results: "asc" (oldest first) or "desc" (newest first, default)
    pub sort_order: Option<String>,
}

/// Response wrapper for step events with total count
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepEventsResponse {
    pub success: bool,
    pub message: String,
    pub data: StepEventsResponseData,
}

/// Step events response data with pagination info
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepEventsResponseData {
    pub workflow_id: String,
    pub instance_id: String,
    pub events: Vec<StepEventResponse>,
    pub count: usize,
    pub total_count: u32,
    pub limit: u32,
    pub offset: u32,
}

/// Individual step event in the response
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepEventResponse {
    /// Event ID from the database
    pub id: i64,
    /// Event type (e.g., "custom")
    pub event_type: String,
    /// Event subtype (e.g., "step_debug_start", "step_debug_end")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    /// Associated checkpoint ID if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Event payload (parsed JSON)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// When the event was created
    pub created_at: DateTime<Utc>,
}

/// Handler to get step events for a workflow execution
///
/// GET /api/runtime/workflows/{workflow_id}/instances/{instance_id}/step-events
///
/// Retrieves debug step events from runtara-environment. The workflow must be
/// compiled with track_events enabled for events to be recorded.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflowId}/instances/{instanceId}/step-events",
    params(
        ("workflowId" = String, Path, description = "Workflow identifier"),
        ("instanceId" = String, Path, description = "Instance identifier (UUID)"),
        StepEventsQuery
    ),
    responses(
        (status = 200, description = "Step events retrieved successfully", body = StepEventsResponse),
        (status = 400, description = "Invalid instance ID format", body = Value),
        (status = 404, description = "Instance not found", body = Value),
        (status = 503, description = "Runtime client not configured", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
pub async fn get_step_events(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    Path((workflow_id, instance_id)): Path<(String, String)>,
    Query(query): Query<StepEventsQuery>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
) -> (StatusCode, Json<Value>) {
    // Parse instance UUID
    let _instance_uuid = match Uuid::parse_str(&instance_id) {
        Ok(uuid) => uuid,
        Err(_) => {
            let error_response = json!({
                "success": false,
                "message": "Invalid instance ID format",
                "data": Value::Null
            });
            return (StatusCode::BAD_REQUEST, Json(error_response));
        }
    };

    // Get runtime client
    let client = match runtime_client {
        Some(c) => c,
        None => {
            let error_response = json!({
                "success": false,
                "message": "Runtime client not configured - step events are not available",
                "data": Value::Null
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error_response));
        }
    };

    // Build list events options from query params
    let limit = query.limit.map(|l| l.min(1000)).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

    let mut options = ListEventsOptions::new()
        .with_limit(limit)
        .with_offset(offset);

    if let Some(event_type) = &query.event_type {
        options = options.with_event_type(event_type);
    }

    if let Some(subtype) = &query.subtype {
        options = options.with_subtype(subtype);
    }

    if let Some(created_after) = query.created_after {
        options = options.with_created_after(created_after);
    }

    if let Some(created_before) = query.created_before {
        options = options.with_created_before(created_before);
    }

    if let Some(payload_contains) = &query.payload_contains {
        options = options.with_payload_contains(payload_contains);
    }

    // Scope-based hierarchy filters
    if let Some(scope_id) = &query.scope_id {
        options = options.with_scope_id(scope_id);
    }

    if let Some(parent_scope_id) = &query.parent_scope_id {
        // "null" string means filter for root-level events (no parent)
        // For that case, use root_scopes_only instead
        if parent_scope_id != "null" {
            options = options.with_parent_scope_id(parent_scope_id);
        } else {
            options = options.with_root_scopes_only();
        }
    }

    if query.root_scopes_only == Some(true) {
        options = options.with_root_scopes_only();
    }

    // Parse sort order: "asc" for oldest first, "desc" (default) for newest first
    if let Some(sort_order_str) = &query.sort_order {
        let sort_order = match sort_order_str.to_lowercase().as_str() {
            "asc" => EventSortOrder::Asc,
            "desc" => EventSortOrder::Desc,
            _ => EventSortOrder::Desc, // Default to desc for invalid values
        };
        options = options.with_sort_order(sort_order);
    }

    // Fetch events from runtara-environment
    match client.list_events(&instance_id, Some(options)).await {
        Ok(result) => {
            // Convert EventSummary to StepEventResponse
            let events: Vec<StepEventResponse> = result
                .events
                .into_iter()
                .map(|event| StepEventResponse {
                    id: event.id,
                    event_type: event.event_type,
                    subtype: event.subtype,
                    checkpoint_id: event.checkpoint_id,
                    payload: event.payload,
                    created_at: event.created_at,
                })
                .collect();

            let count = events.len();

            let response = json!({
                "success": true,
                "message": "Step events retrieved successfully",
                "data": {
                    "workflowId": workflow_id,
                    "instanceId": instance_id,
                    "events": events,
                    "count": count,
                    "totalCount": result.total_count,
                    "limit": result.limit,
                    "offset": result.offset
                }
            });

            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            let error_message = e.to_string();

            // Check for instance not found error
            if error_message.contains("not found") {
                let error_response = json!({
                    "success": false,
                    "message": format!("Instance not found: {}", instance_id),
                    "data": Value::Null
                });
                return (StatusCode::NOT_FOUND, Json(error_response));
            }

            let error_response = json!({
                "success": false,
                "message": format!("Failed to retrieve step events: {}", error_message),
                "data": Value::Null
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Scope ancestor information in the response
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScopeAncestorResponse {
    /// The scope ID
    pub scope_id: String,
    /// Parent scope ID (null for root scopes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_scope_id: Option<String>,
    /// Step ID that created this scope
    pub step_id: String,
    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
    /// Step type (e.g., "Split", "While", "EmbedWorkflow")
    pub step_type: String,
    /// Iteration index for Split/While steps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// When this scope was entered
    pub created_at: DateTime<Utc>,
}

/// Response wrapper for scope ancestors (used for OpenAPI documentation)
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ScopeAncestorsResponse {
    pub success: bool,
    pub message: String,
    pub data: ScopeAncestorsResponseData,
}

/// Scope ancestors response data (used for OpenAPI documentation)
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct ScopeAncestorsResponseData {
    pub instance_id: String,
    pub scope_id: String,
    /// Ancestors ordered from immediate parent to root
    pub ancestors: Vec<ScopeAncestorResponse>,
}

/// Handler to get ancestor scopes for a given scope ID
///
/// GET /api/runtime/workflows/{workflow_id}/instances/{instance_id}/scopes/{scope_id}/ancestors
///
/// Returns the chain of parent scopes from the given scope up to the root,
/// useful for reconstructing the call stack in hierarchical step execution
/// (Split/While/EmbedWorkflow).
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflowId}/instances/{instanceId}/scopes/{scopeId}/ancestors",
    params(
        ("workflowId" = String, Path, description = "Workflow identifier"),
        ("instanceId" = String, Path, description = "Instance identifier (UUID)"),
        ("scopeId" = String, Path, description = "Scope identifier to get ancestors for")
    ),
    responses(
        (status = 200, description = "Scope ancestors retrieved successfully", body = ScopeAncestorsResponse),
        (status = 400, description = "Invalid instance ID format", body = Value),
        (status = 404, description = "Instance or scope not found", body = Value),
        (status = 503, description = "Runtime client not configured", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "workflow-controller"
)]
pub async fn get_scope_ancestors(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    Path((_workflow_id, instance_id, scope_id)): Path<(String, String, String)>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
) -> (StatusCode, Json<Value>) {
    // Parse instance UUID
    let _instance_uuid = match Uuid::parse_str(&instance_id) {
        Ok(uuid) => uuid,
        Err(_) => {
            let error_response = json!({
                "success": false,
                "message": "Invalid instance ID format",
                "data": Value::Null
            });
            return (StatusCode::BAD_REQUEST, Json(error_response));
        }
    };

    // Get runtime client
    let client = match runtime_client {
        Some(c) => c,
        None => {
            let error_response = json!({
                "success": false,
                "message": "Runtime client not configured - scope ancestors are not available",
                "data": Value::Null
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error_response));
        }
    };

    // Fetch scope ancestors from runtara-environment
    match client.get_scope_ancestors(&instance_id, &scope_id).await {
        Ok(scope_infos) => {
            // Convert SDK ScopeInfo to response format
            let ancestors: Vec<ScopeAncestorResponse> = scope_infos
                .into_iter()
                .map(|info| ScopeAncestorResponse {
                    scope_id: info.scope_id,
                    parent_scope_id: info.parent_scope_id,
                    step_id: info.step_id,
                    step_name: info.step_name,
                    step_type: info.step_type,
                    index: info.index,
                    created_at: info.created_at,
                })
                .collect();

            let response = json!({
                "success": true,
                "message": "Scope ancestors retrieved successfully",
                "data": {
                    "instanceId": instance_id,
                    "scopeId": scope_id,
                    "ancestors": ancestors
                }
            });

            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            let error_message = e.to_string();

            // Check for not found errors
            if error_message.contains("not found") {
                let error_response = json!({
                    "success": false,
                    "message": format!("Instance or scope not found: {}/{}", instance_id, scope_id),
                    "data": Value::Null
                });
                return (StatusCode::NOT_FOUND, Json(error_response));
            }

            let error_response = json!({
                "success": false,
                "message": format!("Failed to retrieve scope ancestors: {}", error_message),
                "data": Value::Null
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}

/// Response for pending human input requests
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingInputResponse {
    /// Signal ID to deliver the response to
    pub signal_id: String,
    /// Tool name that requested the input
    pub tool_name: Option<String>,
    /// Message from the AI Agent explaining what input is needed
    pub message: String,
    /// Schema describing expected response fields (same format as workflow inputSchema)
    pub response_schema: Option<Value>,
    /// The AI Agent step that initiated this request
    pub ai_agent_step_id: Option<String>,
    /// Iteration number in the agent loop
    pub iteration: Option<u32>,
    /// Tool call number
    pub call_number: Option<u32>,
    /// When the input was requested
    pub requested_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowActionsQuery {
    pub page: Option<i32>,
    pub size: Option<i32>,
}

/// Get pending human input requests for a running execution.
///
/// Returns any active `external_input_requested` events that haven't been
/// resolved yet (the corresponding tool call step hasn't completed).
/// The frontend uses this to render input forms when an AI Agent is
/// waiting for human-in-the-loop input.
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflowId}/instances/{instanceId}/pending-input",
    params(
        ("workflowId" = String, Path, description = "Workflow ID"),
        ("instanceId" = String, Path, description = "Instance/execution ID"),
    ),
    responses(
        (status = 200, description = "Pending input requests"),
        (status = 404, description = "Instance not found"),
        (status = 503, description = "Runtime client not configured"),
    ),
    tag = "step-events"
)]
pub async fn get_pending_input(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    Path((_workflow_id, instance_id)): Path<(String, String)>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
) -> (StatusCode, Json<Value>) {
    // Parse instance UUID
    if Uuid::parse_str(&instance_id).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "message": "Invalid instance ID format",
                "data": Value::Null
            })),
        );
    }

    // Get runtime client
    let client = match runtime_client {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "success": false,
                    "message": "Runtime client not configured",
                    "data": Value::Null
                })),
            );
        }
    };

    // Query all custom events with subtype "external_input_requested"
    let input_options = ListEventsOptions::new()
        .with_limit(100)
        .with_event_type("custom")
        .with_subtype("external_input_requested")
        .with_sort_order(EventSortOrder::Asc);

    let input_events = match client.list_events(&instance_id, Some(input_options)).await {
        Ok(result) => result.events,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "success": false,
                        "message": format!("Instance not found: {}", instance_id),
                        "data": Value::Null
                    })),
                );
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "message": format!("Failed to query events: {}", msg),
                    "data": Value::Null
                })),
            );
        }
    };

    // Query all step_debug_end events to find completed steps
    // (both AiAgentToolCall and WaitForSignal)
    let end_options = ListEventsOptions::new()
        .with_limit(1000)
        .with_event_type("custom")
        .with_subtype("step_debug_end");

    let end_events = match client.list_events(&instance_id, Some(end_options)).await {
        Ok(result) => result.events,
        Err(_) => vec![], // If we can't query, assume none completed
    };

    // Collect completed step IDs from step_debug_end events
    // The payload fields are at the top level (not nested under "data")
    let completed_tool_ids: std::collections::HashSet<String> = end_events
        .iter()
        .filter_map(|event| {
            event
                .payload
                .as_ref()
                .and_then(|p| p.get("step_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    // Filter to only pending requests (where the tool call hasn't completed)
    // The payload fields are at the top level (not nested under "data")
    let pending: Vec<PendingInputResponse> = input_events
        .iter()
        .filter_map(|event| {
            let data = event.payload.as_ref()?;

            let signal_id = data.get("signal_id")?.as_str()?.to_string();
            let ai_step_id = data.get("ai_agent_step_id").and_then(|v| v.as_str());
            let tool_name = data.get("tool_name").and_then(|v| v.as_str());
            let step_id = data.get("step_id").and_then(|v| v.as_str());
            let call_number = data
                .get("call_number")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);

            // Build the expected step ID to check if it completed.
            // AI Agent tool calls: "{ai_step_id}.tool.{tool_name}.{call_number}"
            // Standalone WaitForSignal: "{step_id}"
            let check_step_id = match (ai_step_id, tool_name, call_number) {
                (Some(step), Some(tool), Some(num)) => {
                    format!("{}.tool.{}.{}", step, tool, num)
                }
                _ => step_id.unwrap_or("").to_string(),
            };

            // Skip if this step already completed
            if !check_step_id.is_empty() && completed_tool_ids.contains(&check_step_id) {
                return None;
            }

            Some(PendingInputResponse {
                signal_id,
                tool_name: tool_name.map(|s| s.to_string()),
                message: data
                    .get("message")
                    .or_else(|| data.get("step_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("External input requested")
                    .to_string(),
                response_schema: data.get("response_schema").cloned(),
                ai_agent_step_id: ai_step_id.map(|s| s.to_string()),
                iteration: data
                    .get("iteration")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
                call_number,
                requested_at: Some(event.created_at),
            })
        })
        .collect();

    let count = pending.len();

    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "message": if count > 0 { "Pending input requests found" } else { "No pending input requests" },
            "data": {
                "instanceId": instance_id,
                "pendingInputs": pending,
                "count": count
            }
        })),
    )
}

#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflowId}/actions",
    params(
        ("workflowId" = String, Path, description = "Workflow ID"),
        WorkflowActionsQuery,
    ),
    responses(
        (status = 200, description = "Open workflow actions"),
        (status = 503, description = "Runtime client not configured"),
    ),
    tag = "actions"
)]
pub async fn list_workflow_open_actions(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path(workflow_id): Path<String>,
    Query(query): Query<WorkflowActionsQuery>,
) -> (StatusCode, Json<Value>) {
    let Some(client) = runtime_client else {
        return workflow_runtime_error_response(WorkflowRuntimeError::RuntimeUnavailable);
    };

    match list_workflow_actions(
        &engine,
        &client,
        &tenant_id,
        &workflow_id,
        query.page,
        query.size,
    )
    .await
    {
        Ok(page) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Workflow actions retrieved",
                "data": page,
            })),
        ),
        Err(error) => workflow_runtime_error_response(error),
    }
}

#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflowId}/instances/{instanceId}/actions",
    params(
        ("workflowId" = String, Path, description = "Workflow ID"),
        ("instanceId" = String, Path, description = "Instance/execution ID"),
    ),
    responses(
        (status = 200, description = "Open workflow instance actions"),
        (status = 404, description = "Instance not found"),
        (status = 503, description = "Runtime client not configured"),
    ),
    tag = "actions"
)]
pub async fn list_workflow_instance_open_actions(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path((workflow_id, instance_id)): Path<(String, String)>,
) -> (StatusCode, Json<Value>) {
    let Some(client) = runtime_client else {
        return workflow_runtime_error_response(WorkflowRuntimeError::RuntimeUnavailable);
    };

    let execution = match engine
        .get_execution_with_metadata(&workflow_id, &instance_id, &tenant_id)
        .await
    {
        Ok(execution) => execution,
        Err(error) => {
            return (
                error.http_status(),
                Json(json!({
                    "success": false,
                    "message": error.to_string(),
                    "data": Value::Null,
                })),
            );
        }
    };

    if execution.instance.status.is_terminal() || !execution.instance.has_pending_input {
        return (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Workflow instance actions retrieved",
                "data": {
                    "workflowId": workflow_id,
                    "instanceId": instance_id,
                    "actions": [],
                    "count": 0,
                },
            })),
        );
    }

    match list_instance_actions(&client, &workflow_id, &instance_id).await {
        Ok(actions) => {
            let count = actions.len();
            (
                StatusCode::OK,
                Json(json!({
                    "success": true,
                    "message": "Workflow instance actions retrieved",
                    "data": {
                        "workflowId": workflow_id,
                        "instanceId": instance_id,
                        "actions": actions,
                        "count": count,
                    },
                })),
            )
        }
        Err(error) => workflow_runtime_error_response(error),
    }
}

#[utoipa::path(
    post,
    path = "/api/runtime/workflows/{workflowId}/instances/{instanceId}/actions/{actionId}/submit",
    params(
        ("workflowId" = String, Path, description = "Workflow ID"),
        ("instanceId" = String, Path, description = "Instance/execution ID"),
        ("actionId" = String, Path, description = "Action ID"),
    ),
    request_body = SubmitWorkflowActionRequest,
    responses(
        (status = 200, description = "Action submitted successfully"),
        (status = 400, description = "Invalid action payload"),
        (status = 404, description = "Instance not found"),
        (status = 409, description = "Action is no longer open"),
        (status = 503, description = "Runtime client not configured"),
    ),
    tag = "actions"
)]
pub async fn submit_workflow_action(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(engine): State<Arc<ExecutionEngine>>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Path((workflow_id, instance_id, action_id)): Path<(String, String, String)>,
    Json(body): Json<SubmitWorkflowActionRequest>,
) -> (StatusCode, Json<Value>) {
    let Some(client) = runtime_client else {
        return workflow_runtime_error_response(WorkflowRuntimeError::RuntimeUnavailable);
    };

    match submit_runtime_workflow_action(
        &engine,
        &client,
        &tenant_id,
        &workflow_id,
        &instance_id,
        &action_id,
        &body.payload,
    )
    .await
    {
        Ok(action) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Workflow action submitted successfully",
                "data": {
                    "workflowId": workflow_id,
                    "instanceId": instance_id,
                    "actionId": action.action_id,
                    "signalId": action.signal_id,
                },
            })),
        ),
        Err(error) => workflow_runtime_error_response(error),
    }
}

/// Request body for submitting human input to a waiting AI Agent.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitSignalRequest {
    /// The signal ID from the pending input request or `external_input_requested` event.
    pub signal_id: String,
    /// The response payload to deliver to the waiting step.
    /// Should conform to the response_schema from the pending input request.
    pub payload: Value,
}

fn workflow_runtime_error_response(error: WorkflowRuntimeError) -> (StatusCode, Json<Value>) {
    match error {
        WorkflowRuntimeError::InvalidRequest(message) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "message": message,
                "data": Value::Null,
            })),
        ),
        WorkflowRuntimeError::NotFound(message) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "message": message,
                "data": Value::Null,
            })),
        ),
        WorkflowRuntimeError::Conflict(message) => (
            StatusCode::CONFLICT,
            Json(json!({
                "success": false,
                "message": message,
                "data": Value::Null,
            })),
        ),
        WorkflowRuntimeError::RuntimeUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "success": false,
                "message": "Runtime client not configured",
                "data": Value::Null,
            })),
        ),
        WorkflowRuntimeError::Runtime(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "message": message,
                "data": Value::Null,
            })),
        ),
    }
}

/// Submit a human response to a waiting AI Agent step.
///
/// Delivers a custom signal to the workflow instance, resuming the AI Agent's
/// WaitForSignal tool call. The signal_id must match the one from
/// the `pending-input` endpoint or the `external_input_requested` event.
#[utoipa::path(
    post,
    path = "/api/runtime/signals/{instanceId}",
    params(
        ("instanceId" = String, Path, description = "Instance/execution ID"),
    ),
    request_body = SubmitSignalRequest,
    responses(
        (status = 200, description = "Signal delivered successfully"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Instance or signal not found"),
        (status = 503, description = "Runtime client not configured"),
    ),
    tag = "signals"
)]
pub async fn submit_signal(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    Path(instance_id): Path<String>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Json(body): Json<SubmitSignalRequest>,
) -> (StatusCode, Json<Value>) {
    // Validate instance ID format
    if Uuid::parse_str(&instance_id).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "message": "Invalid instance ID format",
                "data": Value::Null
            })),
        );
    }

    // Validate signal_id is not empty
    if body.signal_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "message": "signal_id is required",
                "data": Value::Null
            })),
        );
    }

    // Get runtime client
    let client = match runtime_client {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "success": false,
                    "message": "Runtime client not configured",
                    "data": Value::Null
                })),
            );
        }
    };

    // Serialize payload to bytes
    let payload_bytes = match serde_json::to_vec(&body.payload) {
        Ok(bytes) => bytes,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "message": format!("Failed to serialize payload: {}", e),
                    "data": Value::Null
                })),
            );
        }
    };

    // Send custom signal
    match client
        .send_custom_signal(&instance_id, &body.signal_id, Some(&payload_bytes))
        .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Signal delivered successfully",
                "data": {
                    "instanceId": instance_id,
                    "signalId": body.signal_id
                }
            })),
        ),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "success": false,
                        "message": format!("Instance or signal not found: {}", msg),
                        "data": Value::Null
                    })),
                )
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "success": false,
                        "message": format!("Failed to deliver signal: {}", msg),
                        "data": Value::Null
                    })),
                )
            }
        }
    }
}
