// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database operations for runtara-environment.
//!
//! Environment shares the `instances` table with Core but maintains its own
//! `instance_images` table to track which image launched each instance.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Instance record from the database (matches Core's schema).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Instance {
    /// Unique identifier for the instance.
    pub instance_id: String,
    /// Tenant identifier for multi-tenancy isolation.
    pub tenant_id: String,
    /// Current status (pending, running, suspended, completed, failed, cancelled).
    pub status: String,
    /// Last checkpoint ID if instance was checkpointed.
    pub checkpoint_id: Option<String>,
    /// Current attempt number.
    pub attempt: i32,
    /// Maximum allowed attempts.
    pub max_attempts: i32,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance started running.
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished (completed, failed, or cancelled).
    pub finished_at: Option<DateTime<Utc>>,
    /// Output data from successful completion.
    pub output: Option<Vec<u8>>,
    /// Error message from failure.
    pub error: Option<String>,
}

/// Instance with image info (joined from instance_images).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstanceWithImage {
    /// Unique identifier for the instance.
    pub instance_id: String,
    /// Tenant identifier for multi-tenancy isolation.
    pub tenant_id: String,
    /// Current status.
    pub status: String,
    /// Last checkpoint ID.
    pub checkpoint_id: Option<String>,
    /// Current attempt number.
    pub attempt: i32,
    /// Maximum allowed attempts.
    pub max_attempts: i32,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance started running.
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished.
    pub finished_at: Option<DateTime<Utc>>,
    /// Output data.
    pub output: Option<Vec<u8>>,
    /// Error message.
    pub error: Option<String>,
    /// Image ID (from instance_images table).
    pub image_id: Option<String>,
    /// Image name (from images table).
    pub image_name: Option<String>,
}

/// Full instance record with image info and heartbeat.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstanceFull {
    /// Unique identifier for the instance.
    pub instance_id: String,
    /// Tenant identifier for multi-tenancy isolation.
    pub tenant_id: String,
    /// Image ID that this instance was created from.
    pub image_id: Option<String>,
    /// Human-readable image name (from images table).
    pub image_name: Option<String>,
    /// Current status.
    pub status: String,
    /// Output data.
    pub output: Option<Vec<u8>>,
    /// Error message.
    pub error: Option<String>,
    /// Last checkpoint ID.
    pub checkpoint_id: Option<String>,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance started running.
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished.
    pub finished_at: Option<DateTime<Utc>>,
    /// Last heartbeat timestamp (from container_heartbeats table).
    pub heartbeat_at: Option<DateTime<Utc>>,
    /// Current attempt number.
    pub attempt: i32,
    /// Maximum allowed attempts.
    pub max_attempts: i32,
    /// Peak memory usage during execution (in bytes).
    pub memory_peak_bytes: Option<i64>,
    /// Total CPU time consumed during execution (in microseconds).
    pub cpu_usage_usec: Option<i64>,
}

/// Create a new instance record and associate it with an image.
///
/// This creates the instance in Core's table and adds the image mapping
/// in the instance_images table.
pub async fn create_instance(
    pool: &PgPool,
    instance_id: &str,
    tenant_id: &str,
    image_id: &str,
) -> Result<(), sqlx::Error> {
    // Create instance in Core's table
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, status, created_at)
        VALUES ($1, $2, 'pending', NOW())
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .execute(pool)
    .await?;

    // Associate with image
    associate_instance_image(pool, instance_id, image_id, tenant_id).await?;

    Ok(())
}

