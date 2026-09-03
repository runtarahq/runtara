// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runner trait definitions.
//!
//! Defines the abstract interface for instance runners.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::watch;

/// Errors from runner operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RunnerError {
    /// Binary executable was not found.
    #[error("Binary not found: {0}")]
    BinaryNotFound(String),

    /// Execution timed out.
    #[error("Execution timeout")]
    Timeout,

    /// Execution was cancelled.
    #[error("Execution cancelled")]
    Cancelled,

    /// The workflow guest failed to start.
    #[error("Start failed: {0}")]
    StartFailed(String),

    /// No runner capacity is available right now.
    ///
    /// Callers that own a durable queue must return the launch to that queue
    /// rather than await a semaphore permit in a request or worker task.
    #[error("Runner capacity unavailable")]
    CapacityUnavailable,

    /// Process exited with non-zero code.
    #[error("Exit code {exit_code}: {stderr}")]
    ExitCode {
        /// Exit code from the process.
        exit_code: i32,
        /// Standard error output.
        stderr: String,
    },

    /// Output file was not found.
    #[error("Output not found for instance: {0}")]
    OutputNotFound(String),

    /// I/O operation failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Other error.
    #[error("Other: {0}")]
    Other(String),
}

/// Result type for runner operations.
pub type Result<T> = std::result::Result<T, RunnerError>;

/// Result of waiting for a supervisor-owned launch gate.
///
/// A detached runner may reserve a bounded run slot before its owner has
/// finished installing durable ownership.  It must not execute guest code in
/// that interval: the gate makes the handoff explicit, and its deadline makes
/// a process loss during handoff self-releasing rather than a leaked permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartGateOutcome {
    /// The supervisor installed ownership and explicitly allowed guest work.
    Opened,
    /// The supervisor rejected the handoff before guest work began.
    Cancelled,
    /// No owner opened the gate before its absolute handoff deadline.
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartGateState {
    Pending,
    Opened,
    Cancelled,
}

/// A one-way gate between a generation supervisor and a detached runner task.
///
/// The gate starts closed. Only one of [`Self::open`], [`Self::cancel`], or the
/// absolute deadline may win; once it has a terminal outcome it cannot be
/// reopened. The state sits behind a mutex so an expiry racing the supervisor
/// cannot overwrite a successful open (or vice versa), while a watch channel
/// makes the runner's wait cancel-safe without polling.
#[derive(Clone)]
pub struct StartGate {
    state: Arc<Mutex<StartGateState>>,
    updates: watch::Sender<StartGateState>,
    deadline: tokio::time::Instant,
}

impl std::fmt::Debug for StartGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StartGate")
            .field("state", &self.state())
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl StartGate {
    /// Build a closed gate whose owner must decide before `handoff_timeout`.
    pub fn new(handoff_timeout: Duration) -> Self {
        Self::until(tokio::time::Instant::now() + handoff_timeout)
    }

    /// Build a closed gate with an explicit monotonic deadline.
    pub fn until(deadline: tokio::time::Instant) -> Self {
        let (updates, _) = watch::channel(StartGateState::Pending);
        Self {
            state: Arc::new(Mutex::new(StartGateState::Pending)),
            updates,
            deadline,
        }
    }

    /// The absolute monotonic deadline at which a still-closed gate expires.
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }

    /// Allow guest execution exactly once.
    ///
    /// Returns `false` when cancellation or timeout already won the handoff.
    pub fn open(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *state != StartGateState::Pending {
            return false;
        }
        // `wait()` may not have polled precisely at the deadline yet. Do not
        // let a delayed supervisor turn that scheduling gap into a late guest
        // start after durable lease recovery became legal.
        if tokio::time::Instant::now() >= self.deadline {
            *state = StartGateState::Cancelled;
            self.updates.send_replace(StartGateState::Cancelled);
            return false;
        }
        *state = StartGateState::Opened;
        self.updates.send_replace(StartGateState::Opened);
        true
    }

    /// Prevent guest execution exactly once.
    ///
    /// Returns `false` when the gate is already open or cancelled.
    pub fn cancel(&self) -> bool {
        self.transition(StartGateState::Cancelled)
    }

    /// Wait until the owner opens or cancels the gate, or its deadline passes.
    ///
    /// If expiry wins it atomically cancels the gate before returning, so a
    /// delayed supervisor cannot later start the old generation.
    pub async fn wait(&self) -> StartGateOutcome {
        let mut updates = self.updates.subscribe();
        loop {
            match *updates.borrow_and_update() {
                StartGateState::Opened => return StartGateOutcome::Opened,
                StartGateState::Cancelled => return StartGateOutcome::Cancelled,
                StartGateState::Pending => {}
            }

            tokio::select! {
                changed = updates.changed() => {
                    // The sender belongs to the gate itself, so a closed
                    // channel can only happen while all gate owners are being
                    // dropped. Treat it as a cancellation rather than letting
                    // a runner retain a permit indefinitely.
                    if changed.is_err() {
                        return StartGateOutcome::Cancelled;
                    }
                }
                _ = tokio::time::sleep_until(self.deadline) => {
                    if self.cancel() {
                        return StartGateOutcome::TimedOut;
                    }
                }
            }
        }
    }

    fn state(&self) -> StartGateState {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn transition(&self, next: StartGateState) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *state != StartGateState::Pending {
            return false;
        }
        *state = next;
        self.updates.send_replace(next);
        true
    }
}

