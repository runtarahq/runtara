// Metrics HTTP handlers
// This module demonstrates the correct pattern: thin handlers that delegate to services
// ✓ No DTOs defined here (they're in dto/metrics.rs)
// ✓ No database queries (delegated to MetricsService)
// ✓ Handlers only handle HTTP concerns

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;

use crate::api::dto::metrics::*;
use crate::metrics::MetricsService;
use crate::runtime_client::{GetTenantMetricsOptions, MetricsGranularity, RuntimeClient};

/// Get metrics for a specific workflow
#[utoipa::path(
    get,
    path = "/api/runtime/metrics/workflows/{workflow_id}",
    params(
        ("workflow_id" = String, Path, description = "Workflow identifier"),
        ("startTime" = Option<String>, Query, description = "Start time (ISO 8601), defaults to 24h ago"),
        ("endTime" = Option<String>, Query, description = "End time (ISO 8601), defaults to now"),
        ("version" = Option<i32>, Query, description = "Specific version, or all versions if not specified"),
        ("granularity" = Option<String>, Query, description = "Time granularity: 'hourly' or 'daily' (default: daily)")
    ),
    responses(
        (status = 200, description = "Daily metrics retrieved successfully", body = WorkflowMetricsDailyResponse),
        (status = 200, description = "Hourly metrics retrieved successfully", body = WorkflowMetricsHourlyResponse),
        (status = 500, description = "Internal server error", body = MetricsResponse)
    ),
    tag = "metrics-controller"
)]
pub async fn get_workflow_metrics(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(workflow_id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> (StatusCode, Json<Value>) {
    let metrics_service = MetricsService::new(pool);

    // Default time range: last 24 hours
    let end_time = query.end_time.unwrap_or_else(Utc::now);
    let start_time = query
        .start_time
        .unwrap_or_else(|| end_time - Duration::hours(24));

    let granularity = query.granularity.as_deref().unwrap_or("daily");

    let result = match granularity {
        "hourly" => {
            let metrics = metrics_service
                .get_workflow_metrics_hourly(
                    &tenant_id,
                    &workflow_id,
                    query.version,
                    start_time,
                    end_time,
                )
                .await;

            match metrics {
                Ok(data) => json!({
                    "success": true,
                    "message": "Hourly metrics retrieved successfully",
                    "data": {
                        "workflowId": workflow_id,
                        "version": query.version,
                        "startTime": start_time,
                        "endTime": end_time,
                        "granularity": "hourly",
                        "metrics": data
                    }
                }),
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "success": false,
                            "message": format!("Failed to retrieve metrics: {}", e),
                            "data": null
                        })),
                    );
                }
            }
        }
        _ => {
            // Default to daily
            let metrics = metrics_service
                .get_workflow_metrics_daily(
                    &tenant_id,
                    &workflow_id,
                    query.version,
                    start_time,
                    end_time,
                )
                .await;

            match metrics {
                Ok(data) => json!({
                    "success": true,
                    "message": "Daily metrics retrieved successfully",
                    "data": {
                        "workflowId": workflow_id,
                        "version": query.version,
                        "startTime": start_time,
                        "endTime": end_time,
                        "granularity": "daily",
                        "metrics": data
                    }
                }),
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "success": false,
                            "message": format!("Failed to retrieve metrics: {}", e),
                            "data": null
                        })),
                    );
                }
            }
        }
    };

    (StatusCode::OK, Json(result))
}

/// Get overall statistics for a workflow (all time)
#[utoipa::path(
    get,
    path = "/api/runtime/metrics/workflows/{workflow_id}/stats",
    params(
        ("workflow_id" = String, Path, description = "Workflow identifier"),
        ("version" = Option<i32>, Query, description = "Specific version, or all versions if not specified")
    ),
    responses(
        (status = 200, description = "Statistics retrieved successfully", body = WorkflowStatsResponse),
        (status = 404, description = "No statistics found", body = MetricsResponse),
        (status = 500, description = "Internal server error", body = MetricsResponse)
    ),
    tag = "metrics-controller"
)]
pub async fn get_workflow_stats(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(pool): State<PgPool>,
    Path(workflow_id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> (StatusCode, Json<Value>) {
    let metrics_service = MetricsService::new(pool);

    match metrics_service
        .get_workflow_overall_stats(&tenant_id, &workflow_id, query.version)
        .await
    {
        Ok(Some(stats)) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Overall statistics retrieved successfully",
                "data": {
                    "workflowId": workflow_id,
                    "version": query.version,
                    "stats": stats
                }
            })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "message": "No statistics found for this workflow",
                "data": null
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "message": format!("Failed to retrieve statistics: {}", e),
                "data": null
            })),
        ),
    }
}

/// Get tenant-level metrics aggregated across all workflows (hourly)
#[utoipa::path(
    get,
    path = "/api/runtime/metrics/tenant",
    params(
        ("startTime" = Option<String>, Query, description = "Start time (ISO 8601), defaults to 24 hours ago"),
        ("endTime" = Option<String>, Query, description = "End time (ISO 8601), defaults to now"),
        ("granularity" = Option<String>, Query, description = "Time granularity: 'hourly' or 'daily' (default: hourly)")
    ),
    responses(
        (status = 200, description = "Tenant metrics retrieved successfully", body = TenantMetricsResponse),
        (status = 503, description = "Runtara environment not configured", body = MetricsResponse),
        (status = 500, description = "Internal server error", body = MetricsResponse)
    ),
    tag = "metrics-controller"
)]
pub async fn get_tenant_metrics(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(_pool): State<PgPool>,
    State(runtime_client): State<Option<Arc<RuntimeClient>>>,
    Query(query): Query<MetricsQuery>,
) -> (StatusCode, Json<Value>) {
    // RuntimeClient is required for tenant metrics (data is in runtara-environment)
    let runtime_client = match runtime_client {
        Some(client) => client,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "success": false,
                    "message": "Runtara environment not configured. Tenant metrics require runtara-environment connection.",
                    "data": null
                })),
            );
        }
    };

    // Default time range: last 24 hours
    let end_time = query.end_time.unwrap_or_else(Utc::now);
    let start_time = query
        .start_time
        .unwrap_or_else(|| end_time - Duration::hours(24));

    // Parse granularity from query parameter
    let granularity = match query.granularity.as_deref() {
        Some("daily") => MetricsGranularity::Daily,
        _ => MetricsGranularity::Hourly, // Default to hourly
    };

    let options = GetTenantMetricsOptions::new(&tenant_id)
        .with_start_time(start_time)
        .with_end_time(end_time)
        .with_granularity(granularity);

    match runtime_client.get_tenant_metrics(options).await {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({
                "success": true,
                "message": "Tenant metrics retrieved successfully",
                "data": {
                    "tenantId": result.tenant_id,
                    "startTime": result.start_time,
                    "endTime": result.end_time,
                    "granularity": format!("{:?}", result.granularity).to_lowercase(),
                    "metrics": result.buckets
                }
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "success": false,
                "message": format!("Failed to retrieve tenant metrics: {}", e),
                "data": null
            })),
        ),
    }
}
