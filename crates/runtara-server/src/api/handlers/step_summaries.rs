use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use runtara_management_sdk::{ListStepSummariesOptions, StepSortOrder, StepStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::runtime_client::RuntimeClient;

/// Query parameters for step summaries endpoint
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct StepSummariesQuery {
    /// Limit number of results (default: 100, max: 1000)
    pub limit: Option<u32>,
    /// Pagination offset
    pub offset: Option<u32>,
    /// Sort order: "asc" (oldest first) or "desc" (newest first, default)
    pub sort_order: Option<String>,
    /// Filter by status: "running", "completed", or "failed"
    pub status: Option<String>,
    /// Filter by step type (e.g., "Http", "Transform", "Agent")
    pub step_type: Option<String>,
    /// Filter by scope ID (for hierarchical steps in Split/While/StartScenario)
    pub scope_id: Option<String>,
    /// Filter by parent scope ID
    pub parent_scope_id: Option<String>,
    /// When true, only return steps from root scopes (no parent)
    pub root_scopes_only: Option<bool>,
}

/// Response wrapper for step summaries (used for OpenAPI documentation)
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct StepSummariesResponse {
    pub success: bool,
    pub message: String,
    pub data: StepSummariesResponseData,
}

/// Step summaries response data with pagination info (used for OpenAPI documentation)
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct StepSummariesResponseData {
    pub scenario_id: String,
    pub instance_id: String,
    pub steps: Vec<StepSummaryResponse>,
    pub count: usize,
    pub total_count: u32,
    pub limit: u32,
    pub offset: u32,
}

/// Individual step summary in the response
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepSummaryResponse {
    /// Unique step identifier
    pub step_id: String,
    /// Human-readable step name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
    /// Step type (e.g., "Http", "Transform", "Agent")
    pub step_type: String,
    /// Step execution status
    pub status: String,
    /// When the step started
    pub started_at: DateTime<Utc>,
    /// When the step completed (null if still running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Execution duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Step input data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
    /// Step output data (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Value>,
    /// Error details (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    /// Step's scope ID for hierarchy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    /// Parent scope ID for nesting
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_scope_id: Option<String>,
}

/// Handler to get step summaries for a scenario execution
///
/// GET /api/runtime/scenarios/{scenario_id}/instances/{instance_id}/steps
///
/// Returns unified step records with paired start/end events. Each step appears
/// once with its complete lifecycle information (inputs, outputs, duration, status).
#[utoipa::path(
    get,
    path = "/api/runtime/scenarios/{scenarioId}/instances/{instanceId}/steps",
    params(
        ("scenarioId" = String, Path, description = "Scenario identifier"),
        ("instanceId" = String, Path, description = "Instance identifier (UUID)"),
        StepSummariesQuery
    ),
    responses(
        (status = 200, description = "Step summaries retrieved successfully", body = StepSummariesResponse),
        (status = 400, description = "Invalid instance ID format or invalid parameter", body = Value),
        (status = 404, description = "Instance not found", body = Value),
        (status = 503, description = "Runtime client not configured", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "scenario-controller"
)]
pub async fn get_step_summaries(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    Path((scenario_id, instance_id)): Path<(String, String)>,
    Query(query): Query<StepSummariesQuery>,
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
                "message": "Runtime client not configured - step summaries are not available",
                "data": Value::Null
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error_response));
        }
    };

    // Build options from query params
    let limit = query.limit.map(|l| l.min(1000)).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

    let mut options = ListStepSummariesOptions::new()
        .with_limit(limit)
        .with_offset(offset);

    // Parse sort order
    if let Some(sort_order_str) = &query.sort_order {
        let sort_order = match sort_order_str.to_lowercase().as_str() {
            "asc" => StepSortOrder::Asc,
            "desc" => StepSortOrder::Desc,
            _ => StepSortOrder::Desc,
        };
        options = options.with_sort_order(sort_order);
    }

    // Parse status filter
    if let Some(status_str) = &query.status {
        let status = match status_str.to_lowercase().as_str() {
            "running" => Some(StepStatus::Running),
            "completed" => Some(StepStatus::Completed),
            "failed" => Some(StepStatus::Failed),
            _ => None,
        };
        if let Some(s) = status {
            options = options.with_status(s);
        }
    }

    if let Some(step_type) = &query.step_type {
        options = options.with_step_type(step_type);
    }

    if let Some(scope_id) = &query.scope_id {
        options = options.with_scope_id(scope_id);
    }

    if let Some(parent_scope_id) = &query.parent_scope_id {
        options = options.with_parent_scope_id(parent_scope_id);
    }

    if query.root_scopes_only == Some(true) {
        options = options.with_root_scopes_only();
    }

    // Fetch step summaries from runtara-environment
    match client
        .list_step_summaries(&instance_id, Some(options))
        .await
    {
        Ok(result) => {
            // Fetch instance info to check if instance is in terminal state (SMO-228 fix)
            let instance_terminal_state = match client.get_instance_info(&instance_id).await {
                Ok(info) => {
                    use runtara_management_sdk::InstanceStatus;
                    match info.status {
                        InstanceStatus::Failed => Some("failed"),
                        InstanceStatus::Cancelled => Some("cancelled"),
                        InstanceStatus::Completed => Some("completed"),
                        _ => None,
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        instance_id = %instance_id,
                        error = %e,
                        "Failed to get instance info for step status override"
                    );
                    None
                }
            };

            if let Some(terminal_state) = instance_terminal_state {
                tracing::debug!(
                    instance_id = %instance_id,
                    instance_status = terminal_state,
                    "Instance is terminal, non-terminal steps will inherit this state (SMO-228)"
                );
            }

            // Convert SDK StepSummary to response format
            let steps: Vec<StepSummaryResponse> = result
                .steps
                .into_iter()
                .map(|step| {
                    // SMO-228 fix: If instance is terminal but step is not, step inherits instance state
                    let status = match step.status {
                        StepStatus::Running => {
                            // Step is non-terminal, inherit instance state if instance is terminal
                            instance_terminal_state.unwrap_or("running").to_string()
                        }
                        StepStatus::Completed => "completed".to_string(),
                        StepStatus::Failed => "failed".to_string(),
                    };

                    StepSummaryResponse {
                        step_id: step.step_id,
                        step_name: step.step_name,
                        step_type: step.step_type,
                        status,
                        started_at: step.started_at,
                        completed_at: step.completed_at,
                        duration_ms: step.duration_ms,
                        inputs: step.inputs,
                        outputs: step.outputs,
                        error: step.error,
                        scope_id: step.scope_id,
                        parent_scope_id: step.parent_scope_id,
                    }
                })
                .collect();

            let count = steps.len();

            let response = json!({
                "success": true,
                "message": "Step summaries retrieved successfully",
                "data": {
                    "scenarioId": scenario_id,
                    "instanceId": instance_id,
                    "steps": steps,
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
                "message": format!("Failed to retrieve step summaries: {}", error_message),
                "data": Value::Null
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response))
        }
    }
}