/// Options for launching an instance.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Immutable identifier for this physical launch attempt.
    ///
    /// An instance may park and later resume several times.  Each attempt needs
    /// a distinct owner so stale task cleanup, monitor results, and cancellation
    /// can never affect a newer attempt of the same durable instance.
    pub launch_id: String,
    /// Instance ID (UUID)
    pub instance_id: String,
    /// Tenant ID
    pub tenant_id: String,
    /// Path to the image's composed `workflow.wasm`.
    pub wasm_path: std::path::PathBuf,
    /// Input data for the instance
    pub input: Value,
    /// Execution timeout
    pub timeout: Duration,
    /// Checkpoint ID to resume from (for wakes/resumes)
    pub checkpoint_id: Option<String>,
    /// Custom environment variables (applied after system vars, can override)
    pub env: std::collections::HashMap<String, String>,
    /// The instance's enriched input envelope, exactly as it was just written
    /// to the store, for a caller that has the authoritative bytes in hand.
    ///
    /// `None` means "read them back from the store", which is what a wake or a
    /// resume MUST do: their `input` field is a relaunch placeholder, not the
    /// instance's real input, so serving it to the guest would silently change
    /// what a woken workflow sees. Only the first-start path may set this, and
    /// only once `store_instance_input` has actually succeeded.
    pub prepersisted_input: Option<Vec<u8>>,
    /// Optional supervisor gate that must open before the detached task loads
    /// or invokes the guest. Direct/legacy callers leave this unset; durable
    /// dispatchers always provide one.
    pub start_gate: Option<StartGate>,
}

/// Handle for a launched instance (detached execution).
#[derive(Debug, Clone)]
pub struct RunnerHandle {
    /// Immutable identifier for this physical launch attempt.
    pub launch_id: String,
    /// Unique identifier for this launch.
    pub handle_id: String,
    /// Instance ID
    pub instance_id: String,
    /// Tenant ID
    pub tenant_id: String,
    /// When the instance was started
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Resource metrics sampled while the process is alive.
    pub metrics: Option<std::sync::Arc<tokio::sync::Mutex<ContainerMetrics>>>,
}

/// Resource metrics collected from the instance execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerMetrics {
    /// Peak memory usage in bytes
    pub memory_peak_bytes: Option<u64>,
    /// Current memory usage in bytes (at time of collection)
    pub memory_current_bytes: Option<u64>,
    /// Total CPU time in microseconds
    pub cpu_usage_usec: Option<u64>,
}

/// Result of a synchronous instance execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchResult {
    /// Instance ID.
    pub instance_id: String,
    /// Whether execution succeeded.
    pub success: bool,
    /// Output data from successful execution.
    pub output: Option<Value>,
    /// Error message from failed execution (user-facing).
    pub error: Option<String>,
    /// Raw stderr output from the container (for debugging/logging).
    /// This is separate from `error` to allow product to decide whether to show it to users.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Resource metrics from execution.
    #[serde(default)]
    pub metrics: ContainerMetrics,
}

/// Cancellation token for stopping execution.
pub type CancelToken = Arc<AtomicBool>;

/// How much of a runner's concurrency bound is currently spoken for.
///
/// `held` answers "is the stage full"; `oldest_held_ms` answers the question a
/// count cannot, which is whether a full stage is turning work over as fast as
/// the host allows or holding work that never leaves. Those look identical on a
/// gauge and call for opposite responses, so the age is the point of this type
/// rather than a nicety: a runner pinned at its bound with a permit held for
/// forty minutes is stalled, and the same runner pinned with permits recycling
/// every few seconds is merely busy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerOccupancy {
    /// Concurrency bound this runner enforces.
    pub limit: u64,
    /// Permits currently held.
    ///
    /// Read from the semaphore rather than counted from any bookkeeping map, so
    /// it stays authoritative even in the window where a permit has been taken
    /// but its acquisition time is not yet recorded.
    pub held: u64,
    /// Age of the longest-held permit, if anything is running.
    pub oldest_held_ms: Option<u64>,
    /// Instance holding that longest-held permit.
    pub oldest_instance_id: Option<String>,
    /// Runs this runner has begun executing, since process start.
    pub runs_started: u64,
    /// Runs this runner has finished executing, since process start.
    ///
    /// "Finished" means the guest stopped and gave its permit back, which
    /// includes a run that parked itself to await a wake or a signal. It is
    /// therefore the throughput of the executing stage, and deliberately not a
    /// count of instances reaching a terminal status — those differ whenever
    /// durable workflows are in play, and conflating them would report a
    /// healthy parking workload as a flood of completions.
    pub runs_finished: u64,
}

