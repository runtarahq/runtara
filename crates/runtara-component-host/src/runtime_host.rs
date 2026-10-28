// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The run services behind the workflow API.
//!
//! [`RuntimeHost`] carries the guest-visible semantics of a run's checkpoints,
//! signals, events, sleeps and terminal publication. The workflow API backend
//! (`api_launches`) serves `/v1` from it, under the run's root lease.
//!
//! Layering: this crate stays persistence-agnostic. The trait is DEFINED here;
//! the production implementation lives in `runtara-environment`, delegating to
//! `runtara-core::instance_handlers` over `Arc<dyn Persistence>`.
//!
//! A plain `durable-sleep` is a `durable-sleep-checkpoint` under
//! [`DURABLE_SLEEP_CHECKPOINT_ID`].

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Checkpoint id used for plain `durable-sleep`: the host glue aliases
/// `durable-sleep` to `durable-sleep-checkpoint` under this key. The value is
/// persisted in existing checkpoints, so it must not change.
pub const DURABLE_SLEEP_CHECKPOINT_ID: &str = "__direct_workflow_runtime_durable_sleep";

/// WIT mirror of the authoritative managed-input state.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lower,
)]
#[component(variant)]
pub enum RuntimeInputState {
    /// Awaiting input before the persistence deadline.
    #[component(name = "open")]
    Open,
    /// Immutable accepted response, retained for replay.
    #[component(name = "accepted")]
    Accepted(Vec<u8>),
    /// Closed without a response; value is a machine-readable reason.
    #[component(name = "closed")]
    Closed(String),
}

/// WIT mirror of `runtime.signal-info`.
///
/// Field order and kebab names must match the WIT record exactly — wasmtime
/// type-checks them against the component's import at instantiation.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lower,
)]
#[component(record)]
pub struct RuntimeSignalInfo {
    /// One of "cancel" | "pause" | "resume" | "shutdown".
    #[component(name = "signal-type")]
    pub signal_type: String,
    /// Identity of the delivered lifecycle command.
    #[component(name = "command-id")]
    pub command_id: String,
    /// Signal payload bytes.
    pub payload: Vec<u8>,
    /// Checkpoint the signal targets, when scoped.
    #[component(name = "checkpoint-id")]
    pub checkpoint_id: Option<String>,
}

/// WIT mirror of `runtime.custom-signal-info`.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lower,
)]
#[component(record)]
pub struct RuntimeCustomSignalInfo {
    /// Identity of this retained value, distinct from its checkpoint address.
    #[component(name = "signal-id")]
    pub signal_id: String,
    /// Checkpoint/wait address the retained value targets.
    #[component(name = "checkpoint-id")]
    pub checkpoint_id: String,
    /// Signal payload bytes.
    pub payload: Vec<u8>,
}

/// WIT mirror of `runtime.checkpoint-result`.
#[derive(
    Debug, Clone, PartialEq, Eq, wasmtime::component::ComponentType, wasmtime::component::Lower,
)]
#[component(record)]
pub struct RuntimeCheckpointResult {
    /// True when an existing checkpoint was found (resume path).
    pub found: bool,
    /// The stored state on a hit; empty on a miss.
    pub state: Vec<u8>,
    /// Pending instance-wide signal, if any.
    #[component(name = "pending-signal")]
    pub pending_signal: Option<RuntimeSignalInfo>,
    /// Pending custom signal scoped to this checkpoint id, if any.
    #[component(name = "custom-signal")]
    pub custom_signal: Option<RuntimeCustomSignalInfo>,
}

