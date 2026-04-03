/// Metrics-related DTOs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::metrics::{ScenarioMetricsDaily, ScenarioMetricsHourly};

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

/// Response data for scenario metrics endpoint
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioMetricsData {
    pub scenario_id: String,
    pub version: Option<i32>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub granularity: String,
    pub metrics: Vec<ScenarioMetricsDaily>,
}

/// Response for scenario metrics (daily)
#[derive(Serialize, ToSchema)]
pub struct ScenarioMetricsDailyResponse {
    pub success: bool,
    pub message: String,
    pub data: ScenarioMetricsData,
}

/// Response data for scenario metrics hourly endpoint
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioMetricsHourlyData {
    pub scenario_id: String,
    pub version: Option<i32>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub granularity: String,
    pub metrics: Vec<ScenarioMetricsHourly>,
}

/// Response for scenario metrics (hourly)
#[derive(Serialize, ToSchema)]
pub struct ScenarioMetricsHourlyResponse {
    pub success: bool,
    pub message: String,
    pub data: ScenarioMetricsHourlyData,
}

/// Statistics data
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioStatsData {
    pub scenario_id: String,
    pub version: Option<i32>,
    pub stats: ScenarioStats,
}

/// Overall scenario statistics
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioStats {
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

/// Response for scenario statistics
#[derive(Serialize, ToSchema)]
pub struct ScenarioStatsResponse {
    pub success: bool,
    pub message: String,
    pub data: ScenarioStatsData,
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