/// Trait for instance runners.
///
/// Runners are responsible for launching and managing workflow guests.
/// `EmbeddedWasmRunner` is the only production implementation; `MockRunner`
/// backs the tests.
///
/// Runners read instance output from persistence (runtara-core) after process exit.
/// Database writes (registration, status updates) are handled by the caller.
#[async_trait]
pub trait Runner: Send + Sync {
    /// Runner type identifier (e.g., "wasm-embedded", "mock")
    fn runner_type(&self) -> &'static str;

    /// Run an instance synchronously, waiting for completion.
    ///
    /// This method blocks until the instance completes, times out, or is cancelled.
    async fn run(
        &self,
        options: &LaunchOptions,
        cancel_token: Option<CancelToken>,
    ) -> Result<LaunchResult>;

    /// Launch an instance without waiting for completion (fire-and-forget).
    ///
    /// Returns a handle that can be used to check status or stop the instance.
    /// The caller is responsible for registering the instance in the database.
    async fn launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle>;

    /// Launch an instance only when runner capacity is immediately available.
    ///
    /// A full runner returns [`RunnerError::CapacityUnavailable`] without
    /// parking the caller on an in-memory permit waiter. Durable dispatchers
    /// use this method; [`Self::launch_detached`] remains for legacy callers
    /// that intentionally await capacity.
    async fn try_launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle>;

    /// Check if an instance is still running.
    async fn is_running(&self, handle: &RunnerHandle) -> bool;

    /// Stop a running instance.
    async fn stop(&self, handle: &RunnerHandle) -> Result<()>;

    /// Collect metrics and cleanup after instance has finished.
    ///
    /// Returns (output, error, metrics).
    async fn collect_result(
        &self,
        handle: &RunnerHandle,
    ) -> (Option<Value>, Option<String>, ContainerMetrics);

    /// Current occupancy of this runner's concurrency bound, if it has one.
    ///
    /// Defaults to `None` — "this runner does not report occupancy" — which is
    /// distinct from `Some` with a zero `held`, i.e. "nothing is running". A
    /// caller must render the two differently: collapsing an unavailable source
    /// to zero is how a dashboard invents an idle system that is actually
    /// unobserved.
    fn occupancy(&self) -> Option<RunnerOccupancy> {
        None
    }

    /// Wait for the instance to exit, polling with the given interval.
    ///
    /// The default implementation polls [`Runner::is_running`] at `poll_interval`.
    /// Runners that can await their run directly should override it.
    ///
    /// Implementations must be cancel-safe: when the surrounding `select!` drops
    /// this future on a timeout, no resources should leak.
    async fn wait_for_exit(&self, handle: &RunnerHandle, poll_interval: Duration) {
        while self.is_running(handle).await {
            tokio::time::sleep(poll_interval).await;
        }
    }
}

#[cfg(test)]
mod start_gate_tests {
    use super::{StartGate, StartGateOutcome};
    use std::time::Duration;

    #[tokio::test]
    async fn gate_opens_exactly_once() {
        let gate = StartGate::new(Duration::from_secs(1));
        let waiter = gate.clone();
        let waiting = tokio::spawn(async move { waiter.wait().await });

        assert!(gate.open());
        assert!(!gate.open(), "a gate may not be opened twice");
        assert!(!gate.cancel(), "an opened gate may not be cancelled later");
        assert_eq!(
            waiting.await.expect("gate waiter must not panic"),
            StartGateOutcome::Opened
        );
    }

    #[tokio::test]
    async fn deadline_closes_a_gate_before_a_late_owner_can_open_it() {
        let gate = StartGate::new(Duration::from_millis(10));
        let outcome = tokio::time::timeout(Duration::from_secs(1), gate.wait())
            .await
            .expect("gate deadline must wake its waiter");
        assert_eq!(outcome, StartGateOutcome::TimedOut);
        assert!(
            !gate.open(),
            "an owner delayed beyond the durable handoff deadline must not start a guest"
        );
    }
}
