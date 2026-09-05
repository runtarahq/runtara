// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Persistence operations for runtara-core.
//!
//! Provides all durable storage access functions for instances, checkpoints, events, and signals.

#[cfg(all(test, feature = "db-integration-tests"))]
use runtara_core::domain::EventType as CoreEventType;
use runtara_core::domain::InstanceStatus as CoreInstanceStatus;
use runtara_core::domain::SignalType as CoreSignalType;

use std::sync::Arc;

use crate::rows::DbResult;

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use ::runtara_core::error::CoreError;
use ::runtara_core::persistence::{InstanceCompletionMetrics, InstanceMetricsSink};

/// PostgreSQL-backed persistence implementation.
#[derive(Clone)]
pub struct PostgresPersistence {
    pool: PgPool,
    metrics_sink: Option<Arc<dyn InstanceMetricsSink>>,
}

impl PostgresPersistence {
    /// Create a new Postgres-backed persistence implementation.
    ///
    /// Reports no completion metrics until a sink is attached with
    /// [`Self::with_metrics_sink`].
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            metrics_sink: None,
        }
    }

    /// Report terminal-state metrics to `sink`.
    ///
    /// Core assembles the facts; the host decides what they mean and where
    /// they go. See [`InstanceMetricsSink`].
    #[must_use]
    pub fn with_metrics_sink(mut self, sink: Arc<dyn InstanceMetricsSink>) -> Self {
        self.metrics_sink = Some(sink);
        self
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

impl TryFrom<InstanceMetricRow> for InstanceCompletionMetrics {
    type Error = sqlx::Error;
    fn try_from(row: InstanceMetricRow) -> Result<Self, Self::Error> {
        Ok(Self {
            tenant_id: row.tenant_id,
            status: crate::encoding::status_from_str(&row.status)?,
            termination_reason: row.termination_reason,
            started_at: row.started_at,
            finished_at: row.finished_at,
            memory_peak_bytes: row.memory_peak_bytes.and_then(|v| u64::try_from(v).ok()),
            cpu_usage_usec: row.cpu_usage_usec.and_then(|v| u64::try_from(v).ok()),
        })
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

/// Terminal statuses a completion is reported for.
fn is_reportable_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

/// Read back the terminal row and hand it to the host's sink.
///
/// A missing row or a read error is logged and dropped: reporting must never
/// fail a completion.
async fn report_completion(sink: &dyn InstanceMetricsSink, pool: &PgPool, instance_id: &str) {
    match fetch_instance_metric_row(pool, instance_id).await {
        Ok(Some(row)) => match row.try_into() {
            Ok(metrics) => sink.on_terminal(&metrics),
            Err(error) => tracing::warn!(%error, "Invalid completion metric state"),
        },
        Ok(None) => tracing::warn!(
            instance_id = %instance_id,
            "Skipped completion metric because instance row was not found"
        ),
        Err(error) => tracing::warn!(
            instance_id = %instance_id,
            error = %error,
            "Skipped completion metric"
        ),
    }
}

// ============================================================================
// Record Types
// ============================================================================

use ::runtara_core::persistence::{
    CheckpointRecord, CompleteInstanceParams, CustomSignalRecord, EventRecord, EventVocabulary,
    InstanceRecord, ListEventsFilter, ListPairedRecordsFilter, PairedRecordSummary, Persistence,
    SignalRecord,
};

// ============================================================================
// Shared Operations
// ============================================================================
// The instance + sleep families live in crate::ops_common::ops and
// are materialized onto PostgresPersistence via the macros below. The inline
// free functions they replaced have been removed; callers in this module's
// tests (see the `tests` submodule) reach the shared ops through
// `PostgresPersistence::op_*` instead.

crate::ops_common::ops::impl_instance_ops!(
    PostgresPersistence,
    PgPool,
    crate::dialect::PostgresDialect
);
crate::ops_common::ops::impl_sleep_ops!(
    PostgresPersistence,
    PgPool,
    crate::dialect::PostgresDialect
);
crate::ops_common::ops::impl_checkpoint_ops!(
    PostgresPersistence,
    PgPool,
    crate::dialect::PostgresDialect
);
crate::ops_common::ops::impl_signal_ops!(
    PostgresPersistence,
    PgPool,
    crate::dialect::PostgresDialect
);
crate::ops_common::ops::impl_event_ops!(
    PostgresPersistence,
    PgPool,
    crate::dialect::PostgresDialect
);
crate::ops_common::ops::impl_paired_record_ops!(
    PostgresPersistence,
    PgPool,
    crate::dialect::PostgresDialect
);
crate::ops_common::ops::impl_retention_ops!(
    PostgresPersistence,
    PgPool,
    crate::dialect::PostgresDialect
);

// ============================================================================
// Remaining Instance Operations (pre-shared — migrated in later phases)
// ============================================================================

// `store_instance_input` is migrated to the shared layer:
// see PostgresPersistence::op_store_instance_input (crate::ops_common::ops::instances).

// ============================================================================
// Checkpoint Operations
// ============================================================================
// `save_checkpoint`, `load_checkpoint`, `list_checkpoints`, `count_checkpoints`
// are migrated to the shared layer:
// see PostgresPersistence::op_save_checkpoint / op_load_checkpoint /
// op_list_checkpoints / op_count_checkpoints
// (crate::ops_common::ops::checkpoints).

/// Load the latest checkpoint for an instance.
pub async fn load_latest_checkpoint(
    pool: &PgPool,
    instance_id: &str,
) -> Result<Option<CheckpointRecord>, CoreError> {
    let record = sqlx::query_as::<_, crate::rows::CheckpointRow>(
        r#"
        SELECT instance_id, checkpoint_id, state, created_at
        FROM checkpoints
        WHERE instance_id = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(pool)
    .await
    .db()?;

    Ok(record.map(|r| r.0))
}

/// Retry attempt record from the database.
/// These are stored in the checkpoints table with is_retry_attempt = true.
///
/// Test-only: nothing in production reads the retry audit trail back. It
/// exists so `save_retry_attempt`'s upsert-in-place behaviour can be asserted,
/// so it is gated to match the `tests` module that uses it.
#[cfg(all(test, feature = "db-integration-tests"))]
#[derive(Debug, Clone, sqlx::FromRow)]
struct RetryAttemptRecord {
    /// Retry attempt number (1-indexed).
    pub attempt_number: i32,
    /// Error message from this attempt.
    pub error_message: Option<String>,
}

/// Save a retry attempt record for audit trail.
/// Retry attempts are stored in the checkpoints table with a unique checkpoint_id.
async fn save_retry_attempt(
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

/// Load retry history for a checkpoint. Test-only; see `RetryAttemptRecord`.
#[cfg(all(test, feature = "db-integration-tests"))]
async fn load_retry_history(
    pool: &PgPool,
    instance_id: &str,
    checkpoint_id: &str,
) -> Result<Vec<RetryAttemptRecord>, CoreError> {
    let pattern = format!("{}::retry::%", checkpoint_id);

    let records = sqlx::query_as::<_, RetryAttemptRecord>(
        r#"
        SELECT attempt_number, error_message
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
    .await
    .db()?;

    Ok(records)
}

// ============================================================================
// Event Operations
// ============================================================================

/// Insert an instance event.
async fn insert_event(pool: &PgPool, event: &EventRecord) -> Result<(), CoreError> {
    sqlx::query(
        r#"
        INSERT INTO instance_events (instance_id, event_type, checkpoint_id, payload, created_at, subtype)
        VALUES ($1, $2::instance_event_type, $3, $4, $5, $6)
        "#,
    )
    .bind(&event.instance_id)
    .bind(crate::encoding::event_type_to_str(event.event_type))
    .bind(&event.checkpoint_id)
    .bind(&event.payload)
    .bind(event.created_at)
    .bind(&event.subtype)
    .execute(pool)
    .await.db()?;

    Ok(())
}

// `list_events`, `count_events`, `list_step_summaries`, `count_step_summaries`
// are migrated to the shared layer:
// see PostgresPersistence::op_list_events / op_count_events /
// op_list_step_summaries / op_count_step_summaries
// (crate::ops_common::ops::{events, step_summaries}).

// ============================================================================
// Signal Operations
// ============================================================================

/// Insert or update a pending signal.
/// Uses ON CONFLICT to replace existing signal for the same instance.
async fn insert_signal(
    pool: &PgPool,
    instance_id: &str,
    signal_type: CoreSignalType,
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
    .bind(crate::encoding::signal_type_to_str(signal_type))
    .bind(payload_opt)
    .execute(pool)
    .await
    .db()?;

    Ok(())
}

/// Insert or update a pending custom signal scoped to a checkpoint.
async fn insert_custom_signal(
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
    .await
    .db()?;

    Ok(())
}

// `get_pending_signal`, `acknowledge_signal`, `take_pending_custom_signal`
// are migrated to the shared layer:
// see PostgresPersistence::op_get_pending_signal / op_acknowledge_signal /
// op_take_pending_custom_signal (crate::ops_common::ops::signals).

// Health, sleep, and active-count operations are migrated to the shared layer:
// see PostgresPersistence::op_health_check, op_count_active_instances,
// op_set_instance_sleep, op_clear_instance_sleep, op_get_sleeping_instances_due
// (crate::ops_common::ops::{instances, sleep}).

#[async_trait::async_trait]
impl Persistence for PostgresPersistence {
    async fn register_instance(&self, instance_id: &str, tenant_id: &str) -> Result<(), CoreError> {
        Self::op_register_instance(&self.pool, instance_id, tenant_id).await
    }

    async fn try_register_instance(
        &self,
        instance_id: &str,
        tenant_id: &str,
        input: Option<&[u8]>,
    ) -> Result<bool, CoreError> {
        Self::op_try_register_instance(&self.pool, instance_id, tenant_id, input).await
    }

    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError> {
        Self::op_get_instance(&self.pool, instance_id).await
    }

    async fn get_instance_meta(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceRecord>, CoreError> {
        Self::op_get_instance_meta(&self.pool, instance_id).await
    }

    async fn update_instance_status(
        &self,
        instance_id: &str,
        status: CoreInstanceStatus,
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
        let target_status = crate::encoding::status_to_str(params.status);
        // Only read the previous status when it can change the outcome. It exists
        // to stop a completion metric being recorded twice, and that recording
        // is gated on the TARGET status being one we record — so for every
        // other transition, a park above all, the read was fetched and thrown
        // away. A launch that parks pays for it once per instance.
        let records_metric =
            self.metrics_sink.is_some() && is_reportable_terminal_status(target_status);
        let previous_was_terminal = if records_metric {
            match fetch_instance_status(&self.pool, &instance_id).await {
                Ok(Some(status)) => is_reportable_terminal_status(&status),
                Ok(None) => false,
                Err(error) => {
                    tracing::warn!(
                        instance_id = %instance_id,
                        error = %error,
                        "Could not read previous instance status before OTLP metric recording"
                    );
                    false
                }
            }
        } else {
            false
        };

        let applied = Self::op_complete_instance_unified(&self.pool, params).await?;
        if applied && records_metric && !previous_was_terminal {
            // `records_metric` is only true when a sink is wired.
            if let Some(sink) = &self.metrics_sink {
                report_completion(sink.as_ref(), &self.pool, &instance_id).await;
            }
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
        signal_type: CoreSignalType,
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
        status: Option<CoreInstanceStatus>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        Self::op_list_instances(&self.pool, tenant_id, status, limit, offset).await
    }

    async fn health_check(&self) -> Result<bool, CoreError> {
        Self::op_health_check(&self.pool).await
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

    async fn mark_instance_running(
        &self,
        instance_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        Self::op_mark_instance_running(&self.pool, instance_id, started_at).await
    }

    async fn mark_instance_started(
        &self,
        instance_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<bool, CoreError> {
        Self::op_mark_instance_started(&self.pool, instance_id, started_at).await
    }

    async fn clear_instance_sleep(&self, instance_id: &str) -> Result<(), CoreError> {
        Self::op_clear_instance_sleep(&self.pool, instance_id).await
    }

    async fn claim_sleeping_instance(&self, instance_id: &str) -> Result<bool, CoreError> {
        Self::op_claim_sleeping_instance(&self.pool, instance_id).await
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
        retry_at: DateTime<Utc>,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        Self::op_claim_sleeping_instances_due(&self.pool, limit, retry_at).await
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

    async fn list_paired_records(
        &self,
        instance_id: &str,
        vocabulary: &EventVocabulary,
        filter: &ListPairedRecordsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PairedRecordSummary>, CoreError> {
        Self::op_list_paired_records(&self.pool, instance_id, vocabulary, filter, limit, offset)
            .await
    }

    async fn count_paired_records(
        &self,
        instance_id: &str,
        vocabulary: &EventVocabulary,
        filter: &ListPairedRecordsFilter,
    ) -> Result<i64, CoreError> {
        Self::op_count_paired_records(&self.pool, instance_id, vocabulary, filter).await
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

    async fn delete_paired_events_older_than(
        &self,
        vocabulary: &EventVocabulary,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<u64, CoreError> {
        Self::op_delete_paired_events_older_than(&self.pool, vocabulary, older_than, limit).await
    }
}

// `get_terminal_instances_older_than`, `delete_instances_batch`,
// `list_instances` are migrated to the shared layer:
// see PostgresPersistence::op_get_terminal_instances_older_than /
// op_delete_instances_batch / op_list_instances
// (crate::ops_common::ops::{retention, instances}).

#[cfg(all(test, feature = "db-integration-tests"))]
mod tests {
    use uuid::Uuid;
    /// Concurrent claimers of the same due batch must never both get a row.
    ///
    /// This is the invariant the wake scheduler leans on once it polls back to
    /// back and runs a batch concurrently: a duplicate claim means two guests
    /// running the same in-flight step. Two things uphold it, and this test
    /// pins the outcome rather than either mechanism. The subquery's
    /// `sleep_until IS NOT NULL` predicate is the load-bearing half -- under
    /// READ COMMITTED a claimer that blocks on a rival's row lock re-checks
    /// that qual through EvalPlanQual, finds `sleep_until` already cleared, and
    /// skips the row. `FOR UPDATE SKIP LOCKED` then keeps claimers from
    /// blocking on each other at all, which is what makes the batch claim fast
    /// rather than what makes it correct. (Verified: removing the locking
    /// clause keeps this test green.)
    ///
    /// The candidate set is deliberately large and the claimers many so the
    /// claims genuinely overlap; with a handful of rows each UPDATE finishes
    /// before a rival's subquery even runs.
    #[tokio::test]
    async fn concurrent_batch_claims_never_hand_out_the_same_instance() {
        let backend = std::sync::Arc::new(PostgresPersistence::new(test_pool().await));
        let tenant = format!("skip-locked-{}", uuid::Uuid::new_v4());
        let due_at = chrono::Utc::now() - chrono::Duration::seconds(60);

        const ROWS: usize = 400;
        const CLAIMERS: usize = 6;

        let mut ids = Vec::with_capacity(ROWS);
        for _ in 0..ROWS {
            let id = uuid::Uuid::new_v4().to_string();
            backend.register_instance(&id, &tenant).await.unwrap();
            backend
                .update_instance_status(&id, CoreInstanceStatus::Suspended, None)
                .await
                .unwrap();
            backend.set_instance_sleep(&id, due_at).await.unwrap();
            ids.push(id);
        }

        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..CLAIMERS {
            let backend = std::sync::Arc::clone(&backend);
            tasks.spawn(async move {
                backend
                    .claim_sleeping_instances_due(
                        ROWS as i64,
                        Utc::now() + chrono::Duration::seconds(120),
                    )
                    .await
                    .expect("claim failed")
            });
        }

        let mut seen = std::collections::HashSet::new();
        let mut duplicates = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            for record in joined.expect("claimer task panicked") {
                if !seen.insert(record.instance_id.clone()) {
                    duplicates.push(record.instance_id);
                }
            }
        }
        assert!(
            duplicates.is_empty(),
            "{} instance(s) were claimed by more than one claimer, e.g. {:?}",
            duplicates.len(),
            &duplicates[..duplicates.len().min(3)]
        );

        // A round need not drain everything: `SKIP LOCKED` means a claimer
        // steps past rows a rival holds rather than waiting for them, so some
        // rows fall to a later poll. That is the trade the clause makes, and
        // the scheduler covers it by polling again immediately after a full
        // batch. What must hold is that nothing is claimed twice (above) and
        // nothing is stranded (below).
        let ours: std::collections::HashSet<_> = ids.iter().cloned().collect();
        assert!(
            seen.intersection(&ours).count() > 0,
            "the claimers must make progress"
        );

        // Keep polling, as the scheduler would, until none of our rows is due.
        // Bounded rather than unbounded so a genuine strand fails the test
        // instead of hanging it. This deliberately does not assert that *we*
        // claimed every row: the lib tests share one database, so a rival test
        // polling the same due set may take some of them -- which is fine, and
        // is itself the shape the scheduler is built for.
        for _ in 0..10 {
            let batch = backend
                .claim_sleeping_instances_due(
                    ROWS as i64,
                    Utc::now() + chrono::Duration::seconds(120),
                )
                .await
                .expect("follow-up claim failed");
            for record in &batch {
                assert!(
                    seen.insert(record.instance_id.clone()),
                    "the follow-up claim must not re-hand out {}",
                    record.instance_id
                );
            }
            if batch.is_empty() {
                break;
            }
        }

        // A claim leases rather than clears, so "claimed" is no longer "has no
        // deadline" — it is "has a deadline in the future". What must not exist
        // is a row still overdue after the polling loop (nobody took it) or a
        // row with no deadline at all, which is the unrecoverable state the
        // lease exists to avoid.
        let now = Utc::now();
        let mut stranded = Vec::new();
        for id in &ids {
            let inst = backend.get_instance(id).await.unwrap().unwrap();
            match inst.sleep_until {
                None => stranded.push(format!("{id} (no deadline)")),
                Some(due) if due <= now => stranded.push(format!("{id} (still overdue)")),
                Some(_) => {}
            }
        }
        assert!(
            stranded.is_empty(),
            "{} row(s) were skipped and never became claimable again -- stranded, \
             e.g. {:?}",
            stranded.len(),
            &stranded[..stranded.len().min(3)]
        );

        let _ = backend.delete_instances_batch(&ids).await;
    }

    /// The paired-event sweep must remove the caller's paired payloads past
    /// the window and leave everything else — lifecycle events, recent paired
    /// events, and any subtype the caller did not name — alone. Losing a
    /// `completed` event would erase the run's history.
    #[tokio::test]
    async fn paired_event_sweep_spares_lifecycle_and_recent_events() {
        use ::runtara_core::persistence::{EventRecord, EventVocabulary, EventVocabularySpec};

        // Only the two subtypes named here may be swept. The rest of the
        // vocabulary is irrelevant to a DELETE but must still be supplied.
        let vocabulary = |start: &'static str, end: &'static str| {
            EventVocabulary::new(EventVocabularySpec {
                start_subtype: start,
                end_subtype: end,
                correlation_key: "step_id",
                kind_key: "step_type",
                label_key: "step_name",
                inputs_key: "inputs",
                outputs_key: "outputs",
                error_key: "error",
                error_flag_key: "_error",
                launched_at_key: "launched_at_ms",
                settled_at_key: "settled_at_ms",
            })
            .expect("valid vocabulary")
        };
        let backend = PostgresPersistence::new(test_pool().await);
        let id = uuid::Uuid::new_v4().to_string();
        backend
            .register_instance(&id, "sweep-tenant")
            .await
            .unwrap();

        let old = chrono::Utc::now() - chrono::Duration::hours(48);
        let recent = chrono::Utc::now();
        let event = |subtype: Option<&str>, event_type: &str, at| EventRecord {
            id: None,
            instance_id: id.clone(),
            event_type: crate::encoding::event_type_from_str(event_type).unwrap(),
            checkpoint_id: None,
            payload: Some(b"{}".to_vec()),
            created_at: at,
            subtype: subtype.map(str::to_string),
        };

        backend
            .insert_event(&event(Some("step_debug_start"), "custom", old))
            .await
            .unwrap();
        backend
            .insert_event(&event(Some("step_debug_end"), "custom", old))
            .await
            .unwrap();
        backend
            .insert_event(&event(Some("step_debug_start"), "custom", recent))
            .await
            .unwrap();
        backend
            .insert_event(&event(None, "completed", old))
            .await
            .unwrap();
        backend
            .insert_event(&event(Some("workflow_log"), "custom", old))
            .await
            .unwrap();

        let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);

        // A vocabulary this producer does not use must sweep nothing, which is
        // what proves the subtypes come from the parameter rather than from a
        // literal inside the kernel.
        let swept_by_a_foreign_vocabulary = backend
            .delete_paired_events_older_than(&vocabulary("unit_start", "unit_end"), cutoff, 100)
            .await
            .expect("sweep failed");
        assert_eq!(
            swept_by_a_foreign_vocabulary, 0,
            "a vocabulary naming other subtypes must not touch these events"
        );

        let deleted = backend
            .delete_paired_events_older_than(
                &vocabulary("step_debug_start", "step_debug_end"),
                cutoff,
                100,
            )
            .await
            .expect("sweep failed");
        assert_eq!(deleted, 2, "only the two aged paired events may go");

        let filter = ::runtara_core::persistence::ListEventsFilter::default();
        let left = backend.list_events(&id, &filter, 50, 0).await.unwrap();
        let subtypes: Vec<_> = left
            .iter()
            .map(|e| {
                (
                    crate::encoding::event_type_to_str(e.event_type),
                    e.subtype.as_deref(),
                )
            })
            .collect();
        assert_eq!(left.len(), 3, "survivors: {subtypes:?}");
        assert!(
            subtypes.contains(&("completed", None)),
            "the lifecycle event is the run's history and must survive: {subtypes:?}"
        );
        assert!(
            subtypes.contains(&("custom", Some("workflow_log"))),
            "non-debug custom events must survive: {subtypes:?}"
        );
        assert!(
            subtypes.contains(&("custom", Some("step_debug_start"))),
            "a debug event inside the window must survive: {subtypes:?}"
        );

        let _ = backend
            .delete_instances_batch(std::slice::from_ref(&id))
            .await;
    }

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
        assert_eq!(instance.status, CoreInstanceStatus::Pending);

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
            CoreInstanceStatus::Running,
            Some(Utc::now()),
        )
        .await;
        assert!(result.is_ok());

        let instance = PostgresPersistence::op_get_instance(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Running);
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
            CompleteInstanceParams::new(&instance_id_str, CoreInstanceStatus::Completed)
                .with_output(output_data),
        )
        .await;
        assert!(result.is_ok());

        let instance = PostgresPersistence::op_get_instance(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Completed);
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
            CompleteInstanceParams::new(&instance_id_str, CoreInstanceStatus::Failed)
                .with_error("test error"),
        )
        .await;
        assert!(result.is_ok());

        let instance = PostgresPersistence::op_get_instance(&pool, &instance_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Failed);
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
            event_type: CoreEventType::Started,
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

        let result = insert_signal(
            &pool,
            &instance_id.to_string(),
            CoreSignalType::Cancel,
            b"reason",
        )
        .await;
        assert!(result.is_ok());

        let signal = PostgresPersistence::op_get_pending_signal(&pool, &instance_id.to_string())
            .await
            .unwrap();
        assert!(signal.is_some());
        let signal = signal.unwrap();
        assert_eq!(signal.signal_type, CoreSignalType::Cancel);
        assert_eq!(signal.payload, Some(b"reason".to_vec()));

        cleanup_test_instance(&pool, instance_id).await;
    }

    #[tokio::test]
    async fn test_get_pending_signal() {
        let pool = test_pool().await;

        let instance_id = Uuid::new_v4();
        create_test_instance(&pool, instance_id, "test-tenant").await;

        insert_signal(&pool, &instance_id.to_string(), CoreSignalType::Pause, b"")
            .await
            .unwrap();

        let signal = PostgresPersistence::op_get_pending_signal(&pool, &instance_id.to_string())
            .await
            .unwrap();
        assert!(signal.is_some());
        assert_eq!(signal.unwrap().signal_type, CoreSignalType::Pause);

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

        insert_signal(&pool, &instance_id.to_string(), CoreSignalType::Cancel, b"")
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
            CoreInstanceStatus::Running,
            None,
        )
        .await
        .unwrap();
        PostgresPersistence::op_update_instance_status(
            &pool,
            &instance2.to_string(),
            CoreInstanceStatus::Suspended,
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
            CoreInstanceStatus::Suspended,
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
    async fn test_health_check() {
        let pool = test_pool().await;

        let result = PostgresPersistence::op_health_check(&pool).await;
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
    /// Reads both columns straight from the row rather than through
    /// `InstanceRecord`, so the assertion cannot be satisfied by whatever the
    /// record projection happens to carry. (`get_instance` does now project
    /// `exit_code`; this helper predates that and still isolates the merge
    /// behaviour from the projection.)
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
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Completed)
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
        p.update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
            .await
            .unwrap();

        let applied = p
            .complete_instance(
                CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Completed)
                    .if_running()
                    .with_output(b"done"),
            )
            .await
            .expect("Failed to complete instance");

        assert!(applied);

        let instance = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Completed);

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
                CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Completed)
                    .if_running()
                    .with_output(b"done"),
            )
            .await
            .expect("Query should succeed");

        assert!(!applied);

        let instance = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Pending); // unchanged

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
            .complete_instance(CompleteInstanceParams::new(
                &missing,
                CoreInstanceStatus::Completed,
            ))
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
            .complete_instance(CompleteInstanceParams::new(
                &instance_id,
                CoreInstanceStatus::Completed,
            ))
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
            .complete_instance(
                CompleteInstanceParams::new(&missing, CoreInstanceStatus::Completed).if_running(),
            )
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
            .complete_instance(CompleteInstanceParams::new(
                &instance_id,
                CoreInstanceStatus::Running,
            ))
            .await
            .expect("non-terminal transition should succeed");
        assert!(applied);

        let instance = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert_eq!(instance.status, CoreInstanceStatus::Running);
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
        p.update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
            .await
            .unwrap();

        // Suspend the way a drain / durable sleep does: stamps finished_at +
        // termination_reason. 'sleeping' is a member of the Postgres
        // `termination_reason` ENUM, so the `$3::termination_reason` cast in
        // the unified op resolves.
        p.complete_instance(
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Suspended)
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
        p.update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
            .await
            .unwrap();

        let running = p.get_instance(&instance_id).await.unwrap().unwrap();
        assert_eq!(running.status, CoreInstanceStatus::Running);
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
        p.update_instance_status(&instance_id, CoreInstanceStatus::Running, Some(Utc::now()))
            .await
            .unwrap();

        // First write sets both termination fields. 'crashed' is a member of
        // the Postgres `termination_reason` ENUM.
        p.complete_instance(
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Failed)
                .with_termination("crashed", Some(137)),
        )
        .await
        .expect("first completion should succeed");

        let (reason, code) = ci_read_term_fields(&pool, &instance_id).await;
        assert_eq!(reason.as_deref(), Some("crashed"));
        assert_eq!(code, Some(137));

        // Second write without termination/exit fields must not clobber.
        p.complete_instance(CompleteInstanceParams::new(
            &instance_id,
            CoreInstanceStatus::Failed,
        ))
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

        p.insert_signal(&instance_id, CoreSignalType::Pause, b"")
            .await
            .unwrap();
        p.insert_signal(&instance_id, CoreSignalType::Cancel, b"new reason")
            .await
            .unwrap();

        let signal = p
            .get_pending_signal(&instance_id)
            .await
            .unwrap()
            .expect("upserted signal should be pending");

        // `pending_signals` is keyed by instance_id, so the second insert
        // replaces the first outright — the empty first payload is gone.
        assert_eq!(signal.signal_type, CoreSignalType::Cancel);
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
        p.insert_signal(&instance_id, CoreSignalType::Pause, b"")
            .await
            .unwrap();

        let signal = p
            .get_pending_signal(&instance_id)
            .await
            .unwrap()
            .expect("signal should be pending");

        assert_eq!(signal.signal_type, CoreSignalType::Pause);
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

        p.update_instance_status(&instance1, CoreInstanceStatus::Running, None)
            .await
            .unwrap();

        let running = p
            .list_instances(Some(&tenant), Some(CoreInstanceStatus::Running), 10, 0)
            .await
            .expect("Failed to list instances");

        assert_eq!(running.len(), 1);
        assert_eq!(running[0].instance_id, instance1);
        assert_eq!(running[0].status, CoreInstanceStatus::Running);

        // The still-pending sibling is excluded by the status filter.
        let pending = p
            .list_instances(Some(&tenant), Some(CoreInstanceStatus::Pending), 10, 0)
            .await
            .expect("Failed to list instances");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].instance_id, instance2);

        cleanup_misc_instance(&pool, &instance1).await;
        cleanup_misc_instance(&pool, &instance2).await;
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
    // These exercise the Postgres paired-record CTE
    // (`dialect::postgres::PostgresDialect::sql_list_paired_records` /
    // `sql_count_paired_records`) end to end: the `MATERIALIZED` + `OFFSET 0`
    // planner fences, the BYTEA payload -> `convert_from(...)::jsonb` decode,
    // the start/end pairing join and every filter it supports.
    //
    // `PairedRecordStatus` and `EventSortOrder` are NOT among the names
    // postgres.rs imports from its parent module, so `use super::*` does not
    // bring them into scope. They are imported here under family-specific
    // aliases so this block cannot collide with a plain `use` of the same
    // items elsewhere in the test module.
    use ::runtara_core::persistence::EventSortOrder as StepSummarySortOrder;
    use ::runtara_core::persistence::PairedRecordStatus as StepSummaryStatus;
    use ::runtara_core::persistence::{EventVocabulary, EventVocabularySpec};

    /// The workflow DSL's own naming, which these fixtures emit.
    ///
    /// It lives here, in a test, rather than in this crate's source: the whole
    /// point of the vocabulary parameter is that the kernel names none of
    /// this. `runtara-environment` holds the production copy.
    fn workflow_vocabulary() -> EventVocabulary {
        EventVocabulary::new(EventVocabularySpec {
            start_subtype: "step_debug_start",
            end_subtype: "step_debug_end",
            correlation_key: "step_id",
            kind_key: "step_type",
            label_key: "step_name",
            inputs_key: "inputs",
            outputs_key: "outputs",
            error_key: "error",
            error_flag_key: "_error",
            launched_at_key: "launched_at_ms",
            settled_at_key: "settled_at_ms",
        })
        .expect("valid vocabulary")
    }

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
            event_type: CoreEventType::Custom,
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
            event_type: CoreEventType::Custom,
            checkpoint_id: None,
            payload: Some(serde_json::to_vec(&payload).unwrap()),
            created_at: Utc::now(),
            subtype: Some("step_debug_end".to_string()),
        };
        persistence.insert_event(&event).await.unwrap();
    }

    /// Default (unfiltered) paired-record filter.
    fn step_summary_filter(sort_order: StepSummarySortOrder) -> ListPairedRecordsFilter {
        ListPairedRecordsFilter {
            sort_order,
            status: None,
            kind: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
            correlation_ids: None,
        }
    }

    /// The whole query must run on a vocabulary that shares nothing with the
    /// workflow DSL — different subtypes, a different correlation key, a
    /// different output-failure envelope.
    ///
    /// This is the test that would have failed before the vocabulary became a
    /// parameter: with the DSL's names spelled into the SQL, none of these
    /// events pair, and the failing record reads as completed. Asserting
    /// against DSL-named fixtures cannot catch that, because it agrees with
    /// hardcoded names by construction.
    #[tokio::test]
    async fn test_list_paired_records_under_a_foreign_vocabulary() {
        let pool = test_pool().await;
        let persistence = PostgresPersistence::new(pool.clone());

        let instance_id = register_step_summary_instance(&persistence, "foreign-vocab").await;

        let vocabulary = EventVocabulary::new(EventVocabularySpec {
            start_subtype: "unit_start",
            end_subtype: "unit_end",
            correlation_key: "unit_id",
            kind_key: "unit_kind",
            label_key: "unit_label",
            inputs_key: "given",
            outputs_key: "produced",
            error_key: "failure",
            error_flag_key: "_failed",
            launched_at_key: "began_ms",
            settled_at_key: "ended_ms",
        })
        .expect("valid vocabulary");

        let insert = |subtype: &'static str, payload: serde_json::Value| {
            let persistence = &persistence;
            let instance_id = instance_id.clone();
            async move {
                persistence
                    .insert_event(&EventRecord {
                        id: None,
                        instance_id,
                        event_type: CoreEventType::Custom,
                        checkpoint_id: None,
                        payload: Some(serde_json::to_vec(&payload).unwrap()),
                        created_at: Utc::now(),
                        subtype: Some(subtype.to_string()),
                    })
                    .await
                    .unwrap();
            }
        };

        // A record that completes.
        insert(
            "unit_start",
            serde_json::json!({
                "unit_id": "u-1",
                "unit_kind": "Fetch",
                "unit_label": "Fetch it",
                "given": {"url": "https://example.test"},
            }),
        )
        .await;
        insert(
            "unit_end",
            serde_json::json!({
                "unit_id": "u-1",
                "produced": {"count": 7},
                "began_ms": 1_700_000_000_100i64,
                "ended_ms": 1_700_000_000_500i64,
            }),
        )
        .await;

        // A record that fails only through the output envelope, under this
        // vocabulary's flag rather than the DSL's `_error`.
        insert(
            "unit_start",
            serde_json::json!({"unit_id": "u-2", "unit_kind": "Call"}),
        )
        .await;
        insert(
            "unit_end",
            serde_json::json!({
                "unit_id": "u-2",
                "produced": {"_failed": true, "failure": {"message": "downstream refused"}},
            }),
        )
        .await;

        // An event carrying the workflow DSL's own names must be ignored
        // entirely: it is not part of the vocabulary being asked about.
        insert(
            "step_debug_start",
            serde_json::json!({"step_id": "s-1", "step_type": "Http"}),
        )
        .await;

        let filter = step_summary_filter(StepSummarySortOrder::Asc);
        let records = persistence
            .list_paired_records(&instance_id, &vocabulary, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(
            records.len(),
            2,
            "only this vocabulary's records may appear: {records:?}"
        );

        assert_eq!(records[0].correlation_id, "u-1");
        assert_eq!(records[0].kind, "Fetch");
        assert_eq!(records[0].label, Some("Fetch it".to_string()));
        assert_eq!(records[0].status, StepSummaryStatus::Completed);
        assert_eq!(
            records[0].inputs,
            Some(serde_json::json!({"url": "https://example.test"}))
        );
        assert_eq!(records[0].outputs, Some(serde_json::json!({"count": 7})));
        assert_eq!(records[0].launched_at_ms, Some(1_700_000_000_100));
        assert_eq!(records[0].settled_at_ms, Some(1_700_000_000_500));

        assert_eq!(records[1].correlation_id, "u-2");
        assert_eq!(
            records[1].status,
            StepSummaryStatus::Failed,
            "the output envelope's own flag must mark the record failed"
        );
        assert_eq!(
            records[1].error,
            Some(serde_json::json!({"message": "downstream refused"}))
        );

        assert_eq!(
            persistence
                .count_paired_records(&instance_id, &vocabulary, &filter)
                .await
                .unwrap(),
            2
        );

        // Filtering rides on the same vocabulary-supplied keys.
        let mut kind_filter = step_summary_filter(StepSummarySortOrder::Asc);
        kind_filter.kind = Some("Call".to_string());
        let filtered = persistence
            .list_paired_records(&instance_id, &vocabulary, &kind_filter, 100, 0)
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].correlation_id, "u-2");

        // ...and the DSL vocabulary sees only its own half-open record.
        let dsl_records = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();
        assert_eq!(dsl_records.len(), 1);
        assert_eq!(dsl_records[0].correlation_id, "s-1");
        assert_eq!(dsl_records[0].status, StepSummaryStatus::Running);

        cleanup_step_summary_instance(&pool, &instance_id).await;
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
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert!(steps.is_empty());

        let count = persistence
            .count_paired_records(&instance_id, &workflow_vocabulary(), &filter)
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
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-1");
        assert_eq!(steps[0].label, Some("Fetch Data".to_string()));
        assert_eq!(steps[0].kind, "Http");
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
                event_type: CoreEventType::Custom,
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
                event_type: CoreEventType::Custom,
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
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
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
                event_type: CoreEventType::Custom,
                checkpoint_id: None,
                payload: Some(serde_json::to_vec(&payload).unwrap()),
                created_at: Utc::now(),
                subtype: Some("step_debug_end".to_string()),
            })
            .await
            .unwrap();

        let filter = step_summary_filter(StepSummarySortOrder::Desc);

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
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
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-running");
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
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-failed");
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
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
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

        let failed_filter = ListPairedRecordsFilter {
            status: Some(StepSummaryStatus::Failed),
            ..filter
        };
        // Count is instance-scoped, so this is not a whole-table count.
        assert_eq!(
            persistence
                .count_paired_records(&instance_id, &workflow_vocabulary(), &failed_filter)
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
        let filter = ListPairedRecordsFilter {
            status: Some(StepSummaryStatus::Completed),
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-1");

        // Filter by running
        let filter = ListPairedRecordsFilter {
            status: Some(StepSummaryStatus::Running),
            ..filter
        };

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-2");

        // Filter by failed
        let filter = ListPairedRecordsFilter {
            status: Some(StepSummaryStatus::Failed),
            ..filter
        };

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-3");

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

        let filter = ListPairedRecordsFilter {
            kind: Some("Http".to_string()),
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-http");
        assert_eq!(steps[0].kind, "Http");

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

        let filter = ListPairedRecordsFilter {
            correlation_ids: Some(vec!["step-a".to_string(), "step-\"quoted\"".to_string()]),
            ..step_summary_filter(StepSummarySortOrder::Asc)
        };

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 2);
        // Postgres orders these by the start event's BIGSERIAL `id`, so ascending
        // order is insertion order regardless of `created_at` clock resolution.
        assert_eq!(steps[0].correlation_id, "step-a");
        assert_eq!(steps[1].correlation_id, "step-\"quoted\"");

        let count = persistence
            .count_paired_records(&instance_id, &workflow_vocabulary(), &filter)
            .await
            .unwrap();
        assert_eq!(count, 2);

        // An id that matches nothing filters everything out.
        let filter = ListPairedRecordsFilter {
            correlation_ids: Some(vec!["missing".to_string()]),
            ..filter
        };
        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();
        assert!(steps.is_empty());
        assert_eq!(
            persistence
                .count_paired_records(&instance_id, &workflow_vocabulary(), &filter)
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
            .count_paired_records(&instance_id, &workflow_vocabulary(), &filter)
            .await
            .unwrap();
        assert_eq!(count, 5);

        // Get first page (limit 2)
        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 2, 0)
            .await
            .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].correlation_id, "step-1");
        assert_eq!(steps[1].correlation_id, "step-2");

        // Get second page
        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 2, 2)
            .await
            .unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].correlation_id, "step-3");
        assert_eq!(steps[1].correlation_id, "step-4");

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
        let filter = ListPairedRecordsFilter {
            root_scopes_only: true,
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        // Both step-root and step-scoped have no parent_scope_id
        assert_eq!(steps.len(), 2);
        let step_ids: Vec<_> = steps.iter().map(|s| s.correlation_id.as_str()).collect();
        assert!(step_ids.contains(&"step-root"));
        assert!(step_ids.contains(&"step-scoped"));

        // Filter by scope: the start/end pairing joins on
        // `COALESCE(scope_id,'')`, so a scoped step must still resolve to
        // 'completed' rather than dangling as 'running'.
        let filter = ListPairedRecordsFilter {
            scope_id: Some("sc_main".to_string()),
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-scoped");
        assert_eq!(steps[0].scope_id, Some("sc_main".to_string()));
        assert_eq!(steps[0].status, StepSummaryStatus::Completed);

        // Filter by parent scope
        let filter = ListPairedRecordsFilter {
            parent_scope_id: Some("sc_main".to_string()),
            ..step_summary_filter(StepSummarySortOrder::Desc)
        };

        let steps = persistence
            .list_paired_records(&instance_id, &workflow_vocabulary(), &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].correlation_id, "step-nested");

        cleanup_step_summary_instance(&pool, &instance_id).await;
    }
}
