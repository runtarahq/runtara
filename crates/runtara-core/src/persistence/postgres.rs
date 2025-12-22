// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Persistence operations for runtara-core.
//!
//! Provides all durable storage access functions for instances, checkpoints, events, and signals.

#![allow(dead_code)] // Fields and functions used in tests and by handlers

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::CoreError;

/// PostgreSQL-backed persistence implementation.
#[derive(Clone)]
pub struct PostgresPersistence {
    pool: PgPool,
}

impl PostgresPersistence {
    /// Create a new Postgres-backed persistence implementation.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// ============================================================================
// Record Types
// ============================================================================

use super::{
    CheckpointRecord, CustomSignalRecord, EventRecord, InstanceRecord, ListEventsFilter,
    Persistence, SignalRecord, WakeEntry,
};

// ============================================================================
// Instance Operations
// ============================================================================

/// Create a new instance record.
pub async fn create_instance(
    pool: &PgPool,
    instance_id: &str,
    tenant_id: &str,
) -> Result<(), CoreError> {
    sqlx::query(
        r#"
        INSERT INTO instances (instance_id, tenant_id, definition_version, status, created_at)
        VALUES ($1, $2, 1, 'pending'::instance_status, NOW())
        "#,
    )
    .bind(instance_id)
    .bind(tenant_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// UUID used for self-registered instances (no image/definition).
/// This is a well-known UUID that indicates the instance registered itself.
pub const SELF_REGISTERED_DEFINITION_ID: Uuid = Uuid::from_u128(0);

/// Create a self-registered instance record.
/// Used when an instance registers itself without being started via the management API.
/// Get an instance by ID.
pub async fn get_instance(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<InstanceRecord>, CoreError> {
    let record = sqlx::query_as::<_, InstanceRecord>(
        r#"
        SELECT instance_id, tenant_id, definition_version,
               status::text as status, checkpoint_id, attempt, max_attempts,
               created_at, started_at, finished_at, output, error, sleep_until
        FROM instances
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await?;

    Ok(record)
}

/// Update instance status.
pub async fn update_instance_status(
    pool: &PgPool,
    instance_id: &str,
    status: &str,
    started_at: Option<DateTime<Utc>>,
) -> Result<(), CoreError> {
    let result = if let Some(started) = started_at {
        sqlx::query(
            r#"
            UPDATE instances
            SET status = $2::instance_status, started_at = $3
            WHERE instance_id = $1
            "#,
        )
        .bind(instance_id)
        .bind(status)
        .bind(started)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            r#"
            UPDATE instances
            SET status = $2::instance_status
            WHERE instance_id = $1
            "#,
        )
        .bind(instance_id)
        .bind(status)
        .execute(pool)
        .await?
    };

    if result.rows_affected() == 0 {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }

    Ok(())
}

/// Update instance's current checkpoint ID.
pub async fn update_instance_checkpoint(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
) -> Result<(), CoreError> {
    let result = sqlx::query(
        r#"
        UPDATE instances
        SET checkpoint_id = $2
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }

    Ok(())
}

/// Mark instance as completed (success or failure).
pub async fn complete_instance(
    pool: &PgPool,
    instance_id: &str,
    output: Option<&[u8]>,
    error: Option<&str>,
) -> Result<(), CoreError> {
    let status = if error.is_some() {
        "failed"
    } else {
        "completed"
    };

    let result = sqlx::query(
        r#"
        UPDATE instances
        SET status = $2::instance_status,
            finished_at = NOW(),
            output = $3,
            error = $4
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .bind(status)
    .bind(output)
    .bind(error)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }

    Ok(())
}

// ============================================================================
// Checkpoint Operations
// ============================================================================

/// Save a checkpoint (append-only).
/// Uses ON CONFLICT to handle duplicate checkpoint IDs gracefully.
pub async fn save_checkpoint(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
    state: &[u8],
) -> Result<(), CoreError> {
    sqlx::query(
        r#"
        INSERT INTO checkpoints (instance_id, checkpoint_id, state, created_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (instance_id, checkpoint_id) DO UPDATE
        SET state = EXCLUDED.state, created_at = NOW()
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(state)
    .execute(pool)
    .await
    .map_err(|e| CoreError::CheckpointSaveFailed {
        instance_id: instance_id.to_string(),
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Load a specific checkpoint by ID.
pub async fn load_checkpoint(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
) -> Result<Option<CheckpointRecord>, CoreError> {
    let record = sqlx::query_as::<_, CheckpointRecord>(
        r#"
        SELECT id, instance_id, checkpoint_id, state, created_at
        FROM checkpoints
        WHERE instance_id = $1 AND checkpoint_id = $2
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .fetch_optional(pool)
    .await?;

    Ok(record)
}

/// Load the latest checkpoint for an instance.
pub async fn load_latest_checkpoint(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<CheckpointRecord>, CoreError> {
    let record = sqlx::query_as::<_, CheckpointRecord>(
        r#"
        SELECT id, instance_id, checkpoint_id, state, created_at
        FROM checkpoints
        WHERE instance_id = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await?;

    Ok(record)
}

/// List checkpoints for an instance with filtering and pagination.
pub async fn list_checkpoints(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id_filter: Option<&str>,
    limit: i64,
    offset: i64,
    created_after: Option<DateTime<Utc>>,
    created_before: Option<DateTime<Utc>>,
) -> Result<Vec<CheckpointRecord>, CoreError> {
    let records = sqlx::query_as::<_, CheckpointRecord>(
        r#"
        SELECT id, instance_id, checkpoint_id, state, created_at
        FROM checkpoints
        WHERE instance_id = $1
          AND ($2::TEXT IS NULL OR checkpoint_id = $2)
          AND ($3::TIMESTAMPTZ IS NULL OR created_at >= $3)
          AND ($4::TIMESTAMPTZ IS NULL OR created_at < $4)
        ORDER BY created_at DESC
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id_filter)
    .bind(created_after)
    .bind(created_before)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(records)
}

/// Retry attempt record from the database.
/// These are stored in the checkpoints table with is_retry_attempt = true.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RetryAttemptRecord {
    /// Database primary key.
    pub id: i64,
    /// Instance this retry attempt belongs to.
    pub instance_id: String,
    /// Base checkpoint identifier (the durable function's cache key).
    pub checkpoint_id: String,
    /// Retry attempt number (1-indexed).
    pub attempt_number: i32,
    /// Error message from this attempt.
    pub error_message: Option<String>,
    /// When the retry attempt was recorded.
    pub created_at: DateTime<Utc>,
}

/// Save a retry attempt record for audit trail.
/// Retry attempts are stored in the checkpoints table with a unique checkpoint_id.
pub async fn save_retry_attempt(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
    attempt_number: i32,
    error_message: Option<&str>,
) -> Result<(), CoreError> {
    // Create a unique checkpoint_id for this retry attempt
    let retry_checkpoint_id = format!("{}::retry::{}", checkpoint_id, attempt_number);

    sqlx::query(
        r#"
        INSERT INTO checkpoints (instance_id, checkpoint_id, state, is_retry_attempt, attempt_number, error_message, created_at)
        VALUES ($1, $2, '', true, $3, $4, NOW())
        ON CONFLICT (instance_id, checkpoint_id) DO UPDATE
        SET attempt_number = EXCLUDED.attempt_number,
            error_message = EXCLUDED.error_message,
            created_at = NOW()
        "#,
    )
    .bind(instance_id)
    .bind(&retry_checkpoint_id)
    .bind(attempt_number)
    .bind(error_message)
    .execute(pool)
    .await
    .map_err(|e| CoreError::CheckpointSaveFailed {
        instance_id: instance_id.to_string(),
        reason: e.to_string(),
    })?;

    Ok(())
}

/// Load retry history for a checkpoint (for debugging/audit).
/// Returns all retry attempts for the given base checkpoint_id.
pub async fn load_retry_history(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
) -> Result<Vec<RetryAttemptRecord>, CoreError> {
    let pattern = format!("{}::retry::%", checkpoint_id);

    let records = sqlx::query_as::<_, RetryAttemptRecord>(
        r#"
        SELECT id, instance_id, checkpoint_id, attempt_number, error_message, created_at
        FROM checkpoints
        WHERE instance_id = $1
          AND checkpoint_id LIKE $2
          AND is_retry_attempt = true
        ORDER BY attempt_number ASC
        "#,
    )
    .bind(instance_id)
    .bind(&pattern)
    .fetch_all(pool)
    .await?;

    Ok(records)
}

/// Count checkpoints for an instance with filtering.
pub async fn count_checkpoints(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id_filter: Option<&str>,
    created_after: Option<DateTime<Utc>>,
    created_before: Option<DateTime<Utc>>,
) -> Result<i64, CoreError> {
    let count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM checkpoints
        WHERE instance_id = $1
          AND ($2::TEXT IS NULL OR checkpoint_id = $2)
          AND ($3::TIMESTAMPTZ IS NULL OR created_at >= $3)
          AND ($4::TIMESTAMPTZ IS NULL OR created_at < $4)
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id_filter)
    .bind(created_after)
    .bind(created_before)
    .fetch_one(pool)
    .await?;

    Ok(count.0)
}

// ============================================================================
// Wake Queue Operations
// ============================================================================

/// Schedule a wake for an instance.
/// Uses ON CONFLICT to replace existing wake entry for the same instance.
pub async fn schedule_wake(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
    wake_at: DateTime<Utc>,
) -> Result<(), CoreError> {
    sqlx::query(
        r#"
        INSERT INTO wake_queue (instance_id, checkpoint_id, wake_at, created_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (instance_id) DO UPDATE
        SET checkpoint_id = EXCLUDED.checkpoint_id,
            wake_at = EXCLUDED.wake_at,
            created_at = NOW()
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(wake_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get a wake entry for an instance.
pub async fn get_wake_entry(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<WakeEntry>, CoreError> {
    let record = sqlx::query_as::<_, WakeEntry>(
        r#"
        SELECT id, instance_id, checkpoint_id, wake_at, created_at
        FROM wake_queue
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await?;

    Ok(record)
}

/// Clear wake entry for an instance.
pub async fn clear_wake(pool: &PgPool, instance_id: &str) -> Result<(), CoreError> {
    sqlx::query(
        r#"
        DELETE FROM wake_queue
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .execute(pool)
    .await?;

    Ok(())
}

// ============================================================================
// Event Operations
// ============================================================================

/// Insert an instance event.
pub async fn insert_event(pool: &PgPool, event: &EventRecord) -> Result<(), CoreError> {
    sqlx::query(
        r#"
        INSERT INTO instance_events (instance_id, event_type, checkpoint_id, payload, created_at, subtype)
        VALUES ($1, $2::instance_event_type, $3, $4, $5, $6)
        "#,
    )
    .bind(&event.instance_id)
    .bind(&event.event_type)
    .bind(&event.checkpoint_id)
    .bind(&event.payload)
    .bind(event.created_at)
    .bind(&event.subtype)
    .execute(pool)
    .await?;

    Ok(())
}

/// List events for an instance with filtering and pagination.
///
/// Supports filtering by event_type, subtype, time range, and full-text search
/// in the JSON payload. Events are returned in reverse chronological order.
pub async fn list_events(
    pool: &PgPool,
    instance_id: &str,
    filter: &ListEventsFilter,
    limit: i64,
    offset: i64,
) -> Result<Vec<EventRecord>, CoreError> {
    // For PostgreSQL, we use convert_from to search text within BYTEA payload
    // The payload is expected to be valid UTF-8 JSON when subtype is set
    let records = sqlx::query_as::<_, EventRecord>(
        r#"
        SELECT id, instance_id, event_type::text as event_type, checkpoint_id, payload, created_at, subtype
        FROM instance_events
        WHERE instance_id = $1
          AND ($2::TEXT IS NULL OR event_type::text = $2)
          AND ($3::TEXT IS NULL OR subtype = $3)
          AND ($4::TIMESTAMPTZ IS NULL OR created_at >= $4)
          AND ($5::TIMESTAMPTZ IS NULL OR created_at < $5)
          AND ($6::TEXT IS NULL OR (
              payload IS NOT NULL
              AND convert_from(payload, 'UTF8') ILIKE '%' || $6 || '%'
          ))
        ORDER BY created_at DESC, id DESC
        LIMIT $7 OFFSET $8
        "#,
    )
    .bind(instance_id)
    .bind(&filter.event_type)
    .bind(&filter.subtype)
    .bind(filter.created_after)
    .bind(filter.created_before)
    .bind(&filter.payload_contains)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(records)
}

/// Count events for an instance with filtering.
pub async fn count_events(
    pool: &PgPool,
    instance_id: &str,
    filter: &ListEventsFilter,
) -> Result<i64, CoreError> {
    let count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM instance_events
        WHERE instance_id = $1
          AND ($2::TEXT IS NULL OR event_type::text = $2)
          AND ($3::TEXT IS NULL OR subtype = $3)
          AND ($4::TIMESTAMPTZ IS NULL OR created_at >= $4)
          AND ($5::TIMESTAMPTZ IS NULL OR created_at < $5)
          AND ($6::TEXT IS NULL OR (
              payload IS NOT NULL
              AND convert_from(payload, 'UTF8') ILIKE '%' || $6 || '%'
          ))
        "#,
    )
    .bind(instance_id)
    .bind(&filter.event_type)
    .bind(&filter.subtype)
    .bind(filter.created_after)
    .bind(filter.created_before)
    .bind(&filter.payload_contains)
    .fetch_one(pool)
    .await?;

    Ok(count.0)
}

// ============================================================================
// Signal Operations
// ============================================================================

/// Insert or update a pending signal.
/// Uses ON CONFLICT to replace existing signal for the same instance.
pub async fn insert_signal(
    pool: &PgPool,
    instance_id: &str,
    signal_type: &str,
    payload: &[u8],
) -> Result<(), CoreError> {
    let payload_opt = if payload.is_empty() {
        None
    } else {
        Some(payload)
    };

    sqlx::query(
        r#"
        INSERT INTO pending_signals (instance_id, signal_type, payload, created_at)
        VALUES ($1, $2::signal_type, $3, NOW())
        ON CONFLICT (instance_id) DO UPDATE
        SET signal_type = EXCLUDED.signal_type,
            payload = EXCLUDED.payload,
            created_at = NOW(),
            acknowledged_at = NULL
        "#,
    )
    .bind(instance_id)
    .bind(signal_type)
    .bind(payload_opt)
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert or update a pending custom signal scoped to a checkpoint.
pub async fn insert_custom_signal(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
    payload: &[u8],
) -> Result<(), CoreError> {
    let payload_opt = if payload.is_empty() {
        None
    } else {
        Some(payload)
    };

    sqlx::query(
        r#"
        INSERT INTO pending_checkpoint_signals (instance_id, checkpoint_id, payload, created_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (instance_id, checkpoint_id) DO UPDATE
        SET payload = EXCLUDED.payload,
            created_at = NOW()
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .bind(payload_opt)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get pending signal for an instance (not yet acknowledged).
pub async fn get_pending_signal(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<SignalRecord>, CoreError> {
    let record = sqlx::query_as::<_, SignalRecord>(
        r#"
        SELECT instance_id, signal_type::text as signal_type, payload, created_at, acknowledged_at
        FROM pending_signals
        WHERE instance_id = $1 AND acknowledged_at IS NULL
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await?;

    Ok(record)
}

/// Take a pending custom signal for a checkpoint (delete and return it).
pub async fn take_pending_custom_signal(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
) -> Result<Option<CustomSignalRecord>, CoreError> {
    let record = sqlx::query_as::<_, CustomSignalRecord>(
        r#"
        DELETE FROM pending_checkpoint_signals
        WHERE instance_id = $1 AND checkpoint_id = $2
        RETURNING instance_id, checkpoint_id, payload, created_at
        "#,
    )
    .bind(instance_id)
    .bind(checkpoint_id)
    .fetch_optional(pool)
    .await?;

    Ok(record)
}

/// Acknowledge a signal.
pub async fn acknowledge_signal(pool: &PgPool, instance_id: &str) -> Result<(), CoreError> {
    sqlx::query(
        r#"
        UPDATE pending_signals
        SET acknowledged_at = NOW()
        WHERE instance_id = $1 AND acknowledged_at IS NULL
        "#,
    )
    .bind(instance_id)
    .execute(pool)
    .await?;

    Ok(())
}

// ============================================================================
// Health Operations
// ============================================================================

/// Count active instances (running or suspended).
pub async fn count_active_instances(pool: &PgPool) -> Result<i64, CoreError> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM instances
        WHERE status IN ('running', 'suspended')
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.0)
}

// ============================================================================
// Sleep Operations
// ============================================================================

/// Set the sleep_until timestamp for an instance.
pub async fn set_instance_sleep(
    pool: &PgPool,
    instance_id: &str,
    sleep_until: DateTime<Utc>,
) -> Result<(), CoreError> {
    let result = sqlx::query(
        r#"
        UPDATE instances
        SET sleep_until = $2
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .bind(sleep_until)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }

    Ok(())
}

/// Clear the sleep_until timestamp for an instance.
pub async fn clear_instance_sleep(pool: &PgPool, instance_id: &str) -> Result<(), CoreError> {
    let result = sqlx::query(
        r#"
        UPDATE instances
        SET sleep_until = NULL
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(CoreError::InstanceNotFound {
            instance_id: instance_id.to_string(),
        });
    }

    Ok(())
}

/// Get instances that are due to wake (sleep_until <= now).
pub async fn get_sleeping_instances_due(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<InstanceRecord>, CoreError> {
    let records = sqlx::query_as::<_, InstanceRecord>(
        r#"
        SELECT instance_id, tenant_id, definition_version,
               status::text as status, checkpoint_id, attempt, max_attempts,
               created_at, started_at, finished_at, output, error, sleep_until
        FROM instances
        WHERE sleep_until IS NOT NULL
          AND sleep_until <= NOW()
          AND status = 'suspended'
        ORDER BY sleep_until ASC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(records)
}

#[async_trait::async_trait]
impl Persistence for PostgresPersistence {
    async fn register_instance(&self, instance_id: &str, tenant_id: &str) -> Result<(), CoreError> {
        create_instance(&self.pool, instance_id, tenant_id).await
    }

    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError> {
        get_instance(&self.pool, instance_id).await
    }

    async fn update_instance_status(
        &self,
        instance_id: &str,
        status: &str,
        started_at: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError> {
        update_instance_status(&self.pool, instance_id, status, started_at).await
    }

    async fn update_instance_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), CoreError> {
        update_instance_checkpoint(&self.pool, instance_id, checkpoint_id).await
    }

    async fn complete_instance(
        &self,
        instance_id: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
    ) -> Result<(), CoreError> {
        complete_instance(&self.pool, instance_id, output, error).await
    }

    async fn save_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<(), CoreError> {
        save_checkpoint(&self.pool, instance_id, checkpoint_id, state).await
    }

    async fn load_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CheckpointRecord>, CoreError> {
        load_checkpoint(&self.pool, instance_id, checkpoint_id).await
    }

    async fn list_checkpoints(
        &self,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        limit: i64,
        offset: i64,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<CheckpointRecord>, CoreError> {
        list_checkpoints(
            &self.pool,
            instance_id,
            checkpoint_id,
            limit,
            offset,
            created_after,
            created_before,
        )
        .await
    }

    async fn count_checkpoints(
        &self,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<i64, CoreError> {
        count_checkpoints(
            &self.pool,
            instance_id,
            checkpoint_id,
            created_after,
            created_before,
        )
        .await
    }

    async fn insert_event(&self, event: &EventRecord) -> Result<(), CoreError> {
        insert_event(&self.pool, event).await
    }

    async fn insert_signal(
        &self,
        instance_id: &str,
        signal_type: &str,
        payload: &[u8],
    ) -> Result<(), CoreError> {
        insert_signal(&self.pool, instance_id, signal_type, payload).await
    }

    async fn get_pending_signal(
        &self,
        instance_id: &str,
    ) -> Result<Option<SignalRecord>, CoreError> {
        get_pending_signal(&self.pool, instance_id).await
    }

    async fn acknowledge_signal(&self, instance_id: &str) -> Result<(), CoreError> {
        acknowledge_signal(&self.pool, instance_id).await
    }

    async fn insert_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        payload: &[u8],
    ) -> Result<(), CoreError> {
        insert_custom_signal(&self.pool, instance_id, checkpoint_id, payload).await
    }

    async fn take_pending_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CustomSignalRecord>, CoreError> {
        take_pending_custom_signal(&self.pool, instance_id, checkpoint_id).await
    }

    async fn save_retry_attempt(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        attempt: i32,
        error_message: Option<&str>,
    ) -> Result<(), CoreError> {
        save_retry_attempt(
            &self.pool,
            instance_id,
            checkpoint_id,
            attempt,
            error_message,
        )
        .await
    }

    async fn list_instances(
        &self,
        tenant_id: Option<&str>,
        status: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        list_instances(&self.pool, tenant_id, status, limit, offset).await
    }

    async fn health_check_db(&self) -> Result<bool, CoreError> {
        health_check_db(&self.pool).await
    }

    async fn count_active_instances(&self) -> Result<i64, CoreError> {
        count_active_instances(&self.pool).await
    }

    async fn set_instance_sleep(
        &self,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        set_instance_sleep(&self.pool, instance_id, sleep_until).await
    }

    async fn clear_instance_sleep(&self, instance_id: &str) -> Result<(), CoreError> {
        clear_instance_sleep(&self.pool, instance_id).await
    }

    async fn get_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        get_sleeping_instances_due(&self.pool, limit).await
    }

    async fn list_events(
        &self,
        instance_id: &str,
        filter: &ListEventsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<EventRecord>, CoreError> {
        list_events(&self.pool, instance_id, filter, limit, offset).await
    }

    async fn count_events(
        &self,
        instance_id: &str,
        filter: &ListEventsFilter,
    ) -> Result<i64, CoreError> {
        count_events(&self.pool, instance_id, filter).await
    }
}

/// Check database health.
pub async fn health_check_db(pool: &PgPool) -> Result<bool, CoreError> {
    let result: Result<(i32,), _> = sqlx::query_as("SELECT 1").fetch_one(pool).await;
    Ok(result.is_ok())
}

/// List instances with optional filtering.
pub async fn list_instances(
    pool: &PgPool,
    tenant_id: Option<&str>,
    status_filter: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<InstanceRecord>, CoreError> {
    let mut query = String::from(
        r#"
        SELECT instance_id, tenant_id, definition_version,
               status::text as status, checkpoint_id, attempt, max_attempts,
               created_at, started_at, finished_at, output, error, sleep_until
        FROM instances
        WHERE 1=1
        "#,
    );

    let mut params: Vec<String> = Vec::new();
    let mut param_idx = 1;

    if let Some(tid) = tenant_id {
        query.push_str(&format!(" AND tenant_id = ${}", param_idx));
        params.push(tid.to_string());
        param_idx += 1;
    }

    if let Some(status) = status_filter {
        query.push_str(&format!(" AND status::text = ${}", param_idx));
        params.push(status.to_string());
        param_idx += 1;
    }

    query.push_str(&format!(
        " ORDER BY created_at DESC LIMIT ${} OFFSET ${}",
        param_idx,
        param_idx + 1
    ));

    // Build and execute the query dynamically
    let mut sqlx_query = sqlx::query_as::<_, InstanceRecord>(&query);

    for param in &params {
        sqlx_query = sqlx_query.bind(param);
    }
    sqlx_query = sqlx_query.bind(limit).bind(offset);

    let records = sqlx_query.fetch_all(pool).await?;

    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/postgresql");

    // Helper to get a test database pool
    async fn test_pool() -> Option<PgPool> {
        let url = std::env::var("TEST_DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url).await.ok()?;
        MIGRATOR.run(&pool).await.ok()?;
        Some(pool)
    }

    // Helper to create a test instance
    async fn create_test_instance(pool: &PgPool, instance_id: Uuid, tenant_id: &str) {
        sqlx::query(
            r#"
            INSERT INTO instances (instance_id, tenant_id, definition_version, status)
            VALUES ($1, $2, 1, 'pending')
            "#,
        )
        .bind(instance_id)
        .bind(tenant_id)
        .execute(pool)
        .await
        .expect("Failed to create test instance");
    }

    // Helper to clean up test data
    async fn cleanup_test_instance(pool: &PgPool, instance_id: Uuid) {
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(instance_id)
            .execute(pool)
            .await
            .ok();
    }

    #[tokio::test]
    async fn test_insert_and_get_instance() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let result = get_instance(&pool, &instance_id.to_string()).await;
        assert!(result.is_ok());
        let instance = result.unwrap();
        assert!(instance.is_some());
        let instance = instance.unwrap();
        assert_eq!(instance.tenant_id, "test-tenant");
        assert_eq!(instance.status, "pending");

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_update_instance_status() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let result =
            update_instance_status(&pool, &instance_id.to_string(), "running", Some(Utc::now()))
                .await;
        assert!(result.is_ok());

        let instance = get_instance(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, "running");
        assert!(instance.started_at.is_some());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_update_instance_checkpoint() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let result =
            update_instance_checkpoint(&pool, &instance_id.to_string(), "checkpoint-1").await;
        assert!(result.is_ok());

        let instance = get_instance(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.checkpoint_id, Some("checkpoint-1".to_string()));

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_complete_instance_success() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let output_data = b"success output";
        let result =
            complete_instance(&pool, &instance_id.to_string(), Some(output_data), None).await;
        assert!(result.is_ok());

        let instance = get_instance(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, "completed");
        assert_eq!(instance.output, Some(output_data.to_vec()));
        assert!(instance.finished_at.is_some());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_complete_instance_failure() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let result =
            complete_instance(&pool, &instance_id.to_string(), None, Some("test error")).await;
        assert!(result.is_ok());

        let instance = get_instance(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, "failed");
        assert_eq!(instance.error, Some("test error".to_string()));
        assert!(instance.finished_at.is_some());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_save_checkpoint_new() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let state = b"test state data";
        let result = save_checkpoint(&pool, &instance_id.to_string(), "cp-1", state).await;
        assert!(result.is_ok());

        let checkpoint = load_checkpoint(&pool, &instance_id.to_string(), "cp-1")
            .await
            .unwrap();
        assert!(checkpoint.is_some());
        assert_eq!(checkpoint.unwrap().state, state.to_vec());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_save_checkpoint_duplicate() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        // Save first checkpoint
        save_checkpoint(&pool, &instance_id.to_string(), "cp-1", b"state-1")
            .await
            .unwrap();

        // Save again with same ID (should update)
        save_checkpoint(&pool, &instance_id.to_string(), "cp-1", b"state-2")
            .await
            .unwrap();

        let checkpoint = load_checkpoint(&pool, &instance_id.to_string(), "cp-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.state, b"state-2".to_vec());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_load_checkpoint_by_id() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        save_checkpoint(&pool, &instance_id.to_string(), "cp-1", b"state-1")
            .await
            .unwrap();
        save_checkpoint(&pool, &instance_id.to_string(), "cp-2", b"state-2")
            .await
            .unwrap();

        let cp1 = load_checkpoint(&pool, &instance_id.to_string(), "cp-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cp1.state, b"state-1".to_vec());

        let cp2 = load_checkpoint(&pool, &instance_id.to_string(), "cp-2")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cp2.state, b"state-2".to_vec());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_load_checkpoint_latest() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        save_checkpoint(&pool, &instance_id.to_string(), "cp-1", b"state-1")
            .await
            .unwrap();
        // Small delay to ensure different timestamps
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        save_checkpoint(&pool, &instance_id.to_string(), "cp-2", b"state-2")
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        save_checkpoint(&pool, &instance_id.to_string(), "cp-3", b"state-3")
            .await
            .unwrap();

        let latest = load_latest_checkpoint(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.checkpoint_id, "cp-3");
        assert_eq!(latest.state, b"state-3".to_vec());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_load_checkpoint_not_found() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let result = load_checkpoint(&pool, &instance_id.to_string(), "nonexistent")
            .await
            .unwrap();
        assert!(result.is_none());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_schedule_wake() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let wake_at = Utc::now() + chrono::Duration::hours(1);
        let result = schedule_wake(&pool, &instance_id.to_string(), "cp-wake", wake_at).await;
        assert!(result.is_ok());

        let entry = get_wake_entry(&pool, &instance_id.to_string())
            .await
            .unwrap();
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().checkpoint_id, "cp-wake");

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_schedule_wake_upsert() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let wake_at1 = Utc::now() + chrono::Duration::hours(1);
        schedule_wake(&pool, &instance_id.to_string(), "cp-1", wake_at1)
            .await
            .unwrap();

        let wake_at2 = Utc::now() + chrono::Duration::hours(2);
        schedule_wake(&pool, &instance_id.to_string(), "cp-2", wake_at2)
            .await
            .unwrap();

        let entry = get_wake_entry(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.checkpoint_id, "cp-2");

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_clear_wake() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let wake_at = Utc::now() + chrono::Duration::hours(1);
        schedule_wake(&pool, &instance_id.to_string(), "cp-wake", wake_at)
            .await
            .unwrap();

        clear_wake(&pool, &instance_id.to_string()).await.unwrap();

        let entry = get_wake_entry(&pool, &instance_id.to_string())
            .await
            .unwrap();
        assert!(entry.is_none());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_insert_event() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let event = EventRecord {
            id: None,
            instance_id: instance_id.to_string(),
            event_type: "started".to_string(),
            checkpoint_id: None,
            payload: None,
            created_at: Utc::now(),
            subtype: None,
        };

        let result = insert_event(&pool, &event).await;
        assert!(result.is_ok());

        // Verify event was inserted
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM instance_events WHERE instance_id = $1")
                .bind(instance_id.to_string())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1);

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_insert_signal() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let result = insert_signal(&pool, &instance_id.to_string(), "cancel", b"reason").await;
        assert!(result.is_ok());

        let signal = get_pending_signal(&pool, &instance_id.to_string())
            .await
            .unwrap();
        assert!(signal.is_some());
        let signal = signal.unwrap();
        assert_eq!(signal.signal_type, "cancel");
        assert_eq!(signal.payload, Some(b"reason".to_vec()));

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_get_pending_signal() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        insert_signal(&pool, &instance_id.to_string(), "pause", b"")
            .await
            .unwrap();

        let signal = get_pending_signal(&pool, &instance_id.to_string())
            .await
            .unwrap();
        assert!(signal.is_some());
        assert_eq!(signal.unwrap().signal_type, "pause");

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_get_pending_signal_none() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let signal = get_pending_signal(&pool, &instance_id.to_string())
            .await
            .unwrap();
        assert!(signal.is_none());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_acknowledge_signal() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        insert_signal(&pool, &instance_id.to_string(), "cancel", b"")
            .await
            .unwrap();
        acknowledge_signal(&pool, &instance_id.to_string())
            .await
            .unwrap();

        // Should no longer return as pending
        let signal = get_pending_signal(&pool, &instance_id.to_string())
            .await
            .unwrap();
        assert!(signal.is_none());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_insert_and_take_custom_signal() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        insert_custom_signal(&pool, &instance_id.to_string(), "wait-1", b"custom-payload")
            .await
            .unwrap();

        // First take should retrieve and delete
        let signal = take_pending_custom_signal(&pool, &instance_id.to_string(), "wait-1")
            .await
            .unwrap()
            .expect("custom signal should exist");
        assert_eq!(signal.checkpoint_id, "wait-1");
        assert_eq!(signal.payload.unwrap(), b"custom-payload".to_vec());

        // Second take should return none
        let signal = take_pending_custom_signal(&pool, &instance_id.to_string(), "wait-1")
            .await
            .unwrap();
        assert!(signal.is_none());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_count_active_instances() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let instance1 = Uuid::new_v4();
        let instance2 = Uuid::new_v4();
        create_test_instance(&pool, instance1, "test-tenant").await;
        create_test_instance(&pool, instance2, "test-tenant").await;

        // Set one to running, one to suspended
        update_instance_status(&pool, &instance1.to_string(), "running", None)
            .await
            .unwrap();
        update_instance_status(&pool, &instance2.to_string(), "suspended", None)
            .await
            .unwrap();

        let count = count_active_instances(&pool).await.unwrap();
        assert!(count >= 2); // At least our 2 test instances

        cleanup_test_instance(&pool, instance1).await;
        cleanup_test_instance(&pool, instance2).await;
    }

    #[tokio::test]
    async fn test_health_check_db() {
        let Some(pool) = test_pool().await else {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
            return;
        };

        let result = health_check_db(&pool).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
