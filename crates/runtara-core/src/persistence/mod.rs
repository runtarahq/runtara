//! Persistence interfaces and backends for runtara-core.
//!
//! This module defines the persistence abstraction and backend implementations.

pub mod postgres;
pub mod sqlite;

pub use self::postgres::PostgresPersistence;
pub use self::sqlite::SqlitePersistence;

use crate::error::CoreError;

/// Instance record from the persistence layer.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstanceRecord {
    /// Unique identifier for the instance.
    pub instance_id: String,
    /// Tenant identifier for multi-tenancy isolation.
    pub tenant_id: String,
    /// Version of the workflow definition.
    pub definition_version: i32,
    /// Current status (pending, running, suspended, completed, failed, cancelled).
    pub status: String,
    /// Last checkpoint ID if instance was checkpointed.
    pub checkpoint_id: Option<String>,
    /// Current attempt number (for retries).
    pub attempt: i32,
    /// Maximum allowed attempts before permanent failure.
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
    /// When a sleeping instance should be woken.
    pub sleep_until: Option<DateTime<Utc>>,
}

/// Checkpoint record from the persistence layer.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CheckpointRecord {
    /// Database primary key.
    pub id: i64,
    /// Instance this checkpoint belongs to.
    pub instance_id: String,
    /// Unique checkpoint identifier within the instance.
    pub checkpoint_id: String,
    /// Serialized state data.
    pub state: Vec<u8>,
    /// When the checkpoint was created.
    pub created_at: DateTime<Utc>,
}

/// Event record from the persistence layer.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EventRecord {
    /// Database primary key (None when inserting new events).
    #[sqlx(default)]
    pub id: Option<i64>,
    /// Instance this event belongs to.
    pub instance_id: String,
    /// Type of event (heartbeat, completed, failed, suspended, custom).
    pub event_type: String,
    /// Associated checkpoint ID if applicable.
    pub checkpoint_id: Option<String>,
    /// Optional event payload data.
    pub payload: Option<Vec<u8>>,
    /// When the event occurred.
    pub created_at: DateTime<Utc>,
    /// Arbitrary subtype for custom events.
    pub subtype: Option<String>,
}

/// Signal record from the persistence layer.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SignalRecord {
    /// Instance this signal is for.
    pub instance_id: String,
    /// Type of signal (cancel, pause, resume).
    pub signal_type: String,
    /// Optional signal payload data.
    pub payload: Option<Vec<u8>>,
    /// When the signal was created.
    pub created_at: DateTime<Utc>,
    /// When the signal was acknowledged by the instance.
    pub acknowledged_at: Option<DateTime<Utc>>,
}

/// Pending custom signal scoped to a specific checkpoint.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CustomSignalRecord {
    /// Instance this signal is for.
    pub instance_id: String,
    /// Target checkpoint/wait key.
    pub checkpoint_id: String,
    /// Optional payload.
    pub payload: Option<Vec<u8>>,
    /// When the signal was created.
    pub created_at: DateTime<Utc>,
}

/// Filter options for listing events.
#[derive(Debug, Clone, Default)]
pub struct ListEventsFilter {
    /// Filter by event type (e.g., "custom", "started", "completed").
    pub event_type: Option<String>,
    /// Filter by subtype (e.g., "step_debug_start", "step_debug_end", "workflow_log").
    pub subtype: Option<String>,
    /// Filter events created at or after this time.
    pub created_after: Option<DateTime<Utc>>,
    /// Filter events created before this time.
    pub created_before: Option<DateTime<Utc>>,
    /// Full-text search in JSON payload content.
    pub payload_contains: Option<String>,
}

/// Wake queue entry from the persistence layer.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WakeEntry {
    /// Database primary key.
    pub id: i64,
    /// Instance to wake.
    pub instance_id: String,
    /// Checkpoint to resume from.
    pub checkpoint_id: String,
    /// When to wake the instance.
    pub wake_at: DateTime<Utc>,
    /// When this wake entry was created.
    pub created_at: DateTime<Utc>,
}
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Persistence interface used by core handlers.
#[allow(missing_docs)]
#[async_trait]
pub trait Persistence: Send + Sync {
    async fn register_instance(&self, instance_id: &str, tenant_id: &str) -> Result<(), CoreError>;

    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError>;

