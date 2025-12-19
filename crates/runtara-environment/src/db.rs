// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Database operations for runtara-environment.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Instance record from the database.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Instance {
    /// Unique identifier for the instance.
    pub instance_id: String,
    /// Tenant identifier for multi-tenancy isolation.
    pub tenant_id: String,
    /// Image ID that this instance was created from.
    pub image_id: Option<String>,
    /// Current status (pending, running, suspended, completed, failed, cancelled).
    pub status: String,
    /// Input data passed when starting the instance.
    pub input: Option<serde_json::Value>,
    /// Output data from successful completion.
    pub output: Option<serde_json::Value>,
    /// Error message from failure.
    pub error: Option<String>,
    /// Last checkpoint ID if instance was checkpointed.
    pub checkpoint_id: Option<String>,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance started running.
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished (completed, failed, or cancelled).
    pub finished_at: Option<DateTime<Utc>>,
    /// Current retry attempt number.
    pub retry_count: i32,
    /// Maximum allowed retries.
    pub max_retries: i32,
}

/// Full instance record with joined image name and heartbeat.
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
    /// Current status (pending, running, suspended, completed, failed, cancelled).
    pub status: String,
    /// Input data passed when starting the instance.
    pub input: Option<serde_json::Value>,
    /// Output data from successful completion.
    pub output: Option<serde_json::Value>,
    /// Error message from failure.
    pub error: Option<String>,
    /// Last checkpoint ID if instance was checkpointed.
    pub checkpoint_id: Option<String>,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance started running.
    pub started_at: Option<DateTime<Utc>>,
    /// When the instance finished (completed, failed, or cancelled).
    pub finished_at: Option<DateTime<Utc>>,
    /// Last heartbeat timestamp (from container_heartbeats table).
    pub heartbeat_at: Option<DateTime<Utc>>,
    /// Current retry attempt number.
    pub retry_count: i32,
    /// Maximum allowed retries.
    pub max_retries: i32,
}

/// Create a new instance record.
pub async fn create_instance(
    pool: &PgPool,
    instance_id: &str,
    tenant_id: &str,
    image_id: &str,
    input: Option<&serde_json::Value>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, image_id, status, input, created_at)
        VALUES ($1, $2, $3, 'pending', $4, NOW())
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .bind(image_id)
    .bind(input)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get an instance by ID.
pub async fn get_instance(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<Instance>, sqlx::Error> {
    sqlx::query_as::<_, Instance>(
        r#"
        SELECT instance_id, tenant_id, image_id, status, input, output, error,
               checkpoint_id, created_at, started_at, finished_at, retry_count, max_retries
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
        SELECT i.instance_id, i.tenant_id, i.image_id, img.name as image_name,
               i.status, i.input, i.output, i.error, i.checkpoint_id,
               i.created_at, i.started_at, i.finished_at,
               ch.last_heartbeat as heartbeat_at, i.retry_count, i.max_retries
        FROM instances i
        LEFT JOIN images img ON i.image_id = img.image_id::text
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
        SET status = $2,
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
    output: Option<&serde_json::Value>,
    error: Option<&str>,
    checkpoint_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE instances
        SET status = $2,
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
) -> Result<Vec<Instance>, sqlx::Error> {
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
        SELECT i.instance_id, i.tenant_id, i.image_id, i.status, i.input, i.output, i.error,
               i.checkpoint_id, i.created_at, i.started_at, i.finished_at, i.retry_count, i.max_retries
        FROM instances i
        LEFT JOIN images img ON i.image_id = img.image_id
        WHERE ($1::TEXT IS NULL OR i.tenant_id = $1)
          AND ($2::TEXT IS NULL OR i.status = $2)
          AND ($3::TEXT IS NULL OR i.image_id = $3)
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

    sqlx::query_as::<_, Instance>(&query)
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
        LEFT JOIN images img ON i.image_id = img.image_id
        WHERE ($1::TEXT IS NULL OR i.tenant_id = $1)
          AND ($2::TEXT IS NULL OR i.status = $2)
          AND ($3::TEXT IS NULL OR i.image_id = $3)
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
        SELECT w.instance_id, i.tenant_id, i.image_id, w.checkpoint_id, w.wake_at
        FROM wake_queue w
        JOIN instances i ON w.instance_id = i.instance_id
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
// Instance Images (for shared Core persistence mode)
// ============================================================================

/// Associate an instance with an image.
///
/// Used when Core owns the instances table but Environment needs to track
/// which image was used to launch each instance.
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