/// Native implementation surface for the runtime interface.
///
/// Semantics contract: each method follows the corresponding
/// `runtara-core::instance_handlers` handler. In particular:
///
/// - `is_cancelled`/`check_signals` acknowledge consumed lifecycle signals
///   server-side (status transitions included) through core's signal
///   acknowledgement.
/// - `durable_sleep_checkpoint` mirrors core `handle_sleep`: persist the
///   checkpoint, then sleep the FULL duration in-process (no resume-remaining
///   math — parity with today's guest-visible behavior; the suspend/re-invoke
///   model arrives in a later phase).
/// - Ordinary errors are returned as guest-visible `Err(String)` (the WIT
///   `result`'s err arm). Host misconfiguration and unconfirmed managed-input
///   abandonment trap: the guest must not recover past a failed mandatory close.
///
/// A run finishes through its entry's return value, which the embedding
/// persists once through [`RuntimeHost::terminal`]; the guest has no terminal
/// call. Scoped child hosts report lifecycle
/// receipts to a root-owned coordinator instead of applying root transitions.
/// The embedding finalizes those effects after invocation teardown; it must
/// supply explicit checkpoint authority and persistent attempt fencing.
/// A run's terminal result, persisted once from the entry's return value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTerminal {
    /// Terminal success with the output payload.
    Completed(Vec<u8>),
    /// Terminal failure with the error payload the run records.
    Failed(Vec<u8>),
}

impl RunTerminal {
    /// The terminal of an invocation that completed or failed; `None` when
    /// it parked, trapped, timed out or was cancelled, which the embedding
    /// records its own way.
    pub fn from_exit(exit: &crate::workflow::InvokeExit) -> Option<Self> {
        match exit {
            crate::workflow::InvokeExit::Completed(output) => Some(Self::Completed(output.clone())),
            crate::workflow::InvokeExit::Failed(error) => Some(Self::Failed(error_payload(error))),
            _ => None,
        }
    }
}

/// The bytes a run records for its error: `details` verbatim when the
/// workflow supplied them (the stdlib's envelope, so a failure records what it
/// always did); else the plain `message` when there is no `code`; else JSON
/// built from the fields.
pub fn error_payload(error: &crate::lifecycle::WorkflowErrorInfo) -> Vec<u8> {
    if let Some(details) = &error.details {
        return details.clone().into_bytes();
    }
    if error.code.is_empty() {
        return error.message.clone().into_bytes();
    }
    let mut object = serde_json::Map::new();
    object.insert("code".into(), error.code.clone().into());
    object.insert("message".into(), error.message.clone().into());
    object.insert("category".into(), error.category.clone().into());
    object.insert("severity".into(), error.severity.clone().into());
    object.insert("retryable".into(), error.retryable.into());
    if let Some(retry_after_ms) = error.retry_after_ms {
        object.insert("retryAfterMs".into(), retry_after_ms.into());
    }
    if let Some(attributes) = &error.attributes {
        object.insert(
            "attributes".into(),
            serde_json::from_str(attributes).unwrap_or_else(|_| attributes.clone().into()),
        );
    }
    serde_json::Value::Object(object).to_string().into_bytes()
}

/// What the workflow API may do to a running `wasi:cli/run` entry.
#[derive(Clone)]
pub struct WorkflowApiControls {
    /// Bring the run's deadline forward to the given time from now (never
    /// later); the run then ends as timed out.
    pub shorten_deadline: Arc<dyn Fn(Duration) + Send + Sync>,
    /// The `runtara:trusted-artifacts/*` pins of the run's artifact, which
    /// decide its trusted calls (`TrustedExecutor::admits`). Empty admits
    /// none.
    pub trusted_pins: Arc<std::collections::HashSet<String>>,
    /// The run's whole-execution cleanup alarm (`POST /v1/run/alarm`);
    /// none outside a run.
    pub(crate) cleanup_alarm: Option<crate::cleanup_alarm::CleanupAlarmState>,
    /// The run's child-run context (`/v1/tasks`); none for a run that
    /// cannot start children.
    pub(crate) execution: Option<Arc<crate::execution_host::ExecutionContext>>,
    /// An expired cleanup alarm ends the run through `shorten_deadline`: the
    /// guest runs elsewhere, so no executor here watches the alarm.
    pub(crate) alarm_shortens_deadline: bool,
}

