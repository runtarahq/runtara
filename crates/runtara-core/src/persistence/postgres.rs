// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Persistence operations for runtara-core.
//!
//! Provides all durable storage access functions for instances, checkpoints, events, and signals.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::CoreError;
use crate::observability::{
    InstanceCompletionMetrics, is_recorded_terminal_status, record_instance_completion,
    record_instance_resources,
};

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

#[derive(Debug, sqlx::FromRow)]
struct InstanceMetricRow {
    tenant_id: String,
    status: String,
    termination_reason: Option<String>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    memory_peak_bytes: Option<i64>,
    cpu_usage_usec: Option<i64>,
}

impl From<InstanceMetricRow> for InstanceCompletionMetrics {
    fn from(row: InstanceMetricRow) -> Self {
        Self {
            tenant_id: row.tenant_id,
            status: row.status,
            termination_reason: row.termination_reason,
            started_at: row.started_at,
            finished_at: row.finished_at,
            memory_peak_bytes: row.memory_peak_bytes.and_then(|v| u64::try_from(v).ok()),
            cpu_usage_usec: row.cpu_usage_usec.and_then(|v| u64::try_from(v).ok()),
        }
    }
}

async fn fetch_instance_status(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT status::text
        FROM instances
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await
}

async fn fetch_instance_metric_row(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<InstanceMetricRow>, sqlx::Error> {
    sqlx::query_as::<_, InstanceMetricRow>(
        r#"
        SELECT
            tenant_id,
            status::text AS status,
            termination_reason::text AS termination_reason,
            started_at,
            finished_at,
            memory_peak_bytes,
            cpu_usage_usec
        FROM instances
        WHERE instance_id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await
}

async fn record_completion_from_db(pool: &PgPool, instance_id: &str) {
    match fetch_instance_metric_row(pool, instance_id).await {
        Ok(Some(row)) => record_instance_completion(&row.into()),
        Ok(None) => tracing::warn!(
            instance_id = %instance_id,
            "Skipped OTLP workflow completion metric because instance row was not found"
        ),
        Err(error) => tracing::warn!(
            instance_id = %instance_id,
            error = %error,
            "Skipped OTLP workflow completion metric"
        ),
    }
}

async fn record_resources_from_db(pool: &PgPool, instance_id: &str) {
    match fetch_instance_metric_row(pool, instance_id).await {
        Ok(Some(row)) => record_instance_resources(&row.into()),
        Ok(None) => tracing::warn!(
            instance_id = %instance_id,
            "Skipped OTLP workflow resource metric because instance row was not found"
        ),
        Err(error) => tracing::warn!(
            instance_id = %instance_id,
            error = %error,
            "Skipped OTLP workflow resource metric"
        ),
    }
}

// ============================================================================
// Record Types
// ============================================================================

use super::{
    CheckpointRecord, CompleteInstanceParams, CustomSignalRecord, EventRecord, InstanceRecord,
    ListEventsFilter, ListStepSummariesFilter, Persistence, SignalRecord, StepSummaryRecord,
};

// ============================================================================
// Shared Operations
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
        let instance_id = params.instance_id.to_string();
        let target_status = params.status.to_string();
        let previous_was_terminal = match fetch_instance_status(&self.pool, &instance_id).await {
            Ok(Some(status)) => is_recorded_terminal_status(&status),
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(
                    instance_id = %instance_id,
                    error = %error,
                    "Could not read previous instance status before OTLP metric recording"
                );
                false
            }
        };

        let applied = Self::op_complete_instance_unified(&self.pool, params).await?;
        if applied && is_recorded_terminal_status(&target_status) && !previous_was_terminal {
            record_completion_from_db(&self.pool, &instance_id).await;
        }

        Ok(applied)
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

    async fn claim_sleeping_instance(&self, instance_id: &str) -> Result<bool, CoreError> {
        Self::op_claim_sleeping_instance(&self.pool, instance_id).await
    }

    async fn mark_for_recovery(
        &self,
        instance_id: &str,
        attempt: i32,
        marker: Option<&str>,
    ) -> Result<(), CoreError> {
        Self::op_mark_for_recovery(&self.pool, instance_id, attempt, marker).await
    }

    async fn get_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        Self::op_get_sleeping_instances_due(&self.pool, limit).await
    }

    async fn claim_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        Self::op_claim_sleeping_instances_due(&self.pool, limit).await
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
        let result =
            update_instance_metrics(&self.pool, instance_id, memory_peak_bytes, cpu_usage_usec)
                .await;

        if result.is_ok() && (memory_peak_bytes.is_some() || cpu_usage_usec.is_some()) {
            record_resources_from_db(&self.pool, instance_id).await;
        }

        result
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

    async fn delete_debug_events_older_than(
        &self,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<u64, CoreError> {
        Self::op_delete_debug_events_older_than(&self.pool, older_than, limit).await
    }
}

