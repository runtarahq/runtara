use crate::runtime_types::{InstanceStatus, ListStepSummariesOptions, StepSortOrder, StepStatus};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
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
    /// Filter by status: "running", "suspended", "completed", or "failed".
    /// An unfinished step reads "suspended" while its instance is suspended
    /// and takes the instance's status once the instance is terminal.
    pub status: Option<String>,
    /// Filter by step type (e.g., "Http", "Transform", "Agent")
    pub step_type: Option<String>,
    /// Filter by scope ID (for hierarchical steps in Split/While/EmbedWorkflow)
    pub scope_id: Option<String>,
    /// Filter by parent scope ID
    pub parent_scope_id: Option<String>,
    /// When true, only return steps from root scopes (no parent)
    pub root_scopes_only: Option<bool>,
    /// Comma-separated list of step IDs to restrict the result to
    /// (step IDs containing commas cannot be filtered on)
    pub step_ids: Option<String>,
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
    pub workflow_id: String,
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
    /// Step execution status: "running", "suspended" (unfinished while its
    /// instance is suspended), "completed", "failed", or the terminal status
    /// of its instance for a step that never finished
    pub status: String,
    /// When the step started
    pub started_at: DateTime<Utc>,
    /// When the step completed (null if still running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Execution duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Real launch wall-clock (epoch ms) of a parallel branch's async work.
    /// Present only for steps that ran concurrently; with `settled_at_ms` it
    /// gives the true overlapping interval, versus `started_at`/`duration_ms`
    /// (derived from the sequential assemble-order event rows). Consumers prefer
    /// `[launched_at_ms, settled_at_ms]` when both are present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launched_at_ms: Option<i64>,
    /// Real settle wall-clock (epoch ms) of a parallel branch's async work. See
    /// `launched_at_ms`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at_ms: Option<i64>,
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

fn error_from_output_envelope(outputs: Option<&Value>) -> Option<Value> {
    let outputs = outputs?;
    if outputs.get("_error").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }

    Some(
        outputs
            .get("error")
            .cloned()
            // Kept identical to runtara-core's own envelope fallback
            // (`persistence::common::row::error_from_output_envelope`). Core's
            // value wins where both run, so a divergence here would surface as
            // one failure reported two different ways.
            .unwrap_or_else(|| json!("Output reported _error=true")),
    )
}

/// The label an unfinished step takes from its instance: the terminal status
/// once the instance has ended, `suspended` while it is suspended, else
/// `running`.
fn unfinished_step_label(instance_status: Option<InstanceStatus>) -> &'static str {
    match instance_status {
        Some(InstanceStatus::Failed) => "failed",
        Some(InstanceStatus::Cancelled) => "cancelled",
        Some(InstanceStatus::Completed) => "completed",
        Some(InstanceStatus::Suspended) => "suspended",
        _ => "running",
    }
}

/// The store filter a requested status maps to, and whether it can match at
/// all: `running` and `suspended` both select unfinished steps, which match
/// only when their label under the current instance status is the one asked
/// for. `None` for an unknown status (no filter).
fn status_filter(requested: &str, unfinished_label: &str) -> Option<(StepStatus, bool)> {
    match requested.to_lowercase().as_str() {
        label @ ("running" | "suspended") => Some((StepStatus::Running, label == unfinished_label)),
        "completed" => Some((StepStatus::Completed, true)),
        "failed" => Some((StepStatus::Failed, true)),
        _ => None,
    }
}

fn step_status_and_error(
    status: StepStatus,
    unfinished_label: &str,
    outputs: Option<&Value>,
    error: Option<Value>,
) -> (String, Option<Value>) {
    let output_error = error_from_output_envelope(outputs);
    let error = error.or(output_error);
    let status = match status {
        StepStatus::Running => unfinished_label.to_string(),
        StepStatus::Completed if error.is_some() => "failed".to_string(),
        StepStatus::Completed => "completed".to_string(),
        StepStatus::Failed => "failed".to_string(),
    };

    (status, error)
}

