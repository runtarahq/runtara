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
    /// How/why the instance reached its terminal state.
    #[sqlx(default)]
    pub termination_reason: Option<String>,
    /// Process exit code if available.
    #[sqlx(default)]
    pub exit_code: Option<i32>,
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
    /// Whether this checkpoint is marked for compensation (saga pattern).
    #[sqlx(default)]
    pub is_compensatable: bool,
    /// Step ID to execute for compensation/rollback.
    #[sqlx(default)]
    pub compensation_step_id: Option<String>,
    /// Serialized data for the compensation step.
    #[sqlx(default)]
    pub compensation_data: Option<Vec<u8>>,
    /// Current state of compensation (none, pending, triggered, completed, failed).
    #[sqlx(default)]
    pub compensation_state: Option<String>,
    /// Order in which to execute compensation (higher = compensate first).
    #[sqlx(default)]
    pub compensation_order: i32,
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

/// Sort order for event queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventSortOrder {
    /// Newest events first (default).
    #[default]
    Desc,
    /// Oldest events first.
    Asc,
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
    /// Filter by scope_id in the event payload (for hierarchy filtering).
    /// When set, only events with matching scope_id in their payload are returned.
    pub scope_id: Option<String>,
    /// Filter by parent_scope_id in the event payload (for hierarchy filtering).
    /// When set, only events with matching parent_scope_id in their payload are returned.
    /// Use this to get direct children of a scope.
    pub parent_scope_id: Option<String>,
    /// When true, only return events that have no parent_scope_id (root-level scopes).
    /// This is useful for getting top-level execution scopes.
    pub root_scopes_only: bool,
    /// Sort order for events by created_at.
    pub sort_order: EventSortOrder,
}

// ============================================================================
// Step Summary Types (for paired step_debug_start/end events)
// ============================================================================

/// Status of a step execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    /// Step is currently running (has start event, no end event yet).
    Running,
    /// Step completed successfully.
    Completed,
    /// Step failed with an error.
    Failed,
}

/// Summary of a step execution, pairing step_debug_start and step_debug_end events.
#[derive(Debug, Clone)]
pub struct StepSummaryRecord {
    /// Unique step identifier within the instance.
    pub step_id: String,
    /// Human-readable step name.
    pub step_name: Option<String>,
    /// Step type (e.g., "Agent", "Conditional", "Split").
    pub step_type: String,
    /// Current status of the step.
    pub status: StepStatus,
    /// When the step started executing.
    pub started_at: DateTime<Utc>,
    /// When the step completed (None if still running).
    pub completed_at: Option<DateTime<Utc>>,
    /// Duration in milliseconds (None if still running).
    pub duration_ms: Option<i64>,
    /// Step inputs from step_debug_start payload.
    pub inputs: Option<serde_json::Value>,
    /// Step outputs from step_debug_end payload.
    pub outputs: Option<serde_json::Value>,
    /// Error details from step_debug_end payload (if failed).
    pub error: Option<serde_json::Value>,
    /// Scope ID for nested execution contexts (Split/While/StartScenario).
    pub scope_id: Option<String>,
    /// Parent scope ID for hierarchy.
    pub parent_scope_id: Option<String>,
}

/// Filter options for listing step summaries.
#[derive(Debug, Clone, Default)]
pub struct ListStepSummariesFilter {
    /// Sort order for steps by started_at.
    pub sort_order: EventSortOrder,
    /// Filter by step status.
    pub status: Option<StepStatus>,
    /// Filter by step type (e.g., "Agent", "Conditional").
    pub step_type: Option<String>,
    /// Filter by scope_id (for steps within a specific scope).
    pub scope_id: Option<String>,
    /// Filter by parent_scope_id (for direct children of a scope).
    pub parent_scope_id: Option<String>,
    /// When true, only return steps with no parent_scope_id (root-level steps).
    pub root_scopes_only: bool,
}

