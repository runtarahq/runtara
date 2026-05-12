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
    CheckpointRecord, CompleteInstanceParams, CustomSignalRecord, EventRecord, InstanceRecord,
    ListEventsFilter, ListStepSummariesFilter, Persistence, SignalRecord, StepSummaryRecord,
    WakeEntry,
};

// ============================================================================
// Shared Operations (SYN-394)
// ============================================================================
// The instance + sleep families live in crate::persistence::common::ops and
// are materialized onto PostgresPersistence via the macros below. The inline
// free functions they replaced have been removed; callers in this module's
// tests (see the `tests` submodule) reach the shared ops through
// `PostgresPersistence::op_*` instead.

crate::persistence::common::ops::impl_instance_ops!(
    PostgresPersistence,
    PgPool,
    crate::persistence::dialect::PostgresDialect
);
crate::persistence::common::ops::impl_sleep_ops!(
    PostgresPersistence,
    PgPool,
    crate::persistence::dialect::PostgresDialect
);
crate::persistence::common::ops::impl_checkpoint_ops!(
    PostgresPersistence,
    PgPool,
    crate::persistence::dialect::PostgresDialect
);
crate::persistence::common::ops::impl_signal_ops!(
    PostgresPersistence,
    PgPool,
    crate::persistence::dialect::PostgresDialect
);
crate::persistence::common::ops::impl_event_ops!(
    PostgresPersistence,
    PgPool,
    crate::persistence::dialect::PostgresDialect
);
crate::persistence::common::ops::impl_step_summary_ops!(
    PostgresPersistence,
    PgPool,
    crate::persistence::dialect::PostgresDialect
);
crate::persistence::common::ops::impl_retention_ops!(
    PostgresPersistence,
    PgPool,
    crate::persistence::dialect::PostgresDialect
);

// ============================================================================
// Remaining Instance Operations (pre-shared — migrated in later phases)
// ============================================================================

/// UUID used for self-registered instances (no image/definition).
/// This is a well-known UUID that indicates the instance registered itself.
pub const SELF_REGISTERED_DEFINITION_ID: Uuid = Uuid::from_u128(0);

