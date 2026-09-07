// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database operations for runtara-environment.
//!
//! Environment shares the `instances` table with Core but maintains its own
//! `instance_images` table to track which image launched each instance.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::instance_repository::ListInstancesOptions;

/// Instance with image info (joined from instance_images).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstanceWithImage {
    /// Unique identifier for the instance.
    pub instance_id: String,
    /// Tenant identifier for multi-tenancy isolation.
    pub tenant_id: String,
    /// Current status.
    pub status: String,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance started running.
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished.
    pub finished_at: Option<DateTime<Utc>>,
    /// Error message (user-facing).
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
    /// Input data provided when starting the instance.
    pub input: Option<Vec<u8>>,
    /// Output data.
    pub output: Option<Vec<u8>>,
    /// Error message (user-facing).
    pub error: Option<String>,
    /// Raw stderr output from the container (for debugging/logging).
    pub stderr: Option<String>,
    /// Last checkpoint ID.
    pub checkpoint_id: Option<String>,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance started running.
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished.
    pub finished_at: Option<DateTime<Utc>>,
    /// Current attempt number.
    pub attempt: i32,
    /// Maximum allowed attempts.
    pub max_attempts: i32,
    /// Peak memory usage during execution (in bytes).
    pub memory_peak_bytes: Option<i64>,
    /// Total CPU time consumed during execution (in microseconds).
    pub cpu_usage_usec: Option<i64>,
    /// How the instance terminated (completed, application_error, crashed, timeout, etc.).
    pub termination_reason: Option<String>,
    /// Process exit code (if available).
    pub exit_code: Option<i32>,
}

/// The stored status and owning tenant of one instance.
///
/// Returns the status as the column spells it rather than a decoded enum:
/// its one caller interpolates the label into a message a user reads.
pub async fn instance_identity(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<(String, String)>, sqlx::Error> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT status::TEXT, tenant_id FROM instances WHERE instance_id = $1")
            .bind(instance_id)
            .fetch_optional(pool)
            .await?;

    Ok(row)
}