/// Error history record for structured error tracking.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ErrorHistoryRecord {
    /// Database primary key.
    pub id: i64,
    /// Instance this error belongs to.
    pub instance_id: String,
    /// Checkpoint ID where error occurred (if applicable).
    pub checkpoint_id: Option<String>,
    /// Step ID where error occurred (if applicable).
    pub step_id: Option<String>,
    /// Machine-readable error code (e.g., "RATE_LIMITED").
    pub error_code: String,
    /// Human-readable error message.
    pub error_message: String,
    /// Error category (unknown, transient, permanent, business).
    pub category: String,
    /// Error severity (info, warning, error, critical).
    pub severity: String,
    /// Retry hint (unknown, retry_immediately, retry_with_backoff, retry_after, do_not_retry).
    pub retry_hint: Option<String>,
    /// Milliseconds for retry_after hint.
    pub retry_after_ms: Option<i64>,
    /// Additional context as JSON.
    pub attributes: Option<serde_json::Value>,
    /// Cause error ID for error chains.
    pub cause_error_id: Option<i64>,
    /// When the error was recorded.
    pub created_at: DateTime<Utc>,
}

/// Compensation log record for audit trail.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CompensationLogRecord {
    /// Database primary key.
    pub id: i64,
    /// Instance this compensation is for.
    pub instance_id: String,
    /// Checkpoint being compensated.
    pub checkpoint_id: String,
    /// Step executed for compensation.
    pub compensation_step_id: String,
    /// Attempt number (for retries).
    pub attempt_number: i32,
    /// When compensation started.
    pub started_at: DateTime<Utc>,
    /// When compensation finished (None if still running).
    pub finished_at: Option<DateTime<Utc>>,
    /// Whether compensation succeeded.
    pub success: Option<bool>,
    /// Error message if compensation failed.
    pub error_message: Option<String>,
    /// Reference to error_history entry.
    pub error_id: Option<i64>,
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

    /// Complete an instance with termination tracking metadata.
    ///
    /// This version includes `termination_reason` and `exit_code` for unambiguous
    /// identification of how/why the instance terminated.
    ///
    /// Default implementation delegates to `complete_instance_extended`, ignoring
    /// the termination fields.
    #[allow(clippy::too_many_arguments)]
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
        // Default: delegate to extended, ignoring termination fields
        let _ = termination_reason;
        let _ = exit_code;
        self.complete_instance_extended(instance_id, status, output, error, stderr, checkpoint_id)
            .await
    }

    /// Complete an instance with termination tracking, only if status is 'running'.
    ///
    /// Returns true if the update was applied, false if skipped (instance already
    /// has a terminal status).
    #[allow(clippy::too_many_arguments)]
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
        // Default: always apply (returns true)
        self.complete_instance_with_termination(
            instance_id,
            status,
            termination_reason,
            exit_code,
            output,
            error,
            stderr,
            checkpoint_id,
        )
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

    // ========================================================================
    // Step Summaries (paired step_debug_start/end events)
    // ========================================================================

    /// List step summaries for an instance, pairing step_debug_start and step_debug_end events.
    ///
    /// Returns unified step execution records with status, timing, inputs/outputs.
    /// Steps are matched by step_id within the same scope context.
    async fn list_step_summaries(
        &self,
        instance_id: &str,
        filter: &ListStepSummariesFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<StepSummaryRecord>, CoreError>;

    /// Count step summaries for an instance with filtering.
    async fn count_step_summaries(
        &self,
        instance_id: &str,
        filter: &ListStepSummariesFilter,
    ) -> Result<i64, CoreError>;

    // ========================================================================
    // Structured Error Tracking (optional - default implementations no-op)
    // ========================================================================

    /// Record a structured error in the error history table.
    ///
    /// Returns the error ID for chaining or reference.
    #[allow(clippy::too_many_arguments)]
    async fn record_error(
        &self,
        _instance_id: &str,
        _checkpoint_id: Option<&str>,
        _step_id: Option<&str>,
        _error_code: &str,
        _error_message: &str,
        _category: &str,
        _severity: &str,
        _retry_hint: Option<&str>,
        _retry_after_ms: Option<i64>,
        _attributes: Option<&serde_json::Value>,
        _cause_error_id: Option<i64>,
    ) -> Result<i64, CoreError> {
        // Default: no-op, return 0
        Ok(0)
    }

    /// Get the most recent error for an instance.
    async fn get_last_error(
        &self,
        _instance_id: &str,
    ) -> Result<Option<ErrorHistoryRecord>, CoreError> {
        // Default: no-op
        Ok(None)
    }

    /// List errors for an instance.
    async fn list_errors(
        &self,
        _instance_id: &str,
        _limit: i64,
        _offset: i64,
    ) -> Result<Vec<ErrorHistoryRecord>, CoreError> {
        // Default: empty list
        Ok(vec![])
    }

    // ========================================================================
    // Compensation Framework (optional - default implementations no-op)
    // ========================================================================

    /// Mark a checkpoint as compensatable (for saga pattern).
    async fn register_compensatable_checkpoint(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
        _compensation_step_id: &str,
        _compensation_data: Option<&[u8]>,
        _compensation_order: i32,
    ) -> Result<(), CoreError> {
        // Default: no-op
        Ok(())
    }

    /// Get all compensatable checkpoints for an instance (in reverse order).
    async fn get_compensatable_checkpoints(
        &self,
        _instance_id: &str,
    ) -> Result<Vec<CheckpointRecord>, CoreError> {
        // Default: empty list
        Ok(vec![])
    }

    /// Update the compensation state of a checkpoint.
    async fn set_checkpoint_compensation_state(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
        _state: &str,
    ) -> Result<(), CoreError> {
        // Default: no-op
        Ok(())
    }

    /// Update the instance-level compensation state.
    async fn set_instance_compensation_state(
        &self,
        _instance_id: &str,
        _state: &str,
        _reason: Option<&str>,
    ) -> Result<(), CoreError> {
        // Default: no-op
        Ok(())
    }

    /// Log a compensation attempt.
    async fn log_compensation_attempt(
        &self,
        _instance_id: &str,
        _checkpoint_id: &str,
        _compensation_step_id: &str,
        _success: bool,
        _error_message: Option<&str>,
        _error_id: Option<i64>,
    ) -> Result<(), CoreError> {
        // Default: no-op
        Ok(())
    }

    /// Count pending compensations for an instance.
    async fn count_pending_compensations(&self, _instance_id: &str) -> Result<i64, CoreError> {
        // Default: 0
        Ok(0)
    }

    /// Check if all compensations for an instance succeeded.
    async fn all_compensations_succeeded(&self, _instance_id: &str) -> Result<bool, CoreError> {
        // Default: true (no compensations = all succeeded)
        Ok(true)
    }

    // ========================================================================
    // Data Retention / Cleanup (optional - default implementations no-op)
    // ========================================================================

    /// Get terminal instance IDs older than the specified timestamp.
    ///
    /// Only returns instances with terminal status: completed, failed, cancelled.
    /// Returns instance IDs ordered by finished_at (oldest first) for batch processing.
    async fn get_terminal_instances_older_than(
        &self,
        _older_than: DateTime<Utc>,
        _limit: i64,
    ) -> Result<Vec<String>, CoreError> {
        // Default: empty list (no cleanup supported)
        Ok(vec![])
    }

    /// Delete instances by their IDs.
    ///
    /// This deletes from the instances table; child tables with ON DELETE CASCADE
    /// (checkpoints, events, signals, etc.) are automatically cleaned up.
    ///
    /// Environment implementations should override this to clean up environment-specific
    /// tables (container_registry, container_status, etc.) before calling the parent.
    ///
    /// Returns the count of deleted instances.
    async fn delete_instances_batch(&self, _instance_ids: &[String]) -> Result<u64, CoreError> {
        // Default: no-op (no deletion supported)
        Ok(0)
    }
}
