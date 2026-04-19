use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use utoipa::ToSchema;

/// Hourly metrics for a workflow
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkflowMetricsHourly {
    pub id: i64,
    pub tenant_id: String,
    pub workflow_id: String,
    pub version: i32,
    pub hour_bucket: DateTime<Utc>,
    pub invocation_count: i32,
    pub success_count: i32,
    pub failure_count: i32,
    pub timeout_count: i32,
    pub total_duration_seconds: Option<f64>,
    pub min_duration_seconds: Option<f64>,
    pub max_duration_seconds: Option<f64>,
    pub total_memory_mb: Option<f64>,
    pub min_memory_mb: Option<f64>,
    pub max_memory_mb: Option<f64>,
    pub total_queue_duration_seconds: Option<f64>,
    pub min_queue_duration_seconds: Option<f64>,
    pub max_queue_duration_seconds: Option<f64>,
    pub total_processing_overhead_seconds: Option<f64>,
    pub min_processing_overhead_seconds: Option<f64>,
    pub max_processing_overhead_seconds: Option<f64>,
    pub side_effect_counts: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Daily aggregated metrics
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowMetricsDaily {
    pub tenant_id: String,
    pub workflow_id: String,
    pub version: i32,
    pub day_bucket: Option<DateTime<Utc>>,
    pub invocation_count: Option<i64>,
    pub success_count: Option<i64>,
    pub failure_count: Option<i64>,
    pub timeout_count: Option<i64>,
    pub avg_duration_seconds: Option<f64>,
    pub min_duration_seconds: Option<f64>,
    pub max_duration_seconds: Option<f64>,
    pub avg_memory_mb: Option<f64>,
    pub min_memory_mb: Option<f64>,
    pub max_memory_mb: Option<f64>,
    pub avg_queue_duration_seconds: Option<f64>,
    pub min_queue_duration_seconds: Option<f64>,
    pub max_queue_duration_seconds: Option<f64>,
    pub avg_processing_overhead_seconds: Option<f64>,
    pub min_processing_overhead_seconds: Option<f64>,
    pub max_processing_overhead_seconds: Option<f64>,
    pub success_rate_percent: Option<f64>,
}

/// Metrics service for querying aggregated metrics
pub struct MetricsService {
    pool: PgPool,
}

impl MetricsService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get hourly metrics for a workflow within a time range
    pub async fn get_workflow_metrics_hourly(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: Option<i32>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<Vec<WorkflowMetricsHourly>, sqlx::Error> {
        if let Some(v) = version {
            sqlx::query_as!(
                WorkflowMetricsHourly,
                r#"
                SELECT id, tenant_id, workflow_id, version, hour_bucket,
                       invocation_count, success_count, failure_count, timeout_count,
                       total_duration_seconds, min_duration_seconds, max_duration_seconds,
                       total_memory_mb, min_memory_mb, max_memory_mb,
                       total_queue_duration_seconds, min_queue_duration_seconds, max_queue_duration_seconds,
                       total_processing_overhead_seconds, min_processing_overhead_seconds, max_processing_overhead_seconds,
                       side_effect_counts, created_at, updated_at
                FROM workflow_metrics_hourly
                WHERE tenant_id = $1
                  AND workflow_id = $2
                  AND version = $3
                  AND hour_bucket >= $4
                  AND hour_bucket < $5
                ORDER BY hour_bucket ASC
                "#,
                tenant_id,
                workflow_id,
                v,
                start_time,
                end_time
            )
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as!(
                WorkflowMetricsHourly,
                r#"
                SELECT id, tenant_id, workflow_id, version, hour_bucket,
                       invocation_count, success_count, failure_count, timeout_count,
                       total_duration_seconds, min_duration_seconds, max_duration_seconds,
                       total_memory_mb, min_memory_mb, max_memory_mb,
                       total_queue_duration_seconds, min_queue_duration_seconds, max_queue_duration_seconds,
                       total_processing_overhead_seconds, min_processing_overhead_seconds, max_processing_overhead_seconds,
                       side_effect_counts, created_at, updated_at
                FROM workflow_metrics_hourly
                WHERE tenant_id = $1
                  AND workflow_id = $2
                  AND hour_bucket >= $3
                  AND hour_bucket < $4
                ORDER BY hour_bucket ASC
                "#,
                tenant_id,
                workflow_id,
                start_time,
                end_time
            )
            .fetch_all(&self.pool)
            .await
        }
    }

    /// Get daily metrics for a workflow within a time range
    pub async fn get_workflow_metrics_daily(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: Option<i32>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<Vec<WorkflowMetricsDaily>, sqlx::Error> {
        if let Some(v) = version {
            sqlx::query_as!(
                WorkflowMetricsDaily,
                r#"
                SELECT tenant_id as "tenant_id!", workflow_id as "workflow_id!", version as "version!",
                       day_bucket,
                       invocation_count,
                       success_count,
                       failure_count,
                       timeout_count,
                       avg_duration_seconds,
                       min_duration_seconds,
                       max_duration_seconds,
                       avg_memory_mb,
                       min_memory_mb,
                       max_memory_mb,
                       avg_queue_duration_seconds,
                       min_queue_duration_seconds,
                       max_queue_duration_seconds,
                       avg_processing_overhead_seconds,
                       min_processing_overhead_seconds,
                       max_processing_overhead_seconds,
                       success_rate_percent::double precision as success_rate_percent
                FROM workflow_metrics_daily
                WHERE tenant_id = $1
                  AND workflow_id = $2
                  AND version = $3
                  AND day_bucket >= $4
                  AND day_bucket < $5
                ORDER BY day_bucket ASC
                "#,
                tenant_id,
                workflow_id,
                v,
                start_time,
                end_time
            )
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as!(
                WorkflowMetricsDaily,
                r#"
                SELECT tenant_id as "tenant_id!", workflow_id as "workflow_id!", version as "version!",
                       day_bucket,
                       invocation_count,
                       success_count,
                       failure_count,
                       timeout_count,
                       avg_duration_seconds,
                       min_duration_seconds,
                       max_duration_seconds,
                       avg_memory_mb,
                       min_memory_mb,
                       max_memory_mb,
                       avg_queue_duration_seconds,
                       min_queue_duration_seconds,
                       max_queue_duration_seconds,
                       avg_processing_overhead_seconds,
                       min_processing_overhead_seconds,
                       max_processing_overhead_seconds,
                       success_rate_percent::double precision as success_rate_percent
                FROM workflow_metrics_daily
                WHERE tenant_id = $1
                  AND workflow_id = $2
                  AND day_bucket >= $3
                  AND day_bucket < $4
                ORDER BY day_bucket ASC
                "#,
                tenant_id,
                workflow_id,
                start_time,
                end_time
            )
            .fetch_all(&self.pool)
            .await
        }
    }

    /// Get tenant-level metrics aggregated across all workflows (hourly)
    pub async fn get_tenant_metrics_hourly(
        &self,
        tenant_id: &str,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<Vec<HashMap<String, serde_json::Value>>, sqlx::Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                hour_bucket,
                SUM(invocation_count) as invocation_count,
                SUM(success_count) as success_count,
                SUM(failure_count) as failure_count,
                SUM(timeout_count) as timeout_count,
                CASE
                    WHEN SUM(invocation_count) > 0
                    THEN SUM(total_duration_seconds) / SUM(invocation_count)
                    ELSE 0::double precision
                END as avg_duration_seconds,
                CASE
                    WHEN SUM(invocation_count) > 0
                    THEN SUM(total_memory_mb) / SUM(invocation_count)
                    ELSE 0::double precision
                END as avg_memory_mb,
                CASE
                    WHEN SUM(invocation_count) > 0
                    THEN ROUND((SUM(success_count)::numeric / SUM(invocation_count)::numeric * 100), 2)::double precision
                    ELSE 0::double precision
                END as success_rate_percent
            FROM workflow_metrics_hourly
            WHERE tenant_id = $1
              AND hour_bucket >= $2
              AND hour_bucket < $3
            GROUP BY hour_bucket
            ORDER BY hour_bucket ASC
            "#,
            tenant_id,
            start_time,
            end_time
        )
        .fetch_all(&self.pool)
        .await?;

        let result = rows
            .into_iter()
            .map(|row| {
                let mut map = HashMap::new();
                map.insert("hourBucket".to_string(), serde_json::json!(row.hour_bucket));
                map.insert(
                    "invocationCount".to_string(),
                    serde_json::json!(row.invocation_count),
                );
                map.insert(
                    "successCount".to_string(),
                    serde_json::json!(row.success_count),
                );
                map.insert(
                    "failureCount".to_string(),
                    serde_json::json!(row.failure_count),
                );
                map.insert(
                    "timeoutCount".to_string(),
                    serde_json::json!(row.timeout_count),
                );
                map.insert(
                    "avgDurationSeconds".to_string(),
                    serde_json::json!(row.avg_duration_seconds),
                );
                map.insert(
                    "avgMemoryMb".to_string(),
                    serde_json::json!(row.avg_memory_mb),
                );
                map.insert(
                    "successRatePercent".to_string(),
                    serde_json::json!(row.success_rate_percent),
                );
                map
            })
            .collect();

        Ok(result)
    }

    /// Get overall statistics for a workflow (all time)
    pub async fn get_workflow_overall_stats(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: Option<i32>,
    ) -> Result<Option<HashMap<String, serde_json::Value>>, sqlx::Error> {
        // Get aggregated stats from hourly metrics table
        let row = sqlx::query!(
            r#"
            SELECT
                SUM(invocation_count) as total_invocations,
                SUM(success_count) as total_successes,
                SUM(failure_count) as total_failures,
                SUM(timeout_count) as total_timeouts,
                CASE
                    WHEN SUM(invocation_count) > 0
                    THEN SUM(total_duration_seconds) / SUM(invocation_count)
                    ELSE 0::double precision
                END as avg_duration_seconds,
                MIN(min_duration_seconds) as min_duration_seconds,
                MAX(max_duration_seconds) as max_duration_seconds,
                CASE
                    WHEN SUM(invocation_count) > 0
                    THEN SUM(total_memory_mb) / SUM(invocation_count)
                    ELSE 0::double precision
                END as avg_memory_mb,
                MIN(min_memory_mb) as min_memory_mb,
                MAX(max_memory_mb) as max_memory_mb,
                CASE
                    WHEN SUM(invocation_count) > 0
                    THEN SUM(total_queue_duration_seconds) / SUM(invocation_count)
                    ELSE 0::double precision
                END as avg_queue_duration_seconds,
                MIN(min_queue_duration_seconds) as min_queue_duration_seconds,
                MAX(max_queue_duration_seconds) as max_queue_duration_seconds,
                CASE
                    WHEN SUM(invocation_count) > 0
                    THEN SUM(total_processing_overhead_seconds) / SUM(invocation_count)
                    ELSE 0::double precision
                END as avg_processing_overhead_seconds,
                MIN(min_processing_overhead_seconds) as min_processing_overhead_seconds,
                MAX(max_processing_overhead_seconds) as max_processing_overhead_seconds,
                CASE
                    WHEN SUM(invocation_count) > 0
                    THEN ROUND((SUM(success_count)::numeric / SUM(invocation_count)::numeric * 100), 2)::double precision
                    ELSE 0::double precision
                END as success_rate_percent
            FROM workflow_metrics_hourly
            WHERE tenant_id = $1
              AND workflow_id = $2
              AND ($3::integer IS NULL OR version = $3)
            "#,
            tenant_id,
            workflow_id,
            version
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(r) = row {
            let mut map = HashMap::new();
            map.insert(
                "totalInvocations".to_string(),
                serde_json::json!(r.total_invocations),
            );
            map.insert(
                "totalSuccesses".to_string(),
                serde_json::json!(r.total_successes),
            );
            map.insert(
                "totalFailures".to_string(),
                serde_json::json!(r.total_failures),
            );
            map.insert(
                "totalTimeouts".to_string(),
                serde_json::json!(r.total_timeouts),
            );
            map.insert(
                "avgDurationSeconds".to_string(),
                serde_json::json!(r.avg_duration_seconds),
            );
            map.insert(
                "minDurationSeconds".to_string(),
                serde_json::json!(r.min_duration_seconds),
            );
            map.insert(
                "maxDurationSeconds".to_string(),
                serde_json::json!(r.max_duration_seconds),
            );
            map.insert(
                "avgMemoryMb".to_string(),
                serde_json::json!(r.avg_memory_mb),
            );
            map.insert(
                "minMemoryMb".to_string(),
                serde_json::json!(r.min_memory_mb),
            );
            map.insert(
                "maxMemoryMb".to_string(),
                serde_json::json!(r.max_memory_mb),
            );
            map.insert(
                "avgQueueDurationSeconds".to_string(),
                serde_json::json!(r.avg_queue_duration_seconds),
            );
            map.insert(
                "minQueueDurationSeconds".to_string(),
                serde_json::json!(r.min_queue_duration_seconds),
            );
            map.insert(
                "maxQueueDurationSeconds".to_string(),
                serde_json::json!(r.max_queue_duration_seconds),
            );
            map.insert(
                "avgProcessingOverheadSeconds".to_string(),
                serde_json::json!(r.avg_processing_overhead_seconds),
            );
            map.insert(
                "minProcessingOverheadSeconds".to_string(),
                serde_json::json!(r.min_processing_overhead_seconds),
            );
            map.insert(
                "maxProcessingOverheadSeconds".to_string(),
                serde_json::json!(r.max_processing_overhead_seconds),
            );
            map.insert(
                "successRatePercent".to_string(),
                serde_json::json!(r.success_rate_percent),
            );

            Ok(Some(map))
        } else {
            Ok(None)
        }
    }

    /// Record an execution completion into workflow_metrics_hourly.
    ///
    /// This replaces the old PostgreSQL trigger that fired on workflow_executions
    /// updates. Now that executions go through runtara-environment, we insert
    /// metrics directly after getting the result back.
    pub async fn record_execution_completion(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
        success: bool,
        duration_seconds: f64,
        max_memory_mb: Option<f64>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        let hour_bucket = now
            .date_naive()
            .and_hms_opt(now.hour(), 0, 0)
            .map(|dt| dt.and_utc())
            .unwrap_or(now);

        let success_count: i32 = if success { 1 } else { 0 };
        let failure_count: i32 = if success { 0 } else { 1 };
        let memory = max_memory_mb.unwrap_or(0.0);

        sqlx::query!(
            r#"
            INSERT INTO workflow_metrics_hourly (
                tenant_id, workflow_id, version, hour_bucket,
                invocation_count, success_count, failure_count, timeout_count,
                total_duration_seconds, min_duration_seconds, max_duration_seconds,
                total_memory_mb, min_memory_mb, max_memory_mb,
                updated_at
            )
            VALUES ($1, $2, $3, $4, 1, $5, $6, 0, $7, $7, $7, $8, $9, $9, NOW())
            ON CONFLICT (tenant_id, workflow_id, version, hour_bucket)
            DO UPDATE SET
                invocation_count = workflow_metrics_hourly.invocation_count + 1,
                success_count = workflow_metrics_hourly.success_count + $5,
                failure_count = workflow_metrics_hourly.failure_count + $6,
                total_duration_seconds = workflow_metrics_hourly.total_duration_seconds + $7,
                min_duration_seconds = LEAST(workflow_metrics_hourly.min_duration_seconds, $7),
                max_duration_seconds = GREATEST(workflow_metrics_hourly.max_duration_seconds, $7),
                total_memory_mb = workflow_metrics_hourly.total_memory_mb + $8,
                min_memory_mb = LEAST(workflow_metrics_hourly.min_memory_mb, $9),
                max_memory_mb = GREATEST(workflow_metrics_hourly.max_memory_mb, $9),
                updated_at = NOW()
            "#,
            tenant_id,
            workflow_id,
            version,
            hour_bucket,
            success_count,
            failure_count,
            duration_seconds,
            memory,
            max_memory_mb,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