/// Get full instance details including image name and heartbeat.
pub async fn get_instance_full(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<InstanceFull>, sqlx::Error> {
    sqlx::query_as::<_, InstanceFull>(
        r#"
        SELECT i.instance_id, i.tenant_id, ii.image_id, img.name as image_name,
               i.status::TEXT as status, i.input, i.output, i.error, i.stderr, i.checkpoint_id,
               i.created_at, i.started_at, i.finished_at,
               i.attempt, i.max_attempts,
               i.memory_peak_bytes, i.cpu_usage_usec,
               i.termination_reason::TEXT as termination_reason, i.exit_code
        FROM instances i
        LEFT JOIN instance_images ii ON i.instance_id = ii.instance_id
        LEFT JOIN images img ON ii.image_id = img.image_id
        WHERE i.instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await
}

// Ordinary instance state transitions (update, complete, metrics and stderr)
// are delegated to the Core Persistence trait. The atomic start claim below is
// the narrow exception: it must write Core's instance row and Environment's
// image binding as one database transaction. Join reads remain here too.

/// Status filter as a bindable array. An empty list means "no status filter"
/// rather than "match nothing", so callers can pass a filtered/deduped vector
/// straight through without special-casing the empty case.
fn status_filter(options: &ListInstancesOptions) -> Option<&[String]> {
    options
        .statuses
        .as_deref()
        .filter(|statuses| !statuses.is_empty())
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
        SELECT i.instance_id, i.tenant_id, i.status::TEXT as status,
               i.created_at, i.started_at, i.finished_at,
               i.error, ii.image_id, img.name as image_name
        FROM instances i
        LEFT JOIN instance_images ii ON i.instance_id = ii.instance_id
        LEFT JOIN images img ON ii.image_id = img.image_id
        WHERE ($1::TEXT IS NULL OR i.tenant_id = $1)
          AND ($2::TEXT[] IS NULL OR i.status::TEXT = ANY($2::TEXT[]))
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
        .bind(status_filter(options))
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

/// Count a tenant's instances in the given statuses.
///
/// Separate from [`count_instances`] on purpose. That one backs pagination, so
/// it carries every optional filter and joins the image tables; the optional
/// filters are written `$n IS NULL OR col = $n`, which is not sargable, and the
/// status compare casts the enum to text, so neither `idx_instances_status` nor
/// any other index applies and it degrades to a sequential scan.
///
/// The admission gate only ever wants "how many are active for this tenant",
/// and it runs on every intake. Binding the status list as the enum array and
/// dropping the joins keeps it on `idx_instances_status`, so its cost tracks the
/// number of ACTIVE instances rather than the size of the table. That matters
/// because the table is dominated by suspended rows: under the old shape the
/// gate got steadily slower the more instances were parked, which throttled
/// intake exactly when a large sleeping population had accumulated.
pub async fn count_instances_by_status(
    pool: &PgPool,
    tenant_id: Option<&str>,
    statuses: &[String],
    ceiling: i64,
) -> Result<i64, sqlx::Error> {
    // Stop counting at `ceiling`. The caller is an admission gate asking "am I
    // at the cap", so every row scanned past the cap changes nothing it can
    // decide. Without the bound the count is O(active instances), and a backlog
    // of pending work is precisely when the gate is consulted most and when
    // that set is largest - the count then becomes the thing throttling intake.
    let count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*) FROM (
            SELECT 1
            FROM instances
            WHERE ($1::TEXT IS NULL OR tenant_id = $1)
              AND status = ANY($2::instance_status[])
            LIMIT $3
        ) capped
        "#,
    )
    .bind(tenant_id)
    .bind(statuses)
    .bind(ceiling)
    .fetch_one(pool)
    .await?;

    Ok(count.0)
}

/// The parked count's query text.
///
/// Built rather than written inline so the test that asserts on its query plan
/// can EXPLAIN the statement this function runs, instead of a copy that could
/// drift away from it.
///
/// `parked` is spliced, and that is the whole point of the shape. See
/// [`count_parked_instances`] for why a bound status cannot work here.
pub(crate) fn parked_count_sql(parked: &str) -> String {
    format!(
        r#"
        SELECT COUNT(*)
        FROM instances
        WHERE tenant_id = $1
          AND status = '{parked}'
        "#
    )
}

/// Count a tenant's parked instances, with no ceiling.
///
/// The sibling above stops at a bound because its caller only needs to know
/// whether a cap is reached. A viewer wants the real figure, and this answers it
/// from `idx_instances_suspended_tenant` (025) instead of by reading the table:
/// an index-only scan over one tenant's parked rows. Measured over 500k
/// instances with 350k suspended, counting one tenant's 306k: 15,622 buffers
/// before that index, 261 after.
pub async fn count_parked_instances(
    pool: &PgPool,
    tenant_id: &str,
    parked: &str,
) -> Result<i64, sqlx::Error> {
    // Why the status is spliced rather than bound, which is the whole reason
    // this is not just the capped query without its LIMIT.
    //
    // 025 is a partial index with the predicate `status = 'suspended'`, and the
    // planner applies a partial index only when it can prove the query's own
    // predicate implies it. That proof needs a constant. The obvious form,
    // `status = ANY($2::instance_status[])` with the statuses bound, does not
    // give it one: sqlx sends a `Vec<String>` as `text[]`, so what the planner
    // actually sees is `(('{suspended}'::text[])::instance_status[])` — a cast
    // through the enum's input function, which is `stable` rather than
    // `immutable` and therefore not folded to a constant before index matching.
    // The proof fails, the index is skipped in silence, and the count reverts
    // to reading the table. Measured, and pinned by
    // `the_parked_count_reaches_its_partial_index`.
    //
    // Splicing is safe because the value is never a caller's input:
    // `status_name` maps a Rust enum to one of six fixed identifiers, and the
    // only caller passes `InstanceStatus::Suspended`. It is an argument rather
    // than a literal written here so the label keeps one spelling — the one the
    // rest of the crate already uses — and a renamed variant cannot leave this
    // query silently disagreeing with it.
    let count: (i64,) = sqlx::query_as(&parked_count_sql(parked))
        .bind(tenant_id)
        .fetch_one(pool)
        .await?;

    Ok(count.0)
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
          AND ($2::TEXT[] IS NULL OR i.status::TEXT = ANY($2::TEXT[]))
          AND ($3::TEXT IS NULL OR ii.image_id = $3)
          AND ($4::TEXT IS NULL OR img.name LIKE $4)
          AND ($5::TIMESTAMPTZ IS NULL OR i.created_at >= $5)
          AND ($6::TIMESTAMPTZ IS NULL OR i.created_at < $6)
          AND ($7::TIMESTAMPTZ IS NULL OR i.finished_at >= $7)
          AND ($8::TIMESTAMPTZ IS NULL OR i.finished_at < $8)
        "#,
    )
    .bind(options.tenant_id.as_deref())
    .bind(status_filter(options))
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

// ============================================================================
// Instance Images
// ============================================================================

// ============================================================================
// Tenant Metrics
// ============================================================================

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
/// Aggregates instance execution metrics into time buckets of
/// `bucket_seconds`, using aggregate functions for statistics.
///
/// Buckets are aligned by flooring the Unix epoch to a multiple of the width
/// rather than by `date_trunc`. That admits widths `date_trunc` has no unit for -
/// six minutes, two hours - and it fixes a latent inconsistency along the way:
/// `date_trunc` on a `timestamptz` truncates in the *session* time zone, which
/// nothing in this codebase pins, while `bucket_time` was always reported as UTC.
/// Flooring the epoch is UTC unconditionally.
///
/// Returns all buckets in the time range, including empty ones (with zero counts).
pub async fn get_tenant_metrics(
    pool: &PgPool,
    tenant_id: &str,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    bucket_seconds: u32,
) -> Result<Vec<MetricsBucketRow>, sqlx::Error> {
    // A zero width divides by zero inside the query. Callers are validated at
    // both the HTTP boundary and in `handle_get_tenant_metrics`, so reaching
    // here with one is a bug rather than bad input; clamp instead of panicking.
    let bucket_seconds = f64::from(bucket_seconds.max(1));

    // The spine and the aggregate derive their bucket key with the same
    // expression, so the join keys align by construction. Getting that wrong is
    // the one way this query fails quietly - every bucket would read as empty.
    let query = r#"
        WITH time_series AS (
            SELECT generate_series(
                to_timestamp(floor(extract(epoch FROM $2::timestamptz)::float8 / $4::float8) * $4::float8),
                $3::timestamptz,
                make_interval(secs => $4::float8)
            ) AS bucket_time
        ),
        metrics AS (
            SELECT
                to_timestamp(floor(extract(epoch FROM i.finished_at)::float8 / $4::float8) * $4::float8)
                    AS bucket_time,
                COUNT(*) AS invocation_count,
                SUM(CASE WHEN i.status = 'completed' THEN 1 ELSE 0 END) AS success_count,
                SUM(CASE WHEN i.status = 'failed' THEN 1 ELSE 0 END) AS failure_count,
                SUM(CASE WHEN i.status = 'cancelled' THEN 1 ELSE 0 END) AS cancelled_count,
                (AVG(CASE WHEN i.started_at IS NOT NULL AND i.finished_at IS NOT NULL
                    THEN EXTRACT(EPOCH FROM (i.finished_at - i.started_at)) * 1000
                    ELSE NULL END))::FLOAT8 AS avg_duration_ms,
                (MIN(CASE WHEN i.started_at IS NOT NULL AND i.finished_at IS NOT NULL
                    THEN EXTRACT(EPOCH FROM (i.finished_at - i.started_at)) * 1000
                    ELSE NULL END))::FLOAT8 AS min_duration_ms,
                (MAX(CASE WHEN i.started_at IS NOT NULL AND i.finished_at IS NOT NULL
                    THEN EXTRACT(EPOCH FROM (i.finished_at - i.started_at)) * 1000
                    ELSE NULL END))::FLOAT8 AS max_duration_ms,
                AVG(i.memory_peak_bytes)::FLOAT8 AS avg_memory_bytes,
                MAX(i.memory_peak_bytes) AS max_memory_bytes
            FROM instances i
            WHERE i.tenant_id = $1
              AND i.finished_at >= $2
              AND i.finished_at < $3
              AND i.status IN ('completed', 'failed', 'cancelled')
            GROUP BY 1
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
    "#;

    sqlx::query_as::<_, MetricsBucketRow>(query)
        .bind(tenant_id)
        .bind(start_time)
        .bind(end_time)
        .bind(bucket_seconds)
        .fetch_all(pool)
        .await
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod integration_tests;

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
        assert!(options.statuses.is_none());
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
            statuses: Some(vec!["running".to_string()]),
            ..Default::default()
        };

        assert_eq!(options.statuses, Some(vec!["running".to_string()]));
        assert_eq!(status_filter(&options), Some(&["running".to_string()][..]));
    }

    #[test]
    fn test_list_instances_options_with_multiple_statuses() {
        let options = ListInstancesOptions {
            statuses: Some(vec!["failed".to_string(), "cancelled".to_string()]),
            ..Default::default()
        };

        assert_eq!(
            status_filter(&options),
            Some(&["failed".to_string(), "cancelled".to_string()][..])
        );
    }

    #[test]
    fn test_status_filter_treats_empty_list_as_unfiltered() {
        // An empty array bound into `= ANY(...)` would match no rows at all,
        // which is not what "no status filter" means.
        let options = ListInstancesOptions {
            statuses: Some(Vec::new()),
            ..Default::default()
        };

        assert_eq!(status_filter(&options), None);
        assert_eq!(status_filter(&ListInstancesOptions::default()), None);
    }

    #[test]
    fn test_list_instances_options_with_image_filters() {
        let options = ListInstancesOptions {
            image_id: Some("img-123".to_string()),
            image_name_prefix: Some("workflow:".to_string()),
            ..Default::default()
        };

        assert_eq!(options.image_id, Some("img-123".to_string()));
        assert_eq!(options.image_name_prefix, Some("workflow:".to_string()));
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
            statuses: Some(vec!["completed".to_string()]),
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
        assert_eq!(options.statuses, Some(vec!["completed".to_string()]));
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
            statuses: Some(vec!["running".to_string()]),
            limit: 10,
            ..Default::default()
        };

        let cloned = options.clone();

        assert_eq!(options.tenant_id, cloned.tenant_id);
        assert_eq!(options.statuses, cloned.statuses);
        assert_eq!(options.limit, cloned.limit);
    }

    // ==========================================================================
    // Instance struct tests
    // ==========================================================================

    // ==========================================================================
    // InstanceWithImage struct tests
    // ==========================================================================

    #[test]
    fn test_instance_with_image_debug() {
        let instance = InstanceWithImage {
            instance_id: "inst-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status: "running".to_string(),
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: None,
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
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
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
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
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
            input: Some(b"{\"key\":\"value\"}".to_vec()),
            output: None,
            error: None,
            stderr: None,
            checkpoint_id: Some("cp-5".to_string()),
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: None,
            attempt: 1,
            max_attempts: 3,
            memory_peak_bytes: Some(536_870_912), // 512 MB
            cpu_usage_usec: Some(1_500_000),      // 1.5 seconds
            termination_reason: None,
            exit_code: None,
        };

        let debug_str = format!("{:?}", instance);
        assert!(debug_str.contains("InstanceFull"));
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
            input: Some(b"{}".to_vec()),
            output: Some(b"result".to_vec()),
            error: None,
            stderr: None,
            checkpoint_id: Some("cp-10".to_string()),
            created_at: now,
            started_at: Some(now),
            finished_at: Some(now),
            attempt: 1,
            max_attempts: 3,
            memory_peak_bytes: Some(1_073_741_824), // 1 GB
            cpu_usage_usec: Some(5_000_000),        // 5 seconds
            termination_reason: None,
            exit_code: None,
        };

        let cloned = instance.clone();

        assert_eq!(instance.instance_id, cloned.instance_id);
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
            input: None,
            output: None,
            error: None,
            stderr: None,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            attempt: 0,
            max_attempts: 3,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
            termination_reason: None,
            exit_code: None,
        };

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
            input: Some(b"{\"task\":\"compute\"}".to_vec()),
            output: Some(b"done".to_vec()),
            error: None,
            stderr: None,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            attempt: 1,
            max_attempts: 1,
            memory_peak_bytes: Some(2_147_483_648), // 2 GB
            cpu_usage_usec: Some(120_000_000),      // 2 minutes
            termination_reason: Some("completed".to_string()),
            exit_code: Some(0),
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
            input: None,
            output: Some(b"done".to_vec()),
            error: None,
            stderr: None,
            checkpoint_id: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            attempt: 1,
            max_attempts: 1,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
            termination_reason: None,
            exit_code: None,
        };

        assert!(instance.memory_peak_bytes.is_none());
        assert!(instance.cpu_usage_usec.is_none());
    }
}
