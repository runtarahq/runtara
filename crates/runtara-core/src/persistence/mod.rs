//! Persistence interfaces and backends for runtara-core.
//!
//! This module defines the persistence abstraction and backend implementations.

/// In-memory backend, for tests and for hosts that need no durability.
#[cfg(any(test, feature = "test-support"))]
pub mod memory;

/// The executable definition of the [`Persistence`] contract, for backends to
/// prove themselves against.
#[cfg(any(test, feature = "test-support"))]
pub mod conformance;

pub mod vocabulary;

/// Atomic optional ownership for isolated invocation persistence.
pub mod invocations;

pub use self::vocabulary::{EventVocabulary, EventVocabularySpec};

use crate::domain::{EventType, InstanceStatus, SignalType};
use crate::error::CoreError;

/// Instance record from the persistence layer.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    /// Unique identifier for the instance.
    pub instance_id: String,
    /// Tenant identifier for multi-tenancy isolation.
    pub tenant_id: String,
    /// Version of the workflow definition.
    pub definition_version: i32,
    /// Current status (pending, running, suspended, completed, failed, cancelled).
    pub status: InstanceStatus,
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
    pub input: Option<Vec<u8>>,
    /// Optional user-defined execution label.
    pub run_label: Option<String>,
    /// Output data from successful completion.
    pub output: Option<Vec<u8>>,
    /// Error message from failure.
    pub error: Option<String>,
    /// When a sleeping instance should be woken.
    pub sleep_until: Option<DateTime<Utc>>,
    /// Scheduled or most recently claimed host wake cause. Preserved across claim/retry and execution
    /// start; lifecycle commands can clear an obsolete wake intent.
    pub wake_reason: Option<crate::domain::WakeReason>,
    /// How/why the instance reached its terminal state.
    pub termination_reason: Option<String>,
    /// Process exit code if available.
    pub exit_code: Option<i32>,
    /// Consecutive no-progress auto-restarts after an Environment restart.
    /// Reset to 0 when the instance's checkpoint count advances between
    /// recoveries. The host owns recovery bookkeeping.
    pub recovery_attempts: i32,
    /// Checkpoint count observed at the last auto-recovery, as text. Compared
    /// against the current count to distinguish "made progress" from "stuck".
    pub recovery_marker: Option<String>,
}

/// Checkpoint record from the persistence layer.
#[derive(Debug, Clone)]
pub struct CheckpointRecord {
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
    /// Position in the instance's append sequence, assigned by the store.
    ///
    /// `None` before the event is stored — a caller building one to append has
    /// no position yet. Readers rely on it to order events written within the
    /// same clock tick, so a store must assign values that increase with
    /// insertion order.
    pub id: Option<i64>,
    /// Instance this event belongs to.
    pub instance_id: String,
    /// Type of event (heartbeat, completed, failed, suspended, custom).
    pub event_type: EventType,
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
#[derive(Debug, Clone)]
pub struct SignalRecord {
    /// Opaque identity of this command; replacements receive a fresh identity.
    pub command_id: String,
    /// Instance this signal is for.
    pub instance_id: String,
    /// Type of signal (cancel, pause, shutdown).
    pub signal_type: SignalType,
    /// Optional signal payload data.
    pub payload: Option<Vec<u8>>,
    /// When the signal was created.
    pub created_at: DateTime<Utc>,
    /// When the signal was acknowledged by the instance.
    pub acknowledged_at: Option<DateTime<Utc>>,
}

impl SignalRecord {
    /// Borrow the command facts used by the pure lifecycle policy.
    pub fn command(&self) -> crate::lifecycle::Command<'_> {
        crate::lifecycle::Command {
            id: &self.command_id,
            kind: self.signal_type,
            acknowledged: self.acknowledged_at.is_some(),
        }
    }
}

/// Instance whose pending cancellation was applied without a running guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelledInstance {
    /// Cancelled instance identity.
    pub instance_id: String,
    /// Tenant to notify when releasing execution admission.
    pub tenant_id: String,
}

