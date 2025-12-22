//! SQLite-backed persistence implementation.

use std::path::Path;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

use crate::error::CoreError;

use super::{
    CheckpointRecord, CustomSignalRecord, EventRecord, InstanceRecord, Persistence, SignalRecord,
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

#[async_trait::async_trait]
impl Persistence for SqlitePersistence {
    async fn register_instance(&self, instance_id: &str, tenant_id: &str) -> Result<(), CoreError> {
        sqlx::query(
            r#"
            INSERT INTO instances (instance_id, tenant_id, definition_version, status, created_at)
            VALUES (?, ?, 1, 'pending', CURRENT_TIMESTAMP)
            "#,
        )
        .bind(instance_id)
        .bind(tenant_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError> {
        let record = sqlx::query_as::<_, InstanceRecord>(
            r#"
            SELECT instance_id, tenant_id, definition_version,
                   status as status, checkpoint_id, attempt, max_attempts,
                   created_at, started_at, finished_at, output, error, sleep_until
            FROM instances
            WHERE instance_id = ?
            "#,
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    async fn update_instance_status(
        &self,
        instance_id: &str,
        status: &str,
        started_at: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError> {
        if let Some(started) = started_at {
            sqlx::query(
                r#"
                UPDATE instances
                SET status = ?, started_at = ?
                WHERE instance_id = ?
                "#,
            )
            .bind(status)
            .bind(started)
            .bind(instance_id)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE instances
                SET status = ?
                WHERE instance_id = ?
                "#,
            )
            .bind(status)
            .bind(instance_id)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    async fn update_instance_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), CoreError> {
        sqlx::query(
            r#"
            UPDATE instances
            SET checkpoint_id = ?
            WHERE instance_id = ?
            "#,
        )
        .bind(checkpoint_id)
        .bind(instance_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn complete_instance(
        &self,
        instance_id: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
    ) -> Result<(), CoreError> {
        sqlx::query(
            r#"
            UPDATE instances
            SET status = CASE
                    WHEN ?1 IS NOT NULL THEN 'failed'
                    ELSE 'completed'
                END,
                finished_at = CURRENT_TIMESTAMP,
                output = ?2,
                error = ?1
            WHERE instance_id = ?3
            "#,
        )
        .bind(error)
        .bind(output)
        .bind(instance_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn save_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<(), CoreError> {
        sqlx::query(
            r#"
            INSERT INTO checkpoints (instance_id, checkpoint_id, state, created_at)
            VALUES (?, ?, ?, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(instance_id)
        .bind(checkpoint_id)
        .bind(state)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn load_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CheckpointRecord>, CoreError> {
        let record = sqlx::query_as::<_, CheckpointRecord>(
            r#"
            SELECT id, instance_id, checkpoint_id, state, created_at
            FROM checkpoints
            WHERE instance_id = ? AND checkpoint_id = ?
            "#,
        )
        .bind(instance_id)
        .bind(checkpoint_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
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
        // Use SQLite's NULL coalescing pattern similar to Postgres
        let rows = sqlx::query_as::<_, CheckpointRecord>(
            r#"
            SELECT id, instance_id, checkpoint_id, state, created_at
            FROM checkpoints
            WHERE instance_id = ?1
              AND (?2 IS NULL OR checkpoint_id = ?2)
              AND (?3 IS NULL OR created_at >= ?3)
              AND (?4 IS NULL OR created_at < ?4)
            ORDER BY created_at DESC
            LIMIT ?5 OFFSET ?6
            "#,
        )
        .bind(instance_id)
        .bind(checkpoint_id)
        .bind(created_after)
        .bind(created_before)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    async fn count_checkpoints(
        &self,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<i64, CoreError> {
        let count: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM checkpoints
            WHERE instance_id = ?1
              AND (?2 IS NULL OR checkpoint_id = ?2)
              AND (?3 IS NULL OR created_at >= ?3)
              AND (?4 IS NULL OR created_at < ?4)
            "#,
        )
        .bind(instance_id)
        .bind(checkpoint_id)
        .bind(created_after)
        .bind(created_before)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0)
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
        let record = sqlx::query_as::<_, SignalRecord>(
            r#"
            SELECT instance_id, signal_type, payload, created_at, acknowledged_at
            FROM pending_signals
            WHERE instance_id = ?
            "#,
        )
        .bind(instance_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    async fn acknowledge_signal(&self, instance_id: &str) -> Result<(), CoreError> {
        sqlx::query(
            r#"
            UPDATE pending_signals
            SET acknowledged_at = CURRENT_TIMESTAMP
            WHERE instance_id = ? AND acknowledged_at IS NULL
            "#,
        )
        .bind(instance_id)
        .execute(&self.pool)
        .await?;

        Ok(())
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
        let mut tx = self.pool.begin().await?;

        let record = sqlx::query_as::<_, CustomSignalRecord>(
            r#"
            SELECT instance_id, checkpoint_id, payload, created_at
            FROM pending_custom_signals
            WHERE instance_id = ? AND checkpoint_id = ?
            "#,
        )
        .bind(instance_id)
        .bind(checkpoint_id)
        .fetch_optional(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            DELETE FROM pending_custom_signals
            WHERE instance_id = ? AND checkpoint_id = ?
            "#,
        )
        .bind(instance_id)
        .bind(checkpoint_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(record)
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
        .await?;

        Ok(())
    }

    async fn list_instances(
        &self,
        tenant_id: Option<&str>,
        status: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        let records = sqlx::query_as::<_, InstanceRecord>(
            r#"
            SELECT instance_id, tenant_id, definition_version,
                   status as status, checkpoint_id, attempt, max_attempts,
                   created_at, started_at, finished_at, output, error, sleep_until
            FROM instances
            WHERE (?1 IS NULL OR tenant_id = ?1)
              AND (?2 IS NULL OR status = ?2)
            ORDER BY created_at DESC
            LIMIT ?3 OFFSET ?4
            "#,
        )
        .bind(tenant_id)
        .bind(status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    async fn health_check_db(&self) -> Result<bool, CoreError> {
        let result: Result<(i64,), _> = sqlx::query_as("SELECT 1").fetch_one(&self.pool).await;
        Ok(result.is_ok())
    }

    async fn count_active_instances(&self) -> Result<i64, CoreError> {
        let row: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM instances
            WHERE status IN ('running', 'suspended')
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }

    async fn set_instance_sleep(
        &self,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let result = sqlx::query(
            r#"
            UPDATE instances
            SET sleep_until = ?
            WHERE instance_id = ?
            "#,
        )
        .bind(sleep_until)
        .bind(instance_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(CoreError::InstanceNotFound {
                instance_id: instance_id.to_string(),
            });
        }

        Ok(())
    }

    async fn clear_instance_sleep(&self, instance_id: &str) -> Result<(), CoreError> {
        let result = sqlx::query(
            r#"
            UPDATE instances
            SET sleep_until = NULL
            WHERE instance_id = ?
            "#,
        )
        .bind(instance_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(CoreError::InstanceNotFound {
                instance_id: instance_id.to_string(),
            });
        }

        Ok(())
    }

    async fn get_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        let records = sqlx::query_as::<_, InstanceRecord>(
            r#"
            SELECT instance_id, tenant_id, definition_version,
                   status as status, checkpoint_id, attempt, max_attempts,
                   created_at, started_at, finished_at, output, error, sleep_until
            FROM instances
            WHERE sleep_until IS NOT NULL
              AND sleep_until <= datetime('now')
              AND status = 'suspended'
            ORDER BY sleep_until ASC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