// `get_terminal_instances_older_than`, `delete_instances_batch`,
// `list_instances` are migrated to the shared layer:
// see PostgresPersistence::op_get_terminal_instances_older_than /
// op_delete_instances_batch / op_list_instances
// (crate::persistence::common::ops::{retention, instances}).

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests {
    use super::*;

    use crate::migrations::POSTGRES as MIGRATOR;

    // Helper to get a test database pool
    async fn test_pool() -> PgPool {
        let url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .expect("db-integration-tests requires TEST_RUNTARA_DATABASE_URL");
        let pool = PgPool::connect(&url)
            .await
            .expect("required core test database must accept connections");
        MIGRATOR
            .run(&pool)
            .await
            .expect("required core migrations must succeed");
        pool
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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        let result =
            PostgresPersistence::op_load_checkpoint(&pool, &instance_id.to_string(), "nonexistent")
                .await
                .unwrap();
        assert!(result.is_none());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_insert_event() {
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

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
        let pool = test_pool().await;

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        insert_custom_signal(&pool, &instance_id.to_string(), "wait-1", b"custom-payload")
            .await
            .unwrap();

        // First read retrieves the signal.
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

        // Reads are non-destructive: a second read (as happens on
        // replay-from-start) returns the same signal, not None — this is what
        // lets a drained/resumed WaitForSignal re-read its consumed signal
        // instead of dead-hanging.
        let signal = PostgresPersistence::op_take_pending_custom_signal(
            &pool,
            &instance_id.to_string(),
            "wait-1",
        )
        .await
        .unwrap()
        .expect("custom signal should still exist (non-destructive read)");
        assert_eq!(signal.checkpoint_id, "wait-1");
        assert_eq!(signal.payload.unwrap(), b"custom-payload".to_vec());

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_count_active_instances() {
        let pool = test_pool().await;

        let instance1 = Uuid::new_v4();
        let instance2 = Uuid::new_v4();
        create_test_instance(&pool, instance1, "test-tenant").await;
        create_test_instance(&pool, instance2, "test-tenant").await;

        // Set one running, one suspended: only the running one is counted.
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

        // Scoped to this test's own two rows. `op_count_active_instances` is a
        // database-global `COUNT(*) WHERE status = 'running'`, so a delta taken
        // across it is only stable while nothing else in the suite holds a row
        // in `running` — which the complete_instance family does, by
        // construction. Counting these two ids answers the same question and
        // cannot be moved by a concurrent test.
        let ids = [instance1.to_string(), instance2.to_string()];
        let mine_running = |pool: PgPool, ids: [String; 2]| async move {
            let (n,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM instances WHERE status = 'running' AND instance_id = ANY($1)",
            )
            .bind(&ids[..])
            .fetch_one(&pool)
            .await
            .unwrap();
            n
        };

        assert_eq!(
            mine_running(pool.clone(), ids.clone()).await,
            1,
            "exactly the running one of this test's two instances is counted"
        );

        // The global counter must see it too, whatever else is running.
        assert!(
            PostgresPersistence::op_count_active_instances(&pool)
                .await
                .unwrap()
                >= 1
        );

        // Park the running one. A suspended instance holds no concurrency slot,
        // so parked work cannot hold a cap closed against fresh registrations.
        PostgresPersistence::op_update_instance_status(
            &pool,
            &instance1.to_string(),
            "suspended",
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            mine_running(pool.clone(), ids.clone()).await,
            0,
            "suspending the running instance must free its slot"
        );

        cleanup_test_instance(&pool, instance1).await;
        cleanup_test_instance(&pool, instance2).await;
    }

    #[tokio::test]
    async fn test_health_check_db() {
        let pool = test_pool().await;

        let result = PostgresPersistence::op_health_check_db(&pool).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    // -------------------------------------------------------------------
    // Unified complete_instance coverage (SYN-395). These drive the
    // `Persistence` trait rather than the `op_*` statics so the trait's own
    // layer (previous-status read + OTLP recording around
    // `op_complete_instance_unified`) is exercised too, and so the
    // bool/`InstanceNotFound` contract is asserted where callers actually
    // see it.
    //
    // Every id is freshly generated because CI runs this suite against one
    // shared `runtara_test` database and `instance_id` is a plain TEXT
    // primary key with no ON CONFLICT on register.
    //
    // KNOWN PARALLEL-SUITE INTERACTION: four of these tests hold a row in
    // `status = 'running'` across await points, and
    // `op_count_active_instances` is a database-global
    // `COUNT(*) WHERE status = 'running'` with no tenant scope. The
    // sibling `test_count_active_instances` asserts a *delta* over that
    // global count (`after == before - 1`), so it fails whenever one of
    // these tests flips a row into or out of `running` between its two
    // reads. Measured on a live shared database: 0/20 failures with this
    // family idle, 7/20 with it running concurrently. The defect is the
    // unscoped delta assertion, not this family — a guarded update cannot
    // be exercised without a `running` row. Fix
    // `test_count_active_instances` to measure only its own rows before
    // landing these.
    // -------------------------------------------------------------------

    /// Fresh, collision-proof instance id for this family.
    fn ci_instance_id() -> String {
        format!("pg-complete-instance-{}", Uuid::new_v4())
    }

    /// String-keyed cleanup: `cleanup_test_instance` takes a `Uuid`, but
    /// this family registers through the trait with prefixed String ids.
    async fn ci_cleanup(pool: &PgPool, instance_id: &str) {
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(instance_id)
            .execute(pool)
            .await
            .ok();
    }

    /// Read the two termination columns straight from the row.
    ///
    /// `exit_code` is the reason this helper exists: `op_get_instance`'s
    /// SELECT list does not include it, and `InstanceRecord::exit_code`
    /// carries `#[sqlx(default)]`, so `get_instance(..).exit_code` is
    /// *always* `None` no matter what the column holds. Asserting through
    /// `InstanceRecord` would pass even if the COALESCE were deleted.
    /// (`termination_reason` *is* projected — as `termination_reason::text`
    /// — but is read here too so both halves of the COALESCE pair come
    /// from the same place.)
    ///
    /// `termination_reason` is a Postgres ENUM, so it is cast to text here:
    /// sqlx cannot decode a custom enum type into `String`.
    async fn ci_read_term_fields(
        pool: &PgPool,
        instance_id: &str,
    ) -> (Option<String>, Option<i32>) {
        sqlx::query_as::<_, (Option<String>, Option<i32>)>(
            "SELECT termination_reason::text, exit_code FROM instances WHERE instance_id = $1",
        )
        .bind(instance_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_complete_instance_extended() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = ci_instance_id();
        p.register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        p.complete_instance(
            CompleteInstanceParams::new(&instance_id, "completed")
                .with_output(b"output data")
                .with_stderr("stderr output")
                .with_checkpoint("final-checkpoint"),
        )
        .await
        .expect("Failed to complete instance");

        // Verify via raw query (InstanceRecord doesn't include stderr).
        // `status` is cast to text: it is an ENUM column in Postgres.
        let row: (String, Option<Vec<u8>>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT status::text, output, stderr, checkpoint_id \
             FROM instances WHERE instance_id = $1",
        )
        .bind(&instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row.0, "completed");
        assert_eq!(row.1, Some(b"output data".to_vec()));
        assert_eq!(row.2, Some("stderr output".to_string()));
        assert_eq!(row.3, Some("final-checkpoint".to_string()));

        ci_cleanup(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_complete_instance_if_running_success() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = ci_instance_id();
        p.register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();
        p.update_instance_status(&instance_id, "running", Some(Utc::now()))
            .await
            .unwrap();

        let applied = p
            .complete_instance(
                CompleteInstanceParams::new(&instance_id, "completed")
                    .if_running()
                    .with_output(b"done"),
            )
            .await
            .expect("Failed to complete instance");

        assert!(applied);

        let instance = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert_eq!(instance.status, "completed");

        ci_cleanup(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_complete_instance_if_running_skipped() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = ci_instance_id();
        p.register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();
        // Status is 'pending', not 'running'

        let applied = p
            .complete_instance(
                CompleteInstanceParams::new(&instance_id, "completed")
                    .if_running()
                    .with_output(b"done"),
            )
            .await
            .expect("Query should succeed");

        assert!(!applied);

        let instance = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert_eq!(instance.status, "pending"); // unchanged

        ci_cleanup(&pool, &instance_id).await;
    }

    /// Unguarded completion against a missing row must raise
    /// `InstanceNotFound` — callers rely on this to distinguish a truly
    /// unknown instance from a race-lost guarded update.
    #[tokio::test]
    async fn test_complete_instance_unguarded_miss_returns_instance_not_found() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        // Generated rather than a fixed literal such as "never-registered":
        // this database is shared across the whole suite, so the id must be
        // one no other test could have inserted.
        let missing = ci_instance_id();

        let err = p
            .complete_instance(CompleteInstanceParams::new(&missing, "completed"))
            .await
            .expect_err("must error when unguarded update finds nothing");

        assert!(
            matches!(&err, CoreError::InstanceNotFound { instance_id } if instance_id == &missing),
            "expected InstanceNotFound, got {err:?}"
        );
    }

    /// Unguarded completion against a live instance returns `Ok(true)`.
    #[tokio::test]
    async fn test_complete_instance_unguarded_success_returns_true() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = ci_instance_id();
        p.register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let applied = p
            .complete_instance(CompleteInstanceParams::new(&instance_id, "completed"))
            .await
            .expect("unguarded success should not error");
        assert!(applied, "unguarded update on existing row returns true");

        ci_cleanup(&pool, &instance_id).await;
    }

    /// Guarded completion against a missing row returns `Ok(false)`,
    /// not `InstanceNotFound`. This is the "row was already cleaned up"
    /// race outcome.
    #[tokio::test]
    async fn test_complete_instance_guarded_miss_returns_false() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        // Generated for the same shared-database reason as the unguarded
        // miss above.
        let missing = ci_instance_id();

        let applied = p
            .complete_instance(CompleteInstanceParams::new(&missing, "completed").if_running())
            .await
            .expect("guarded miss must not error");
        assert!(!applied);
    }

    /// Non-terminal status (`running`) must leave `finished_at` NULL —
    /// the CASE clause only fires for terminal statuses.
    #[tokio::test]
    async fn test_complete_instance_non_terminal_preserves_finished_at() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = ci_instance_id();
        p.register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let applied = p
            .complete_instance(CompleteInstanceParams::new(&instance_id, "running"))
            .await
            .expect("non-terminal transition should succeed");
        assert!(applied);

        let instance = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert_eq!(instance.status, "running");
        assert!(
            instance.finished_at.is_none(),
            "non-terminal status must not set finished_at"
        );

        ci_cleanup(&pool, &instance_id).await;
    }

    /// Relaunch/resume re-registers via
    /// `update_instance_status("running", Some(..))`. A row that ran before
    /// may carry a stale `finished_at` / `termination_reason` from a prior
    /// suspend or drain force-stop; the running transition must clear both so
    /// the resumed run never renders a negative duration
    /// (`finished_at < started_at`).
    #[tokio::test]
    async fn test_update_instance_status_running_clears_finished_at() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = ci_instance_id();
        p.register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();
        p.update_instance_status(&instance_id, "running", Some(Utc::now()))
            .await
            .unwrap();

        // Suspend the way a drain / durable sleep does: stamps finished_at +
        // termination_reason. 'sleeping' is a member of the Postgres
        // `termination_reason` ENUM, so the `$3::termination_reason` cast in
        // the unified op resolves.
        p.complete_instance(
            CompleteInstanceParams::new(&instance_id, "suspended")
                .with_termination("sleeping", None),
        )
        .await
        .expect("suspend should succeed");
        let suspended = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert!(
            suspended.finished_at.is_some(),
            "precondition: suspend stamps finished_at"
        );

        // Relaunch: re-register into running with a later started_at.
        p.update_instance_status(&instance_id, "running", Some(Utc::now()))
            .await
            .unwrap();

        let running = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert_eq!(running.status, "running");
        assert!(
            running.finished_at.is_none(),
            "relaunch into running must clear the stale finished_at"
        );

        let (reason, _code) = ci_read_term_fields(&pool, &instance_id).await;
        assert!(
            reason.is_none(),
            "relaunch into running must clear the stale termination_reason"
        );

        ci_cleanup(&pool, &instance_id).await;
    }

    /// `termination_reason` and `exit_code` use COALESCE semantics:
    /// passing `None` leaves the existing values intact. Verified via a raw
    /// query rather than `get_instance`, because `InstanceRecord::exit_code`
    /// is never populated by `op_get_instance` — see `ci_read_term_fields`.
    #[tokio::test]
    async fn test_complete_instance_termination_fields_coalesce() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = ci_instance_id();
        p.register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();
        p.update_instance_status(&instance_id, "running", Some(Utc::now()))
            .await
            .unwrap();

        // First write sets both termination fields. 'crashed' is a member of
        // the Postgres `termination_reason` ENUM.
        p.complete_instance(
            CompleteInstanceParams::new(&instance_id, "failed")
                .with_termination("crashed", Some(137)),
        )
        .await
        .expect("first completion should succeed");

        let (reason, code) = ci_read_term_fields(&pool, &instance_id).await;
        assert_eq!(reason.as_deref(), Some("crashed"));
        assert_eq!(code, Some(137));

        // Second write without termination/exit fields must not clobber.
        p.complete_instance(CompleteInstanceParams::new(&instance_id, "failed"))
            .await
            .expect("second completion should succeed");

        let (reason, code) = ci_read_term_fields(&pool, &instance_id).await;
        assert_eq!(
            reason.as_deref(),
            Some("crashed"),
            "termination_reason must be preserved across subsequent writes"
        );
        assert_eq!(
            code,
            Some(137),
            "exit_code must be preserved across subsequent writes"
        );

        ci_cleanup(&pool, &instance_id).await;
    }

    // ========================================================================
    // Miscellaneous operation tests: signals, retries, listings, metrics
    // ========================================================================
    //
    // These tests drive the `Persistence` trait rather than the `op_*` statics:
    // `insert_signal`, `insert_custom_signal`, `update_instance_metrics` and
    // `update_instance_stderr` were never migrated to `common/ops` and survive
    // as free functions in this module, so the trait is the only uniform entry
    // point across the whole family.
    //
    // Every id and tenant is uniquified per run because the integration suite
    // runs against one shared `runtara_test` database and rows survive between
    // tests; `op_register_instance` is a bare INSERT with no ON CONFLICT.

    /// Unique instance id for this family. `instances.instance_id` is TEXT, so
    /// it need not be a bare UUID.
    fn misc_instance_id(kind: &str) -> String {
        format!("pg-misc-{}-{}", kind, Uuid::new_v4())
    }

    /// Unique tenant id for this family, so tenant-scoped listings never see
    /// rows left behind by other tests.
    fn misc_tenant_id(kind: &str) -> String {
        format!("pg-misc-tenant-{}-{}", kind, Uuid::new_v4())
    }

    /// String-taking cleanup counterpart to `cleanup_test_instance` (which
    /// takes a `Uuid`). Child tables cascade on delete.
    async fn cleanup_misc_instance(pool: &PgPool, instance_id: &str) {
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(instance_id)
            .execute(pool)
            .await
            .ok();
    }

    #[tokio::test]
    async fn test_get_instance_not_found() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        // Unique id: a fixed "nonexistent" literal could be created by another
        // test against this shared database.
        let missing = misc_instance_id("absent");

        let result = p
            .get_instance(&missing)
            .await
            .expect("Query should succeed");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_checkpoints() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("list-cps");
        p.register_instance(&instance_id, &misc_tenant_id("list-cps"))
            .await
            .unwrap();

        p.save_checkpoint(&instance_id, "cp-1", b"state-1")
            .await
            .unwrap();
        p.save_checkpoint(&instance_id, "cp-2", b"state-2")
            .await
            .unwrap();
        p.save_checkpoint(&instance_id, "cp-3", b"state-3")
            .await
            .unwrap();

        let checkpoints = p
            .list_checkpoints(&instance_id, None, 10, 0, None, None)
            .await
            .expect("Failed to list checkpoints");

        // Scoped to this instance's own id, so the shared database's other rows
        // cannot perturb the count.
        assert_eq!(checkpoints.len(), 3);

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_checkpoints_with_filter() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("filter-cps");
        p.register_instance(&instance_id, &misc_tenant_id("filter-cps"))
            .await
            .unwrap();

        p.save_checkpoint(&instance_id, "cp-1", b"state-1")
            .await
            .unwrap();
        p.save_checkpoint(&instance_id, "cp-2", b"state-2")
            .await
            .unwrap();

        let checkpoints = p
            .list_checkpoints(&instance_id, Some("cp-1"), 10, 0, None, None)
            .await
            .expect("Failed to list checkpoints");

        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].checkpoint_id, "cp-1");

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_count_checkpoints() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("count-cps");
        p.register_instance(&instance_id, &misc_tenant_id("count-cps"))
            .await
            .unwrap();

        p.save_checkpoint(&instance_id, "cp-1", b"state-1")
            .await
            .unwrap();
        p.save_checkpoint(&instance_id, "cp-2", b"state-2")
            .await
            .unwrap();

        let count = p
            .count_checkpoints(&instance_id, None, None, None)
            .await
            .expect("Failed to count checkpoints");

        // Instance-scoped count, not a whole-table count.
        assert_eq!(count, 2);

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_signal_upsert() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("sig-upsert");
        p.register_instance(&instance_id, &misc_tenant_id("sig-upsert"))
            .await
            .unwrap();

        p.insert_signal(&instance_id, "pause", b"").await.unwrap();
        p.insert_signal(&instance_id, "cancel", b"new reason")
            .await
            .unwrap();

        let signal = p
            .get_pending_signal(&instance_id)
            .await
            .unwrap()
            .expect("upserted signal should be pending");

        // `pending_signals` is keyed by instance_id, so the second insert
        // replaces the first outright — the empty first payload is gone.
        assert_eq!(signal.signal_type, "cancel");
        assert_eq!(signal.payload, Some(b"new reason".to_vec()));

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_signal_empty_payload_stored_as_null() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("sig-empty");
        p.register_instance(&instance_id, &misc_tenant_id("sig-empty"))
            .await
            .unwrap();

        // `insert_signal` (the free function in this module) maps an empty
        // `&[u8]` to `None` before binding, so the column goes NULL rather
        // than holding a zero-length blob, and a reader sees `None` and never
        // `Some(vec![])`. Nothing else covers that mapping, so pin it here.
        p.insert_signal(&instance_id, "pause", b"").await.unwrap();

        let signal = p
            .get_pending_signal(&instance_id)
            .await
            .unwrap()
            .expect("signal should be pending");

        assert_eq!(signal.signal_type, "pause");
        assert!(
            signal.payload.is_none(),
            "empty payload must round-trip as NULL on Postgres"
        );

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_custom_signal_upsert() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("custom-upsert");
        p.register_instance(&instance_id, &misc_tenant_id("custom-upsert"))
            .await
            .unwrap();

        p.insert_custom_signal(&instance_id, "wait-1", b"payload-1")
            .await
            .unwrap();
        p.insert_custom_signal(&instance_id, "wait-1", b"payload-2")
            .await
            .unwrap();

        let signal = p
            .take_pending_custom_signal(&instance_id, "wait-1")
            .await
            .unwrap()
            .expect("custom signal should exist");

        // ON CONFLICT (instance_id, checkpoint_id) DO UPDATE: the later
        // payload wins.
        assert_eq!(signal.payload, Some(b"payload-2".to_vec()));

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_save_retry_attempt() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("retry");
        p.register_instance(&instance_id, &misc_tenant_id("retry"))
            .await
            .unwrap();

        p.save_retry_attempt(&instance_id, "durable-fn-1", 1, Some("connection error"))
            .await
            .expect("Failed to save retry attempt");

        // The synthetic `::retry::N` checkpoint exists.
        let checkpoint = p
            .load_checkpoint(&instance_id, "durable-fn-1::retry::1")
            .await
            .unwrap();
        assert!(checkpoint.is_some());

        // Postgres carries dedicated retry columns, so assert them directly
        // rather than inferring the retry from the checkpoint_id string.
        let row: (bool, Option<i32>, Option<String>) = sqlx::query_as(
            "SELECT is_retry_attempt, attempt_number, error_message \
             FROM checkpoints WHERE instance_id = $1 AND checkpoint_id = $2",
        )
        .bind(&instance_id)
        .bind("durable-fn-1::retry::1")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.0, "retry rows must be flagged is_retry_attempt");
        assert_eq!(row.1, Some(1));
        assert_eq!(row.2, Some("connection error".to_string()));

        // A second attempt writes a distinct row, and the audit trail reads
        // back in attempt order.
        p.save_retry_attempt(&instance_id, "durable-fn-1", 2, Some("timeout"))
            .await
            .unwrap();

        let history = load_retry_history(&pool, &instance_id, "durable-fn-1")
            .await
            .expect("Failed to load retry history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].attempt_number, 1);
        assert_eq!(history[1].attempt_number, 2);
        assert_eq!(history[1].error_message, Some("timeout".to_string()));

        // Re-saving the same attempt upserts in place (ON CONFLICT DO UPDATE)
        // instead of adding a row.
        p.save_retry_attempt(&instance_id, "durable-fn-1", 2, Some("timeout again"))
            .await
            .unwrap();

        let history = load_retry_history(&pool, &instance_id, "durable-fn-1")
            .await
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].error_message, Some("timeout again".to_string()));

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_instances() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let tenant1 = misc_tenant_id("list-a");
        let tenant2 = misc_tenant_id("list-b");
        let instance1 = misc_instance_id("list-a");
        let instance2 = misc_instance_id("list-b");

        p.register_instance(&instance1, &tenant1).await.unwrap();
        p.register_instance(&instance2, &tenant2).await.unwrap();

        // An exact count over `list_instances(None, None, ..)` would be a
        // whole-database assertion and cannot hold against the shared
        // `runtara_test` database, so the unfiltered path is exercised with a
        // membership check instead. It is sound because the listing is
        // `ORDER BY created_at DESC`, the suite runs `--test-threads=1`, and
        // every test deletes its rows — so these two are the newest rows in the
        // table at query time and are well inside the limit.
        let all = p
            .list_instances(None, None, 100, 0)
            .await
            .expect("Failed to list instances");
        let all_ids: Vec<&str> = all.iter().map(|i| i.instance_id.as_str()).collect();
        assert!(all_ids.contains(&instance1.as_str()));
        assert!(all_ids.contains(&instance2.as_str()));

        // The exact-count assertion survives once it is scoped to a tenant that
        // only this test uses.
        let tenant1_only = p
            .list_instances(Some(&tenant1), None, 10, 0)
            .await
            .expect("Failed to list instances");
        assert_eq!(tenant1_only.len(), 1);
        assert_eq!(tenant1_only[0].instance_id, instance1);
        assert_eq!(tenant1_only[0].tenant_id, tenant1);

        let tenant2_only = p
            .list_instances(Some(&tenant2), None, 10, 0)
            .await
            .expect("Failed to list instances");
        assert_eq!(tenant2_only.len(), 1);
        assert_eq!(tenant2_only[0].instance_id, instance2);

        cleanup_misc_instance(&pool, &instance1).await;
        cleanup_misc_instance(&pool, &instance2).await;
    }

    #[tokio::test]
    async fn test_list_instances_by_status() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        // Filtering by status with a None tenant would be a whole-database
        // assertion against the shared suite database; scoping to a
        // test-unique tenant keeps the exact count meaningful here.
        let tenant = misc_tenant_id("by-status");
        let instance1 = misc_instance_id("by-status-1");
        let instance2 = misc_instance_id("by-status-2");

        p.register_instance(&instance1, &tenant).await.unwrap();
        p.register_instance(&instance2, &tenant).await.unwrap();

        p.update_instance_status(&instance1, "running", None)
            .await
            .unwrap();

        let running = p
            .list_instances(Some(&tenant), Some("running"), 10, 0)
            .await
            .expect("Failed to list instances");

        assert_eq!(running.len(), 1);
        assert_eq!(running[0].instance_id, instance1);
        assert_eq!(running[0].status, "running");

        // The still-pending sibling is excluded by the status filter.
        let pending = p
            .list_instances(Some(&tenant), Some("pending"), 10, 0)
            .await
            .expect("Failed to list instances");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].instance_id, instance2);

        cleanup_misc_instance(&pool, &instance1).await;
        cleanup_misc_instance(&pool, &instance2).await;
    }

    #[tokio::test]
    async fn test_update_instance_metrics() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("metrics");
        p.register_instance(&instance_id, &misc_tenant_id("metrics"))
            .await
            .unwrap();

        p.update_instance_metrics(&instance_id, Some(1024 * 1024), Some(500_000))
            .await
            .expect("Failed to update metrics");

        let row: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT memory_peak_bytes, cpu_usage_usec FROM instances WHERE instance_id = $1",
        )
        .bind(&instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, Some(1024 * 1024));
        assert_eq!(row.1, Some(500_000));

        // The second write is where the semantics live:
        // `update_instance_metrics` is `SET x = COALESCE(x, $n)`, so the first
        // non-NULL write sticks and every later one is silently ignored rather
        // than overwriting it.
        p.update_instance_metrics(&instance_id, Some(9_999_999), Some(1))
            .await
            .expect("Failed to update metrics");

        let row: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT memory_peak_bytes, cpu_usage_usec FROM instances WHERE instance_id = $1",
        )
        .bind(&instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.0,
            Some(1024 * 1024),
            "COALESCE keeps the first recorded memory peak"
        );
        assert_eq!(
            row.1,
            Some(500_000),
            "COALESCE keeps the first recorded CPU usage"
        );

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_update_instance_stderr() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("stderr");
        p.register_instance(&instance_id, &misc_tenant_id("stderr"))
            .await
            .unwrap();

        p.update_instance_stderr(&instance_id, "Error: something went wrong\n")
            .await
            .expect("Failed to update stderr");

        let row: (Option<String>,) =
            sqlx::query_as("SELECT stderr FROM instances WHERE instance_id = $1")
                .bind(&instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, Some("Error: something went wrong\n".to_string()));

        // The second write matters here as in `test_update_instance_metrics`:
        // `update_instance_stderr` is `SET stderr = COALESCE(stderr, $2)`, so
        // the first capture is preserved and a later one is ignored.
        p.update_instance_stderr(&instance_id, "second capture\n")
            .await
            .expect("Failed to update stderr");

        let row: (Option<String>,) =
            sqlx::query_as("SELECT stderr FROM instances WHERE instance_id = $1")
                .bind(&instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            row.0,
            Some("Error: something went wrong\n".to_string()),
            "COALESCE keeps the first captured stderr"
        );

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_store_instance_input() {
        let pool = test_pool().await;
        let p = PostgresPersistence::new(pool.clone());

        let instance_id = misc_instance_id("input");
        p.register_instance(&instance_id, &misc_tenant_id("input"))
            .await
            .unwrap();

        let input_data = br#"{"key": "value"}"#;
        p.store_instance_input(&instance_id, input_data)
            .await
            .expect("Failed to store input");

        let row: (Option<Vec<u8>>,) =
            sqlx::query_as("SELECT input FROM instances WHERE instance_id = $1")
                .bind(&instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, Some(input_data.to_vec()));

        // `store_instance_input` is a plain overwrite (no COALESCE), unlike the
        // metrics/stderr writers above.
        let replacement = br#"{"key": "replaced"}"#;
        p.store_instance_input(&instance_id, replacement)
            .await
            .expect("Failed to store input");

        let row: (Option<Vec<u8>>,) =
            sqlx::query_as("SELECT input FROM instances WHERE instance_id = $1")
                .bind(&instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, Some(replacement.to_vec()));

        cleanup_misc_instance(&pool, &instance_id).await;
    }

    // ========================================================================
    // Step Summaries Tests
    // ========================================================================
    //
    // These exercise the Postgres step-summary CTE
    // (`dialect::postgres::PostgresDialect::sql_list_step_summaries` /
    // `sql_count_step_summaries`) end to end: the `MATERIALIZED` + `OFFSET 0`
    // planner fences, the BYTEA payload -> `convert_from(...)::jsonb` decode,
    // the start/end pairing join and every filter it supports.
    //
    // `StepStatus` and `EventSortOrder` are NOT among the names postgres.rs
    // imports from its parent module, so `use super::*` does not bring them
    // into scope. They are imported here under family-specific aliases so this
    // block cannot collide with a plain `use` of the same items elsewhere in
    // the test module.
    use crate::persistence::EventSortOrder as StepSummarySortOrder;
    use crate::persistence::StepStatus as StepSummaryStatus;

    /// Unique instance id for one step-summary test.
    ///
    /// Every test in this family shares one `runtara_test` database with every
    /// other test in the suite, and `instance_id` is a TEXT primary key with no
    /// ON CONFLICT clause behind `register_instance`, so ids must be fresh per
    /// run.
    fn step_summary_instance_id(family: &str) -> String {
        format!("pg-stepsum-{family}-{}", Uuid::new_v4())
    }

    /// Register a fresh instance for one step-summary test.
    ///
    /// Both the id and the tenant are derived from the caller's family label, so
    /// no two tests share a tenant either — nothing in this family filters by
    /// tenant today, but a shared tenant is exactly what makes a future
    /// tenant-scoped assertion elsewhere in the suite flaky.
    async fn register_step_summary_instance(
        persistence: &PostgresPersistence,
        family: &str,
    ) -> String {
        let instance_id = step_summary_instance_id(family);
        persistence
            .register_instance(&instance_id, &format!("pg-stepsum-tenant-{family}"))
            .await
            .unwrap();
        instance_id
    }

    /// Delete a step-summary test instance (its `instance_events` rows go with
    /// it via ON DELETE CASCADE).
    ///
    /// The module-level `cleanup_test_instance` takes a `Uuid`; these ids are
    /// prefixed Strings, so they need their own String-taking cleanup.
    async fn cleanup_step_summary_instance(pool: &PgPool, instance_id: &str) {
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(instance_id)
            .execute(pool)
            .await
            .ok();
    }

    /// Helper to insert a step_debug_start event.
    ///
    /// Goes through `Persistence::insert_event`, which binds the payload to the
    /// BYTEA `payload` column; the CTE decodes it with
    /// `convert_from(payload, 'UTF8')::jsonb` under the `subtype` predicate.
    #[allow(clippy::too_many_arguments)]
    async fn insert_step_start_pg(
        persistence: &PostgresPersistence,
        instance_id: &str,
        step_id: &str,
        step_name: Option<&str>,
        step_type: &str,
        scope_id: Option<&str>,
        parent_scope_id: Option<&str>,
        inputs: Option<serde_json::Value>,
    ) {
        let mut payload = serde_json::json!({
            "step_id": step_id,
            "step_type": step_type,
        });
        if let Some(name) = step_name {
            payload["step_name"] = serde_json::json!(name);
        }
        if let Some(scope) = scope_id {
            payload["scope_id"] = serde_json::json!(scope);
        }
        if let Some(parent) = parent_scope_id {
            payload["parent_scope_id"] = serde_json::json!(parent);
        }
        if let Some(inp) = inputs {
            payload["inputs"] = inp;
        }

        let event = EventRecord {
            id: None,
            instance_id: instance_id.to_string(),
            event_type: "custom".to_string(),
            checkpoint_id: None,
            payload: Some(serde_json::to_vec(&payload).unwrap()),
            created_at: Utc::now(),
            subtype: Some("step_debug_start".to_string()),
        };
        persistence.insert_event(&event).await.unwrap();
    }

    /// Helper to insert a step_debug_end event.
    async fn insert_step_end_pg(
        persistence: &PostgresPersistence,
        instance_id: &str,
        step_id: &str,
        scope_id: Option<&str>,
        outputs: Option<serde_json::Value>,
        error: Option<serde_json::Value>,
    ) {
        let mut payload = serde_json::json!({
            "step_id": step_id,
        });
        if let Some(scope) = scope_id {
            payload["scope_id"] = serde_json::json!(scope);
        }
        if let Some(out) = outputs {
            payload["outputs"] = out;
        }
        if let Some(err) = error {
            payload["error"] = err;
        }

        let event = EventRecord {
            id: None,
            instance_id: instance_id.to_string(),
            event_type: "custom".to_string(),
            checkpoint_id: None,
            payload: Some(serde_json::to_vec(&payload).unwrap()),
            created_at: Utc::now(),
            subtype: Some("step_debug_end".to_string()),
        };
        persistence.insert_event(&event).await.unwrap();
    }

    /// Default (unfiltered) step-summary filter.
    fn step_summary_filter(sort_order: StepSummarySortOrder) -> ListStepSummariesFilter {
        ListStepSummariesFilter {
            sort_order,
            status: None,
            step_type: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
            step_ids: None,
        }
    }

    #[tokio::test]
    async fn test_list_step_summaries_empty() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "empty").await;

        let filter = step_summary_filter(StepSummarySortOrder::Desc);

        // Both queries are scoped to `instance_id`, so an empty result here is
        // unaffected by rows other tests leave in the shared database.
        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert!(steps.is_empty());

        let count = persistence
            .count_step_summaries(&instance_id, &filter)
            .await
            .unwrap();

        assert_eq!(count, 0);

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_completed_step() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "completed").await;

        // Insert a completed step (start + end events)
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-1",
            Some("Fetch Data"),
            "Http",
            None,
            None,
            Some(serde_json::json!({"url": "/api/data"})),
        )
        .await;

        // Gap between the two `created_at` values so the duration is a real,
        // non-zero millisecond count.
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        insert_step_end_pg(
            &persistence,
            &instance_id,
            "step-1",
            None,
            Some(serde_json::json!({"count": 42})),
            None,
        )
        .await;

        let filter = step_summary_filter(StepSummarySortOrder::Desc);

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-1");
        assert_eq!(steps[0].step_name, Some("Fetch Data".to_string()));
        assert_eq!(steps[0].step_type, "Http");
        assert_eq!(steps[0].status, StepSummaryStatus::Completed);
        assert!(steps[0].completed_at.is_some());
        assert!(steps[0].duration_ms.is_some());
        // Stronger than a bare `is_some`: assert the value actually reflects
        // the >= 20ms gap rather than merely being present. A sub-minute gap
        // cannot distinguish a whole-span duration from a single-interval-field
        // one — `test_list_step_summaries_duration_spans_minutes` covers that.
        assert!(steps[0].duration_ms.unwrap() >= 10);
        // The inputs come back through the JSONB -> TEXT round-trip in the
        // outer SELECT and must still parse to the value that was written.
        assert_eq!(
            steps[0].inputs,
            Some(serde_json::json!({"url": "/api/data"}))
        );
        assert_eq!(steps[0].outputs, Some(serde_json::json!({"count": 42})));
        // A sequential step's end event carries no launch/settle pair.
        assert_eq!(steps[0].launched_at_ms, None);
        assert_eq!(steps[0].settled_at_ms, None);

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    /// A step longer than a minute must report its whole span.
    ///
    /// `EXTRACT(MILLISECONDS FROM interval)` returns only the interval's
    /// seconds *field* scaled to milliseconds, so it wraps every 60 seconds: a
    /// 90-second step reported 30000 and an exactly-N-minute step reported 0.
    /// Sub-minute gaps — which is all the other tests in this family exercise —
    /// pass under both the wrapping and the correct expression, so this test
    /// needs a gap over a minute.
    ///
    /// `insert_event` persists the caller's `created_at` verbatim, so the gap
    /// is constructed by backdating the start event rather than by sleeping.
    #[tokio::test]
    async fn test_list_step_summaries_duration_spans_minutes() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "longdur").await;

        let completed_at = Utc::now();
        let started_at = completed_at - chrono::Duration::seconds(90);

        persistence
            .insert_event(&EventRecord {
                id: None,
                instance_id: instance_id.clone(),
                event_type: "custom".to_string(),
                checkpoint_id: None,
                payload: Some(
                    serde_json::to_vec(&serde_json::json!({
                        "step_id": "slow-step",
                        "step_type": "DurableSleep",
                    }))
                    .unwrap(),
                ),
                created_at: started_at,
                subtype: Some("step_debug_start".to_string()),
            })
            .await
            .unwrap();

        persistence
            .insert_event(&EventRecord {
                id: None,
                instance_id: instance_id.clone(),
                event_type: "custom".to_string(),
                checkpoint_id: None,
                payload: Some(
                    serde_json::to_vec(&serde_json::json!({
                        "step_id": "slow-step",
                        "outputs": {"slept": true},
                    }))
                    .unwrap(),
                ),
                created_at: completed_at,
                subtype: Some("step_debug_end".to_string()),
            })
            .await
            .unwrap();

        let filter = step_summary_filter(StepSummarySortOrder::Desc);

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].status, StepSummaryStatus::Completed);

        // 90_000, not the 30_000 the wrapping expression produced. The bound is
        // loose only to absorb the microsecond truncation in the timestamp
        // round-trip, not a minute-sized wrap.
        let duration_ms = steps[0].duration_ms.unwrap();
        assert!(
            (89_999..=90_001).contains(&duration_ms),
            "expected ~90000ms for a 90s step, got {duration_ms}"
        );

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_surfaces_launch_settle_when_present() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "launchsettle").await;

        insert_step_start_pg(
            &persistence,
            &instance_id,
            "branch-b",
            Some("Branch B"),
            "Agent",
            None,
            None,
            None,
        )
        .await;

        // A step_debug_end payload carrying the real parallel-branch launch/settle
        // pair (as the concurrent scheduler emits). The summary must surface them as
        // epoch-ms columns so the timeline/replay can prefer the overlapping interval.
        let payload = serde_json::json!({
            "step_id": "branch-b",
            "outputs": {"status_code": 200},
            "duration_ms": 3,
            "launched_at_ms": 1_700_000_000_100_i64,
            "settled_at_ms": 1_700_000_000_500_i64,
        });
        persistence
            .insert_event(&EventRecord {
                id: None,
                instance_id: instance_id.clone(),
                event_type: "custom".to_string(),
                checkpoint_id: None,
                payload: Some(serde_json::to_vec(&payload).unwrap()),
                created_at: Utc::now(),
                subtype: Some("step_debug_end".to_string()),
            })
            .await
            .unwrap();

        let filter = step_summary_filter(StepSummarySortOrder::Desc);

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        // Postgres reads these as `(ej->>'launched_at_ms')::bigint`; the cast
        // must survive the JSON-number -> text -> bigint hop intact.
        assert_eq!(steps[0].launched_at_ms, Some(1_700_000_000_100));
        assert_eq!(steps[0].settled_at_ms, Some(1_700_000_000_500));

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_running_step() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "running").await;

        // Insert only start event (no end = running)
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-running",
            None,
            "Transform",
            None,
            None,
            None,
        )
        .await;

        let filter = step_summary_filter(StepSummarySortOrder::Desc);

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-running");
        assert_eq!(steps[0].status, StepSummaryStatus::Running);
        assert!(steps[0].completed_at.is_none());
        assert!(steps[0].duration_ms.is_none());

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_failed_step() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "failed").await;

        // Insert a failed step
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-failed",
            Some("Call API"),
            "Http",
            None,
            None,
            None,
        )
        .await;

        insert_step_end_pg(
            &persistence,
            &instance_id,
            "step-failed",
            None,
            None,
            Some(serde_json::json!({"message": "Connection refused"})),
        )
        .await;

        let filter = step_summary_filter(StepSummarySortOrder::Desc);

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-failed");
        assert_eq!(steps[0].status, StepSummaryStatus::Failed);
        assert!(steps[0].error.is_some());
        // The CTE emits `(ej->'error')::text` and the shared row mapper parses
        // it back; check the round-trip, not just presence, since the
        // `error != 'null'` status test depends on that same text form.
        assert_eq!(
            steps[0].error,
            Some(serde_json::json!({"message": "Connection refused"}))
        );

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_output_error_envelope_is_failed() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "outputerr").await;

        insert_step_start_pg(
            &persistence,
            &instance_id,
            "agent-step",
            Some("Call Agent"),
            "Agent",
            None,
            None,
            None,
        )
        .await;

        insert_step_end_pg(
            &persistence,
            &instance_id,
            "agent-step",
            None,
            Some(serde_json::json!({
                "_error": true,
                "error": {"message": "Capability failed"}
            })),
            None,
        )
        .await;

        let filter = step_summary_filter(StepSummarySortOrder::Desc);

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        // Status comes from `ej->'outputs'->>'_error' = 'true'` — the JSON
        // boolean must render as the text 'true' for this branch to fire.
        assert_eq!(steps[0].status, StepSummaryStatus::Failed);
        assert_eq!(
            steps[0].error,
            Some(serde_json::json!({"message": "Capability failed"}))
        );

        let failed_filter = ListStepSummariesFilter {
            status: Some(StepSummaryStatus::Failed),
            ..filter
        };
        // Count is instance-scoped, so this is not a whole-table count.
        assert_eq!(
            persistence
                .count_step_summaries(&instance_id, &failed_filter)
                .await
                .unwrap(),
            1
        );

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_filter_by_status() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "bystatus").await;

        // Insert completed step
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-1",
            None,
            "Http",
            None,
            None,
            None,
        )
        .await;
        insert_step_end_pg(
            &persistence,
            &instance_id,
            "step-1",
            None,
            Some(serde_json::json!({})),
            None,
        )
        .await;

        // Insert running step
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-2",
            None,
            "Transform",
            None,
            None,
            None,
        )
        .await;

        // Insert failed step
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-3",
            None,
            "Http",
            None,
            None,
            None,
        )
        .await;
        insert_step_end_pg(
            &persistence,
            &instance_id,
            "step-3",
            None,
            None,
            Some(serde_json::json!({"error": true})),
        )
        .await;

        // Filter by completed
        let filter = ListStepSummariesFilter {
            status: Some(StepSummaryStatus::Completed),
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-1");

        // Filter by running
        let filter = ListStepSummariesFilter {
            status: Some(StepSummaryStatus::Running),
            ..filter
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-2");

        // Filter by failed
        let filter = ListStepSummariesFilter {
            status: Some(StepSummaryStatus::Failed),
            ..filter
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-3");

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_filter_by_step_type() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "bytype").await;

        // Insert Http step
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-http",
            None,
            "Http",
            None,
            None,
            None,
        )
        .await;
        insert_step_end_pg(&persistence, &instance_id, "step-http", None, None, None).await;

        // Insert Transform step
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-transform",
            None,
            "Transform",
            None,
            None,
            None,
        )
        .await;
        insert_step_end_pg(
            &persistence,
            &instance_id,
            "step-transform",
            None,
            None,
            None,
        )
        .await;

        let filter = ListStepSummariesFilter {
            step_type: Some("Http".to_string()),
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-http");
        assert_eq!(steps[0].step_type, "Http");

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_filter_by_step_ids() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "byids").await;

        // The quoted id proves the filter binds ids as data (a JSON array
        // expanded by `jsonb_array_elements_text`) rather than splicing them
        // into the SQL text.
        for step_id in ["step-a", "step-b", "step-\"quoted\""] {
            insert_step_start_pg(
                &persistence,
                &instance_id,
                step_id,
                None,
                "Http",
                None,
                None,
                None,
            )
            .await;
            insert_step_end_pg(&persistence, &instance_id, step_id, None, None, None).await;
        }

        let filter = ListStepSummariesFilter {
            step_ids: Some(vec!["step-a".to_string(), "step-\"quoted\"".to_string()]),
            ..step_summary_filter(StepSummarySortOrder::Asc)
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 2);
        // Postgres orders these by the start event's BIGSERIAL `id`, so ascending
        // order is insertion order regardless of `created_at` clock resolution.
        assert_eq!(steps[0].step_id, "step-a");
        assert_eq!(steps[1].step_id, "step-\"quoted\"");

        let count = persistence
            .count_step_summaries(&instance_id, &filter)
            .await
            .unwrap();
        assert_eq!(count, 2);

        // An id that matches nothing filters everything out.
        let filter = ListStepSummariesFilter {
            step_ids: Some(vec!["missing".to_string()]),
            ..filter
        };
        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();
        assert!(steps.is_empty());
        assert_eq!(
            persistence
                .count_step_summaries(&instance_id, &filter)
                .await
                .unwrap(),
            0
        );

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_pagination() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "pagination").await;

        // Insert 5 steps back to back, with no sleeps to separate their
        // timestamps: paging is by the start event's BIGSERIAL `id`, so
        // insertion order is already a total order and equal `created_at`
        // values cannot make the pages overlap.
        for i in 1..=5 {
            insert_step_start_pg(
                &persistence,
                &instance_id,
                &format!("step-{}", i),
                None,
                "Http",
                None,
                None,
                None,
            )
            .await;
            insert_step_end_pg(
                &persistence,
                &instance_id,
                &format!("step-{}", i),
                None,
                None,
                None,
            )
            .await;
        }

        let filter = step_summary_filter(StepSummarySortOrder::Asc);

        // Total count for THIS instance only (other tests' rows live in the
        // same table).
        let count = persistence
            .count_step_summaries(&instance_id, &filter)
            .await
            .unwrap();
        assert_eq!(count, 5);

        // Get first page (limit 2)
        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 2, 0)
            .await
            .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step_id, "step-1");
        assert_eq!(steps[1].step_id, "step-2");

        // Get second page
        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 2, 2)
            .await
            .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step_id, "step-3");
        assert_eq!(steps[1].step_id, "step-4");

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }

    #[tokio::test]
    async fn test_list_step_summaries_with_scopes() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "scopes").await;

        // Root level step
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-root",
            None,
            "Http",
            None,
            None,
            None,
        )
        .await;
        insert_step_end_pg(&persistence, &instance_id, "step-root", None, None, None).await;

        // Step in scope
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-scoped",
            None,
            "Transform",
            Some("sc_main"),
            None,
            None,
        )
        .await;
        insert_step_end_pg(
            &persistence,
            &instance_id,
            "step-scoped",
            Some("sc_main"),
            None,
            None,
        )
        .await;

        // Nested step
        insert_step_start_pg(
            &persistence,
            &instance_id,
            "step-nested",
            None,
            "Http",
            Some("sc_child"),
            Some("sc_main"),
            None,
        )
        .await;
        insert_step_end_pg(
            &persistence,
            &instance_id,
            "step-nested",
            Some("sc_child"),
            None,
            None,
        )
        .await;

        // Filter by root scopes only
        let filter = ListStepSummariesFilter {
            root_scopes_only: true,
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        // Both step-root and step-scoped have no parent_scope_id
        assert_eq!(steps.len(), 2);
        let step_ids: Vec<_> = steps.iter().map(|s| s.step_id.as_str()).collect();
        assert!(step_ids.contains(&"step-root"));
        assert!(step_ids.contains(&"step-scoped"));

        // Filter by scope: the start/end pairing joins on
        // `COALESCE(scope_id,'')`, so a scoped step must still resolve to
        // 'completed' rather than dangling as 'running'.
        let filter = ListStepSummariesFilter {
            scope_id: Some("sc_main".to_string()),
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-scoped");
        assert_eq!(steps[0].scope_id, Some("sc_main".to_string()));
        assert_eq!(steps[0].status, StepSummaryStatus::Completed);

        // Filter by parent scope
        let filter = ListStepSummariesFilter {
            parent_scope_id: Some("sc_main".to_string()),
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-nested");

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }
}
