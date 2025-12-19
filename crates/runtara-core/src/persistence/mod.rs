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
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// Instance this event belongs to.
    pub instance_id: String,
    /// Type of event (heartbeat, completed, failed, suspended).
    pub event_type: String,
    /// Associated checkpoint ID if applicable.
    pub checkpoint_id: Option<String>,
    /// Optional event payload data.
    pub payload: Option<Vec<u8>>,
    /// When the event occurred.
    pub created_at: DateTime<Utc>,
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
}