/// Update execution metrics for an instance.
///
/// Stores cgroup-collected resource usage metrics (memory, CPU) after container execution.
/// Only updates if metrics are not already set (first writer wins).
pub async fn update_instance_metrics(
    pool: &PgPool,
    instance_id: &str,
    memory_peak_bytes: Option<u64>,
    cpu_usage_usec: Option<u64>,
) -> Result<(), CoreError> {
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

/// Update instance stderr (raw container stderr output).
///
/// Stores stderr from container execution for debugging/logging purposes.
/// Only updates if stderr is not already set (first writer wins).
pub async fn update_instance_stderr(
    pool: &PgPool,
    instance_id: &str,
    stderr: &str,
) -> Result<(), CoreError> {
    sqlx::query(
        r#"
        UPDATE instances
        SET stderr = COALESCE(stderr, $2)
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .bind(stderr)
    .execute(pool)
    .await?;

    Ok(())
}

// `store_instance_input` is migrated to the shared layer:
// see PostgresPersistence::op_store_instance_input (crate::persistence::common::ops::instances).

// ============================================================================
// Checkpoint Operations
// ============================================================================
// `save_checkpoint`, `load_checkpoint`, `list_checkpoints`, `count_checkpoints`
// are migrated to the shared layer:
// see PostgresPersistence::op_save_checkpoint / op_load_checkpoint /
// op_list_checkpoints / op_count_checkpoints
// (crate::persistence::common::ops::checkpoints).

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
        INSERT INTO instance_events (
            instance_id,
            event_type,
            checkpoint_id,
            payload,
            payload_json,
            created_at,
            subtype
        )
        VALUES ($1, $2::instance_event_type, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(&event.instance_id)
    .bind(&event.event_type)
    .bind(&event.checkpoint_id)
    .bind(&event.payload)
    .bind(&event.payload_json)
    .bind(event.created_at)
    .bind(&event.subtype)
    .execute(pool)
    .await?;

    Ok(())
}

// `list_events`, `count_events`, `list_step_summaries`, `count_step_summaries`
// are migrated to the shared layer:
// see PostgresPersistence::op_list_events / op_count_events /
// op_list_step_summaries / op_count_step_summaries
// (crate::persistence::common::ops::{events, step_summaries}).

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

// `get_pending_signal`, `acknowledge_signal`, `take_pending_custom_signal`
// are migrated to the shared layer:
// see PostgresPersistence::op_get_pending_signal / op_acknowledge_signal /
// op_take_pending_custom_signal (crate::persistence::common::ops::signals).

// Health, sleep, and active-count operations are migrated to the shared layer:
// see PostgresPersistence::op_health_check_db, op_count_active_instances,
// op_set_instance_sleep, op_clear_instance_sleep, op_get_sleeping_instances_due
// (crate::persistence::common::ops::{instances, sleep}).

#[async_trait::async_trait]
impl Persistence for PostgresPersistence {
    async fn register_instance(&self, instance_id: &str, tenant_id: &str) -> Result<(), CoreError> {
        Self::op_register_instance(&self.pool, instance_id, tenant_id).await
    }

    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError> {
        Self::op_get_instance(&self.pool, instance_id).await
    }

    async fn update_instance_status(
        &self,
        instance_id: &str,
        status: &str,
        started_at: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError> {
        Self::op_update_instance_status(&self.pool, instance_id, status, started_at).await
    }

    async fn update_instance_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), CoreError> {
        Self::op_update_instance_checkpoint(&self.pool, instance_id, checkpoint_id).await
    }

    async fn complete_instance(
        &self,
        params: CompleteInstanceParams<'_>,
    ) -> Result<bool, CoreError> {
        Self::op_complete_instance_unified(&self.pool, params).await
    }

    async fn save_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<(), CoreError> {
        Self::op_save_checkpoint(&self.pool, instance_id, checkpoint_id, state).await
    }

    async fn load_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CheckpointRecord>, CoreError> {
        Self::op_load_checkpoint(&self.pool, instance_id, checkpoint_id).await
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
        Self::op_list_checkpoints(
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
        Self::op_count_checkpoints(
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
        Self::op_get_pending_signal(&self.pool, instance_id).await
    }

    async fn acknowledge_signal(&self, instance_id: &str) -> Result<(), CoreError> {
        Self::op_acknowledge_signal(&self.pool, instance_id).await
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
        Self::op_take_pending_custom_signal(&self.pool, instance_id, checkpoint_id).await
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
        Self::op_list_instances(&self.pool, tenant_id, status, limit, offset).await
    }

    async fn health_check_db(&self) -> Result<bool, CoreError> {
        Self::op_health_check_db(&self.pool).await
    }

    async fn count_active_instances(&self) -> Result<i64, CoreError> {
        Self::op_count_active_instances(&self.pool).await
    }

    async fn set_instance_sleep(
        &self,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        Self::op_set_instance_sleep(&self.pool, instance_id, sleep_until).await
    }

    async fn clear_instance_sleep(&self, instance_id: &str) -> Result<(), CoreError> {
        Self::op_clear_instance_sleep(&self.pool, instance_id).await
    }

    async fn get_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        Self::op_get_sleeping_instances_due(&self.pool, limit).await
    }

    async fn list_events(
        &self,
        instance_id: &str,
        filter: &ListEventsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<EventRecord>, CoreError> {
        Self::op_list_events(&self.pool, instance_id, filter, limit, offset).await
    }

    async fn count_events(
        &self,
        instance_id: &str,
        filter: &ListEventsFilter,
    ) -> Result<i64, CoreError> {
        Self::op_count_events(&self.pool, instance_id, filter).await
    }

    async fn list_step_summaries(
        &self,
        instance_id: &str,
        filter: &ListStepSummariesFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<StepSummaryRecord>, CoreError> {
        Self::op_list_step_summaries(&self.pool, instance_id, filter, limit, offset).await
    }

    async fn count_step_summaries(
        &self,
        instance_id: &str,
        filter: &ListStepSummariesFilter,
    ) -> Result<i64, CoreError> {
        Self::op_count_step_summaries(&self.pool, instance_id, filter).await
    }

    async fn update_instance_metrics(
        &self,
        instance_id: &str,
        memory_peak_bytes: Option<u64>,
        cpu_usage_usec: Option<u64>,
    ) -> Result<(), CoreError> {
        update_instance_metrics(&self.pool, instance_id, memory_peak_bytes, cpu_usage_usec).await
    }

    async fn update_instance_stderr(
        &self,
        instance_id: &str,
        stderr: &str,
    ) -> Result<(), CoreError> {
        update_instance_stderr(&self.pool, instance_id, stderr).await
    }

    async fn store_instance_input(&self, instance_id: &str, input: &[u8]) -> Result<(), CoreError> {
        Self::op_store_instance_input(&self.pool, instance_id, input).await
    }

    async fn get_terminal_instances_older_than(
        &self,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<String>, CoreError> {
        Self::op_get_terminal_instances_older_than(&self.pool, older_than, limit).await
    }

    async fn delete_instances_batch(&self, instance_ids: &[String]) -> Result<u64, CoreError> {
        Self::op_delete_instances_batch(&self.pool, instance_ids).await
    }
}

// `get_terminal_instances_older_than`, `delete_instances_batch`,
// `list_instances` are migrated to the shared layer:
// see PostgresPersistence::op_get_terminal_instances_older_than /
// op_delete_instances_batch / op_list_instances
// (crate::persistence::common::ops::{retention, instances}).

#[cfg(test)]
mod tests {
    use super::*;

    use crate::migrations::POSTGRES as MIGRATOR;

    // Helper to get a test database pool
    async fn test_pool() -> Option<PgPool> {
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL").ok()?;
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

        let result = PostgresPersistence::op_get_instance(&pool, &instance_id.to_string()).await;
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

        let result = PostgresPersistence::op_update_instance_status(
            &pool,
            &instance_id.to_string(),
            "running",
            Some(Utc::now()),
        )
        .await;
        assert!(result.is_ok());

        let instance = PostgresPersistence::op_get_instance(&pool, &instance_id.to_string())
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

        let result = PostgresPersistence::op_update_instance_checkpoint(
            &pool,
            &instance_id.to_string(),
            "checkpoint-1",
        )
        .await;
        assert!(result.is_ok());

        let instance = PostgresPersistence::op_get_instance(&pool, &instance_id.to_string())
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
        let instance_id_str = instance_id.to_string();
        let result = PostgresPersistence::op_complete_instance_unified(
            &pool,
            CompleteInstanceParams::new(&instance_id_str, "completed").with_output(output_data),
        )
        .await;
        assert!(result.is_ok());

        let instance = PostgresPersistence::op_get_instance(&pool, &instance_id.to_string())
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

        let instance_id_str = instance_id.to_string();
        let result = PostgresPersistence::op_complete_instance_unified(
            &pool,
            CompleteInstanceParams::new(&instance_id_str, "failed").with_error("test error"),
        )
        .await;
        assert!(result.is_ok());

        let instance = PostgresPersistence::op_get_instance(&pool, &instance_id.to_string())
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
        let result =
            PostgresPersistence::op_save_checkpoint(&pool, &instance_id.to_string(), "cp-1", state)
                .await;
        assert!(result.is_ok());

        let checkpoint =
            PostgresPersistence::op_load_checkpoint(&pool, &instance_id.to_string(), "cp-1")
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
        PostgresPersistence::op_save_checkpoint(
            &pool,
            &instance_id.to_string(),
            "cp-1",
            b"state-1",
        )
        .await
        .unwrap();

        // Save again with same ID (should update)
        PostgresPersistence::op_save_checkpoint(
            &pool,
            &instance_id.to_string(),
            "cp-1",
            b"state-2",
        )
        .await
        .unwrap();

        let checkpoint =
            PostgresPersistence::op_load_checkpoint(&pool, &instance_id.to_string(), "cp-1")
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

        PostgresPersistence::op_save_checkpoint(
            &pool,
            &instance_id.to_string(),
            "cp-1",
            b"state-1",
        )
        .await
        .unwrap();
        PostgresPersistence::op_save_checkpoint(
            &pool,
            &instance_id.to_string(),
            "cp-2",
            b"state-2",
        )
        .await
        .unwrap();

        let cp1 = PostgresPersistence::op_load_checkpoint(&pool, &instance_id.to_string(), "cp-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cp1.state, b"state-1".to_vec());

        let cp2 = PostgresPersistence::op_load_checkpoint(&pool, &instance_id.to_string(), "cp-2")
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

        PostgresPersistence::op_save_checkpoint(
            &pool,
            &instance_id.to_string(),
            "cp-1",
            b"state-1",
        )
        .await
        .unwrap();
        // Small delay to ensure different timestamps
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        PostgresPersistence::op_save_checkpoint(
            &pool,
            &instance_id.to_string(),
            "cp-2",
            b"state-2",
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        PostgresPersistence::op_save_checkpoint(
            &pool,
            &instance_id.to_string(),
            "cp-3",
            b"state-3",
        )
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

        let result =
            PostgresPersistence::op_load_checkpoint(&pool, &instance_id.to_string(), "nonexistent")
                .await
                .unwrap();
        assert!(result.is_none());

        cleanup_test_instance(&pool, instance_id).await;
    }

    // NOTE: Legacy wake_queue tests removed - wake scheduling now uses sleep_until column
    // on the instances table (see migration 003_drop_wake_queue.sql)

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
            payload_json: None,
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

        let signal = PostgresPersistence::op_get_pending_signal(&pool, &instance_id.to_string())
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

        let signal = PostgresPersistence::op_get_pending_signal(&pool, &instance_id.to_string())
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

        let signal = PostgresPersistence::op_get_pending_signal(&pool, &instance_id.to_string())
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
        PostgresPersistence::op_acknowledge_signal(&pool, &instance_id.to_string())
            .await
            .unwrap();

        // Should no longer return as pending
        let signal = PostgresPersistence::op_get_pending_signal(&pool, &instance_id.to_string())
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
        let signal = PostgresPersistence::op_take_pending_custom_signal(
            &pool,
            &instance_id.to_string(),
            "wait-1",
        )
        .await
        .unwrap()
        .expect("custom signal should exist");
        assert_eq!(signal.checkpoint_id, "wait-1");
        assert_eq!(signal.payload.unwrap(), b"custom-payload".to_vec());

        // Second take should return none
        let signal = PostgresPersistence::op_take_pending_custom_signal(
            &pool,
            &instance_id.to_string(),
            "wait-1",
        )
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
        PostgresPersistence::op_update_instance_status(
            &pool,
            &instance1.to_string(),
            "running",
            None,
        )
        .await
        .unwrap();
        PostgresPersistence::op_update_instance_status(
            &pool,
            &instance2.to_string(),
            "suspended",
            None,
        )
        .await
        .unwrap();

        let count = PostgresPersistence::op_count_active_instances(&pool)
            .await
            .unwrap();
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

        let result = PostgresPersistence::op_health_check_db(&pool).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