impl WorkflowApiControls {
    /// Controls for a launch whose guest runs outside this process. It has no
    /// in-process child runs, so those calls are answered as for a run
    /// without them. Its cleanup alarm ends the run when it expires, through
    /// `shorten_deadline`, which is how the runner ends it.
    pub fn remote(
        shorten_deadline: Arc<dyn Fn(Duration) + Send + Sync>,
        trusted_pins: Arc<std::collections::HashSet<String>>,
    ) -> Self {
        Self {
            shorten_deadline,
            trusted_pins,
            cleanup_alarm: Some(crate::cleanup_alarm::CleanupAlarmState::default()),
            execution: None,
            alarm_shortens_deadline: true,
        }
    }
}

impl std::fmt::Debug for WorkflowApiControls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkflowApiControls")
    }
}

#[async_trait::async_trait]
pub trait RuntimeHost: Send + Sync {
    /// This run's instance id.
    fn instance_id(&self) -> Result<String, String>;
    /// The binding the run's workflow API launch is registered under, asked
    /// for when its entry starts. `None` (a host without a root lease, such
    /// as a test host) gets a local binding that is never fenced.
    fn workflow_api_binding(&self) -> Option<runtara_workflow_api::Binding> {
        None
    }
    /// Persist the run's terminal result. The embedding calls it once, after
    /// the entry returned and cleanup finished; it is never bound for the
    /// guest. A host that does not own the instance's terminal state (a
    /// scoped child, most test doubles) keeps the default no-op.
    async fn terminal(&self, terminal: RunTerminal) -> Result<(), String> {
        let _ = terminal;
        Ok(())
    }
    /// Whether a durable write of this run was refused because another
    /// execution now holds it. Its workflow API launch is fenced from then on.
    fn superseded(&self) -> bool {
        false
    }
    /// Whether this run's execution still holds the run, read durably: false
    /// once recovery revoked its lease or another launch holds it. A host
    /// without a lease always holds its run.
    async fn holds_run(&self) -> bool {
        true
    }
    /// Emit a custom event (`kind` becomes the event subtype).
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String>;
    /// Whether step-level debug instrumentation is enabled for this run.
    fn debug_mode_enabled(&self) -> Result<bool, String>;
    /// Suspend at a breakpoint: acknowledge a pause and mark suspended.
    async fn breakpoint_pause(&self) -> Result<(), String>;
    /// Liveness heartbeat.
    async fn heartbeat(&self) -> Result<(), String>;
    /// Read a lifecycle command without acknowledging it or publishing status.
    /// May be rate-limited. The guest retains observed intent until cleanup and
    /// uses `handle_checkpoint_signal` to acknowledge the exact command later.
    async fn poll_signal(&self) -> Result<Option<RuntimeSignalInfo>, String>;
    /// True when a cancel signal is pending or was already consumed.
    async fn is_cancelled(&self) -> Result<bool, String>;
    /// Poll lifecycle signals; true when a stop-like signal was handled and
    /// the guest should return.
    async fn check_signals(&self) -> Result<bool, String>;
    /// Poll a custom signal scoped to `checkpoint_id`.
    async fn poll_custom_signal(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String>;
    /// Register a managed wait under this host's immutable execution authority.
    async fn register_input(
        &self,
        _descriptor: Vec<u8>,
        _deadline_ms: Option<u64>,
    ) -> Result<(), String> {
        Err("runtime does not support managed inputs".into())
    }
    /// Authoritative response read and deadline arbitration.
    async fn poll_input(&self, _signal_id: String) -> Result<RuntimeInputState, String> {
        Err("runtime does not support managed inputs".into())
    }
    /// Abandon a wait under this host's authority; accepted responses are retained.
    /// The native import traps on failure so guest error handling cannot continue
    /// with an orphaned request. Supervision must settle the failed execution.
    async fn close_input(&self, _signal_id: String) -> Result<RuntimeInputState, String> {
        Err("runtime does not support managed inputs".into())
    }
    /// Read-only checkpoint lookup.
    async fn get_checkpoint(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String>;
    /// Combined save/load checkpoint (see core `handle_checkpoint`).
    async fn checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String>;
    /// React to a pending signal reported by a checkpoint result; true when a
    /// stop-like signal was handled and the guest should return.
    async fn handle_checkpoint_signal(
        &self,
        signal_type: String,
        command_id: String,
    ) -> Result<bool, String>;
    /// Record a retry attempt (write-only audit trail).
    async fn record_retry_attempt(
        &self,
        checkpoint_id: String,
        attempt_number: u32,
        error_message: Option<String>,
    ) -> Result<(), String>;
    /// Persist a wake checkpoint, then sleep the full duration in-process.
    async fn durable_sleep_checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String>;
    /// The continuation an operation-scoped call site saved for `attempt`, if
    /// any. `op_hash` is the sha256 of the site's canonical checkpoint key; the
    /// host scopes it to this instance. Hosts without typed agent suspension
    /// refuse, so a suspending step fails loudly instead of losing state.
    async fn operation_continuation_load(
        &self,
        _op_hash: String,
        _attempt: u32,
    ) -> Result<Option<Vec<u8>>, String> {
        Err("runtime does not support typed agent suspension".into())
    }
    /// Persist the continuation of a suspended operation (at most
    /// `runtara_agent_suspension::MAX_CONTINUATION_BYTES`), replacing any
    /// earlier one for the same `(op_hash, attempt)`.
    async fn operation_continuation_store(
        &self,
        _op_hash: String,
        _attempt: u32,
        _state: Vec<u8>,
    ) -> Result<(), String> {
        Err("runtime does not support typed agent suspension".into())
    }
    /// Close the operation's instance wait, if it registered one, so a retry
    /// of the operation registers afresh. Idempotent: an operation without a
    /// wait, or with a closed one, is fine.
    async fn operation_wait_close(&self, _op_hash: String) -> Result<(), String> {
        Err("runtime does not support typed agent suspension".into())
    }
    /// Drop everything kept for an operation: its continuation and its
    /// settled or closed instance wait. Idempotent.
    async fn operation_release(&self, _op_hash: String) -> Result<(), String> {
        Err("runtime does not support typed agent suspension".into())
    }
    /// Milliseconds since the UNIX epoch, as the guest sees them.
    ///
    /// Defaults to the wall clock, which is what every production host uses. It
    /// is a trait method rather than a free function so a harness standing in
    /// for the wake scheduler can advance to a park's deadline instead of
    /// waiting out real time: a resumed durable Delay compares `now-ms` against
    /// its stored deadline to decide whether to continue or re-park, so without
    /// a movable clock a park can only ever be observed re-parking.
    fn now_ms(&self) -> Result<u64, String> {
        wall_clock_now_ms()
    }

    /// How the host launched this run, which decides whether a trusted call
    /// under an earlier approved pin may run (`TrustedExecutor::admits`).
    /// Host authority, never read from the guest. The default is the
    /// strictest, `Start`; a host that relaunches parked runs overrides it,
    /// and a wrapper must delegate.
    fn trusted_launch(&self) -> crate::trusted::TrustedLaunch {
        crate::trusted::TrustedLaunch::Start
    }

    /// Note that the run is about to wait inside its execution (a timer):
    /// it keeps its resources while it waits, unlike a durable suspension.
    /// The returned marker covers the wait until it is dropped. The default
    /// does not track waits; a wrapper must delegate.
    fn in_process_wait(&self) -> Option<Box<dyn std::any::Any + Send>> {
        None
    }
}

/// Milliseconds since the UNIX epoch (the default `now-ms` implementation).
fn wall_clock_now_ms() -> Result<u64, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| "current UNIX timestamp does not fit in u64 milliseconds".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_epoch_scaled() {
        let value = wall_clock_now_ms().expect("clock after epoch");
        // 2020-01-01 in ms — sanity floor that catches unit mistakes
        // (seconds vs milliseconds) without pinning a wall clock.
        assert!(value > 1_577_836_800_000);
    }
}