/// Pending custom signal scoped to a specific checkpoint.
#[derive(Debug, Clone)]
pub struct CustomSignalRecord {
    /// Identity of this retained value, distinct from its checkpoint address.
    pub signal_id: String,
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
    pub event_type: Option<EventType>,
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

/// Execution facts about an instance that reached a terminal state.
///
/// A plain data carrier: Core assembles it from its own row and hands it to an
/// [`InstanceMetricsSink`]. Core does not know what a host does with it — the
/// OpenTelemetry vocabulary, the exporter and the attribute names all belong to
/// whoever implements the sink.
#[derive(Debug, Clone)]
pub struct InstanceCompletionMetrics {
    /// Tenant identifier for the invocation.
    pub tenant_id: String,
    /// Terminal status: completed, failed, or cancelled.
    pub status: InstanceStatus,
    /// Optional terminal reason such as timeout or heartbeat_timeout.
    pub termination_reason: Option<String>,
    /// When execution began.
    pub started_at: Option<DateTime<Utc>>,
    /// When execution reached a terminal state.
    pub finished_at: Option<DateTime<Utc>>,
    /// Peak memory collected by the runner cgroup.
    pub memory_peak_bytes: Option<u64>,
    /// CPU usage collected by the runner cgroup.
    pub cpu_usage_usec: Option<u64>,
}

impl InstanceCompletionMetrics {
    /// Wall-clock execution time, when both ends of the interval are known.
    pub fn duration_seconds(&self) -> Option<f64> {
        let started_at = self.started_at?;
        let finished_at = self.finished_at?;
        finished_at
            .signed_duration_since(started_at)
            .to_std()
            .ok()
            .map(|d| d.as_secs_f64())
    }
}

/// Notified when an instance reaches a terminal state, for a host that reports
/// on completions.
///
/// Exists for the same reason as
/// [`InstanceEventObserver`](crate::instance_handlers::InstanceEventObserver):
/// Core cannot depend on the crate that owns the telemetry pipeline, so it
/// defines the shape and the host implements it. A host that wires no sink
/// simply reports nothing; Core's behaviour is identical either way.
///
/// Called on the completion path, so implementations must be cheap and
/// non-blocking.
pub trait InstanceMetricsSink: Send + Sync {
    /// An instance reached `completed`, `failed`, or `cancelled`.
    fn on_terminal(&self, metrics: &InstanceCompletionMetrics);
}
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
/// The optional fields split into two groups, and the difference matters.
///
/// `output` and `error` are **replaced**: passing `None` clears whatever was
/// there, so a failing transition cannot leave a stale success payload behind.
/// `termination_reason`, `exit_code`, `stderr` and `checkpoint_id` are
/// **merged**: passing `None` leaves what an earlier transition recorded, so a
/// later status change does not erase the reason a run ended. Both halves are
/// pinned by the conformance suite. The required fields
/// Blob and identifier fields borrow from the caller; status is a domain value.
///
/// Build with [`CompleteInstanceParams::new`] and the chained `with_*`
/// setters.
#[derive(Debug, Clone)]
pub struct CompleteInstanceParams<'a> {
    /// Instance being completed.
    pub instance_id: &'a str,
    /// Target status. One of `completed`, `failed`, `cancelled`,
    /// `suspended`, or `running` (for mid-execution transitions that
    /// carry metadata but don't finalize the instance).
    pub status: InstanceStatus,
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
    pub fn new(instance_id: &'a str, status: InstanceStatus) -> Self {
        Self {
            instance_id,
            status,
            guard: CompleteInstanceGuard::Any,
            output: None,
            error: None,
            stderr: None,
            checkpoint_id: None,
            termination_reason: None,
            exit_code: None,
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
#[async_trait]
pub trait Persistence: Send + Sync {
    /// Optional atomic invocation fencing. Callers requiring durable fences must
    /// reject absence rather than use check-then-write persistence. Legacy calls
    /// are unchanged.
    fn invocation_fences(&self) -> Option<&dyn invocations::InvocationFences> {
        None
    }

    /// Insert a new instance row for `instance_id`, owned by `tenant_id`, in
    /// its initial pending state.
    ///
    /// The id is the primary key, so a second call for the same id is an
    /// error, not an update. A caller that treats the id as an idempotency key
    /// and needs to know which way the race went should use
    /// [`Self::try_register_instance`], which reports it.
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
        self.try_register_instance_with_label(instance_id, tenant_id, input, None)
            .await
    }

    /// Atomically persist the start label with the instance and input. A replay
    /// never writes to the existing instance. Callers compare metadata on a lost claim.
    async fn try_register_instance_with_label(
        &self,
        instance_id: &str,
        tenant_id: &str,
        input: Option<&[u8]>,
        run_label: Option<&str>,
    ) -> Result<bool, CoreError> {
        if run_label.is_some() {
            return Err(CoreError::ValidationError {
                field: "runLabel".into(),
                message: "This persistence backend does not support labeled starts".into(),
            });
        }
        if self.get_instance(instance_id).await?.is_some() {
            return Ok(false);
        }
        self.register_instance(instance_id, tenant_id).await?;
        if let Some(input) = input {
            self.store_instance_input(instance_id, input).await?;
        }
        Ok(true)
    }

    /// Read an instance's full row, launch input included.
    ///
    /// `Ok(None)` for an id that was never registered — an unknown instance is
    /// an answer here, not an error. Use [`Self::get_instance_meta`] instead
    /// whenever the input is not what the caller came for; that blob is the
    /// expensive part of this row.
    async fn get_instance(&self, instance_id: &str) -> Result<Option<InstanceRecord>, CoreError>;

    /// Like [`Self::get_instance`] but without the `input` blob, for callers
    /// that only need status/tenant/recovery state.
    ///
    /// The returned record always has `input: None` — that is the point, not a
    /// missing row. Never use this when the input is what you came for; the
    /// launch payload can be large, and reading it back on every status check
    /// is what this exists to avoid.
    ///
    /// The default reads the whole row and drops the input, which is correct
    /// but not cheap — it still pays to fetch the blob. Backends that can
    /// project it away in the query should override this; what they must not
    /// do is return the input, which is the one thing every caller here is
    /// trying to avoid loading.
    async fn get_instance_meta(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceRecord>, CoreError> {
        Ok(self.get_instance(instance_id).await?.map(|mut instance| {
            instance.input = None;
            instance
        }))
    }

    /// Set an instance's status, stamping `started_at` when one is supplied
    /// and leaving it untouched when it is not.
    ///
    /// Supplying `started_at` says the row is entering a run, and carries one
    /// required guarantee with it: `finished_at` and `termination_reason` are
    /// cleared in the same write. A row that ran before may still hold both
    /// from an earlier suspend or drain force-stop; those describe a run that
    /// is no longer over, and leaving them puts `finished_at` before
    /// `started_at`, which renders a resumed run as a negative duration.
    /// `exit_code` is deliberately not part of the clear. Omitting
    /// `started_at` writes the status alone and touches nothing else.
    ///
    /// The clear is not optional for a backend to implement, and it is not
    /// only this method's concern: the default
    /// [`Self::mark_instance_running`] and [`Self::mark_instance_started`]
    /// both route here with a `started_at`, so they inherit it, and a backend
    /// overriding either owes the same clear there.
    ///
    /// Past that, the raw write: it applies the status it is given and guards
    /// the transition not at all. A caller stamping `running` after a launch
    /// wants [`Self::mark_instance_started`], which refuses once the run has
    /// moved past the pre-run states; a wake or resume wants
    /// [`Self::mark_instance_running`], which is deliberately unguarded
    /// because it promotes from `suspended` — the state the other one refuses.
    /// A terminal transition wants [`Self::complete_instance`], which is what
    /// stamps `finished_at`.
    ///
    /// Errors with [`CoreError::InstanceNotFound`] if no row matched.
    async fn update_instance_status(
        &self,
        instance_id: &str,
        status: InstanceStatus,
        started_at: Option<DateTime<Utc>>,
    ) -> Result<(), CoreError>;

    /// Point an instance at the checkpoint it most recently wrote, so a
    /// relaunch knows where to resume from.
    ///
    /// Errors with [`CoreError::InstanceNotFound`] if no row matched.
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
    /// see its documentation for the per-field semantics (which fields are
    /// replaced and which are merged, guard against races).
    ///
    /// `finished_at` is stamped for `completed`, `failed`, `cancelled` **and**
    /// `suspended`. Parking counts: it ends the attempt that was in flight,
    /// even though the instance will run again. A `running` transition carries
    /// metadata without finalizing anything and stamps nothing. A supplied
    /// `termination_reason` is written on the same transition.
    ///
    /// Those two fields are what [`Self::update_instance_status`] clears when a
    /// later call supplies a `started_at`. The pairing is the whole reason a
    /// resumed run does not report a negative duration, so a backend that
    /// declines to stamp here silently weakens the clear over there.
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

    /// Write the serialized `state` for `(instance_id, checkpoint_id)`.
    ///
    /// Saving the same pair again **refreshes** it rather than failing.
    /// Replay is ordinary here: a relaunched instance re-runs the durable
    /// steps it already checkpointed, and a save that rejected the repeat
    /// would turn every recovery into an error.
    ///
    /// A refresh restamps `created_at` to the time of the rewrite, so the
    /// timestamp is when this state was written and not when the key was
    /// first used. [`Self::list_checkpoints`] pages on that field, so a
    /// backend that keeps the original stamp pages a replayed instance in a
    /// different order than one that does not.
    async fn save_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<(), CoreError>;

    /// Read back one checkpoint by `(instance_id, checkpoint_id)`.
    ///
    /// `Ok(None)` for a pair that was never saved — a step that has not run
    /// yet, which is what a replaying instance is asking about.
    async fn load_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CheckpointRecord>, CoreError>;

    /// Page through an instance's checkpoints, newest first.
    ///
    /// Ordered by `(created_at, checkpoint_id)` descending, comparing the id
    /// **bytewise**. Without a total order, `offset` walks a set the store is
    /// free to re-shuffle between pages, and a paginating caller silently
    /// skips and repeats rows.
    ///
    /// The id is a tie-break and nothing more: its order carries no meaning of
    /// its own, it just has to be the same order every time and on every
    /// backend. Bytewise is what makes that last part true — a SQL backend
    /// sorting text under its database collation orders `-`, `_` and case
    /// differently from every backend that compares the raw bytes, so it must
    /// ask for the byte order explicitly.
    ///
    /// Ties are rare rather than routine: a backend that stamps `created_at`
    /// per write has to land two writes inside one tick of its clock to
    /// produce one. The tie-break is here so that the order is defined when
    /// that happens, not because it happens often.
    ///
    /// Every filter is optional and narrows the set: `checkpoint_id` to a
    /// single id, `created_after` inclusive, `created_before` exclusive — a
    /// half-open window, so back-to-back pages tile a time range without
    /// double-counting the boundary.
    async fn list_checkpoints(
        &self,
        instance_id: &str,
        checkpoint_id: Option<&str>,
        limit: i64,
        offset: i64,
        created_after: Option<DateTime<Utc>>,
        created_before: Option<DateTime<Utc>>,
    ) -> Result<Vec<CheckpointRecord>, CoreError>;

    /// Count what [`Self::list_checkpoints`] would return for the same
    /// filters, ignoring `limit`/`offset`.
    ///
    /// The total a paginating caller reports, not the size of the page it
    /// just read.
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
    /// write time. Readers order events by this
    /// timestamp, and [`Self::list_paired_records`] derives every duration from
    /// the delta between a record's paired start and end events, so a
    /// receive-time stamp silently reorders the timeline and rewrites every
    /// duration into the interval between two writes.
    async fn insert_event(&self, event: &EventRecord) -> Result<(), CoreError>;

    /// Store a fresh lifecycle command, replacing the previous slot. An unacknowledged
    /// cancellation dominates subsequent commands and retains its identity and payload.
    async fn insert_signal(
        &self,
        instance_id: &str,
        signal_type: SignalType,
        payload: &[u8],
    ) -> Result<(), CoreError>;

    /// Read the lifecycle command waiting for an instance, if one is.
    ///
    /// Only *unacknowledged* commands: once
    /// [`Self::acknowledge_signal`] accepts one it must never be handed back.
    /// A guest acknowledges on read precisely so the command is consumed once,
    /// and redelivering a cancel or shutdown would re-suspend a relaunched
    /// instance on a command it already handled.
    async fn get_pending_signal(
        &self,
        instance_id: &str,
    ) -> Result<Option<SignalRecord>, CoreError>;

    /// Atomically acknowledge exactly the delivered command and apply its lifecycle
    /// transition and suspension event. Shutdown also schedules immediate wake.
    /// Returns false for a replaced/missing command, a type mismatch, or a transition
    /// that would revive a terminal instance. Repeating an accepted acknowledgment
    /// returns true without applying its transition again. No new receipt may
    /// overwrite an accepted terminal outcome, including cancellation receipts.
    async fn acknowledge_signal(
        &self,
        instance_id: &str,
        command_id: &str,
        signal_type: SignalType,
    ) -> Result<bool, CoreError> {
        Ok(self
            .apply_lifecycle_command(instance_id, command_id, signal_type)
            .await?
            .accepted())
    }

    /// Evaluate core command policy against locked state and atomically apply its
    /// effects. Return the typed disposition, retaining idempotency information.
    async fn apply_lifecycle_command(
        &self,
        instance_id: &str,
        command_id: &str,
        signal_type: SignalType,
    ) -> Result<crate::lifecycle::Decision, CoreError>;

    /// Evaluate the core parking guard and commit suspension metadata and its
    /// deadline atomically. A concurrent terminal transition cannot be overwritten.
    async fn park_instance(
        &self,
        instance_id: &str,
        request: crate::lifecycle::ParkRequest,
    ) -> Result<crate::lifecycle::Decision, CoreError>;

    /// Atomically cancel suspended instances with pending cancel commands, clear
    /// their wake deadlines, and acknowledge those exact commands. Returns only
    /// newly cancelled instances. Active runs and terminal instances are untouched.
    /// `Some(id)` targets an API request; `None` recovers interrupted delivery in
    /// bounded batches. Locked instances may be skipped for the next recovery pass.
    async fn cancel_suspended_instances(
        &self,
        instance_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<CancelledInstance>, CoreError>;

    /// Replace the retained value at an instance/checkpoint address. Last write
    /// wins; every successful write receives a fresh signal ID, even for identical
    /// payloads. This is neither a queue nor retry deduplication. Returns that ID.
    async fn put_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        payload: &[u8],
    ) -> Result<String, CoreError>;

    /// Read the current retained value without consuming it. Replays see the
    /// same identity and payload until a later write replaces it. The checkpoint
    /// ID is an address; the returned signal ID identifies the stored value.
    async fn get_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<CustomSignalRecord>, CoreError>;

    /// Record that a durable step failed and is being retried.
    ///
    /// An audit trail: nothing in this crate reads it back, and no execution
    /// decision depends on it. Re-saving the same `(checkpoint_id, attempt)`
    /// updates that record in place rather than appending a duplicate.
    async fn save_retry_attempt(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        attempt: i32,
        error_message: Option<&str>,
    ) -> Result<(), CoreError>;

    /// Page through instances, newest first, optionally narrowed to one
    /// tenant and/or one status.
    ///
    /// Ordered by `(created_at, instance_id)` descending, comparing the id
    /// **bytewise**. Without a total order, `offset` walks a set the store is
    /// free to re-shuffle between pages, and a paginating caller silently
    /// skips and repeats rows.
    ///
    /// The id is a tie-break and nothing more: its order carries no meaning
    /// of its own, it just has to be the same order every time and on every
    /// backend. Bytewise is what makes that last part true — a SQL backend
    /// sorting text under its database collation orders `-`, `_` and case
    /// differently from every backend that compares the raw bytes, so it must
    /// ask for the byte order explicitly.
    ///
    /// Ties are not exotic here: a bulk launch registers instances as fast as
    /// the store will take them, and whether two land inside one tick of its
    /// clock is a property of the clock rather than of the workload.
    ///
    /// The returned records carry no `input`, like
    /// [`Self::get_instance_meta`] — a listing that loaded every launch
    /// payload would pay for the one field none of its callers want.
    async fn list_instances(
        &self,
        tenant_id: Option<&str>,
        status: Option<InstanceStatus>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError>;

    /// Whether the store is reachable and answering.
    async fn health_check(&self) -> Result<bool, CoreError>;

    /// Count the instances currently occupying a concurrency slot, meaning
    /// those that are `running`.
    ///
    /// `suspended` is deliberately excluded. Durable sleep and a signal-wait
    /// both park an instance there, parking is a steady state rather than a
    /// transient one, and a workflow can sit suspended for days while running
    /// no code and holding no host resource. Counting those rows would let a
    /// handful of long-parked workflows hold a concurrency cap closed
    /// indefinitely.
    ///
    /// A row left `running` by a crashed host still counts, and nothing in
    /// this crate reaps one — the heartbeat monitor that does lives in the
    /// embedding host.
    async fn count_active_instances(&self) -> Result<i64, CoreError>;

    /// Promote an instance to `running` on a relaunch, preserving its
    /// original `started_at`.
    ///
    /// For wake and resume, which promote from `suspended` — a state
    /// [`Self::mark_instance_started`] deliberately refuses. The default is the
    /// read-then-write this replaces; a backend that can do it in one operation
    /// should.
    async fn mark_instance_running(
        &self,
        instance_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let started_at = match self.get_instance(instance_id).await {
            Ok(Some(instance)) => instance.started_at.unwrap_or(started_at),
            _ => started_at,
        };
        self.update_instance_status(
            instance_id,
            crate::domain::InstanceStatus::Running,
            Some(started_at),
        )
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
    /// The default reads then writes, which is adequate for a backend whose
    /// store cannot express a guarded update in one operation. One that can
    /// should override it.
    async fn mark_instance_started(
        &self,
        instance_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<bool, CoreError> {
        match self.get_instance(instance_id).await? {
            Some(inst)
                if matches!(
                    inst.status,
                    InstanceStatus::Pending | InstanceStatus::Running
                ) =>
            {
                self.update_instance_status(
                    instance_id,
                    crate::domain::InstanceStatus::Running,
                    Some(inst.started_at.unwrap_or(started_at)),
                )
                .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Set the sleep_until timestamp for an instance.
    /// Terminal instances retain no wake deadline, including when this write
    /// races with cancellation or completion.
    async fn set_instance_sleep(
        &self,
        instance_id: &str,
        sleep_until: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        self.schedule_wake(instance_id, sleep_until, crate::domain::WakeReason::Timer)
            .await
    }

    /// Atomically persist a wake deadline and its host-side cause. Terminal
    /// instances retain no scheduled wake. A wake reason is never a guest signal.
    async fn schedule_wake(
        &self,
        instance_id: &str,
        deadline: DateTime<Utc>,
        reason: crate::domain::WakeReason,
    ) -> Result<(), CoreError>;

    /// Clear the sleep_until timestamp for an instance.
    async fn clear_instance_sleep(&self, instance_id: &str) -> Result<(), CoreError>;

    /// Atomically claim one due sleeping instance before waking it.
    ///
    /// The per-row form, for a caller that already holds a single instance id.
    /// The scheduler's wake path claims a whole batch at once — see
    /// [`Persistence::claim_sleeping_instances_due`], which is what it calls.
    ///
    /// Clear `sleep_until` only while the instance is still suspended with a
    /// present `sleep_until` that is **already due** (`sleep_until <= now`),
    /// and report whether this caller won the claim: `true` if it did, `false`
    /// if another waker (or another scheduler sharing this store) already took
    /// it. Callers MUST launch only when this returns `true` — this is what
    /// prevents concurrent double-launch of the same instance. On a launch
    /// failure after a successful claim, re-stamp `sleep_until` via
    /// [`Persistence::schedule_wake`], carrying the instance's existing
    /// `wake_reason` over, so it is retried without losing why it was parked.
    ///
    /// The due-ness check is what makes a lease a lease: a batch claim pushes
    /// `sleep_until` into the future rather than clearing it, and an instance
    /// leased that way must lose a later claim until the lease expires.
    ///
    /// The test and the mutation must be **one indivisible operation** — a
    /// conditional statement whose own report of what it changed is the claim,
    /// or a claim taken under the store's own lock. Read-then-clear does not
    /// qualify: two callers can each read a claimable row before either clears
    /// it, and both win.
    ///
    /// There is deliberately no default. Double-launch protection is the whole
    /// reason this method exists, and a backend cannot inherit it from a
    /// composition of the other methods on this trait — only from a statement
    /// its own store executes indivisibly. A backend that genuinely cannot do
    /// that must say so by never claiming (`Ok(false)`), which parks instances
    /// rather than running them twice.
    async fn claim_sleeping_instance(&self, instance_id: &str) -> Result<bool, CoreError>;

    /// Get instances that are due to wake (sleep_until <= now).
    ///
    /// Selects without claiming, so two callers see the same rows. The wake
    /// path claims as it selects instead — see
    /// [`Persistence::claim_sleeping_instances_due`].
    async fn get_sleeping_instances_due(
        &self,
        limit: i64,
    ) -> Result<Vec<InstanceRecord>, CoreError>;

    /// Select **and claim** up to `limit` due sleeping instances in one step,
    /// leasing them until `retry_at`.
    ///
    /// This is the wake path: the scheduler relaunches exactly what this
    /// returns, and calls nothing else to decide what is due.
    ///
    /// `retry_at` must be strictly in the future. A lease already in the past
    /// leaves every claimed row immediately re-claimable, which is the
    /// double-launch this method exists to prevent; backends stamp what they
    /// are given and cannot repair it.
    ///
    /// Move `sleep_until` forward to `retry_at` rather than clearing it, so a
    /// caller that dies between claiming and launching does not strand its
    /// batch: the rows simply become due again when the lease expires. Clearing
    /// leaves a row `suspended` with no deadline, which is exactly what a
    /// signal waiter looks like, so no sweep can tell them apart — the wake
    /// scan skips it for having no deadline and the retention sweep skips it
    /// for not being terminal, and the instance is parked for good.
    ///
    /// Every returned record is already claimed — the caller owns it and must
    /// launch it, exactly as if [`Persistence::claim_sleeping_instance`] had
    /// returned `true` — and carries its new `retry_at` deadline, not the one
    /// it was selected on. On a launch failure, re-stamp `sleep_until` via
    /// [`Persistence::schedule_wake`], passing the record's own existing
    /// `wake_reason`, so the instance is retried sooner than the lease would.
    /// Not [`Persistence::set_instance_sleep`]: that one hardcodes
    /// [`crate::domain::WakeReason::Timer`], so re-stamping through it would
    /// rewrite why the instance was parked in the first place.
    ///
    /// Separate from `get_sleeping_instances_due` + `claim_sleeping_instance`
    /// because a scheduler that polls back-to-back (rather than sleeping a
    /// fixed interval between batches) keeps re-selecting rows whose claim has
    /// not landed yet. Folding the claim into the selecting statement removes
    /// that window entirely, and costs one round trip per batch instead of one
    /// per instance.
    ///
    /// The selection and the lease must be **one indivisible operation** — a
    /// single statement that returns the rows it just stamped, or a claim taken
    /// under the store's own lock.
    ///
    /// There is deliberately no default. Composing the two operations above
    /// cannot produce this contract: the per-row claim clears `sleep_until` and
    /// the re-stamp is a second statement, so a caller that dies between them
    /// strands that row in precisely the undetectable state described above. A
    /// backend inherits the batch shape but not the guarantee, which is the
    /// failure this signature exists to prevent.
    ///
    /// A backend that cannot select and lease indivisibly must refuse at
    /// startup. It must **not** quietly return an empty `Vec` forever: nothing
    /// distinguishes that from a store with nothing due, so the scheduler logs
    /// an ordinary idle poll while every durable sleep silently never wakes —
    /// no error, no metric, no failed instance.
    async fn claim_sleeping_instances_due(
        &self,
        limit: i64,
        retry_at: DateTime<Utc>,
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
    /// Implementations must also remove the instances' checkpoints, events,
    /// lifecycle signals, custom signals, and retry history. Host-owned
    /// associations must be cleaned up by the host as part of its deletion flow.
    ///
    /// Returns the count of deleted instances.
    async fn delete_instances_batch(&self, _instance_ids: &[String]) -> Result<u64, CoreError> {
        // Default: no-op (no deletion supported)
        Ok(0)
    }

    /// Delete the paired events named by `vocabulary` older than `older_than`,
    /// up to `limit` rows.
    ///
    /// These payloads dominate event storage — on a large run they are the
    /// great majority of rows — but they are only read while a run is recent.
    /// Ageing them out on their own, shorter window keeps event storage bounded
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
