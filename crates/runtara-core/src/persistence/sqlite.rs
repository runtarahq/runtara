//! SQLite-backed persistence implementation.

use std::path::Path;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

use crate::error::CoreError;

use super::{
    CheckpointRecord, CustomSignalRecord, EventRecord, InstanceRecord, ListEventsFilter,
    ListStepSummariesFilter, Persistence, SignalRecord, StepSummaryRecord,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/sqlite");

/// SQLite-backed persistence provider.
#[derive(Clone)]
pub struct SqlitePersistence {
    pool: SqlitePool,
}

impl SqlitePersistence {
    /// Create a new SQLite persistence provider from an existing pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Create and initialize a new SQLite persistence from a file path.
    ///
    /// This convenience constructor handles all setup:
    /// - Creates parent directories if they don't exist
    /// - Creates the database file if it doesn't exist
    /// - Connects to the database with sensible defaults
    /// - Runs all migrations
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the SQLite database file (e.g., ".data/app.db")
    ///
    /// # Example
    ///
    /// ```ignore
    /// let persistence = SqlitePersistence::from_path(".data/embedded.db").await?;
    /// ```
    pub async fn from_path(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        let path = path.as_ref();

        // Create parent directories if needed
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::DatabaseError {
                operation: "create_dir".to_string(),
                details: format!("Failed to create directory {:?}: {}", parent, e),
            })?;
        }

        // Build connection URL
        let path_str = path.to_string_lossy();
        let url = format!("sqlite:{}?mode=rwc", path_str);

        // Create pool with reasonable defaults
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .map_err(|e| CoreError::DatabaseError {
                operation: "connect".to_string(),
                details: format!("Failed to connect to SQLite at {:?}: {}", path, e),
            })?;

        // Run migrations
        MIGRATOR
            .run(&pool)
            .await
            .map_err(|e| CoreError::DatabaseError {
                operation: "migrate".to_string(),
                details: format!("Failed to run migrations: {}", e),
            })?;

        Ok(Self { pool })
    }
}

// Shared operation families materialized by the macros (SYN-394).
crate::persistence::common::ops::impl_instance_ops!(
    SqlitePersistence,
    SqlitePool,
    crate::persistence::dialect::SqliteDialect
);
crate::persistence::common::ops::impl_sleep_ops!(
    SqlitePersistence,
    SqlitePool,
    crate::persistence::dialect::SqliteDialect
);
crate::persistence::common::ops::impl_checkpoint_ops!(
    SqlitePersistence,
    SqlitePool,
    crate::persistence::dialect::SqliteDialect
);
crate::persistence::common::ops::impl_signal_ops!(
    SqlitePersistence,
    SqlitePool,
    crate::persistence::dialect::SqliteDialect
);
crate::persistence::common::ops::impl_event_ops!(
    SqlitePersistence,
    SqlitePool,
    crate::persistence::dialect::SqliteDialect
);
crate::persistence::common::ops::impl_step_summary_ops!(
    SqlitePersistence,
    SqlitePool,
    crate::persistence::dialect::SqliteDialect
);

