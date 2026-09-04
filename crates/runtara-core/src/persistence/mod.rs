//! Persistence interfaces and backends for runtara-core.
//!
//! This module defines the persistence abstraction and backend implementations.

pub(crate) mod common;
pub(crate) mod dialect;
pub mod postgres;
pub mod vocabulary;

pub use self::postgres::PostgresPersistence;
pub use self::vocabulary::{EventVocabulary, EventVocabularySpec};

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
    /// Input data provided at launch time.
    #[sqlx(default)]
    pub input: Option<Vec<u8>>,
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
    /// Consecutive no-progress auto-restarts after an Environment restart.
    /// Reset to 0 when the instance's checkpoint count advances between
    /// recoveries. See [`Persistence::mark_for_recovery`].
    #[sqlx(default)]
    pub recovery_attempts: i32,
    /// Checkpoint count observed at the last auto-recovery, as text. Compared
    /// against the current count to distinguish "made progress" from "stuck".
    #[sqlx(default)]
    pub recovery_marker: Option<String>,
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
    /// Filter by the producer's event subtype. Opaque to this crate.
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
// Paired Record Types
// ============================================================================

/// Status of a paired record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairedRecordStatus {
    /// The start event has arrived and no end event has paired with it yet.
    Running,
    /// The record closed without a failure.
    Completed,
    /// The record closed carrying failure detail, either under the
    /// vocabulary's error key or via its output error flag.
    Failed,
}

/// One unit of work, assembled from the start and end events that share a
/// correlation id within the same scope.
///
/// The field names are this crate's own. What the producer calls them is
/// supplied per query by an [`EventVocabulary`].
#[derive(Debug, Clone)]
pub struct PairedRecordSummary {
    /// Correlation id, unique within the instance and scope.
    pub correlation_id: String,
    /// Human-readable label, if the producer emitted one.
    pub label: Option<String>,
    /// Opaque classifier from the producer, exposed for filtering. This crate
    /// never matches on its value.
    pub kind: String,
    /// Current status of the record.
    pub status: PairedRecordStatus,
    /// When the start event was recorded.
    pub started_at: DateTime<Utc>,
    /// When the end event was recorded (None while still running).
    pub completed_at: Option<DateTime<Utc>>,
    /// Duration in milliseconds (None while still running).
    pub duration_ms: Option<i64>,
    /// Optional real launch wall-clock (epoch ms) of concurrent work, from the
    /// end event's payload. Present only for records that ran concurrently;
    /// pairs with [`Self::settled_at_ms`] to describe the true overlapping
    /// interval, versus `started_at`/`duration_ms` (which this summary derives
    /// from the sequential event rows).
    pub launched_at_ms: Option<i64>,
    /// Optional real settle wall-clock (epoch ms). See [`Self::launched_at_ms`].
    pub settled_at_ms: Option<i64>,
    /// Input recorded on the start event.
    pub inputs: Option<serde_json::Value>,
    /// Output recorded on the end event.
    pub outputs: Option<serde_json::Value>,
    /// Failure detail from the end event, if the record failed.
    pub error: Option<serde_json::Value>,
    /// Scope id for nested execution contexts. Opaque to this crate — it is
    /// only ever compared, never interpreted.
    pub scope_id: Option<String>,
    /// Enclosing scope id, for hierarchy.
    pub parent_scope_id: Option<String>,
}

/// Filter options for listing paired records.
#[derive(Debug, Clone, Default)]
pub struct ListPairedRecordsFilter {
    /// Sort order by start event.
    pub sort_order: EventSortOrder,
    /// Filter by record status.
    pub status: Option<PairedRecordStatus>,
    /// Filter by the producer's opaque classifier.
    pub kind: Option<String>,
    /// Filter by scope_id (records within a specific scope).
    pub scope_id: Option<String>,
    /// Filter by parent_scope_id (direct children of a scope).
    pub parent_scope_id: Option<String>,
    /// When true, only return records with no parent_scope_id (root-level).
    pub root_scopes_only: bool,
    /// Only return records whose correlation id is in this set. `None` means
    /// no correlation-id filtering; an empty vec matches nothing.
    pub correlation_ids: Option<Vec<String>>,
}

