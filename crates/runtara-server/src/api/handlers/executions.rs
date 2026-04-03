/// Handlers for execution-related endpoints
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;

use crate::api::dto::executions::{
    ExecutionFilters, ListAllExecutionsQuery, ListAllExecutionsResponse,
};
use crate::api::repositories::scenarios::ScenarioRepository;
use crate::api::services::executions::ExecutionService;
use crate::runtime_client::RuntimeClient;

/// List all executions across all scenarios with filtering, sorting, and pagination
#[utoipa::path(
    get,
    path = "/api/runtime/executions",
    params(ListAllExecutionsQuery),
    responses(
        (status = 200, description = "List of executions retrieved successfully", body = ListAllExecutionsResponse),
        (status = 400, description = "Invalid request parameters", body = Value),
        (status = 401, description = "Missing Authorization header", body = Value),
        (status = 500, description = "Internal server error", body = Value)
    ),
    tag = "executions-controller"
)]
pub async fn list_all_executions_handler(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Query(query): Query<ListAllExecutionsQuery>,
) -> (StatusCode, Json<Value>) {
    // RuntimeClient is required for listing executions (data is in runtara-environment)
    let runtime_client = match runtime_client {
        Some(client) => client,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "success": false,
                    "error": "Runtara environment not configured. Execution listing requires runtara-environment connection."
                })),
            );
        }
    };

    // Parse and validate query parameters
    let filters = match parse_filters(&query) {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "error": e
                })),
            );
        }
    };

    // Create service with runtime client for proxying to Runtara
    let scenario_repo = Arc::new(ScenarioRepository::new(pool));
    let service = ExecutionService::new(scenario_repo, runtime_client);

    // Call service (proxies to runtara-environment)
    match service
        .list_all_executions(&tenant_id, query.page, query.size, filters)
        .await
    {
        Ok(page) => {
            let response = ListAllExecutionsResponse {
                success: true,
                data: page,
            };
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => {
            tracing::error!("Failed to list executions: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": format!("Failed to list executions: {:?}", e)
                })),
            )
        }
    }
}

/// Parse and validate query parameters into filters
fn parse_filters(query: &ListAllExecutionsQuery) -> Result<ExecutionFilters, String> {
    // Parse statuses from comma-separated string
    let statuses = query.status.as_ref().map(|s| {
        s.split(',')
            .map(|status| status.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    });

    // Validate statuses (must be lowercase to match DB and API response format)
    if let Some(ref status_list) = statuses {
        let valid_statuses = [
            "queued",
            "compiling",
            "running",
            "completed",
            "failed",
            "timeout",
            "cancelled",
        ];
        for status in status_list {
            if !valid_statuses.contains(&status.as_str()) {
                return Err(format!(
                    "Invalid status '{}'. Valid values: queued, compiling, running, completed, failed, timeout, cancelled",
                    status
                ));
            }
        }
    }

    // Parse and validate sort_by
    let sort_by = query.sort_by.as_deref().unwrap_or("completedAt");
    let sort_column = match sort_by {
        "createdAt" => "created_at",
        "completedAt" => "completed_at",
        "status" => "status",
        "scenarioId" => "scenario_id",
        _ => {
            return Err(format!(
                "Invalid sortBy '{}'. Valid values: createdAt, completedAt, status, scenarioId",
                sort_by
            ));
        }
    };

    // Parse and validate sort_order
    let sort_order = query.sort_order.as_deref().unwrap_or("desc");
    let sort_order_sql = match sort_order.to_lowercase().as_str() {
        "asc" => "ASC",
        "desc" => "DESC",
        _ => {
            return Err(format!(
                "Invalid sortOrder '{}'. Valid values: asc, desc",
                sort_order
            ));
        }
    };

    Ok(ExecutionFilters {
        scenario_id: query.scenario_id.clone(),
        statuses,
        created_from: query.created_from,
        created_to: query.created_to,
        completed_from: query.completed_from,
        completed_to: query.completed_to,
        sort_by: sort_column.to_string(),
        sort_order: sort_order_sql.to_string(),
    })
}
