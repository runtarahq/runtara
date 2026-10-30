//! The control service's request and response types: the shapes of the
//! workflow API's `/v1/instances` calls, before their JSON form
//! (`control_host::json`).

/// Why a control call failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// Not authorized: a target outside the rules of the operation, or
    /// any call outside a run's own entry (the host makes `api` real only
    /// there).
    Denied,
    /// Malformed or out-of-range arguments, a payload that does not match
    /// the request's response schema, a command aimed at the caller
    /// itself, or a start past the lineage depth of 16.
    Invalid,
    /// No such instance, workflow or request in the caller's tenant,
    /// including an accepted child whose record was deleted.
    NotFound,
    /// `start` of a workflow whose compilation failed permanently. A
    /// workflow that is not compiled yet is accepted instead.
    NotRunnable,
    /// The target is not a direct child of the caller (`cancel`, `pause`,
    /// `resume`).
    NotChild,
    /// The call needs a calling instance (identity operations and
    /// caller-relative filters), and this one runs outside a run.
    RequiresInstance,
    /// A mutation called outside a compiler-emitted operation scope, so it
    /// would have no replay-safe identity.
    RequiresOperation,
    /// `start` hit the tenant concurrency limit or the control share of
    /// it. With `retry-after-ms` it is retryable; without it the limit
    /// can never admit a child (a limit of at most 1, which the parent
    /// itself holds).
    Capacity,
    /// The same operation replayed with different arguments.
    ReplayConflict,
    /// `start` reused a run label the caller already gave another child.
    LabelConflict,
    /// A request, payload or page over its cap.
    TooLarge,
    /// A transient platform failure; retryable, `retry-after-ms` may hint.
    Unavailable,
    /// The operation is not available in this build.
    Unsupported,
    /// `send-signal` found no open request for the signal: none was ever
    /// opened, or it already closed.
    NotWaiting,
    /// `send-signal` matched several open requests; pass a request id
    /// from `list-pending-signals`.
    Ambiguous,
    /// `send-signal` found the request already answered by another
    /// operation.
    AlreadyAnswered,
    /// `pause` of a run that cannot pause (not started yet, or finished).
    NotPausable,
    /// `resume` of a run that is not explicitly paused.
    NotPaused,
    /// The call ran past its host bound (90 s or the step deadline).
    Timeout,
}

/// A failed control call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlError {
    pub code: ErrorCode,
    /// Human-readable; never carries payloads.
    pub message: String,
    /// Set on retryable failures: `capacity` while the control share is
    /// full (a 3-8 s hint), and optionally `unavailable`.
    pub retry_after_ms: Option<u64>,
}

/// What happens to a child still running when its parent ends. The
/// author must choose; the editor preselects `cancel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParentClosePolicy {
    /// Cancel the child when the parent ends in any way (completion,
    /// failure, cancellation, crash), after a 5 s grace period.
    Cancel,
    /// Leave the child running.
    LeaveRunning,
}

/// Lifecycle state of an instance as control reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InstanceStatus {
    /// Admitted by `start` and not launched yet.
    Queued,
    /// Launched, before execution starts.
    Pending,
    Running,
    /// Parked or paused; see `suspension-reason`.
    Suspended,
    Completed,
    Failed,
    Cancelled,
    /// Admitted but never launched, and now never will be.
    NotStarted,
}

/// Why a `suspended` instance is not running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SuspensionReason {
    /// Explicitly paused. A parked run pauses immediately.
    Paused,
    WaitingSignal,
    WaitingInstances,
    Sleeping,
    Shutdown,
}

/// Identity and lifecycle of one instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceSummary {
    pub instance_id: String,
    pub workflow_id: String,
    pub version: Option<u32>,
    pub run_label: Option<String>,
    pub parent_instance_id: Option<String>,
    pub status: InstanceStatus,
    pub suspension_reason: Option<SuspensionReason>,
    /// Why a terminal instance ended (for example `completed`,
    /// `application_error`, `cancelled`).
    pub termination_reason: Option<String>,
    pub created_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
}

/// The terminal output or error of an instance. Values over their cap
/// are omitted and flagged rather than truncated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalResult {
    /// Output JSON, when inlined.
    pub output: Option<Vec<u8>>,
    /// Size of the full output, also when it was omitted.
    pub output_bytes: Option<u64>,
    pub output_omitted: bool,
    /// Error JSON, when inlined.
    pub error: Option<Vec<u8>>,
    pub error_omitted: bool,
}

/// `get`: the summary plus, for a terminal instance, its result (output
/// inlined up to 1 MiB, error up to 64 KiB).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceDetail {
    pub instance: InstanceSummary,
    pub terminal: TerminalResult,
}