use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Whether a `complete_instance` call should apply unconditionally or only
/// when the target row is still in the `running` state.
///
/// The `OnlyRunning` guard exists to prevent races between two independent
/// writers (typically: the SDK reporting a terminal status, and the
/// container monitor observing a process exit) from clobbering one another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CompleteInstanceGuard {
    /// No guard — the update applies regardless of the current status.
    /// A missing row is reported as [`CoreError::InstanceNotFound`].
    #[default]
    Any,
    /// Apply the update only if the current status is `running`. A miss
    /// (row exists but has a different status) is reported as `Ok(false)`
    /// rather than an error.
    OnlyRunning,
}

/// Parameters for [`Persistence::complete_instance`], transitioning an
/// instance to a terminal or quasi-terminal state.
///
/// All optional fields use COALESCE semantics on the persistence side —
/// `None` leaves the existing column value unchanged. The required fields
/// `instance_id` and `status` borrow from the caller; most call sites
/// already hold `&str` locals and can pass them directly.
///
/// Build with [`CompleteInstanceParams::new`] and the chained `with_*`
/// setters.
#[derive(Debug, Clone, Default)]
pub struct CompleteInstanceParams<'a> {
    /// Instance being completed.
    pub instance_id: &'a str,
    /// Target status. One of `completed`, `failed`, `cancelled`,
    /// `suspended`, or `running` (for mid-execution transitions that
    /// carry metadata but don't finalize the instance).
    pub status: &'a str,
    /// Whether to guard against races by requiring the current status
    /// to be `running`. See [`CompleteInstanceGuard`].
    pub guard: CompleteInstanceGuard,
    /// Output blob from successful completion.
    pub output: Option<&'a [u8]>,
    /// Error message from failure.
    pub error: Option<&'a str>,
    /// Container stderr captured at termination time.
    pub stderr: Option<&'a str>,
    /// Checkpoint identifier to associate with this state.
    pub checkpoint_id: Option<&'a str>,
    /// How/why the instance reached this terminal state (timeout, crash,
    /// shutdown_requested, heartbeat_timeout, oom, etc.).
    pub termination_reason: Option<&'a str>,
    /// Process exit code if available.
    pub exit_code: Option<i32>,
}

impl<'a> CompleteInstanceParams<'a> {
    /// Start a minimal completion request targeting `status`.
    pub fn new(instance_id: &'a str, status: &'a str) -> Self {
        Self {
            instance_id,
            status,
            ..Default::default()
        }
    }

    /// Guard the update against races: only apply when the current status
    /// is `running`.
    #[must_use]
    pub fn if_running(mut self) -> Self {
        self.guard = CompleteInstanceGuard::OnlyRunning;
        self
    }

    /// Attach an output blob.
    #[must_use]
    pub fn with_output(mut self, output: &'a [u8]) -> Self {
        self.output = Some(output);
        self
    }

    /// Attach an error message.
    #[must_use]
    pub fn with_error(mut self, error: &'a str) -> Self {
        self.error = Some(error);
        self
    }

    /// Attach captured stderr.
    #[must_use]
    pub fn with_stderr(mut self, stderr: &'a str) -> Self {
        self.stderr = Some(stderr);
        self
    }

    /// Associate a checkpoint with this state transition.
    #[must_use]
    pub fn with_checkpoint(mut self, checkpoint_id: &'a str) -> Self {
        self.checkpoint_id = Some(checkpoint_id);
        self
    }

    /// Record the termination reason and optional exit code.
    #[must_use]
    pub fn with_termination(mut self, reason: &'a str, exit_code: Option<i32>) -> Self {
        self.termination_reason = Some(reason);
        self.exit_code = exit_code;
        self
    }
}

/// Persistence interface used by core handlers.
#[allow(missing_docs)]
#[async_trait]
pub trait Persistence: Send + Sync {
    async fn register_instance(&self, instance_id: &str, tenant_id: &str) -> Result<(), CoreError>;