#[async_trait::async_trait]
impl Persistence for SqlitePersistence {
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
        instance_id: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
    ) -> Result<(), CoreError> {
        Self::op_complete_instance(&self.pool, instance_id, output, error).await
    }

    async fn complete_instance_extended(
        &self,
        instance_id: &str,
        status: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
        stderr: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> Result<(), CoreError> {
        Self::op_complete_instance_extended(
            &self.pool,
            instance_id,
            status,
            output,
            error,
            stderr,
            checkpoint_id,
        )
        .await
    }

    async fn complete_instance_if_running(
        &self,
        instance_id: &str,
        status: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
        stderr: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> Result<bool, CoreError> {
        Self::op_complete_instance_if_running(
            &self.pool,
            instance_id,
            status,
            output,
            error,
            stderr,
            checkpoint_id,
        )
        .await
    }

    async fn complete_instance_with_termination(
        &self,
        instance_id: &str,
        status: &str,
        termination_reason: Option<&str>,
        exit_code: Option<i32>,
        output: Option<&[u8]>,
        error: Option<&str>,
        stderr: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> Result<(), CoreError> {
        Self::op_complete_instance_with_termination(
            &self.pool,
            instance_id,
            status,
            termination_reason,
            exit_code,
            output,
            error,
            stderr,
            checkpoint_id,
        )
        .await
    }

    async fn complete_instance_with_termination_if_running(
        &self,
        instance_id: &str,
        status: &str,
        termination_reason: Option<&str>,
        exit_code: Option<i32>,
        output: Option<&[u8]>,
        error: Option<&str>,
        stderr: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> Result<bool, CoreError> {
        Self::op_complete_instance_with_termination_if_running(
            &self.pool,
            instance_id,
            status,
            termination_reason,
            exit_code,
            output,
            error,
            stderr,
            checkpoint_id,
        )
        .await
    }

    async fn update_instance_metrics(
        &self,
        instance_id: &str,
        memory_peak_bytes: Option<u64>,
        cpu_usage_usec: Option<u64>,
    ) -> Result<(), CoreError> {
        sqlx::query(
            r#"
            UPDATE instances
            SET memory_peak_bytes = ?1,
                cpu_usage_usec = ?2
            WHERE instance_id = ?3
            "#,
        )
        .bind(memory_peak_bytes.map(|v| v as i64))
        .bind(cpu_usage_usec.map(|v| v as i64))
        .bind(instance_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn update_instance_stderr(
        &self,
        instance_id: &str,
        stderr: &str,
    ) -> Result<(), CoreError> {
        sqlx::query(
            r#"
            UPDATE instances
            SET stderr = ?1
            WHERE instance_id = ?2
            "#,
        )
        .bind(stderr)
        .bind(instance_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn store_instance_input(&self, instance_id: &str, input: &[u8]) -> Result<(), CoreError> {
        Self::op_store_instance_input(&self.pool, instance_id, input).await
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
        sqlx::query(
            r#"
            INSERT INTO instance_events (instance_id, event_type, checkpoint_id, payload, created_at, subtype)
            VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, ?)
            "#,
        )
        .bind(&event.instance_id)
        .bind(&event.event_type)
        .bind(&event.checkpoint_id)
        .bind(&event.payload)
        .bind(&event.subtype)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn insert_signal(
        &self,
        instance_id: &str,
        signal_type: &str,
        payload: &[u8],
    ) -> Result<(), CoreError> {
        sqlx::query(
            r#"
            INSERT INTO pending_signals (instance_id, signal_type, payload, created_at)
            VALUES (?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(instance_id) DO UPDATE SET
                signal_type=excluded.signal_type,
                payload=excluded.payload,
                created_at=excluded.created_at,
                acknowledged_at=NULL
            "#,
        )
        .bind(instance_id)
        .bind(signal_type)
        .bind(payload)
        .execute(&self.pool)
        .await?;

        Ok(())
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
        sqlx::query(
            r#"
            INSERT INTO pending_custom_signals (instance_id, checkpoint_id, payload, created_at)
            VALUES (?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(instance_id, checkpoint_id) DO UPDATE SET
                payload=excluded.payload,
                created_at=excluded.created_at
            "#,
        )
        .bind(instance_id)
        .bind(checkpoint_id)
        .bind(payload)
        .execute(&self.pool)
        .await?;

        Ok(())
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
        // Create a unique checkpoint_id for this retry attempt (matching Postgres behavior)
        let retry_checkpoint_id = format!("{}::retry::{}", checkpoint_id, attempt);

        // SYN-394: wrap sqlx errors in `CheckpointSaveFailed` so callers see
        // the instance context — Postgres already does this; SQLite previously
        // relied on the blanket `From<sqlx::Error> for CoreError` impl and
        // surfaced a generic `DatabaseError` instead.
        sqlx::query(
            r#"
            INSERT INTO checkpoints (instance_id, checkpoint_id, state, created_at)
            VALUES (?, ?, ?, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(instance_id)
        .bind(&retry_checkpoint_id)
        .bind(error_message.unwrap_or("").as_bytes())
        .execute(&self.pool)
        .await
        .map_err(|e| crate::persistence::common::error::wrap_checkpoint_save(e, instance_id))?;

        Ok(())
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

    async fn get_terminal_instances_older_than(
        &self,
        older_than: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<String>, CoreError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT instance_id
            FROM instances
            WHERE status IN ('completed', 'failed', 'cancelled')
              AND finished_at IS NOT NULL
              AND finished_at < ?1
            ORDER BY finished_at ASC
            LIMIT ?2
            "#,
        )
        .bind(older_than)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn delete_instances_batch(&self, instance_ids: &[String]) -> Result<u64, CoreError> {
        if instance_ids.is_empty() {
            return Ok(0);
        }

        // SQLite doesn't support ANY(), so use IN with a prepared list
        let placeholders: Vec<String> = (1..=instance_ids.len())
            .map(|i| format!("?{}", i))
            .collect();
        let query = format!(
            "DELETE FROM instances WHERE instance_id IN ({})",
            placeholders.join(", ")
        );

        let mut sqlx_query = sqlx::query(&query);
        for id in instance_ids {
            sqlx_query = sqlx_query.bind(id);
        }

        let result = sqlx_query.execute(&self.pool).await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{EventSortOrder, StepStatus};
    use uuid::Uuid;

    /// Create an in-memory SQLite pool for testing.
    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("Failed to create in-memory SQLite pool");

        MIGRATOR.run(&pool).await.expect("Failed to run migrations");

        pool
    }

    #[tokio::test]
    async fn test_register_and_get_instance() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        let tenant_id = "test-tenant";

        persistence
            .register_instance(&instance_id, tenant_id)
            .await
            .expect("Failed to register instance");

        let instance = persistence
            .get_instance(&instance_id)
            .await
            .expect("Failed to get instance")
            .expect("Instance should exist");

        assert_eq!(instance.instance_id, instance_id);
        assert_eq!(instance.tenant_id, tenant_id);
        assert_eq!(instance.status, "pending");
    }

    #[tokio::test]
    async fn test_get_instance_not_found() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let result = persistence
            .get_instance("nonexistent")
            .await
            .expect("Query should succeed");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_update_instance_status() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .update_instance_status(&instance_id, "running", Some(Utc::now()))
            .await
            .expect("Failed to update status");

        let instance = persistence
            .get_instance(&instance_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(instance.status, "running");
        assert!(instance.started_at.is_some());
    }

    #[tokio::test]
    async fn test_update_instance_checkpoint() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .update_instance_checkpoint(&instance_id, "checkpoint-1")
            .await
            .expect("Failed to update checkpoint");

        let instance = persistence
            .get_instance(&instance_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(instance.checkpoint_id, Some("checkpoint-1".to_string()));
    }

    #[tokio::test]
    async fn test_complete_instance_success() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let output_data = b"success output";
        persistence
            .complete_instance(&instance_id, Some(output_data), None)
            .await
            .expect("Failed to complete instance");

        let instance = persistence
            .get_instance(&instance_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(instance.status, "completed");
        assert_eq!(instance.output, Some(output_data.to_vec()));
        assert!(instance.finished_at.is_some());
    }

    #[tokio::test]
    async fn test_complete_instance_failure() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .complete_instance(&instance_id, None, Some("test error"))
            .await
            .expect("Failed to complete instance");

        let instance = persistence
            .get_instance(&instance_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(instance.status, "failed");
        assert_eq!(instance.error, Some("test error".to_string()));
        assert!(instance.finished_at.is_some());
    }

    #[tokio::test]
    async fn test_save_and_load_checkpoint() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let state = b"test state data";
        persistence
            .save_checkpoint(&instance_id, "cp-1", state)
            .await
            .expect("Failed to save checkpoint");

        let checkpoint = persistence
            .load_checkpoint(&instance_id, "cp-1")
            .await
            .expect("Failed to load checkpoint")
            .expect("Checkpoint should exist");

        assert_eq!(checkpoint.instance_id, instance_id);
        assert_eq!(checkpoint.checkpoint_id, "cp-1");
        assert_eq!(checkpoint.state, state.to_vec());
    }

    #[tokio::test]
    async fn test_load_checkpoint_not_found() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let result = persistence
            .load_checkpoint(&instance_id, "nonexistent")
            .await
            .expect("Query should succeed");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_checkpoints() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .save_checkpoint(&instance_id, "cp-1", b"state-1")
            .await
            .unwrap();
        persistence
            .save_checkpoint(&instance_id, "cp-2", b"state-2")
            .await
            .unwrap();
        persistence
            .save_checkpoint(&instance_id, "cp-3", b"state-3")
            .await
            .unwrap();

        let checkpoints = persistence
            .list_checkpoints(&instance_id, None, 10, 0, None, None)
            .await
            .expect("Failed to list checkpoints");

        assert_eq!(checkpoints.len(), 3);
    }

    #[tokio::test]
    async fn test_list_checkpoints_with_filter() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .save_checkpoint(&instance_id, "cp-1", b"state-1")
            .await
            .unwrap();
        persistence
            .save_checkpoint(&instance_id, "cp-2", b"state-2")
            .await
            .unwrap();

        let checkpoints = persistence
            .list_checkpoints(&instance_id, Some("cp-1"), 10, 0, None, None)
            .await
            .expect("Failed to list checkpoints");

        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].checkpoint_id, "cp-1");
    }

    #[tokio::test]
    async fn test_count_checkpoints() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .save_checkpoint(&instance_id, "cp-1", b"state-1")
            .await
            .unwrap();
        persistence
            .save_checkpoint(&instance_id, "cp-2", b"state-2")
            .await
            .unwrap();

        let count = persistence
            .count_checkpoints(&instance_id, None, None, None)
            .await
            .expect("Failed to count checkpoints");

        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_insert_event() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let event = EventRecord {
            id: None,
            instance_id: instance_id.clone(),
            event_type: "started".to_string(),
            checkpoint_id: None,
            payload: None,
            created_at: Utc::now(),
            subtype: None,
        };

        persistence
            .insert_event(&event)
            .await
            .expect("Failed to insert event");

        // Verify via raw query
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM instance_events WHERE instance_id = ?")
                .bind(&instance_id)
                .fetch_one(&persistence.pool)
                .await
                .unwrap();

        assert_eq!(count.0, 1);
    }

    #[tokio::test]
    async fn test_insert_and_get_signal() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .insert_signal(&instance_id, "cancel", b"reason")
            .await
            .expect("Failed to insert signal");

        let signal = persistence
            .get_pending_signal(&instance_id)
            .await
            .expect("Failed to get signal")
            .expect("Signal should exist");

        assert_eq!(signal.signal_type, "cancel");
        assert_eq!(signal.payload, Some(b"reason".to_vec()));
    }

    #[tokio::test]
    async fn test_get_pending_signal_none() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let signal = persistence
            .get_pending_signal(&instance_id)
            .await
            .expect("Query should succeed");

        assert!(signal.is_none());
    }

    #[tokio::test]
    async fn test_signal_upsert() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .insert_signal(&instance_id, "pause", b"")
            .await
            .unwrap();
        persistence
            .insert_signal(&instance_id, "cancel", b"new reason")
            .await
            .unwrap();

        let signal = persistence
            .get_pending_signal(&instance_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(signal.signal_type, "cancel");
        assert_eq!(signal.payload, Some(b"new reason".to_vec()));
    }

    #[tokio::test]
    async fn test_acknowledge_signal() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .insert_signal(&instance_id, "cancel", b"")
            .await
            .unwrap();

        persistence
            .acknowledge_signal(&instance_id)
            .await
            .expect("Failed to acknowledge signal");

        let signal = persistence
            .get_pending_signal(&instance_id)
            .await
            .unwrap()
            .unwrap();

        assert!(signal.acknowledged_at.is_some());
    }

    #[tokio::test]
    async fn test_insert_and_take_custom_signal() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .insert_custom_signal(&instance_id, "wait-1", b"custom-payload")
            .await
            .expect("Failed to insert custom signal");

        // First take should retrieve and delete
        let signal = persistence
            .take_pending_custom_signal(&instance_id, "wait-1")
            .await
            .expect("Failed to take custom signal")
            .expect("Custom signal should exist");

        assert_eq!(signal.checkpoint_id, "wait-1");
        assert_eq!(signal.payload, Some(b"custom-payload".to_vec()));

        // Second take should return None
        let signal = persistence
            .take_pending_custom_signal(&instance_id, "wait-1")
            .await
            .expect("Query should succeed");

        assert!(signal.is_none());
    }

    #[tokio::test]
    async fn test_custom_signal_upsert() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .insert_custom_signal(&instance_id, "wait-1", b"payload-1")
            .await
            .unwrap();
        persistence
            .insert_custom_signal(&instance_id, "wait-1", b"payload-2")
            .await
            .unwrap();

        let signal = persistence
            .take_pending_custom_signal(&instance_id, "wait-1")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(signal.payload, Some(b"payload-2".to_vec()));
    }

    #[tokio::test]
    async fn test_save_retry_attempt() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .save_retry_attempt(&instance_id, "durable-fn-1", 1, Some("connection error"))
            .await
            .expect("Failed to save retry attempt");

        // Verify the retry checkpoint was created
        let checkpoint = persistence
            .load_checkpoint(&instance_id, "durable-fn-1::retry::1")
            .await
            .unwrap();

        assert!(checkpoint.is_some());
    }

    #[tokio::test]
    async fn test_list_instances() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance1 = Uuid::new_v4().to_string();
        let instance2 = Uuid::new_v4().to_string();

        persistence
            .register_instance(&instance1, "tenant-1")
            .await
            .unwrap();
        persistence
            .register_instance(&instance2, "tenant-2")
            .await
            .unwrap();

        let all = persistence
            .list_instances(None, None, 10, 0)
            .await
            .expect("Failed to list instances");

        assert_eq!(all.len(), 2);

        let tenant1_only = persistence
            .list_instances(Some("tenant-1"), None, 10, 0)
            .await
            .expect("Failed to list instances");

        assert_eq!(tenant1_only.len(), 1);
        assert_eq!(tenant1_only[0].tenant_id, "tenant-1");
    }

    #[tokio::test]
    async fn test_list_instances_by_status() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance1 = Uuid::new_v4().to_string();
        let instance2 = Uuid::new_v4().to_string();

        persistence
            .register_instance(&instance1, "test-tenant")
            .await
            .unwrap();
        persistence
            .register_instance(&instance2, "test-tenant")
            .await
            .unwrap();

        persistence
            .update_instance_status(&instance1, "running", None)
            .await
            .unwrap();

        let running = persistence
            .list_instances(None, Some("running"), 10, 0)
            .await
            .expect("Failed to list instances");

        assert_eq!(running.len(), 1);
        assert_eq!(running[0].instance_id, instance1);
    }

    #[tokio::test]
    async fn test_health_check_db() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let healthy = persistence
            .health_check_db()
            .await
            .expect("Health check failed");

        assert!(healthy);
    }

    #[tokio::test]
    async fn test_count_active_instances() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance1 = Uuid::new_v4().to_string();
        let instance2 = Uuid::new_v4().to_string();
        let instance3 = Uuid::new_v4().to_string();

        persistence
            .register_instance(&instance1, "test-tenant")
            .await
            .unwrap();
        persistence
            .register_instance(&instance2, "test-tenant")
            .await
            .unwrap();
        persistence
            .register_instance(&instance3, "test-tenant")
            .await
            .unwrap();

        persistence
            .update_instance_status(&instance1, "running", None)
            .await
            .unwrap();
        persistence
            .update_instance_status(&instance2, "suspended", None)
            .await
            .unwrap();
        // instance3 stays pending

        let count = persistence
            .count_active_instances()
            .await
            .expect("Failed to count active instances");

        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_complete_instance_extended() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .complete_instance_extended(
                &instance_id,
                "completed",
                Some(b"output data"),
                None,
                Some("stderr output"),
                Some("final-checkpoint"),
            )
            .await
            .expect("Failed to complete instance extended");

        // Verify via raw query (InstanceRecord doesn't include stderr)
        let row: (String, Option<Vec<u8>>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT status, output, stderr, checkpoint_id FROM instances WHERE instance_id = ?",
        )
        .bind(&instance_id)
        .fetch_one(&persistence.pool)
        .await
        .unwrap();

        assert_eq!(row.0, "completed");
        assert_eq!(row.1, Some(b"output data".to_vec()));
        assert_eq!(row.2, Some("stderr output".to_string()));
        assert_eq!(row.3, Some("final-checkpoint".to_string()));
    }

    #[tokio::test]
    async fn test_complete_instance_if_running_success() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();
        persistence
            .update_instance_status(&instance_id, "running", Some(Utc::now()))
            .await
            .unwrap();

        let applied = persistence
            .complete_instance_if_running(
                &instance_id,
                "completed",
                Some(b"done"),
                None,
                None,
                None,
            )
            .await
            .expect("Failed to complete instance");

        assert!(applied);

        let instance = persistence
            .get_instance(&instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, "completed");
    }

    #[tokio::test]
    async fn test_complete_instance_if_running_skipped() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();
        // Status is 'pending', not 'running'

        let applied = persistence
            .complete_instance_if_running(
                &instance_id,
                "completed",
                Some(b"done"),
                None,
                None,
                None,
            )
            .await
            .expect("Query should succeed");

        assert!(!applied);

        let instance = persistence
            .get_instance(&instance_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(instance.status, "pending"); // unchanged
    }

    #[tokio::test]
    async fn test_update_instance_metrics() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .update_instance_metrics(&instance_id, Some(1024 * 1024), Some(500_000))
            .await
            .expect("Failed to update metrics");

        // Verify via raw query
        let row: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT memory_peak_bytes, cpu_usage_usec FROM instances WHERE instance_id = ?",
        )
        .bind(&instance_id)
        .fetch_one(&persistence.pool)
        .await
        .unwrap();

        assert_eq!(row.0, Some(1024 * 1024));
        assert_eq!(row.1, Some(500_000));
    }

    #[tokio::test]
    async fn test_update_instance_stderr() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        persistence
            .update_instance_stderr(&instance_id, "Error: something went wrong\n")
            .await
            .expect("Failed to update stderr");

        // Verify via raw query
        let row: (Option<String>,) =
            sqlx::query_as("SELECT stderr FROM instances WHERE instance_id = ?")
                .bind(&instance_id)
                .fetch_one(&persistence.pool)
                .await
                .unwrap();

        assert_eq!(row.0, Some("Error: something went wrong\n".to_string()));
    }

    #[tokio::test]
    async fn test_store_instance_input() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let input_data = br#"{"key": "value"}"#;
        persistence
            .store_instance_input(&instance_id, input_data)
            .await
            .expect("Failed to store input");

        // Verify via raw query
        let row: (Option<Vec<u8>>,) =
            sqlx::query_as("SELECT input FROM instances WHERE instance_id = ?")
                .bind(&instance_id)
                .fetch_one(&persistence.pool)
                .await
                .unwrap();

        assert_eq!(row.0, Some(input_data.to_vec()));
    }

    // ========================================================================
    // Step Summaries Tests
    // ========================================================================

    /// Helper to insert a step_debug_start event
    #[allow(clippy::too_many_arguments)]
    async fn insert_step_start(
        persistence: &SqlitePersistence,
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

    /// Helper to insert a step_debug_end event
    async fn insert_step_end(
        persistence: &SqlitePersistence,
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

    #[tokio::test]
    async fn test_list_step_summaries_empty() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        let filter = ListStepSummariesFilter {
            sort_order: EventSortOrder::Desc,
            status: None,
            step_type: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
        };

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
    }

    #[tokio::test]
    async fn test_list_step_summaries_completed_step() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        // Insert a completed step (start + end events)
        insert_step_start(
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

        // Small delay to ensure different timestamps
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        insert_step_end(
            &persistence,
            &instance_id,
            "step-1",
            None,
            Some(serde_json::json!({"count": 42})),
            None,
        )
        .await;

        let filter = ListStepSummariesFilter {
            sort_order: EventSortOrder::Desc,
            status: None,
            step_type: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-1");
        assert_eq!(steps[0].step_name, Some("Fetch Data".to_string()));
        assert_eq!(steps[0].step_type, "Http");
        assert_eq!(steps[0].status, StepStatus::Completed);
        assert!(steps[0].completed_at.is_some());
        assert!(steps[0].duration_ms.is_some());
    }

    #[tokio::test]
    async fn test_list_step_summaries_running_step() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        // Insert only start event (no end = running)
        insert_step_start(
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

        let filter = ListStepSummariesFilter {
            sort_order: EventSortOrder::Desc,
            status: None,
            step_type: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-running");
        assert_eq!(steps[0].status, StepStatus::Running);
        assert!(steps[0].completed_at.is_none());
        assert!(steps[0].duration_ms.is_none());
    }

    #[tokio::test]
    async fn test_list_step_summaries_failed_step() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        // Insert a failed step
        insert_step_start(
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

        insert_step_end(
            &persistence,
            &instance_id,
            "step-failed",
            None,
            None,
            Some(serde_json::json!({"message": "Connection refused"})),
        )
        .await;

        let filter = ListStepSummariesFilter {
            sort_order: EventSortOrder::Desc,
            status: None,
            step_type: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-failed");
        assert_eq!(steps[0].status, StepStatus::Failed);
        assert!(steps[0].error.is_some());
    }

    #[tokio::test]
    async fn test_list_step_summaries_filter_by_status() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        // Insert completed step
        insert_step_start(
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
        insert_step_end(
            &persistence,
            &instance_id,
            "step-1",
            None,
            Some(serde_json::json!({})),
            None,
        )
        .await;

        // Insert running step
        insert_step_start(
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
        insert_step_start(
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
        insert_step_end(
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
            sort_order: EventSortOrder::Desc,
            status: Some(StepStatus::Completed),
            step_type: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-1");

        // Filter by running
        let filter = ListStepSummariesFilter {
            status: Some(StepStatus::Running),
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
            status: Some(StepStatus::Failed),
            ..filter
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-3");
    }

    #[tokio::test]
    async fn test_list_step_summaries_filter_by_step_type() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        // Insert Http step
        insert_step_start(
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
        insert_step_end(&persistence, &instance_id, "step-http", None, None, None).await;

        // Insert Transform step
        insert_step_start(
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
        insert_step_end(
            &persistence,
            &instance_id,
            "step-transform",
            None,
            None,
            None,
        )
        .await;

        let filter = ListStepSummariesFilter {
            sort_order: EventSortOrder::Desc,
            status: None,
            step_type: Some("Http".to_string()),
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-http");
        assert_eq!(steps[0].step_type, "Http");
    }

    #[tokio::test]
    async fn test_list_step_summaries_pagination() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        // Insert 5 steps
        for i in 1..=5 {
            insert_step_start(
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
            insert_step_end(
                &persistence,
                &instance_id,
                &format!("step-{}", i),
                None,
                None,
                None,
            )
            .await;
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        }

        let filter = ListStepSummariesFilter {
            sort_order: EventSortOrder::Asc,
            status: None,
            step_type: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: false,
        };

        // Get total count
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
    }

    #[tokio::test]
    async fn test_list_step_summaries_with_scopes() {
        let pool = test_pool().await;
        let persistence = SqlitePersistence::new(pool);

        let instance_id = Uuid::new_v4().to_string();
        persistence
            .register_instance(&instance_id, "test-tenant")
            .await
            .unwrap();

        // Root level step
        insert_step_start(
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
        insert_step_end(&persistence, &instance_id, "step-root", None, None, None).await;

        // Step in scope
        insert_step_start(
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
        insert_step_end(
            &persistence,
            &instance_id,
            "step-scoped",
            Some("sc_main"),
            None,
            None,
        )
        .await;

        // Nested step
        insert_step_start(
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
        insert_step_end(
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
            sort_order: EventSortOrder::Desc,
            status: None,
            step_type: None,
            scope_id: None,
            parent_scope_id: None,
            root_scopes_only: true,
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

        // Filter by parent scope
        let filter = ListStepSummariesFilter {
            sort_order: EventSortOrder::Desc,
            status: None,
            step_type: None,
            scope_id: None,
            parent_scope_id: Some("sc_main".to_string()),
            root_scopes_only: false,
        };

        let steps = persistence
            .list_step_summaries(&instance_id, &filter, 100, 0)
            .await
            .unwrap();

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "step-nested");
    }
}