/// `get-state`: the summary (with the run's workflow version) plus the
/// run's published state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRead {
    pub instance: InstanceSummary,
    /// The state as a JSON object; none when the run published none.
    pub state: Option<Vec<u8>>,
    /// When the state last changed.
    pub state_updated_at_ms: Option<u64>,
}

/// `start` a child of the calling instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartRequest {
    /// A workflow id, never a slug.
    pub workflow_id: String,
    /// Defaults to the workflow's current version, resolved and stored at
    /// admission so a replay cannot pick a newer one.
    pub version: Option<u32>,
    /// The `{data, variables}` input envelope.
    pub input: Vec<u8>,
    /// Unique per parent for the parent's whole lifetime; 1-1024 bytes of
    /// printable ASCII.
    pub run_label: Option<String>,
    pub parent_close_policy: ParentClosePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartResult {
    pub instance_id: String,
    pub workflow_id: String,
    pub version: u32,
    pub run_label: Option<String>,
    /// The operation had already started this child; nothing new ran.
    pub replayed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortField {
    CreatedAt,
    FinishedAt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// Restrict `query` to the children of one parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentFilter {
    /// The calling instance's children (`requires-instance` without one).
    Caller,
    /// The children of this instance.
    Instance(String),
}

/// `query` instances in the caller's tenant. Filters apply before
/// pagination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryRequest {
    pub workflow_id: Option<String>,
    pub run_label: Option<String>,
    /// Empty means every status.
    pub statuses: Vec<InstanceStatus>,
    pub parent: Option<ParentFilter>,
    pub created_after_ms: Option<u64>,
    pub created_before_ms: Option<u64>,
    pub finished_after_ms: Option<u64>,
    pub finished_before_ms: Option<u64>,
    /// Published-state filters as a JSON array of `{field, op, value}`
    /// (op `eq`, `ne`, `in`, `lt`, `lte`, `gt`, `gte` or `exists`), all of
    /// which must hold. A run without the field does not match.
    pub state: Option<Vec<u8>>,
    pub sort_by: SortField,
    pub order: SortOrder,
    /// 1-100.
    pub page_size: u32,
    pub page_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstancePage {
    pub items: Vec<InstanceSummary>,
    /// Instances matching the filters across all pages.
    pub total: u64,
    pub next_page_token: Option<String>,
}

/// Where `list-pending-signals` looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalScope {
    Instance(String),
    Workflow(String),
    /// The calling instance's children (`requires-instance` without one).
    Children,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSignalsRequest {
    pub scope: SignalScope,
    /// The target's `WaitForSignal` step id.
    pub signal_id: Option<String>,
    pub action_key: Option<String>,
    /// 1-100.
    pub page_size: u32,
    pub page_token: Option<String>,
}

/// One open `WaitForSignal` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSignal {
    pub instance_id: String,
    pub workflow_id: String,
    /// The waiting step's id.
    pub signal_id: String,
    pub request_id: String,
    pub action_key: Option<String>,
    /// JSON Schema the response must match.
    pub response_schema: Option<Vec<u8>>,
    /// Prompt and correlation context, JSON.
    pub context: Option<Vec<u8>>,
    pub requested_at_ms: u64,
    pub deadline_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSignalPage {
    pub items: Vec<PendingSignal>,
    pub next_page_token: Option<String>,
}

/// `send-signal`: answer an open `WaitForSignal` request, validated
/// against its response schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendSignalRequest {
    pub instance_id: String,
    /// The target's `WaitForSignal` step id.
    pub signal_id: String,
    /// Narrow to the request carrying this `action.key`.
    pub action_key: Option<String>,
    /// Pick one request when several are open (`ambiguous` otherwise).
    pub request_id: Option<String>,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendSignalResult {
    pub request_id: String,
    /// This operation had already answered the request.
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelRequest {
    pub instance_id: String,
    pub reason: Option<String>,
    /// Cooperative grace before the forced stop: 0-3600000 ms, default
    /// 5000.
    pub grace_ms: Option<u64>,
}

/// What a lifecycle command did. Acceptance is not arrival: `requested`
/// means the target applies it at its next checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandOutcome {
    Requested,
    /// Took effect at once: a parked run paused, an unlaunched child
    /// cancelled, a paused run relaunched.
    Applied,
    /// The target was already in the requested state.
    Unchanged,
    /// The target had already finished; nothing changed.
    AlreadyTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub instance_id: String,
    pub outcome: CommandOutcome,
    /// This operation had already issued the command.
    pub replayed: bool,
}