/// Handler to get step summaries for a workflow execution
///
/// GET /api/runtime/workflows/{workflow_id}/instances/{instance_id}/steps
///
/// Returns unified step records with paired start/end events. Each step appears
/// once with its complete lifecycle information (inputs, outputs, duration, status).
#[utoipa::path(
    get,
    path = "/api/runtime/workflows/{workflowId}/instances/{instanceId}/steps",
    params(
        ("workflowId" = String, Path, description = "Workflow identifier"),
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
    tag = "workflow-controller"
)]
pub async fn get_step_summaries(
    crate::middleware::tenant_auth::OrgId(_tenant_id): crate::middleware::tenant_auth::OrgId,
    Path((workflow_id, instance_id)): Path<(String, String)>,
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

    // The instance status decides how unfinished steps read: suspended while
    // the instance is suspended, its terminal status once it has ended.
    let instance_status = match client.get_instance_info(&instance_id).await {
        Ok(info) => Some(info.status),
        Err(e) => {
            tracing::warn!(
                instance_id = %instance_id,
                error = %e,
                "Failed to get instance info for step status override"
            );
            None
        }
    };
    let unfinished_label = unfinished_step_label(instance_status);

    // Parse status filter
    let mut filter_matches = true;
    if let Some(status_str) = &query.status
        && let Some((status, matches)) = status_filter(status_str, unfinished_label)
    {
        options = options.with_status(status);
        filter_matches = matches;
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

    if let Some(step_ids) = &query.step_ids {
        let ids: Vec<&str> = step_ids
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .collect();
        if !ids.is_empty() {
            options = options.with_step_ids(ids);
        }
    }

    // Fetch step summaries from runtara-environment
    match client
        .list_step_summaries(&instance_id, Some(options))
        .await
    {
        Ok(mut result) => {
            // A running/suspended filter that the instance status rules out
            // selects nothing.
            if !filter_matches {
                result.steps.clear();
                result.total_count = 0;
            }

            // Convert SDK StepSummary to response format
            let steps: Vec<StepSummaryResponse> = result
                .steps
                .into_iter()
                .map(|step| {
                    // An unfinished step reads as its instance: suspended, or
                    // the terminal status once the instance has ended.
                    let (status, error) = step_status_and_error(
                        step.status,
                        unfinished_label,
                        step.outputs.as_ref(),
                        step.error,
                    );

                    StepSummaryResponse {
                        step_id: step.step_id,
                        step_name: step.step_name,
                        step_type: step.step_type,
                        status,
                        started_at: step.started_at,
                        completed_at: step.completed_at,
                        duration_ms: step.duration_ms,
                        launched_at_ms: step.launched_at_ms,
                        settled_at_ms: step.settled_at_ms,
                        inputs: step.inputs,
                        outputs: step.outputs,
                        error,
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
                    "workflowId": workflow_id,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_step_with_output_error_is_reported_failed() {
        let outputs = json!({
            "_error": true,
            "error": {"message": "Capability failed"}
        });

        let (status, error) =
            step_status_and_error(StepStatus::Completed, "running", Some(&outputs), None);

        assert_eq!(status, "failed");
        assert_eq!(error, Some(json!({"message": "Capability failed"})));
    }

    #[test]
    fn completed_step_without_output_error_stays_completed() {
        let outputs = json!({"ok": true});

        let (status, error) =
            step_status_and_error(StepStatus::Completed, "running", Some(&outputs), None);

        assert_eq!(status, "completed");
        assert_eq!(error, None);
    }

    #[test]
    fn an_unfinished_step_reads_as_its_instance() {
        for (instance, label) in [
            (None, "running"),
            (Some(InstanceStatus::Running), "running"),
            (Some(InstanceStatus::Pending), "running"),
            (Some(InstanceStatus::Suspended), "suspended"),
            (Some(InstanceStatus::Completed), "completed"),
            (Some(InstanceStatus::Failed), "failed"),
            (Some(InstanceStatus::Cancelled), "cancelled"),
        ] {
            let unfinished = unfinished_step_label(instance);
            assert_eq!(unfinished, label, "{instance:?}");
            let (status, _) = step_status_and_error(StepStatus::Running, unfinished, None, None);
            assert_eq!(status, label);
            // A finished step keeps its own status whatever the instance is.
            let (status, _) = step_status_and_error(StepStatus::Completed, unfinished, None, None);
            assert_eq!(status, "completed");
        }
    }

    #[test]
    fn running_and_suspended_filters_match_only_the_current_label() {
        let suspended = unfinished_step_label(Some(InstanceStatus::Suspended));
        let running = unfinished_step_label(Some(InstanceStatus::Running));
        assert_eq!(
            status_filter("suspended", suspended),
            Some((StepStatus::Running, true))
        );
        assert_eq!(
            status_filter("SUSPENDED", running),
            Some((StepStatus::Running, false))
        );
        assert_eq!(
            status_filter("running", running),
            Some((StepStatus::Running, true))
        );
        assert_eq!(
            status_filter("running", suspended),
            Some((StepStatus::Running, false))
        );
        assert_eq!(
            status_filter("completed", suspended),
            Some((StepStatus::Completed, true))
        );
        assert_eq!(
            status_filter("failed", running),
            Some((StepStatus::Failed, true))
        );
        assert_eq!(status_filter("bogus", running), None);
    }
}