/// Get an instance by ID.
pub async fn get_instance(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<Instance>, sqlx::Error> {
    sqlx::query_as::<_, Instance>(
        r#"
        SELECT instance_id, tenant_id, status::TEXT as status, checkpoint_id,
               attempt, max_attempts, created_at, started_at, finished_at,
               output, error
        FROM instances
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await
}

/// Get full instance details including image name and heartbeat.
pub async fn get_instance_full(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<InstanceFull>, sqlx::Error> {
    sqlx::query_as::<_, InstanceFull>(
        r#"
        SELECT i.instance_id, i.tenant_id, ii.image_id, img.name as image_name,
               i.status::TEXT as status, i.output, i.error, i.checkpoint_id,
               i.created_at, i.started_at, i.finished_at,
               ch.last_heartbeat as heartbeat_at, i.attempt, i.max_attempts,
               i.memory_peak_bytes, i.cpu_usage_usec
        FROM instances i
        LEFT JOIN instance_images ii ON i.instance_id = ii.instance_id
        LEFT JOIN images img ON ii.image_id = img.image_id
        LEFT JOIN container_heartbeats ch ON i.instance_id = ch.instance_id
        WHERE i.instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await
}

/// Update instance status.
pub async fn update_instance_status(
    pool: &PgPool,
    instance_id: &str,
    status: &str,
    checkpoint_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE instances
        SET status = $2::instance_status,
            checkpoint_id = COALESCE($3, checkpoint_id),
            started_at = CASE WHEN $2 = 'running' AND started_at IS NULL THEN NOW() ELSE started_at END,
            finished_at = CASE WHEN $2 IN ('completed', 'failed', 'cancelled') THEN NOW() ELSE finished_at END
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .bind(status)
    .bind(checkpoint_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Update instance with output/error on completion.
pub async fn update_instance_result(
    pool: &PgPool,
    instance_id: &str,
    status: &str,
    output: Option<&[u8]>,
    error: Option<&str>,
    checkpoint_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE instances
        SET status = $2::instance_status,
            output = $3,
            error = $4,
            checkpoint_id = COALESCE($5, checkpoint_id),
            finished_at = CASE WHEN $2 IN ('completed', 'failed', 'cancelled', 'suspended') THEN NOW() ELSE finished_at END
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .bind(status)
    .bind(output)
    .bind(error)
    .bind(checkpoint_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Update instance result only if current status is 'running'.
///
/// This prevents the container monitor from overwriting status updates
/// that were already set by runtara-core via SDK events. The SDK event
/// path is authoritative for successful completions; the monitor only
/// handles cases where the container crashed without reporting.
///
/// Returns true if the update was applied, false if skipped (status was not 'running').
pub async fn update_instance_result_if_running(
    pool: &PgPool,
    instance_id: &str,
    status: &str,
    output: Option<&[u8]>,
    error: Option<&str>,
    checkpoint_id: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE instances
        SET status = $2::instance_status,
            output = $3,
            error = $4,
            checkpoint_id = COALESCE($5, checkpoint_id),
            finished_at = CASE WHEN $2 IN ('completed', 'failed', 'cancelled', 'suspended') THEN NOW() ELSE finished_at END
        WHERE instance_id = $1 AND status = 'running'
        "#,
    )
    .bind(instance_id)
    .bind(status)
    .bind(output)
    .bind(error)
    .bind(checkpoint_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Update execution metrics for an instance.
///
/// Stores cgroup-collected resource usage metrics (memory, CPU) after container execution.
/// Only updates if metrics are not already set (first writer wins).
pub async fn update_instance_metrics(
    pool: &PgPool,
    instance_id: &str,
    memory_peak_bytes: Option<u64>,
    cpu_usage_usec: Option<u64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE instances
        SET memory_peak_bytes = COALESCE(memory_peak_bytes, $2),
            cpu_usage_usec = COALESCE(cpu_usage_usec, $3)
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .bind(memory_peak_bytes.map(|v| v as i64))
    .bind(cpu_usage_usec.map(|v| v as i64))
    .execute(pool)
    .await?;

    Ok(())
}

/// Options for listing instances.
#[derive(Debug, Clone, Default)]
pub struct ListInstancesOptions {
    /// Filter by tenant ID.
    pub tenant_id: Option<String>,
    /// Filter by status.
    pub status: Option<String>,
    /// Filter by image ID (exact match).
    pub image_id: Option<String>,
    /// Filter by image name prefix (e.g., "scenario_id:" matches "scenario_id:1", "scenario_id:2").
    pub image_name_prefix: Option<String>,
    /// Filter by created_at >= value.
    pub created_after: Option<DateTime<Utc>>,
    /// Filter by created_at < value.
    pub created_before: Option<DateTime<Utc>>,
    /// Filter by finished_at >= value.
    pub finished_after: Option<DateTime<Utc>>,
    /// Filter by finished_at < value.
    pub finished_before: Option<DateTime<Utc>>,
    /// Order by field and direction.
    pub order_by: Option<String>,
    /// Maximum results to return.
    pub limit: i64,
    /// Pagination offset.
    pub offset: i64,
}

/// List instances with optional filters.
pub async fn list_instances(
    pool: &PgPool,
    options: &ListInstancesOptions,
) -> Result<Vec<InstanceWithImage>, sqlx::Error> {
    // Build ORDER BY clause based on order_by option
    let order_clause = match options.order_by.as_deref() {
        Some("created_at_asc") => "ORDER BY i.created_at ASC",
        Some("finished_at_desc") => "ORDER BY i.finished_at DESC NULLS LAST",
        Some("finished_at_asc") => "ORDER BY i.finished_at ASC NULLS LAST",
        _ => "ORDER BY i.created_at DESC", // default: created_at_desc
    };

    // Escape the image name prefix for LIKE pattern (escape % and _)
    let image_name_pattern = options.image_name_prefix.as_ref().map(|prefix| {
        let escaped = prefix.replace('%', "\\%").replace('_', "\\_");
        format!("{}%", escaped)
    });

    let query = format!(
        r#"
        SELECT i.instance_id, i.tenant_id, i.status::TEXT as status, i.checkpoint_id,
               i.attempt, i.max_attempts, i.created_at, i.started_at, i.finished_at,
               i.output, i.error, ii.image_id, img.name as image_name
        FROM instances i
        LEFT JOIN instance_images ii ON i.instance_id = ii.instance_id
        LEFT JOIN images img ON ii.image_id = img.image_id
        WHERE ($1::TEXT IS NULL OR i.tenant_id = $1)
          AND ($2::TEXT IS NULL OR i.status::TEXT = $2)
          AND ($3::TEXT IS NULL OR ii.image_id = $3)
          AND ($4::TEXT IS NULL OR img.name LIKE $4)
          AND ($5::TIMESTAMPTZ IS NULL OR i.created_at >= $5)
          AND ($6::TIMESTAMPTZ IS NULL OR i.created_at < $6)
          AND ($7::TIMESTAMPTZ IS NULL OR i.finished_at >= $7)
          AND ($8::TIMESTAMPTZ IS NULL OR i.finished_at < $8)
        {}
        LIMIT $9 OFFSET $10
        "#,
        order_clause
    );

    sqlx::query_as::<_, InstanceWithImage>(&query)
        .bind(options.tenant_id.as_deref())
        .bind(options.status.as_deref())
        .bind(options.image_id.as_deref())
        .bind(image_name_pattern.as_deref())
        .bind(options.created_after)
        .bind(options.created_before)
        .bind(options.finished_after)
        .bind(options.finished_before)
        .bind(options.limit)
        .bind(options.offset)
        .fetch_all(pool)
        .await
}

/// Count instances matching filters (for pagination total_count).
pub async fn count_instances(
    pool: &PgPool,
    options: &ListInstancesOptions,
) -> Result<i64, sqlx::Error> {
    // Escape the image name prefix for LIKE pattern (escape % and _)
    let image_name_pattern = options.image_name_prefix.as_ref().map(|prefix| {
        let escaped = prefix.replace('%', "\\%").replace('_', "\\_");
        format!("{}%", escaped)
    });

    let count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM instances i
        LEFT JOIN instance_images ii ON i.instance_id = ii.instance_id
        LEFT JOIN images img ON ii.image_id = img.image_id
        WHERE ($1::TEXT IS NULL OR i.tenant_id = $1)
          AND ($2::TEXT IS NULL OR i.status::TEXT = $2)
          AND ($3::TEXT IS NULL OR ii.image_id = $3)
          AND ($4::TEXT IS NULL OR img.name LIKE $4)
          AND ($5::TIMESTAMPTZ IS NULL OR i.created_at >= $5)
          AND ($6::TIMESTAMPTZ IS NULL OR i.created_at < $6)
          AND ($7::TIMESTAMPTZ IS NULL OR i.finished_at >= $7)
          AND ($8::TIMESTAMPTZ IS NULL OR i.finished_at < $8)
        "#,
    )
    .bind(options.tenant_id.as_deref())
    .bind(options.status.as_deref())
    .bind(options.image_id.as_deref())
    .bind(image_name_pattern.as_deref())
    .bind(options.created_after)
    .bind(options.created_before)
    .bind(options.finished_after)
    .bind(options.finished_before)
    .fetch_one(pool)
    .await?;

    Ok(count.0)
}

/// Wake queue entry.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WakeEntry {
    /// Instance to wake.
    pub instance_id: String,
    /// Tenant ID for the instance.
    pub tenant_id: String,
    /// Image ID for re-launching the instance.
    pub image_id: String,
    /// Checkpoint to resume from.
    pub checkpoint_id: String,
    /// When to wake the instance.
    pub wake_at: DateTime<Utc>,
}

/// Schedule a wake for an instance.
pub async fn schedule_wake(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
    wake_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO wake_queue (instance_id, checkpoint_id, wake_at, created_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (instance_id) DO UPDATE SET
            checkpoint_id = $2,
            wake_at = $3
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(wake_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get pending wakes that are ready to execute.
pub async fn get_pending_wakes(pool: &PgPool, limit: i64) -> Result<Vec<WakeEntry>, sqlx::Error> {
    sqlx::query_as::<_, WakeEntry>(
        r#"
        SELECT w.instance_id, i.tenant_id, ii.image_id, w.checkpoint_id, w.wake_at
        FROM wake_queue w
        JOIN instances i ON w.instance_id = i.instance_id
        JOIN instance_images ii ON w.instance_id = ii.instance_id
        WHERE w.wake_at <= NOW()
        ORDER BY w.wake_at ASC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Remove a wake entry.
pub async fn remove_wake(pool: &PgPool, instance_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM wake_queue WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Health check for database connectivity.
pub async fn health_check(pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await
        .map(|_| true)
}

// ============================================================================
// Instance Images
// ============================================================================

/// Associate an instance with an image.
pub async fn associate_instance_image(
    pool: &PgPool,
    instance_id: &str,
    image_id: &str,
    tenant_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO instance_images (instance_id, image_id, tenant_id, created_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (instance_id) DO UPDATE SET
            image_id = $2,
            tenant_id = $3
        "#,
    )
    .bind(instance_id)
    .bind(image_id)
    .bind(tenant_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get the image ID for an instance.
pub async fn get_instance_image_id(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let result: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT image_id FROM instance_images WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await?;

    Ok(result.map(|(id,)| id))
}

/// Remove instance-image association.
pub async fn remove_instance_image(pool: &PgPool, instance_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM instance_images WHERE instance_id = $1")
        .bind(instance_id)
        .execute(pool)
        .await?;

    Ok(())
}

// ============================================================================
// Tenant Metrics
// ============================================================================

/// Granularity for metrics aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsGranularity {
    /// Hourly buckets.
    Hourly,
    /// Daily buckets.
    Daily,
}

/// Options for tenant metrics aggregation.
#[derive(Debug, Clone)]
pub struct TenantMetricsOptions {
    /// Tenant ID.
    pub tenant_id: String,
    /// Start of time range.
    pub start_time: DateTime<Utc>,
    /// End of time range.
    pub end_time: DateTime<Utc>,
    /// Bucket granularity.
    pub granularity: MetricsGranularity,
}

/// Aggregated metrics bucket from database.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MetricsBucketRow {
    /// Start of bucket (UTC).
    pub bucket_time: DateTime<Utc>,
    /// Total invocations in bucket.
    pub invocation_count: i64,
    /// Successful completions.
    pub success_count: i64,
    /// Failed completions.
    pub failure_count: i64,
    /// Cancelled executions.
    pub cancelled_count: i64,
    /// Average duration in milliseconds.
    pub avg_duration_ms: Option<f64>,
    /// Minimum duration in milliseconds.
    pub min_duration_ms: Option<f64>,
    /// Maximum duration in milliseconds.
    pub max_duration_ms: Option<f64>,
    /// Average peak memory in bytes.
    pub avg_memory_bytes: Option<f64>,
    /// Maximum peak memory in bytes.
    pub max_memory_bytes: Option<i64>,
}

/// Get aggregated tenant metrics.
///
/// Aggregates instance execution metrics into time buckets using PostgreSQL
/// date_trunc for bucket alignment and aggregate functions for statistics.
///
/// Returns all buckets in the time range, including empty ones (with zero counts).
pub async fn get_tenant_metrics(
    pool: &PgPool,
    options: &TenantMetricsOptions,
) -> Result<Vec<MetricsBucketRow>, sqlx::Error> {
    let trunc_unit = match options.granularity {
        MetricsGranularity::Hourly => "hour",
        MetricsGranularity::Daily => "day",
    };

    let interval = match options.granularity {
        MetricsGranularity::Hourly => "1 hour",
        MetricsGranularity::Daily => "1 day",
    };

    // Build query with date_trunc for bucket alignment
    // Duration = finished_at - started_at (only for terminal states with both timestamps)
    // Uses generate_series + LEFT JOIN to include empty buckets for charting
    let query = format!(
        r#"
        WITH time_series AS (
            SELECT generate_series(
                date_trunc('{trunc_unit}', $2::timestamptz),
                date_trunc('{trunc_unit}', $3::timestamptz),
                interval '{interval}'
            ) AS bucket_time
        ),
        metrics AS (
            SELECT
                date_trunc('{trunc_unit}', i.finished_at) AS bucket_time,
                COUNT(*) AS invocation_count,
                COUNT(*) FILTER (WHERE i.status = 'completed') AS success_count,
                COUNT(*) FILTER (WHERE i.status = 'failed') AS failure_count,
                COUNT(*) FILTER (WHERE i.status = 'cancelled') AS cancelled_count,
                AVG(EXTRACT(EPOCH FROM (i.finished_at - i.started_at)) * 1000)
                    FILTER (WHERE i.started_at IS NOT NULL AND i.finished_at IS NOT NULL) AS avg_duration_ms,
                MIN(EXTRACT(EPOCH FROM (i.finished_at - i.started_at)) * 1000)
                    FILTER (WHERE i.started_at IS NOT NULL AND i.finished_at IS NOT NULL) AS min_duration_ms,
                MAX(EXTRACT(EPOCH FROM (i.finished_at - i.started_at)) * 1000)
                    FILTER (WHERE i.started_at IS NOT NULL AND i.finished_at IS NOT NULL) AS max_duration_ms,
                AVG(i.memory_peak_bytes)::FLOAT8
                    FILTER (WHERE i.memory_peak_bytes IS NOT NULL) AS avg_memory_bytes,
                MAX(i.memory_peak_bytes)
                    FILTER (WHERE i.memory_peak_bytes IS NOT NULL) AS max_memory_bytes
            FROM instances i
            WHERE i.tenant_id = $1
              AND i.finished_at >= $2
              AND i.finished_at < $3
              AND i.status IN ('completed', 'failed', 'cancelled')
            GROUP BY date_trunc('{trunc_unit}', i.finished_at)
        )
        SELECT
            ts.bucket_time,
            COALESCE(m.invocation_count, 0) AS invocation_count,
            COALESCE(m.success_count, 0) AS success_count,
            COALESCE(m.failure_count, 0) AS failure_count,
            COALESCE(m.cancelled_count, 0) AS cancelled_count,
            m.avg_duration_ms,
            m.min_duration_ms,
            m.max_duration_ms,
            m.avg_memory_bytes,
            m.max_memory_bytes
        FROM time_series ts
        LEFT JOIN metrics m ON ts.bucket_time = m.bucket_time
        ORDER BY ts.bucket_time ASC
        "#
    );

    sqlx::query_as::<_, MetricsBucketRow>(&query)
        .bind(&options.tenant_id)
        .bind(options.start_time)
        .bind(options.end_time)
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // ListInstancesOptions tests
    // ==========================================================================

    #[test]
    fn test_list_instances_options_default() {
        let options = ListInstancesOptions::default();

        assert!(options.tenant_id.is_none());
        assert!(options.status.is_none());
        assert!(options.image_id.is_none());
        assert!(options.image_name_prefix.is_none());
        assert!(options.created_after.is_none());
        assert!(options.created_before.is_none());
        assert!(options.finished_after.is_none());
        assert!(options.finished_before.is_none());
        assert!(options.order_by.is_none());
        assert_eq!(options.limit, 0);
        assert_eq!(options.offset, 0);
    }

    #[test]
    fn test_list_instances_options_with_tenant() {
        let options = ListInstancesOptions {
            tenant_id: Some("tenant-1".to_string()),
            ..Default::default()
        };

        assert_eq!(options.tenant_id, Some("tenant-1".to_string()));
    }

    #[test]
    fn test_list_instances_options_with_status() {
        let options = ListInstancesOptions {
            status: Some("running".to_string()),
            ..Default::default()
        };

        assert_eq!(options.status, Some("running".to_string()));
    }

    #[test]
    fn test_list_instances_options_with_image_filters() {
        let options = ListInstancesOptions {
            image_id: Some("img-123".to_string()),
            image_name_prefix: Some("scenario:".to_string()),
            ..Default::default()
        };

        assert_eq!(options.image_id, Some("img-123".to_string()));
        assert_eq!(options.image_name_prefix, Some("scenario:".to_string()));
    }

    #[test]
    fn test_list_instances_options_with_date_filters() {
        let now = Utc::now();
        let yesterday = now - chrono::Duration::days(1);

        let options = ListInstancesOptions {
            created_after: Some(yesterday),
            created_before: Some(now),
            finished_after: Some(yesterday),
            finished_before: Some(now),
            ..Default::default()
        };

        assert!(options.created_after.is_some());
        assert!(options.created_before.is_some());
        assert!(options.finished_after.is_some());
        assert!(options.finished_before.is_some());
    }

    #[test]
    fn test_list_instances_options_with_ordering() {
        let options = ListInstancesOptions {
            order_by: Some("created_at_asc".to_string()),
            ..Default::default()
        };

        assert_eq!(options.order_by, Some("created_at_asc".to_string()));
    }

    #[test]
    fn test_list_instances_options_with_pagination() {
        let options = ListInstancesOptions {
            limit: 50,
            offset: 100,
            ..Default::default()
        };

        assert_eq!(options.limit, 50);
        assert_eq!(options.offset, 100);
    }

    #[test]
    fn test_list_instances_options_full() {
        let now = Utc::now();

        let options = ListInstancesOptions {
            tenant_id: Some("tenant-1".to_string()),
            status: Some("completed".to_string()),
            image_id: Some("img-456".to_string()),
            image_name_prefix: Some("workflow:".to_string()),
            created_after: Some(now - chrono::Duration::days(7)),
            created_before: Some(now),
            finished_after: Some(now - chrono::Duration::days(1)),
            finished_before: Some(now),
            order_by: Some("finished_at_desc".to_string()),
            limit: 25,
            offset: 50,
        };

        assert_eq!(options.tenant_id, Some("tenant-1".to_string()));
        assert_eq!(options.status, Some("completed".to_string()));
        assert_eq!(options.image_id, Some("img-456".to_string()));
        assert_eq!(options.image_name_prefix, Some("workflow:".to_string()));
        assert!(options.created_after.is_some());
        assert!(options.created_before.is_some());
        assert!(options.finished_after.is_some());
        assert!(options.finished_before.is_some());
        assert_eq!(options.order_by, Some("finished_at_desc".to_string()));
        assert_eq!(options.limit, 25);
        assert_eq!(options.offset, 50);
    }

    #[test]
    fn test_list_instances_options_debug() {
        let options = ListInstancesOptions {
            tenant_id: Some("test".to_string()),
            ..Default::default()
        };

        let debug_str = format!("{:?}", options);
        assert!(debug_str.contains("ListInstancesOptions"));
        assert!(debug_str.contains("tenant_id"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_list_instances_options_clone() {
        let options = ListInstancesOptions {
            tenant_id: Some("tenant-1".to_string()),
            status: Some("running".to_string()),
            limit: 10,
            ..Default::default()
        };

        let cloned = options.clone();

        assert_eq!(options.tenant_id, cloned.tenant_id);
        assert_eq!(options.status, cloned.status);
        assert_eq!(options.limit, cloned.limit);
    }

    // ==========================================================================
    // Instance struct tests
    // ==========================================================================

    #[test]
    fn test_instance_debug() {
        let instance = Instance {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: "running".to_string(),
            checkpoint_id: Some("cp-1".to_string()),
            attempt: 1,
            max_attempts: 3,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: None,
            output: None,
            error: None,
        };

        let debug_str = format!("{:?}", instance);
        assert!(debug_str.contains("Instance"));
        assert!(debug_str.contains("inst-1"));
        assert!(debug_str.contains("running"));
    }

    #[test]
    fn test_instance_clone() {
        let instance = Instance {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: "completed".to_string(),
            checkpoint_id: Some("cp-1".to_string()),
            attempt: 2,
            max_attempts: 3,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            output: Some(b"result".to_vec()),
            error: None,
        };

        let cloned = instance.clone();

        assert_eq!(instance.instance_id, cloned.instance_id);
        assert_eq!(instance.tenant_id, cloned.tenant_id);
        assert_eq!(instance.status, cloned.status);
        assert_eq!(instance.checkpoint_id, cloned.checkpoint_id);
        assert_eq!(instance.attempt, cloned.attempt);
        assert_eq!(instance.max_attempts, cloned.max_attempts);
        assert_eq!(instance.output, cloned.output);
    }

    #[test]
    fn test_instance_with_error() {
        let instance = Instance {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: "failed".to_string(),
            checkpoint_id: None,
            attempt: 3,
            max_attempts: 3,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            output: None,
            error: Some("Something went wrong".to_string()),
        };

        assert_eq!(instance.status, "failed");
        assert_eq!(instance.error, Some("Something went wrong".to_string()));
        assert!(instance.output.is_none());
    }

    // ==========================================================================
    // InstanceWithImage struct tests
    // ==========================================================================

    #[test]
    fn test_instance_with_image_debug() {
        let instance = InstanceWithImage {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: "running".to_string(),
            checkpoint_id: None,
            attempt: 1,
            max_attempts: 3,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: None,
            output: None,
            error: None,
            image_id: Some("img-123".to_string()),
            image_name: Some("my-workflow:v1".to_string()),
        };

        let debug_str = format!("{:?}", instance);
        assert!(debug_str.contains("InstanceWithImage"));
        assert!(debug_str.contains("img-123"));
        assert!(debug_str.contains("my-workflow:v1"));
    }

    #[test]
    fn test_instance_with_image_clone() {
        let instance = InstanceWithImage {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: "running".to_string(),
            checkpoint_id: None,
            attempt: 1,
            max_attempts: 3,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            output: None,
            error: None,
            image_id: Some("img-123".to_string()),
            image_name: Some("my-workflow".to_string()),
        };

        let cloned = instance.clone();

        assert_eq!(instance.image_id, cloned.image_id);
        assert_eq!(instance.image_name, cloned.image_name);
    }

    #[test]
    fn test_instance_with_image_no_image() {
        let instance = InstanceWithImage {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: "pending".to_string(),
            checkpoint_id: None,
            attempt: 0,
            max_attempts: 3,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            output: None,
            error: None,
            image_id: None,
            image_name: None,
        };

        assert!(instance.image_id.is_none());
        assert!(instance.image_name.is_none());
    }

    // ==========================================================================
    // InstanceFull struct tests
    // ==========================================================================

    #[test]
    fn test_instance_full_debug() {
        let instance = InstanceFull {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            image_id: Some("img-123".to_string()),
            image_name: Some("my-workflow:v1".to_string()),
            status: "running".to_string(),
            output: None,
            error: None,
            checkpoint_id: Some("cp-5".to_string()),
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: None,
            heartbeat_at: Some(Utc::now()),
            attempt: 1,
            max_attempts: 3,
            memory_peak_bytes: Some(536_870_912), // 512 MB
            cpu_usage_usec: Some(1_500_000),      // 1.5 seconds
        };

        let debug_str = format!("{:?}", instance);
        assert!(debug_str.contains("InstanceFull"));
        assert!(debug_str.contains("heartbeat_at"));
        assert!(debug_str.contains("memory_peak_bytes"));
        assert!(debug_str.contains("cpu_usage_usec"));
    }

    #[test]
    fn test_instance_full_clone() {
        let now = Utc::now();
        let instance = InstanceFull {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            image_id: Some("img-123".to_string()),
            image_name: Some("workflow".to_string()),
            status: "completed".to_string(),
            output: Some(b"result".to_vec()),
            error: None,
            checkpoint_id: Some("cp-10".to_string()),
            created_at: now,
            started_at: Some(now),
            finished_at: Some(now),
            heartbeat_at: Some(now),
            attempt: 1,
            max_attempts: 3,
            memory_peak_bytes: Some(1_073_741_824), // 1 GB
            cpu_usage_usec: Some(5_000_000),        // 5 seconds
        };

        let cloned = instance.clone();

        assert_eq!(instance.instance_id, cloned.instance_id);
        assert_eq!(instance.heartbeat_at, cloned.heartbeat_at);
        assert_eq!(instance.output, cloned.output);
        assert_eq!(instance.memory_peak_bytes, cloned.memory_peak_bytes);
        assert_eq!(instance.cpu_usage_usec, cloned.cpu_usage_usec);
    }

    #[test]
    fn test_instance_full_no_heartbeat() {
        let instance = InstanceFull {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            image_id: None,
            image_name: None,
            status: "pending".to_string(),
            output: None,
            error: None,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            heartbeat_at: None,
            attempt: 0,
            max_attempts: 3,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
        };

        assert!(instance.heartbeat_at.is_none());
        assert!(instance.started_at.is_none());
        assert!(instance.memory_peak_bytes.is_none());
        assert!(instance.cpu_usage_usec.is_none());
    }

    #[test]
    fn test_instance_full_with_metrics() {
        let instance = InstanceFull {
            instance_id: "inst-metrics".to_string(),
            tenant_id: "tenant-1".to_string(),
            image_id: Some("img-123".to_string()),
            image_name: Some("cpu-intensive-workflow".to_string()),
            status: "completed".to_string(),
            output: Some(b"done".to_vec()),
            error: None,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            heartbeat_at: None,
            attempt: 1,
            max_attempts: 1,
            memory_peak_bytes: Some(2_147_483_648), // 2 GB
            cpu_usage_usec: Some(120_000_000),      // 2 minutes
        };

        assert_eq!(instance.memory_peak_bytes, Some(2_147_483_648));
        assert_eq!(instance.cpu_usage_usec, Some(120_000_000));
    }

    #[test]
    fn test_instance_full_without_metrics() {
        // Simulates an instance where metrics couldn't be collected
        // (e.g., container exited too quickly or cgroup read failed)
        let instance = InstanceFull {
            instance_id: "inst-no-metrics".to_string(),
            tenant_id: "tenant-1".to_string(),
            image_id: Some("img-123".to_string()),
            image_name: Some("quick-workflow".to_string()),
            status: "completed".to_string(),
            output: Some(b"done".to_vec()),
            error: None,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            heartbeat_at: None,
            attempt: 1,
            max_attempts: 1,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
        };

        assert!(instance.memory_peak_bytes.is_none());
        assert!(instance.cpu_usage_usec.is_none());
    }

    // ==========================================================================
    // WakeEntry struct tests
    // ==========================================================================

    #[test]
    fn test_wake_entry_debug() {
        let entry = WakeEntry {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            image_id: "img-123".to_string(),
            checkpoint_id: "cp-5".to_string(),
            wake_at: Utc::now(),
        };

        let debug_str = format!("{:?}", entry);
        assert!(debug_str.contains("WakeEntry"));
        assert!(debug_str.contains("inst-1"));
        assert!(debug_str.contains("cp-5"));
    }

    #[test]
    fn test_wake_entry_clone() {
        let wake_at = Utc::now();
        let entry = WakeEntry {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            image_id: "img-123".to_string(),
            checkpoint_id: "cp-5".to_string(),
            wake_at,
        };

        let cloned = entry.clone();

        assert_eq!(entry.instance_id, cloned.instance_id);
        assert_eq!(entry.tenant_id, cloned.tenant_id);
        assert_eq!(entry.image_id, cloned.image_id);
        assert_eq!(entry.checkpoint_id, cloned.checkpoint_id);
        assert_eq!(entry.wake_at, cloned.wake_at);
    }

    #[test]
    fn test_wake_entry_future_wake() {
        let future_time = Utc::now() + chrono::Duration::hours(1);
        let entry = WakeEntry {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            image_id: "img-123".to_string(),
            checkpoint_id: "cp-5".to_string(),
            wake_at: future_time,
        };

        assert!(entry.wake_at > Utc::now());
    }
}