    async fn update_instance_status(
        &self,
        instance_id: &str,
        status: &str,
        started_at: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError>;

    async fn update_instance_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<(), CoreError>;

    async fn complete_instance(
        &self,
        instance_id: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
    ) -> Result<(), CoreError>;

    /// Complete an instance with extended fields (stderr, checkpoint).
    ///
    /// This is an extended version of `complete_instance` that supports additional
    /// fields needed by the Environment layer. The default implementation calls
    /// the basic `complete_instance` and ignores the extra fields.
    ///
    /// Environment implementations should override this to store stderr and checkpoint.
    async fn complete_instance_extended(
        &self,
        instance_id: &str,
        status: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
        _stderr: Option<&str>,
        _checkpoint_id: Option<&str>,
    ) -> Result<(), CoreError> {
        // Default: delegate to basic complete_instance, ignoring stderr/checkpoint
        let error_for_basic = if status == "failed" { error } else { None };
        let output_for_basic = if status == "completed" { output } else { None };
        self.complete_instance(instance_id, output_for_basic, error_for_basic)
            .await
    }

    /// Complete an instance only if its current status is 'running'.
    ///
    /// This prevents race conditions where both Core (via SDK) and Environment
    /// (via container monitor) try to complete the same instance. Returns true
    /// if the update was applied, false if skipped.
    ///
    /// Default implementation always applies the update (no guard).
    async fn complete_instance_if_running(
        &self,
        instance_id: &str,
        status: &str,
        output: Option<&[u8]>,
        error: Option<&str>,
        stderr: Option<&str>,
        checkpoint_id: Option<&str>,
    ) -> Result<bool, CoreError> {
        // Default: always apply (returns true)
        self.complete_instance_extended(instance_id, status, output, error, stderr, checkpoint_id)
            .await?;
        Ok(true)
    }

    /// Update execution metrics for an instance (memory, CPU usage).
    ///
    /// This is an environment-specific operation for storing cgroup metrics.
    /// Core implementations can ignore this (default is no-op).
    async fn update_instance_metrics(
        &self,
        _instance_id: &str,
        _memory_peak_bytes: Option<u64>,
        _cpu_usage_usec: Option<u64>,
    ) -> Result<(), CoreError> {
        // Default: no-op (Core doesn't track metrics)
        Ok(())
    }

    /// Update instance stderr output.
    ///
    /// This is an environment-specific operation for storing container stderr.
    /// Core implementations can ignore this (default is no-op).
    async fn update_instance_stderr(
        &self,
        _instance_id: &str,
        _stderr: &str,
    ) -> Result<(), CoreError> {
        // Default: no-op (Core doesn't track stderr)
        Ok(())
    }

    /// Store input data for an instance.
    ///
    /// This is an environment-specific operation for storing instance input.
    /// Core implementations can ignore this (default is no-op).
    async fn store_instance_input(
        &self,
        _instance_id: &str,
        _input: &[u8],
    ) -> Result<(), CoreError> {
        // Default: no-op (Core doesn't store input)
        Ok(())
    }

    async fn save_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<(), CoreError>;

    async fn load_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CheckpointRecord>, CoreError>;

    async fn list_checkpoints(
        &self,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        limit: i64,
        offset: i64,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<CheckpointRecord>, CoreError>;

    async fn count_checkpoints(
        &self,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<i64, CoreError>;

    async fn insert_event(&self, event: &EventRecord) -> Result<(), CoreError>;

    async fn insert_signal(
        &self,
        instance_id: &str,
        signal_type: &str,
        payload: &[u8],
    ) -> Result<(), CoreError>;

    async fn get_pending_signal(
        &self,
        instance_id: &str,
    ) -> Result<Option<SignalRecord>, CoreError>;

    async fn acknowledge_signal(&self, instance_id: &str) -> Result<(), CoreError>;

    async fn insert_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        payload: &[u8],
    ) -> Result<(), CoreError>;

    async fn take_pending_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CustomSignalRecord>, CoreError>;

    async fn save_retry_attempt(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        attempt: i32,
        error_message: Option<&str>,
    ) -> Result<(), CoreError>;

    async fn list_instances(
        &self,
        tenant_id: Option<&str>,
        status: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError>;

    async fn health_check_db(&self) -> Result<bool, CoreError>;

    async fn count_active_instances(&self) -> Result<i64, CoreError>;

    /// Set the sleep_until timestamp for an instance.
    async fn set_instance_sleep(
        &self,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError>;

    /// Clear the sleep_until timestamp for an instance.
    async fn clear_instance_sleep(&self, instance_id: &str) -> Result<(), CoreError>;

    /// Get instances that are due to wake (sleep_until <= now).
    async fn get_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError>;

    /// List events for an instance with filtering and pagination.
    ///
    /// Events are returned in reverse chronological order (newest first).
    async fn list_events(
        &self,
        instance_id: &str,
        filter: &ListEventsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<EventRecord>, CoreError>;

    /// Count events for an instance with filtering.
    async fn count_events(
        &self,
        instance_id: &str,
        filter: &ListEventsFilter,
    ) -> Result<i64, CoreError>;
}