    /// Register an instance, reporting whether this call created the row.
    ///
    /// `Ok(true)` means this caller inserted it; `Ok(false)` means the id was
    /// already taken. Callers that treat the instance id as an idempotency key
    /// can use this to claim the id and learn they lost the race in a single
    /// statement, instead of a speculative `get_instance` before every insert.
    ///
    /// `input` is persisted by the same statement on the backends that can,
    /// rather than by a follow-up `store_instance_input`.
    ///
    /// This default is the naive read-then-insert and is *not* atomic; it
    /// exists so in-memory and test backends need no change. Backends that can
    /// do it in one statement should override it.
    async fn try_register_instance(
        &self,
        instance_id: &str,
        tenant_id: &str,
        input: Option<&[u8]>,
    ) -> Result<bool, CoreError> {
        if self.get_instance(instance_id).await?.is_some() {
            return Ok(false);
        }
        self.register_instance(instance_id, tenant_id).await?;
        if let Some(input) = input {
            self.store_instance_input(instance_id, input).await?;
        }
        Ok(true)
    }

    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError>;

    /// Like [`Self::get_instance`] but without the `input` blob, for callers
    /// that only need status/tenant/recovery state.
    ///
    /// The returned record always has `input: None` — that is the point, not a
    /// missing row. Never use this when the input is what you came for; the
    /// launch payload can be large, and reading it back on every status check
    /// is what this exists to avoid.
    ///
    /// Defaults to the full read so in-memory and test backends need no change.
    async fn get_instance_meta(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceRecord>, CoreError> {
        self.get_instance(instance_id).await
    }

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

    /// Transition an instance to a terminal or quasi-terminal state.
    ///
    /// Single consolidated entry point for what were previously five
    /// overlapping `complete_instance*` variants. The behavior is
    /// controlled entirely by the [`CompleteInstanceParams`] struct —
    /// see its documentation for the per-field semantics (COALESCE vs.
    /// overwrite, terminal-only `finished_at`, guard against races).
    ///
    /// Return value:
    /// - `Ok(true)` — the update matched a row.
    /// - `Ok(false)` — guarded update
    ///   ([`CompleteInstanceGuard::OnlyRunning`]) skipped because the
    ///   current status is not `running`. This is an expected outcome
    ///   during races, not an error.
    /// - `Err(CoreError::InstanceNotFound)` — unguarded update against
    ///   a missing row.
    async fn complete_instance(
        &self,
        params: CompleteInstanceParams<'_>,
    ) -> Result<bool, CoreError>;

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

    /// Append an event to an instance's timeline.
    ///
    /// `event.created_at` is the time the emitter observed, and an
    /// implementation must store it verbatim — never substituting its own
    /// write time by defaulting the column. Readers order events by this
    /// column, and [`Self::list_paired_records`] derives every duration from
    /// the delta between a record's paired start and end events, so a
    /// receive-time stamp silently reorders the timeline and rewrites every
    /// duration into the interval between two writes.
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

    /// Record execution metrics and return the instance's current status and
    /// termination reason, in one statement where the backend supports it.
    ///
    /// For the container monitor, which writes what it collected at termination
    /// and then has to read the status the SDK reported. `None` means no such
    /// instance. The default is the separate write-then-read this replaces.
    async fn update_metrics_returning_status(
        &self,
        instance_id: &str,
        memory_peak_bytes: Option<u64>,
        cpu_usage_usec: Option<u64>,
    ) -> Result<Option<(String, Option<String>)>, CoreError> {
        self.update_instance_metrics(instance_id, memory_peak_bytes, cpu_usage_usec)
            .await?;
        Ok(self
            .get_instance_meta(instance_id)
            .await?
            .map(|i| (i.status, i.termination_reason)))
    }

    /// Promote an instance to `running` on a relaunch, preserving its
    /// original `started_at`.
    ///
    /// For wake and resume, which promote from `suspended` — a state
    /// [`Self::mark_instance_started`] deliberately refuses. The default is the
    /// read-then-write this replaces; SQL backends do it in one statement.
    async fn mark_instance_running(
        &self,
        instance_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let started_at = match self.get_instance(instance_id).await {
            Ok(Some(instance)) => instance.started_at.unwrap_or(started_at),
            _ => started_at,
        };
        self.update_instance_status(instance_id, "running", Some(started_at))
            .await
    }

    /// Promote an instance to `running`, but only if it has not already moved
    /// past the pre-run states. Returns whether the promotion applied.
    ///
    /// Exists because a detached launch returns as soon as the run is spawned:
    /// a workflow that parks immediately (a `Delay` or a `WaitForSignal`) can
    /// be `suspended` before the launching caller stamps `running`. Writing
    /// `running` unconditionally at that point resurrects a parked instance
    /// with no live process behind it, and the container monitor then fails it
    /// as a crash. Callers stamping `running` *after* a launch must use this;
    /// callers that stamp it *before* launching can use
    /// [`Persistence::update_instance_status`] directly.
    ///
    /// The default implementation reads-then-writes and is adequate for
    /// in-memory backends; SQL backends override it with a single guarded
    /// UPDATE.
    async fn mark_instance_started(
        &self,
        instance_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<bool, CoreError> {
        match self.get_instance(instance_id).await? {
            Some(inst) if matches!(inst.status.as_str(), "pending" | "running") => {
                self.update_instance_status(
                    instance_id,
                    "running",
                    Some(inst.started_at.unwrap_or(started_at)),
                )
                .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Set the sleep_until timestamp for an instance.
    async fn set_instance_sleep(
        &self,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError>;

    /// Clear the sleep_until timestamp for an instance.
    async fn clear_instance_sleep(&self, instance_id: &str) -> Result<(), CoreError>;

    /// Atomically claim a due sleeping instance before waking it.
    ///
    /// Conditionally clears `sleep_until` only while the instance is still
    /// `status='suspended'` with a non-null `sleep_until`, and reports whether
    /// this caller won the claim. Returns `true` if it did, `false` if another
    /// waker (or a second Environment sharing this Core DB) already took it.
    /// Callers MUST launch only when this returns `true` — this is what
    /// prevents concurrent double-launch of the same instance. On a launch
    /// failure after a successful claim, re-stamp `sleep_until` via
    /// [`Persistence::set_instance_sleep`] so the instance is retried.
    ///
    /// The default implementation is a non-atomic best-effort fallback for
    /// in-memory/mock backends; the SQL backends override it with a single
    /// conditional UPDATE whose row-count is the claim outcome.
    async fn claim_sleeping_instance(&self, instance_id: &str) -> Result<bool, CoreError> {
        self.clear_instance_sleep(instance_id).await?;
        Ok(true)
    }

    /// Mark an instance for automatic recovery after an Environment restart.
    ///
    /// Sets `status='suspended'`, `termination_reason='environment_restart'`,
    /// `sleep_until=NOW()` (so the wake scheduler relaunches it), and stores the
    /// crash-loop counters `recovery_attempts` / `recovery_marker`. The instance
    /// is then replayed-from-start with the checkpoint cache, so completed
    /// durable steps are served from cache. `marker` is the checkpoint count at
    /// recovery time, used to detect forward progress between recoveries.
    ///
    /// The default implementation suspends the instance and schedules an
    /// immediate wake using the existing building blocks; the SQL backends
    /// override it to also persist the `recovery_attempts`/`recovery_marker`
    /// crash-loop counters in a single atomic UPDATE.
    async fn mark_for_recovery(
        &self,
        instance_id: &str,
        _attempt: i32,
        _marker: Option<&str>,
    ) -> Result<(), CoreError> {
        self.complete_instance(
            CompleteInstanceParams::new(instance_id, "suspended")
                .with_termination("environment_restart", None),
        )
        .await?;
        self.set_instance_sleep(instance_id, chrono::Utc::now())
            .await
    }

    /// Get instances that are due to wake (sleep_until <= now).
    async fn get_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError>;

    /// Claim due sleeping instances for waking, leasing them until `retry_at`.
    ///
    /// The claim moves `sleep_until` forward rather than clearing it, so a
    /// caller that dies between claiming and launching does not strand its
    /// batch: the rows simply become due again when the lease expires. Clearing
    /// leaves a row `suspended` with no deadline, which is exactly what a
    /// signal waiter looks like, so no sweep can tell them apart.
    /// Select **and claim** up to `limit` due sleeping instances in one step.
    ///
    /// Every returned record is already claimed — the caller owns it and must
    /// launch it, exactly as if [`Persistence::claim_sleeping_instance`] had
    /// returned `true`. On a launch failure, re-stamp `sleep_until` via
    /// [`Persistence::set_instance_sleep`] so the instance is retried.
    ///
    /// Separate from `get_sleeping_instances_due` + `claim_sleeping_instance`
    /// because a scheduler that polls back-to-back (rather than sleeping a
    /// fixed interval between batches) keeps re-selecting rows whose claim has
    /// not landed yet. Folding the claim into the selecting statement removes
    /// that window entirely, and costs one round trip per batch instead of one
    /// per instance.
    ///
    /// The default implementation composes the two existing operations and is
    /// correct but non-atomic — adequate for in-memory/mock backends. SQL
    /// backends override it with a single claiming statement.
    async fn claim_sleeping_instances_due(
        &self,
        limit: i64,
        retry_at: DateTime<Utc>,
    ) -> Result<Vec<InstanceRecord>, CoreError> {
        let due = self.get_sleeping_instances_due(limit).await?;
        let mut claimed = Vec::with_capacity(due.len());
        for record in due {
            if self.claim_sleeping_instance(&record.instance_id).await? {
                self.set_instance_sleep(&record.instance_id, retry_at)
                    .await?;
                claimed.push(record);
            }
        }
        Ok(claimed)
    }

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
    // Paired Records
    // ========================================================================

    /// List an instance's paired records, joining each start event to the end
    /// event that shares its correlation id within the same scope.
    ///
    /// `vocabulary` names the subtypes and payload keys of the caller's event
    /// protocol; this crate reads them and interprets none of them.
    async fn list_paired_records(
        &self,
        instance_id: &str,
        vocabulary: &EventVocabulary,
        filter: &ListPairedRecordsFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PairedRecordSummary>, CoreError>;

    /// Count an instance's paired records under the same filter.
    async fn count_paired_records(
        &self,
        instance_id: &str,
        vocabulary: &EventVocabulary,
        filter: &ListPairedRecordsFilter,
    ) -> Result<i64, CoreError>;

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
    /// tables (container_registry, instance_images, etc.) before calling the parent.
    ///
    /// Returns the count of deleted instances.
    async fn delete_instances_batch(&self, _instance_ids: &[String]) -> Result<u64, CoreError> {
        // Default: no-op (no deletion supported)
        Ok(0)
    }

    /// Delete the paired events named by `vocabulary` older than `older_than`,
    /// up to `limit` rows.
    ///
    /// These payloads dominate `instance_events` — on a large run they are the
    /// great majority of rows — but they are only read while a run is recent.
    /// Ageing them out on their own, shorter window keeps the table bounded
    /// during a burst without touching the lifecycle events (`completed`,
    /// `failed`, `suspended`) that are the run's durable history, and without
    /// reducing what producers record in the first place.
    ///
    /// Only the vocabulary's start and end subtypes are removed; this crate
    /// picks no subtypes of its own.
    ///
    /// Callers should loop until this returns fewer than `limit`.
    ///
    /// Returns the count of deleted events.
    async fn delete_paired_events_older_than(
        &self,
        _vocabulary: &EventVocabulary,
        _older_than: DateTime<Utc>,
        _limit: i64,
    ) -> Result<u64, CoreError> {
        // Default: no-op (no retention supported)
        Ok(0)
    }
}
