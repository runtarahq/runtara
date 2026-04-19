/// Metrics-related DTOs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::metrics::{WorkflowMetricsDaily, WorkflowMetricsHourly};

// ============================================================================
// Query Parameters
// ============================================================================

#[derive(Deserialize, ToSchema)]
pub struct MetricsQuery {
    #[serde(rename = "startTime")]
    pub start_time: Option<DateTime<Utc>>,
    #[serde(rename = "endTime")]
    pub end_time: Option<DateTime<Utc>>,
    pub version: Option<i32>,
    pub granularity: Option<String>, // "hourly" or "daily"
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Serialize, ToSchema)]
pub struct MetricsResponse {
    pub success: bool,
    pub message: String,
    pub data: Value,
}

/// Response data for workflow metrics endpoint
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowMetricsData {
    pub workflow_id: String,
    pub version: Option<i32>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub granularity: String,
    pub metrics: Vec<WorkflowMetricsDaily>,
}

/// Response for workflow metrics (daily)
#[derive(Serialize, ToSchema)]
pub struct WorkflowMetricsDailyResponse {
    pub success: bool,
    pub message: String,
    pub data: WorkflowMetricsData,
}

/// Response data for workflow metrics hourly endpoint
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowMetricsHourlyData {
    pub workflow_id: String,
    pub version: Option<i32>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub granularity: String,
    pub metrics: Vec<WorkflowMetricsHourly>,
}

/// Response for workflow metrics (hourly)
#[derive(Serialize, ToSchema)]
pub struct WorkflowMetricsHourlyResponse {
    pub success: bool,
    pub message: String,
    pub data: WorkflowMetricsHourlyData,
}

/// Statistics data
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStatsData {
    pub workflow_id: String,
    pub version: Option<i32>,
    pub stats: WorkflowStats,
}

/// Overall workflow statistics
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStats {
    pub total_invocations: Option<i64>,
    pub total_successes: Option<i64>,
    pub total_failures: Option<i64>,
    pub total_timeouts: Option<i64>,
    pub avg_duration_seconds: Option<f64>,
    pub min_duration_seconds: Option<f64>,
    pub max_duration_seconds: Option<f64>,
    pub p95_duration_seconds: Option<f64>,
    pub p99_duration_seconds: Option<f64>,
    pub avg_memory_mb: Option<f64>,
    pub min_memory_mb: Option<f64>,
    pub max_memory_mb: Option<f64>,
    pub avg_queue_duration_seconds: Option<f64>,
    pub min_queue_duration_seconds: Option<f64>,
    pub max_queue_duration_seconds: Option<f64>,
    pub p95_queue_duration_seconds: Option<f64>,
    pub p99_queue_duration_seconds: Option<f64>,
    pub avg_processing_overhead_seconds: Option<f64>,
    pub min_processing_overhead_seconds: Option<f64>,
    pub max_processing_overhead_seconds: Option<f64>,
    pub success_rate_percent: Option<f64>,
}

/// Response for workflow statistics
#[derive(Serialize, ToSchema)]
pub struct WorkflowStatsResponse {
    pub success: bool,
    pub message: String,
    pub data: WorkflowStatsData,
}

/// Tenant metrics data point
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TenantMetricsDataPoint {
    pub day_bucket: Option<DateTime<Utc>>,
    pub invocation_count: Option<i64>,
    pub success_count: Option<i64>,
    pub failure_count: Option<i64>,
    pub timeout_count: Option<i64>,
    pub avg_duration_seconds: Option<f64>,
    pub avg_memory_mb: Option<f64>,
    pub success_rate_percent: Option<f64>,
}

/// Tenant metrics response data
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TenantMetricsData {
    pub tenant_id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub metrics: Vec<TenantMetricsDataPoint>,
}

/// Response for tenant metrics
#[derive(Serialize, ToSchema)]
pub struct TenantMetricsResponse {
    pub success: bool,
    pub message: String,
    pub data: TenantMetricsData,
}
