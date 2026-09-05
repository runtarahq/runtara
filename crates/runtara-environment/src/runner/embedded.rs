// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Embedded (in-process) workflow runner.
//!
//! Executes composed workflow components through
//! `runtara-component-host::WorkflowExecutor` — env vars from
//! [`super::common::build_env`], output read from runtara-core persistence,
//! stderr in the per-run `stderr.log`. No process per instance: each run is
//! a tokio task with its own wasmtime `Store`.
//!
//! Semantics (vs the retired wasmtime-CLI process runner):
//! - `RunnerHandle.spawned_pid` is `None`. Startup recovery treats pid-less
//!   registry entries as dead, which is exactly right here: an in-process
//!   instance cannot survive a server restart, and resumes go through the
//!   durable checkpoint path.
//! - `stop()` raises a cancel flag; the executor's epoch/watchdog rings end
//!   the run within ~one tick (100 ms).
//! - Memory metrics come from the store's resource limiter (exact guest
//!   linear-memory peak); CPU metrics are absent.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::sync::{OwnedSemaphorePermit, TryAcquireError};
use tokio::time::timeout_at;
use tracing::{debug, error, info, warn};

use runtara_component_host::precompile::{
    PRECOMPILE_NONCE_BYTES, PRECOMPILE_WORKER_ARGUMENT, PrecompileRequest, PrecompileResponse,
    deserialize_trusted_precompiled_component, read_precompile_response_async,
    validate_precompile_response, write_precompile_request_async,
};
use runtara_component_host::{
    EngineConfig, PreparedWorkflow, WorkflowExecutor, WorkflowExit, WorkflowLimits,
    WorkflowRunSpec, WorkflowStartConfirmation, build_engine, spawn_epoch_ticker,
};
use runtara_core::instance_handlers::{
    InstanceHandlerState, SignalAck, SignalType, handle_signal_ack,
};
use runtara_core::persistence::Persistence;

use super::common::{self, WorkflowRunnerConfig};
use super::traits::{
    CancelToken, ContainerMetrics, LaunchOptions, PreparationOccupancy, PreparedLaunch, Result,
    Runner, RunnerError, RunnerHandle, RunnerOccupancy, StartGateOutcome,
};

/// Mark a run `running`, clearing what a previous stop left behind.
///
/// Supplying a `started_at` is what makes the persistence layer also clear
/// `finished_at` and `termination_reason` — the remnants a drain stamps on a row
/// it force-stopped. Without it a relaunched instance executes as `running`
/// while still advertising the moment it stopped, so every duration derived from
/// that pair is wrong, and turns negative once anything moves `started_at` past
/// the stale finish.
///
/// The existing `started_at` is re-used rather than restamped, so a run that
/// suspends and wakes still reports when it first began.
async fn mark_running(persistence: &dyn Persistence, instance_id: &str) {
    // One statement: the COALESCE inside `mark_instance_running` carries the
    // original `started_at` forward, which this used to read the row to do.
    if let Err(e) = persistence
        .mark_instance_running(instance_id, chrono::Utc::now())
        .await
    {
        warn!(instance_id, error = %e, "Failed to mark invoke instance running");
    }
}

/// Per-launch bookkeeping for detached runs.
struct InstanceTask {
    cancel: CancelToken,
    finished: AtomicBool,
    done: tokio::sync::Notify,
}

/// Environment's bridge from the runner-owned in-memory gate to the
/// component-host pre-instantiation boundary.
///
/// It is deliberately invoked only after the host has built its Store/WASI
/// state, immediately before `instantiate_async`. A crash or pause in any
/// earlier runner work therefore leaves the durable gate marker intact for
/// queue recovery instead of advertising a guest that never began.
struct GateWorkflowStartConfirmation {
    gate: super::traits::StartGate,
}

#[async_trait]
impl WorkflowStartConfirmation for GateWorkflowStartConfirmation {
    async fn confirm_before_instantiate(&self) -> anyhow::Result<()> {
        match self.gate.wait_and_confirm().await {
            StartGateOutcome::Opened => Ok(()),
            StartGateOutcome::Cancelled => {
                anyhow::bail!("start gate was cancelled before guest instantiation")
            }
            StartGateOutcome::TimedOut => {
                anyhow::bail!("start gate timed out before guest instantiation")
            }
            StartGateOutcome::ConfirmationFailed => {
                anyhow::bail!("durable start gate confirmation failed before guest instantiation")
            }
        }
    }
}

type TaskRegistry = Arc<Mutex<HashMap<String, Arc<InstanceTask>>>>;

/// Remove a detached task only when this generation still owns the registry
/// entry. A replacement can be installed while an old task is unwinding; an
/// unconditional remove would make the live replacement invisible to stop and
/// monitoring paths.
fn remove_task_if_current(registry: &TaskRegistry, launch_id: &str, task: &Arc<InstanceTask>) {
    let mut tasks = registry
        .lock()
        // This can run while a guest task is already unwinding. Recovering
        // the map is safer than turning a bookkeeping cleanup panic into an
        // abort that permanently leaks the visible runner handle.
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if tasks
        .get(launch_id)
        .is_some_and(|current| Arc::ptr_eq(current, task))
    {
        tasks.remove(launch_id);
    }
}

/// Guarantees that a detached task retires its visible handle even while it
/// unwinds from a panic or cancellation.
///
/// The run permit has its own RAII guard, but the task map is also a capacity
/// signal to monitors. Leaving it `finished = false` after a panic makes a
/// completed generation look permanently live until an unrelated cleanup.
struct TaskCompletionGuard {
    task: Arc<InstanceTask>,
    registry: TaskRegistry,
    launch_id: String,
}

impl Drop for TaskCompletionGuard {
    fn drop(&mut self) {
        self.task.finished.store(true, Ordering::SeqCst);
        remove_task_if_current(&self.registry, &self.launch_id, &self.task);
        self.task.done.notify_waiters();
    }
}

/// In-process workflow runner backed by an embedded wasmtime engine.
pub struct EmbeddedWasmRunner {
    config: WorkflowRunnerConfig,
    /// Address legacy HTTP-composed artifacts use for runtara-core. Modern
    /// HostImport-composed artifacts receive the native runtime host instead.
    core_http_url: Option<String>,
    limits: WorkflowLimits,
    persistence: Arc<dyn Persistence>,
    executor: Arc<WorkflowExecutor>,
    tasks: TaskRegistry,
    /// Independently bounds filesystem, persisted-input, and component work
    /// performed before a guest consumes a live run permit.
    preparation_permits: Arc<tokio::sync::Semaphore>,
    /// The bound `preparation_permits` was built with.
    preparation_limit: usize,
    /// Acquisition times for preparation permits, surfaced separately from
    /// live-run occupancy in the pipeline.
    preparation_slots: PreparationSlotRegistry,
    /// The killable compiler boundary used for durable preparation. It has a
    /// bounded replacement budget separate from the in-process prep permits:
    /// a child stuck in uninterruptible kernel I/O keeps its child slot while
    /// a fresh durable attempt can use the bounded spare capacity.
    precompiler: Arc<dyn ComponentPrecompiler>,
    /// Bounds how many guests execute at once, whatever asks for them.
    ///
    /// `launch_detached` spawns the run and returns, so the caller's own
    /// concurrency limit does not bound execution — it bounds how fast runs are
    /// *started*. Nothing else stops a fast producer stacking guests until the
    /// machine runs out of memory, and each live guest holds a wasmtime store.
    /// A serial caller used to hide this by never asking for more than one at a
    /// time; that is not a limit anything should rely on.
    run_permits: Arc<tokio::sync::Semaphore>,
    /// The bound `run_permits` was built with.
    ///
    /// A `Semaphore` reports what is *available*, never what it started with,
    /// so the total has to be kept alongside it to turn that into occupancy.
    run_limit: usize,
    /// When each currently-held run permit was taken.
    ///
    /// Exists so a full runner can be told apart from a stuck one — see
    /// [`RunnerOccupancy`]. Bounded by `run_limit` (cores x 4 by default), so
    /// it needs no eviction policy; entries are removed by [`RunSlot`]'s drop,
    /// which runs on the success, error and panic paths alike.
    run_slots: RunSlotRegistry,
    /// Runs begun since process start.
    runs_started: Arc<AtomicU64>,
    /// Runs finished since process start, incremented as each permit returns.
    runs_finished: Arc<AtomicU64>,
    /// Shared handler state for per-run [`PersistenceRuntimeHost`]s — the
    /// native runtime interface for HostImport-composed artifacts.
    handler_state: Arc<runtara_core::instance_handlers::InstanceHandlerState>,
}

/// A run permit holder, keyed by launch generation.
#[derive(Clone)]
struct RunSlotEntry {
    instance_id: String,
    taken_at: Instant,
}

/// Acquisition times of the run permits currently held, keyed by launch.
type RunSlotRegistry = Arc<Mutex<HashMap<String, RunSlotEntry>>>;

/// A preparation permit holder, keyed by launch generation.
#[derive(Clone)]
struct PreparationSlotEntry {
    instance_id: String,
    taken_at: Instant,
}

/// Acquisition times of preparation permits currently held, keyed by launch.
type PreparationSlotRegistry = Arc<Mutex<HashMap<String, PreparationSlotEntry>>>;

#[derive(Clone)]
struct PrecompileChildSlotEntry {
    started_at: Instant,
    retired: bool,
}

/// Process-private bookkeeping for live and reaping precompile children.
///
/// The semaphore remains authoritative for the held count; this registry
/// supplies oldest age and the reaping subset for the pipeline so a timeout is
/// never rendered as an idle preparation pool while an unkillable child still
/// exists.
type PrecompileChildRegistry = Arc<Mutex<HashMap<String, PrecompileChildSlotEntry>>>;

/// A held run permit, tied to the moment it was taken.
///
/// The permit alone would bound concurrency perfectly well; this wrapper exists
/// only so that releasing it also retires the acquisition time. Pairing them in
/// one value is what makes the two impossible to drift apart — there is no path
/// that returns a permit without also dropping this.
struct RunSlot {
    /// Dropped with the struct; that release is the whole point of the field.
    _permit: tokio::sync::OwnedSemaphorePermit,
    launch_id: String,
    registry: RunSlotRegistry,
    /// Bumped as the permit returns, so the count of finished runs cannot drift
    /// from the count of released permits.
    finished: Arc<AtomicU64>,
}

/// A held preparation permit. It is intentionally carried inside
/// [`EmbeddedPreparedLaunch`] until the child-validated component reaches the
/// short run-permit handoff. If a preparation outlives its durable lease,
/// dropping the token cannot make the preparation pool look idle early.
struct PreparationSlot {
    _permit: tokio::sync::OwnedSemaphorePermit,
    /// Includes the durable preparation claim incarnation so a recovered
    /// compiler cannot overwrite/remove telemetry for a newer same-launch
    /// attempt that happens to share the dispatcher owner.
    slot_key: String,
    registry: PreparationSlotRegistry,
}

/// Everything a verified pre-run phase hands to the short run-permit phase.
///
/// The opaque outer [`PreparedLaunch`] keeps this runner-specific state out of
/// the durable dispatcher. It owns the exact component returned by the child
/// compiler and releases preparation capacity before a live run permit is
/// requested.
struct EmbeddedPreparedLaunch {
    _preparation_slot: PreparationSlot,
    workflow: PreparedWorkflow,
    input: Vec<u8>,
}

/// A bounded child-process compiler for durable preparations.
///
/// All source-file reads, hashing, and Wasmtime compilation happen behind
/// this interface. The parent only receives a validated serialized component
/// over a private pipe and links it without a filesystem read.
#[async_trait]
trait ComponentPrecompiler: Send + Sync {
    async fn prepare(
        &self,
        executor: &WorkflowExecutor,
        options: &LaunchOptions,
    ) -> Result<PreparedWorkflow>;

    fn child_occupancy(&self) -> Option<PrecompileChildOccupancy> {
        None
    }
}

#[derive(Clone, Copy)]
struct PrecompileChildOccupancy {
    limit: u64,
    held: u64,
    retired: u64,
    oldest_held_ms: Option<u64>,
}

/// Production implementation of [`ComponentPrecompiler`].
///
/// The program is always the current server executable, which exposes the
/// hidden `--internal-precompile-component` command. Keeping both ends in the
/// same trusted binary is part of the provenance contract for Wasmtime's
/// unsafe serialized-component deserialization boundary.
struct ChildComponentPrecompiler {
    child_permits: Arc<tokio::sync::Semaphore>,
    child_limit: usize,
    child_slots: PrecompileChildRegistry,
    /// Child processes still being reaped after a preparation deadline.
    /// They hold a child permit until `wait()` completes, preventing a stuck
    /// mount from becoming an unbounded stream of replacement processes.
    retired_children: Arc<AtomicU64>,
}

impl ChildComponentPrecompiler {
    fn new(preparation_limit: usize) -> Self {
        // Keep one replacement generation available for each active prep
        // worker. A hard cap means permanently unkillable (for example D
        // state) children eventually apply backpressure instead of multiplying
        // forever, while a normal timeout can immediately make forward
        // progress without borrowing a guest run slot.
        let child_limit = max_concurrent_precompile_children(preparation_limit);
        Self {
            child_permits: Arc::new(tokio::sync::Semaphore::new(child_limit)),
            child_limit,
            child_slots: Arc::new(Mutex::new(HashMap::new())),
            retired_children: Arc::new(AtomicU64::new(0)),
        }
    }

    fn worker_program(&self) -> Result<PathBuf> {
        std::env::current_exe().map_err(|error| {
            RunnerError::Other(format!(
                "could not resolve current server executable for internal component precompile worker: {error}"
            ))
        })
    }

    async fn exchange(
        &self,
        options: &LaunchOptions,
    ) -> Result<(PrecompileRequest, PrecompileResponse)> {
        let deadline = options
            .preparation_deadline
            .unwrap_or_else(|| tokio::time::Instant::now() + DEFAULT_PRECOMPILER_TIMEOUT);
        if deadline <= tokio::time::Instant::now() {
            return Err(RunnerError::PreparationTimedOut(
                "precompile worker reached its preparation deadline before launch".to_string(),
            ));
        }

        let child_permit = Arc::clone(&self.child_permits)
            .try_acquire_owned()
            .map_err(|error| match error {
                TryAcquireError::NoPermits => RunnerError::PreparationCapacityUnavailable,
                TryAcquireError::Closed => {
                    RunnerError::Other("precompile child semaphore closed".to_string())
                }
            })?;
        let nonce = precompile_nonce()?;
        let child_slot_key = hex_digest(nonce);
        let request =
            PrecompileRequest::for_artifact(nonce, &options.wasm_path).map_err(|error| {
                RunnerError::StartFailed(format!("build precompile request: {error:#}"))
            })?;
        let program = self.worker_program()?;
        let mut command = Command::new(&program);
        command
            .arg(PRECOMPILE_WORKER_ARGUMENT)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Protocol failures travel over bounded stdout frames. Do not
            // retain arbitrary child diagnostics in the parent process.
            .stderr(Stdio::null());
        let child = command.spawn().map_err(|error| {
            RunnerError::StartFailed(format!(
                "spawn precompile worker {}: {error}",
                program.display()
            ))
        })?;
        self.child_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                child_slot_key.clone(),
                PrecompileChildSlotEntry {
                    started_at: Instant::now(),
                    retired: false,
                },
            );
        let mut child = ManagedPrecompileChild::new(
            child,
            child_permit,
            Arc::clone(&self.retired_children),
            Arc::clone(&self.child_slots),
            child_slot_key,
        );

        let exchange = async {
            let stdin = child.child_mut().stdin.take().ok_or_else(|| {
                RunnerError::StartFailed("precompile worker did not expose stdin".to_string())
            })?;
            let stdout = child.child_mut().stdout.take().ok_or_else(|| {
                RunnerError::StartFailed("precompile worker did not expose stdout".to_string())
            })?;
            let mut stdin = stdin;
            let mut stdout = stdout;
            write_precompile_request_async(&mut stdin, &request)
                .await
                .map_err(|error| {
                    RunnerError::StartFailed(format!("write precompile request: {error:#}"))
                })?;
            stdin.shutdown().await.map_err(|error| {
                RunnerError::StartFailed(format!("close precompile request pipe: {error}"))
            })?;
            let response = read_precompile_response_async(&mut stdout)
                .await
                .map_err(|error| {
                    RunnerError::StartFailed(format!("read precompile response: {error:#}"))
                })?;
            let status = child.child_mut().wait().await.map_err(|error| {
                RunnerError::StartFailed(format!("wait for precompile worker: {error}"))
            })?;
            Ok::<_, RunnerError>((response, status))
        };

        match timeout_at(deadline, exchange).await {
            Ok(Ok((response, status))) => {
                child.disarm();
                if !status.success() {
                    // A valid child failure is framed before it exits nonzero;
                    // preserve that bounded diagnostic rather than reducing it
                    // to a platform-specific exit status.
                    return Err(match validate_precompile_response(&request, &response) {
                        Err(error) => map_precompile_error(&options.wasm_path, error),
                        Ok(_) => RunnerError::StartFailed(format!(
                            "precompile worker exited with unexpected status {status}"
                        )),
                    });
                }
                Ok((request, response))
            }
            Ok(Err(error)) => {
                child.kill_and_reap("precompile worker protocol failure");
                Err(error)
            }
            Err(_) => {
                child.kill_and_reap("precompile worker preparation deadline elapsed");
                Err(RunnerError::PreparationTimedOut(
                    "component precompile worker exceeded the preparation deadline".to_string(),
                ))
            }
        }
    }
}

#[async_trait]
impl ComponentPrecompiler for ChildComponentPrecompiler {
    async fn prepare(
        &self,
        executor: &WorkflowExecutor,
        options: &LaunchOptions,
    ) -> Result<PreparedWorkflow> {
        // A prepared artifact for this exact image may already be linked in
        // this process. Reusing it skips a process spawn and a full component
        // compile that are otherwise paid on every launch of the same
        // workflow. The cache still hands back the digest recorded when the
        // artifact was verified, so the checksum fence below runs unchanged.
        if let Some((cached, source_digest)) = executor.cached_prepared(&options.wasm_path).await {
            validate_expected_workflow_checksum(options, source_digest)?;
            return Ok(cached);
        }

        let (request, response) = self.exchange(options).await?;
        let source_digest = validate_precompile_response(&request, &response)
            .map_err(|error| map_precompile_error(&options.wasm_path, error))?
            .source_digest();
        validate_expected_workflow_checksum(options, source_digest)?;
        // SAFETY: `response` was read from the private stdout pipe of the
        // exact worker command spawned above; that command's internal mode is
        // the component-host protocol writer. The protocol validation checks
        // its nonce, digest, and engine fingerprint before Wasmtime sees it.
        let component = unsafe {
            deserialize_trusted_precompiled_component(executor.engine(), &request, &response)
        }
        .map_err(|error| map_precompile_error(&options.wasm_path, error))?;
        let workflow = executor
            .prepare_precompiled(component)
            .await
            .map_err(|error| {
                RunnerError::StartFailed(format!("link precompiled workflow: {error:#}"))
            })?;
        executor
            .cache_prepared(&options.wasm_path, source_digest, &workflow)
            .await;
        Ok(workflow)
    }

    fn child_occupancy(&self) -> Option<PrecompileChildOccupancy> {
        let held = self
            .child_limit
            .saturating_sub(self.child_permits.available_permits());
        let slots = self
            .child_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        let oldest = slots.values().min_by_key(|entry| entry.started_at);
        Some(PrecompileChildOccupancy {
            limit: u64::try_from(self.child_limit).unwrap_or(u64::MAX),
            held: u64::try_from(held).unwrap_or(u64::MAX),
            retired: u64::try_from(slots.values().filter(|entry| entry.retired).count())
                .unwrap_or(u64::MAX),
            oldest_held_ms: oldest
                .map(|entry| now.saturating_duration_since(entry.started_at).as_millis() as u64),
        })
    }
}

/// Test-only protocol-compatible precompiler.
///
/// Environment integration tests run as a libtest binary rather than the
/// server binary that owns `--internal-precompile-component`. They opt into
/// this implementation explicitly; production construction always uses
/// [`ChildComponentPrecompiler`]. Keeping this behind the database-test
/// feature avoids a hidden in-process fallback in deployed hosts.
#[cfg(feature = "db-integration-tests")]
struct InProcessTestComponentPrecompiler;

#[cfg(feature = "db-integration-tests")]
#[async_trait]
impl ComponentPrecompiler for InProcessTestComponentPrecompiler {
    async fn prepare(
        &self,
        executor: &WorkflowExecutor,
        options: &LaunchOptions,
    ) -> Result<PreparedWorkflow> {
        let request = PrecompileRequest::for_artifact(precompile_nonce()?, &options.wasm_path)
            .map_err(|error| {
                RunnerError::StartFailed(format!("build test precompile request: {error:#}"))
            })?;
        let worker_request = request.clone();
        let response = tokio::task::spawn_blocking(move || {
            runtara_component_host::precompile::precompile_artifact(&worker_request)
                .map(PrecompileResponse::Success)
        })
        .await
        .map_err(|error| RunnerError::Other(format!("test precompile worker panicked: {error}")))?
        .map_err(|error| map_precompile_error(&options.wasm_path, error))?;
        let source_digest = validate_precompile_response(&request, &response)
            .map_err(|error| map_precompile_error(&options.wasm_path, error))?
            .source_digest();
        validate_expected_workflow_checksum(options, source_digest)?;
        // SAFETY: this test helper invokes the same component-host precompile
        // function in-process and immediately wraps its exact output in the
        // protocol response used by production.
        let component = unsafe {
            deserialize_trusted_precompiled_component(executor.engine(), &request, &response)
        }
        .map_err(|error| map_precompile_error(&options.wasm_path, error))?;
        let workflow = executor
            .prepare_precompiled(component)
            .await
            .map_err(|error| {
                RunnerError::StartFailed(format!("link test precompiled workflow: {error:#}"))
            })?;
        Ok(workflow)
    }
}

/// Owns a child process until it either exits normally or a detached reaper
/// observes the result after timeout/cancellation. Its `Drop` is deliberately
/// kill-safe: cancelling a dispatcher task cannot leave a compiler child
/// behind just because the future was dropped mid-pipe exchange.
struct ManagedPrecompileChild {
    child: Option<Child>,
    permit: Option<OwnedSemaphorePermit>,
    retired_children: Arc<AtomicU64>,
    slots: PrecompileChildRegistry,
    slot_key: String,
}

impl ManagedPrecompileChild {
    fn new(
        child: Child,
        permit: OwnedSemaphorePermit,
        retired_children: Arc<AtomicU64>,
        slots: PrecompileChildRegistry,
        slot_key: String,
    ) -> Self {
        Self {
            child: Some(child),
            permit: Some(permit),
            retired_children,
            slots,
            slot_key,
        }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("managed precompile child is present")
    }

    fn disarm(&mut self) {
        self.child.take();
        self.permit.take();
        self.remove_slot();
    }

    fn remove_slot(&self) {
        self.slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.slot_key);
    }

    fn kill_and_reap(&mut self, reason: &'static str) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let Some(permit) = self.permit.take() else {
            return;
        };
        if let Err(error) = child.start_kill() {
            warn!(error = %error, "Could not signal timed-out precompile child; reaper will still wait");
        }
        let retired_children = Arc::clone(&self.retired_children);
        if let Some(slot) = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_mut(&self.slot_key)
        {
            slot.retired = true;
        }
        let slots = Arc::clone(&self.slots);
        let slot_key = self.slot_key.clone();
        let retired = retired_children.fetch_add(1, Ordering::SeqCst) + 1;
        warn!(
            retired_precompile_children = retired,
            reason, "Detached a timed-out precompile child for reaping"
        );
        let reaper = async move {
            if let Err(error) = child.wait().await {
                warn!(error = %error, "Could not reap terminated precompile child");
            }
            let remaining = retired_children
                .fetch_sub(1, Ordering::SeqCst)
                .saturating_sub(1);
            debug!(
                retired_precompile_children = remaining,
                "Reaped detached precompile child"
            );
            slots
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&slot_key);
            drop(permit);
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(reaper);
        } else {
            // A runtime shutdown can call Drop outside Tokio. The kill signal
            // has already been delivered; dropping the permit avoids a leak in
            // a process that is itself exiting.
            drop(reaper);
            self.retired_children.fetch_sub(1, Ordering::SeqCst);
            self.remove_slot();
        }
    }
}

impl Drop for ManagedPrecompileChild {
    fn drop(&mut self) {
        // This path is reached when a caller cancels a preparation future.
        // `kill_and_reap` is synchronous until its detached wait task, so it
        // is safe to call from Drop and preserves the bounded child slot.
        self.kill_and_reap("precompile future cancelled");
    }
}

/// Turn a semaphore reading plus the slot map into an occupancy report.
///
/// Split out from [`Runner::occupancy`] so the decisions it makes are testable
/// without standing up a runner (which needs a live persistence layer): that
/// `held` is read from the semaphore rather than counted from the map, and that
/// the reported age belongs to the *oldest* holder rather than any other.
fn compute_occupancy(
    limit: usize,
    available: usize,
    slots: &HashMap<String, RunSlotEntry>,
    now: Instant,
) -> RunnerOccupancy {
    // Deliberately not `slots.len()`. A permit is taken before its acquisition
    // time is recorded, so during that window the map under-reports; the
    // semaphore never does. The map's only job is answering "how old is the
    // oldest", where a momentarily missing entry costs nothing.
    let held = limit.saturating_sub(available);
    let oldest = slots.values().min_by_key(|entry| entry.taken_at);
    RunnerOccupancy {
        limit: limit as u64,
        held: held as u64,
        oldest_held_ms: oldest
            .map(|entry| now.saturating_duration_since(entry.taken_at).as_millis() as u64),
        oldest_instance_id: oldest.map(|entry| entry.instance_id.clone()),
        // Filled in by the caller, which owns the lifetime counters; this
        // function is about a single instant's occupancy.
        runs_started: 0,
        runs_finished: 0,
    }
}

fn compute_preparation_occupancy(
    limit: usize,
    available: usize,
    slots: &HashMap<String, PreparationSlotEntry>,
    now: Instant,
) -> PreparationOccupancy {
    let held = limit.saturating_sub(available);
    let oldest = slots.values().min_by_key(|entry| entry.taken_at);
    PreparationOccupancy {
        limit: limit as u64,
        held: held as u64,
        oldest_held_ms: oldest
            .map(|entry| now.saturating_duration_since(entry.taken_at).as_millis() as u64),
        oldest_instance_id: oldest.map(|entry| entry.instance_id.clone()),
        precompile_child_limit: None,
        precompile_child_held: None,
        precompile_child_oldest_ms: None,
        precompile_child_retired: None,
    }
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        // Recover from poisoning rather than propagate it. This runs while the
        // task may already be unwinding, so panicking here would abort the
        // process; and a poisoned occupancy map is not a reason to refuse to
        // give a permit back.
        let mut slots = self
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slots.remove(&self.launch_id);
        self.finished.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for PreparationSlot {
    fn drop(&mut self) {
        let mut slots = self
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slots.remove(&self.slot_key);
    }
}

impl EmbeddedWasmRunner {
    /// Build the runner with its own engine + epoch ticker.
    pub fn new(config: WorkflowRunnerConfig, persistence: Arc<dyn Persistence>) -> Result<Self> {
        let engine = build_engine(&EngineConfig::default())
            .map_err(|e| RunnerError::Other(format!("build wasmtime engine: {e:#}")))?;
        spawn_epoch_ticker(Arc::clone(&engine));
        let executor = WorkflowExecutor::new(engine)
            .map_err(|e| RunnerError::Other(format!("build workflow executor: {e:#}")))?;
        let handler_state = Arc::new(runtara_core::instance_handlers::InstanceHandlerState::new(
            Arc::clone(&persistence),
        ));
        let run_limit = max_concurrent_runs();
        warn_if_run_bound_exceeds_memory(run_limit);
        let preparation_limit = max_concurrent_preparations();
        let precompiler = Arc::new(ChildComponentPrecompiler::new(preparation_limit));
        Ok(Self {
            config,
            core_http_url: None,
            limits: limits_from_env(),
            preparation_permits: Arc::new(tokio::sync::Semaphore::new(preparation_limit)),
            preparation_limit,
            preparation_slots: Arc::new(Mutex::new(HashMap::new())),
            precompiler,
            run_permits: Arc::new(tokio::sync::Semaphore::new(run_limit)),
            run_limit,
            run_slots: Arc::new(Mutex::new(HashMap::new())),
            runs_started: Arc::new(AtomicU64::new(0)),
            runs_finished: Arc::new(AtomicU64::new(0)),
            persistence,
            executor: Arc::new(executor),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            handler_state,
        })
    }

    /// Attach an observer that counts guest events as they cross the host.
    ///
    /// Rebuilds the handler state rather than mutating it, because the state is
    /// shared behind an `Arc` by the time anything could ask for this — and a
    /// runner is configured once, at startup, before any run exists.
    pub fn with_event_observer(
        mut self,
        observer: Arc<dyn runtara_core::instance_handlers::InstanceEventObserver>,
    ) -> Self {
        self.handler_state = Arc::new(
            runtara_core::instance_handlers::InstanceHandlerState::new(Arc::clone(
                &self.persistence,
            ))
            .with_event_observer(observer),
        );
        self
    }

    /// Use the protocol-compatible in-process precompiler in database-backed
    /// integration tests.
    ///
    /// A libtest executable does not expose the server's hidden child-worker
    /// subcommand, so tests opt into this explicitly. This method is not
    /// compiled for production builds; deployed runners always use a killable
    /// process boundary.
    #[cfg(feature = "db-integration-tests")]
    pub fn with_in_process_precompiler_for_tests(mut self) -> Self {
        self.precompiler = Arc::new(InProcessTestComponentPrecompiler);
        self
    }

    /// Give legacy HTTP-composed artifacts the core API address.
    ///
    /// This is deliberately a runner setting rather than an inherited process
    /// environment variable: guest execution receives only the explicitly
    /// constructed environment in [`Self::merged_env`].
    pub fn with_core_http_url(mut self, core_http_url: String) -> Self {
        self.core_http_url = Some(core_http_url);
        self
    }

    fn merged_env(&self, options: &LaunchOptions) -> HashMap<String, String> {
        let mut env = common::build_env(
            &self.config,
            &options.instance_id,
            &options.tenant_id,
            options.checkpoint_id.as_deref(),
            self.core_http_url.as_deref(),
        );
        env.extend(options.env.clone());
        env
    }

    fn run_spec(
        &self,
        options: &LaunchOptions,
        env: HashMap<String, String>,
        stderr: Option<std::fs::File>,
        timeout: Duration,
        cancel: Option<CancelToken>,
        prepared_input: Option<Vec<u8>>,
    ) -> WorkflowRunSpec {
        // Always attach the native runtime host. A HostImport-composed
        // artifact consumes it; a legacy composed artifact satisfies the
        // runtime interface internally (HTTP loopback) and never calls it —
        // that indifference is the dual-ABI story: old workflows run
        // unchanged, without a rebuild, through the same spec.
        let debug_mode = env.get("DEBUG_MODE").is_some_and(|value| value == "true");
        let mut host = crate::runtime_host::PersistenceRuntimeHost::new(
            Arc::clone(&self.handler_state),
            options.instance_id.clone(),
            debug_mode,
        )
        // A guest that asks for its input through the host interface gets the
        // same bytes the launch already has, rather than a second read of what
        // was just written. Unset on wake/resume, so those still read the store.
        .with_prepersisted_input(prepared_input);
        // Share the run's cancel flag: it is how the host stops a guest that
        // woke from an interrupted sleep and ignored the cancel, without
        // routing the cancel through the guest's catchable error channel.
        if let Some(token) = cancel.clone() {
            host = host.with_cancel_token(token);
            // Bringing the epoch deadline forward is what makes that flag act
            // promptly: otherwise it is only read on the next 100ms tick, and a
            // guest with no poll site keeps completing steps until then.
            // Engine-global, like the ticker's own increment — other runs'
            // callbacks fire once early, see no cancel, and yield.
            let engine = Arc::clone(self.executor.engine());
            host = host.with_guest_interrupt(Arc::new(move || engine.increment_epoch()));
        }
        let runtime = Arc::new(host);
        WorkflowRunSpec {
            env,
            stderr,
            timeout,
            cancel,
            limits: self.limits.clone(),
            runtime: Some(runtime),
        }
    }

    /// Load the exact durable envelope required by a queued preparation.
    ///
    /// Unlike the legacy synchronous runner path, a durable launch must not
    /// silently invent `{}` when the instance or its committed input envelope
    /// is absent. The dispatcher can then terminalize a malformed launch, and
    /// a cancellable database timeout can safely return a valid one to queue.
    async fn required_prepared_input(&self, options: &LaunchOptions) -> Result<Vec<u8>> {
        if let Some(input) = options.prepersisted_input.clone() {
            return Ok(input);
        }
        let instance = self
            .persistence
            .get_instance(&options.instance_id)
            .await
            .map_err(|error| RunnerError::StartFailed(format!("load instance input: {error}")))?
            .ok_or_else(|| {
                RunnerError::StartFailed(format!("instance {} not found", options.instance_id))
            })?;
        instance.input.ok_or_else(|| {
            RunnerError::StartFailed(format!(
                "instance {} has no persisted input envelope",
                options.instance_id
            ))
        })
    }

    /// Take one independently bounded preparation slot without waiting.
    fn try_take_preparation_slot(&self, options: &LaunchOptions) -> Result<PreparationSlot> {
        let permit = Arc::clone(&self.preparation_permits)
            .try_acquire_owned()
            .map_err(|error| match error {
                TryAcquireError::NoPermits => RunnerError::PreparationCapacityUnavailable,
                TryAcquireError::Closed => {
                    RunnerError::Other("preparation semaphore closed".to_string())
                }
            })?;
        let slot_key = format!(
            "{}:{}",
            options.launch_id,
            options.preparation_attempt.unwrap_or_default()
        );
        self.preparation_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                slot_key.clone(),
                PreparationSlotEntry {
                    instance_id: options.instance_id.clone(),
                    taken_at: Instant::now(),
                },
            );
        Ok(PreparationSlot {
            _permit: permit,
            slot_key,
            registry: Arc::clone(&self.preparation_slots),
        })
    }

    /// Prepare every operation that can block before a live guest permit is
    /// acquired.
    ///
    /// Artifact filesystem work and Wasmtime compilation are performed by a
    /// killable child process. The parent only links its verified response;
    /// persisted-input reads remain async and are bounded by the same durable
    /// preparation deadline. Per-run directory/stderr creation is deliberately
    /// omitted for durable prepared launches: reopening either file in the
    /// parent would reintroduce an unkillable filesystem operation before a
    /// guest permit.
    async fn prepare_embedded_launch(&self, options: &LaunchOptions) -> Result<PreparedLaunch> {
        let preparation_slot = self.try_take_preparation_slot(options)?;
        let workflow = self.precompiler.prepare(&self.executor, options).await?;
        if options.requires_lifecycle_invoke
            && !workflow.is_lifecycle_invoke(self.executor.engine())
        {
            return Err(RunnerError::StartFailed(
                "generated workflow image does not export the current lifecycle invoke entrypoint"
                    .to_string(),
            ));
        }

        // This is a cancellable database operation, so it observes the same
        // absolute preparation lease deadline as the dispatcher. Missing or
        // malformed durable input is a real launch error, never a synthetic
        // `{}` fallback that could hide a stranded instance.
        let input = if let Some(deadline) = options.preparation_deadline {
            tokio::time::timeout_at(deadline, self.required_prepared_input(options))
                .await
                .map_err(|_| {
                    RunnerError::PreparationTimedOut("loading persisted input".to_string())
                })??
        } else {
            self.required_prepared_input(options).await?
        };

        Ok(PreparedLaunch::new(
            &options.launch_id,
            EmbeddedPreparedLaunch {
                _preparation_slot: preparation_slot,
                workflow,
                input,
            },
        ))
    }

    fn task_of(&self, launch_id: &str) -> Option<Arc<InstanceTask>> {
        self.tasks
            .lock()
            .expect("embedded runner task registry poisoned")
            .get(launch_id)
            .cloned()
    }
}

/// How many guests may execute concurrently in this process.
///
/// Each live guest holds a wasmtime store, so this is a memory bound before it
/// is a throughput one: on a small host, letting a fast producer stack runs
/// exhausts the machine and the process is killed, which is a far worse outcome
/// than queueing. `RUNTARA_MAX_CONCURRENT_RUNS`, defaulting to four per core.
/// How many guests this process will execute concurrently.
///
/// Public because it is the pipeline's real execution capacity, and the
/// server's admission ceiling is derived from it: admitting substantially
/// more than this cannot raise throughput, because the surplus can only
/// queue while holding an admission reservation.
pub fn max_concurrent_runs() -> usize {
    std::env::var("RUNTARA_MAX_CONCURRENT_RUNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().saturating_mul(4))
                .unwrap_or(4)
        })
        .clamp(1, 1024)
}

/// How many artifact preparations may execute concurrently in this process.
///
/// Component compilation is CPU-heavy and may include bounded filesystem and
/// database work. It is intentionally not coupled to `RUNTARA_MAX_CONCURRENT_RUNS`:
/// one slow artifact must never borrow all live guest permits. One worker per
/// available core is the conservative default; an explicit setting is capped
/// to keep a typo from turning a preparation burst into a compiler stampede.
fn max_concurrent_preparations() -> usize {
    std::env::var("RUNTARA_PREPARATION_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| *count > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1)
        })
        .clamp(1, 64)
}

/// Maximum live or still-reaping precompile children.
///
/// This includes a deliberately small bounded replacement budget. When a
/// child is stuck in a kernel operation that cannot acknowledge a kill
/// immediately, its permit remains occupied in the detached reaper; the hard
/// cap turns repeated failures into durable backpressure rather than an
/// unbounded process or memory storm.
///
/// Wasmtime compilation can use substantially more memory than the 64 MiB
/// source / 128 MiB serialized protocol caps, so reserve a conservative 768
/// MiB per child and never spend more than half of detected host memory on
/// these helper processes. Hosts whose memory cannot be determined use a
/// single child by default.
fn max_concurrent_precompile_children(preparation_limit: usize) -> usize {
    const CHILD_WORKING_SET_BUDGET_BYTES: u64 = 768 * 1024 * 1024;
    let host_cap = host_memory_bytes()
        .map(|total| {
            usize::try_from((total / 2) / CHILD_WORKING_SET_BUDGET_BYTES)
                .unwrap_or(usize::MAX)
                .max(1)
        })
        .unwrap_or(1);
    let requested = std::env::var("RUNTARA_PRECOMPILE_CHILD_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| *count > 0)
        .unwrap_or_else(|| preparation_limit.saturating_add(1));
    requested.min(host_cap).clamp(1, 4)
}

/// Default ceiling for callers outside the durable queue.
///
/// Durable launches always supply the deadline derived from their
/// `preparing` lease. Keeping a finite fallback preserves the same killable
/// boundary for a direct caller that asks the runner to prepare explicitly.
const DEFAULT_PRECOMPILER_TIMEOUT: Duration = Duration::from_secs(60);

fn precompile_nonce() -> Result<[u8; PRECOMPILE_NONCE_BYTES]> {
    let mut nonce = [0_u8; PRECOMPILE_NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|error| {
        RunnerError::StartFailed(format!("generate precompile nonce entropy: {error}"))
    })?;
    Ok(nonce)
}

fn hex_digest(digest: [u8; PRECOMPILE_NONCE_BYTES]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn map_precompile_error(wasm_path: &Path, error: anyhow::Error) -> RunnerError {
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    }) {
        RunnerError::BinaryNotFound(wasm_path.display().to_string())
    } else {
        RunnerError::StartFailed(format!("precompile workflow component: {error:#}"))
    }
}

/// Match the child-read digest to the immutable direct-workflow metadata.
///
/// Generic components intentionally have no required checksum and keep their
/// established ABI. Generated direct workflows cannot take that fallback:
/// their image metadata is the source-of-truth identity and a changed file is
/// terminalized before it reaches a guest permit.
fn validate_expected_workflow_checksum(
    options: &LaunchOptions,
    actual: [u8; PRECOMPILE_NONCE_BYTES],
) -> Result<()> {
    let Some(expected) = options.expected_workflow_checksum.as_deref() else {
        return Ok(());
    };
    let expected = expected.strip_prefix("sha256:").unwrap_or(expected);
    let actual = hex_digest(actual);
    if expected.len() != actual.len() || !expected.eq_ignore_ascii_case(&actual) {
        return Err(RunnerError::StartFailed(format!(
            "generated workflow artifact digest does not match immutable image metadata (expected {expected}, got {actual})"
        )));
    }
    Ok(())
}

/// Roughly what one live guest costs in resident memory.
///
/// Measured on a 4 GB host holding a million parked instances: waking them with
/// the bound raised to 96 drove the process from ~800 MB to the 2.86 GB the
/// kernel killed it at, while the same work at the default 16 peaked at 1.3 GB
/// and fell back. It is an order-of-magnitude figure for a sanity check, not an
/// accounting of any particular workflow - a guest that allocates will cost
/// more, one that parks immediately less.
const OBSERVED_BYTES_PER_RUN: u64 = 40 * 1024 * 1024;

/// Total memory visible to this host, when it can be read.
///
/// Linux-only and best-effort: the check that uses it is advisory, so anywhere
/// this returns `None` simply skips the warning.
fn host_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    meminfo.lines().find_map(|line| {
        let kb = line.strip_prefix("MemTotal:")?.trim().strip_suffix(" kB")?;
        kb.trim().parse::<u64>().ok().map(|kb| kb * 1024)
    })
}

/// Say so when the configured run bound cannot fit in the host's memory.
///
/// The default is sized to the host, but an operator raising it for throughput
/// gets no feedback until the kernel kills the process - and the symptom then
/// looks like a leak rather than a setting, because memory climbs steadily and
/// the crash lands minutes later somewhere unrelated to the change.
fn warn_if_run_bound_exceeds_memory(permits: usize) {
    let Some(total) = host_memory_bytes() else {
        return;
    };
    let needed = (permits as u64).saturating_mul(OBSERVED_BYTES_PER_RUN);
    if needed > total {
        tracing::warn!(
            max_concurrent_runs = permits,
            approx_required_mb = needed / (1024 * 1024),
            host_memory_mb = total / (1024 * 1024),
            "RUNTARA_MAX_CONCURRENT_RUNS is set above what this host can hold; \
             a burst of concurrent runs may exhaust memory and the process will \
             be killed. Each live guest holds a wasmtime store."
        );
    }
}

fn limits_from_env() -> WorkflowLimits {
    let mut limits = WorkflowLimits::default();
    if let Some(max) = std::env::var("RUNTARA_INSTANCE_MEMORY_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        limits.max_memory_bytes = max;
    }
    limits
}

/// `termination_reason` stamped on an on-signal park — the discriminator the
/// custom-signal waker requires before relaunching a suspended row. A suspend
/// from a pause/breakpoint ack carries no marker and is never signal-woken.
pub(crate) const WAITING_SIGNAL_TERMINATION: &str = "waiting_signal";

/// The earliest timed wake deadline (ms since epoch) across a suspend's wake
/// set, or `None` when every wake is deadline-less (`on-resume`, or a signal
/// wait with no timeout). `suspended` is re-invoke-on-ANY, so the earliest
/// deadline is when the scheduler must relaunch.
fn earliest_wake_deadline_ms(
    wakes: &[runtara_component_host::lifecycle::WorkflowWake],
) -> Option<u64> {
    use runtara_component_host::lifecycle::WorkflowWake;
    wakes
        .iter()
        .filter_map(|wake| match wake {
            WorkflowWake::At(ms) => Some(*ms),
            WorkflowWake::OnSignal(wait) => wait.deadline_ms,
            WorkflowWake::OnResume => None,
        })
        .min()
}

/// True when any wake is an `on-signal` — the instance is parked waiting for an
/// externally-delivered custom signal (the store-freeing Wait path).
fn has_on_signal_wake(wakes: &[runtara_component_host::lifecycle::WorkflowWake]) -> bool {
    use runtara_component_host::lifecycle::WorkflowWake;
    wakes.iter().any(|w| matches!(w, WorkflowWake::OnSignal(_)))
}

/// The checkpoint (signal) ids this park is waiting on, in wake order.
fn on_signal_checkpoint_ids(
    wakes: &[runtara_component_host::lifecycle::WorkflowWake],
) -> Vec<&str> {
    use runtara_component_host::lifecycle::WorkflowWake;
    wakes
        .iter()
        .filter_map(|wake| match wake {
            WorkflowWake::OnSignal(wait) => Some(wait.checkpoint_id.as_str()),
            _ => None,
        })
        .collect()
}

/// Close the window between the guest's last signal poll and the `suspended`
/// write above.
///
/// A signal delivered in that window is otherwise lost: `handle_send_custom_signal`
/// inserts the row and calls `wake_suspended_on_signal`, which no-ops unless the
/// instance is ALREADY `suspended` + `waiting_signal` — and at that moment it is
/// still `running`. The park then completes, and for a wait with no timeout
/// `sleep_until` stays NULL with no other wake path, so the instance is stranded
/// forever with its signal sitting in the table.
///
/// Re-reading here, after the row is visible to the waker, closes it: if the
/// signal is already present we self-wake by stamping `sleep_until = now`, which
/// is exactly what the waker would have done. The read is non-destructive
/// (`take_pending_custom_signal` retains the row, despite its name), so the
/// replayed guest still observes the signal.
///
/// Returns true when it woke the instance.
async fn wake_if_signal_already_arrived(
    persistence: &dyn Persistence,
    instance_id: &str,
    wakes: &[runtara_component_host::lifecycle::WorkflowWake],
) -> bool {
    for checkpoint_id in on_signal_checkpoint_ids(wakes) {
        match persistence
            .take_pending_custom_signal(instance_id, checkpoint_id)
            .await
        {
            Ok(Some(_)) => {
                if let Err(e) = persistence
                    .set_instance_sleep(instance_id, chrono::Utc::now())
                    .await
                {
                    warn!(instance_id, error = %e, "Failed to self-wake after a signal raced the park");
                    return false;
                }
                info!(
                    instance_id,
                    checkpoint_id, "Signal arrived while parking; woke the instance immediately"
                );
                return true;
            }
            Ok(None) => {}
            Err(e) => {
                warn!(instance_id, error = %e, "Could not re-check for a signal that raced the park")
            }
        }
    }
    false
}

/// Park an invoke-shaped instance that returned `outcome::suspended` (the
/// store-freeing durable-sleep / wait-for-signal paths). Stamps
/// `status='suspended'`, plus `sleep_until=deadline` when there is a TIMED wake
/// (`at`, or `on-signal` with a timeout). The guest already persisted its resume
/// checkpoint before exiting, so there is no output/checkpoint work here.
///
/// - `at(deadline)` / `on-signal{deadline}` → suspended + `sleep_until=deadline`
///   (the wake scheduler relaunches at the deadline).
/// - `on-signal` with NO deadline → suspended, `sleep_until` left NULL; the
///   custom-signal waker stamps it when the signal arrives (the only wake path).
/// - only `on-resume` (breakpoint/drain pause) → left untouched: those recorded
///   `status=suspended` inline via their ack, and stamping `sleep_until` would
///   wrongly schedule an immediate wake.
///
/// The park is `if_running`-guarded (a guest that already reported a terminal
/// complete/fail must not be resurrected as suspended — the same race guard
/// `handle_instance_event`'s suspend path uses), and stamps a
/// `termination_reason` marker naming the wake shape: `waiting_signal` for
/// on-signal parks (the ONLY rows the custom-signal waker may relaunch — a
/// pause/breakpoint suspend has no marker and must never be signal-woken) or
/// `sleeping` for pure timed parks. Relaunch clears the marker with the
/// running transition.
/// Terminal backstop for a cancel the guest never acknowledged.
///
/// Status `cancelled` is otherwise written only when the guest observes the
/// signal and acks it. A workflow artifact compiled before the Delay poll site
/// existed has no way to observe one, so without this a cancelled run reports
/// whatever it reached on its own — usually `completed`, a silent success for a
/// run the user stopped.
///
/// Runs after the guest is gone, so nothing inside the workflow can intercept
/// it, and it makes no assumptions about call ordering — which is what makes it
/// the floor under the host-side escalation in `PersistenceRuntimeHost`. That
/// escalation is the fast path; this one is the guarantee.
///
/// A cancel landing in the instant a run legitimately finishes is recorded
/// `cancelled` (the ack overwrites the terminal status). Deliberate: a cancel
/// was requested and demonstrably not honoured, and reporting clean success for
/// it is the failure mode this exists to prevent.
async fn enforce_unacked_cancel(persistence: &Arc<dyn Persistence>, instance_id: &str) {
    // `acknowledged_at` is re-checked even though `get_pending_signal` already
    // filters on `acknowledged_at IS NULL`: defence in depth. This backstop
    // overwrites a terminal status, so a regression in that predicate must not
    // re-cancel a run whose guest handled its signal properly. The check is free.
    match persistence.get_pending_signal(instance_id).await {
        Ok(Some(signal)) if signal.signal_type == "cancel" && signal.acknowledged_at.is_none() => {
            warn!(
                instance_id = %instance_id,
                "Run ended with an unacknowledged cancel; recording cancelled"
            );
            let state = InstanceHandlerState::new(Arc::clone(persistence));
            if let Err(e) = handle_signal_ack(
                &state,
                SignalAck {
                    instance_id: instance_id.to_string(),
                    signal_type: SignalType::SignalCancel as i32,
                    acknowledged: true,
                },
            )
            .await
            {
                error!(instance_id = %instance_id, error = %e, "Failed to record cancelled");
            }
        }
        Ok(_) => {}
        Err(e) => {
            warn!(
                instance_id = %instance_id,
                error = %e,
                "Could not check for an unacknowledged cancel after the run"
            );
        }
    }
}

async fn park_invoke_suspend(
    persistence: &dyn Persistence,
    instance_id: &str,
    wakes: &[runtara_component_host::lifecycle::WorkflowWake],
) {
    let deadline_ms = earliest_wake_deadline_ms(wakes);
    if deadline_ms.is_none() && !has_on_signal_wake(wakes) {
        // Pure on-resume: already handled by the ack path.
        return;
    }
    let wake_marker = if has_on_signal_wake(wakes) {
        WAITING_SIGNAL_TERMINATION
    } else {
        "sleeping"
    };
    // status first, then sleep_until: the wake scan requires BOTH
    // `status='suspended'` AND `sleep_until IS NOT NULL`, so neither ordering
    // exposes a half-parked instance to a premature claim.
    match persistence
        .complete_instance(
            runtara_core::persistence::CompleteInstanceParams::new(instance_id, "suspended")
                .if_running()
                .with_termination(wake_marker, None),
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            // Already terminal (or otherwise not running) — a malformed guest
            // that completed/failed and THEN returned suspended, or the
            // monitor's timeout landed first. Never overwrite, never schedule.
            warn!(
                instance_id,
                "Invoke suspend ignored: instance is not running (terminal status preserved)"
            );
            return;
        }
        Err(e) => {
            warn!(instance_id, error = %e, "Failed to mark instance suspended after invoke suspend");
        }
    }
    if wake_if_signal_already_arrived(persistence, instance_id, wakes).await {
        return;
    }
    let Some(deadline_ms) = deadline_ms else {
        // Deadline-less on-signal: parked as suspended; the waker relaunches it.
        return;
    };
    let Some(deadline) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(deadline_ms as i64)
    else {
        warn!(
            instance_id,
            deadline_ms, "Suspend deadline out of range; leaving sleep_until unset"
        );
        return;
    };
    if let Err(e) = persistence.set_instance_sleep(instance_id, deadline).await {
        warn!(instance_id, error = %e, "Failed to set sleep_until after invoke suspend");
    }
}

fn invoke_metrics_of(result: &runtara_component_host::InvokeRunResult) -> ContainerMetrics {
    ContainerMetrics {
        memory_peak_bytes: Some(result.memory_peak_bytes),
        memory_current_bytes: Some(result.memory_peak_bytes),
        ..Default::default()
    }
}

fn metrics_of(result: &runtara_component_host::WorkflowRunResult) -> ContainerMetrics {
    ContainerMetrics {
        memory_peak_bytes: Some(result.memory_peak_bytes),
        memory_current_bytes: Some(result.memory_peak_bytes),
        ..Default::default()
    }
}

#[async_trait]
impl Runner for EmbeddedWasmRunner {
    fn runner_type(&self) -> &'static str {
        "wasm-embedded"
    }

    fn occupancy(&self) -> Option<RunnerOccupancy> {
        let slots = self
            .run_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut occupancy = compute_occupancy(
            self.run_limit,
            self.run_permits.available_permits(),
            &slots,
            Instant::now(),
        );
        occupancy.runs_started = self.runs_started.load(Ordering::Relaxed);
        occupancy.runs_finished = self.runs_finished.load(Ordering::Relaxed);
        Some(occupancy)
    }

    fn preparation_occupancy(&self) -> Option<PreparationOccupancy> {
        let slots = self
            .preparation_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut occupancy = compute_preparation_occupancy(
            self.preparation_limit,
            self.preparation_permits.available_permits(),
            &slots,
            Instant::now(),
        );
        if let Some(children) = self.precompiler.child_occupancy() {
            occupancy.precompile_child_limit = Some(children.limit);
            occupancy.precompile_child_held = Some(children.held);
            occupancy.precompile_child_oldest_ms = children.oldest_held_ms;
            occupancy.precompile_child_retired = Some(children.retired);
        }
        Some(occupancy)
    }

    async fn try_prepare_launch(&self, options: &LaunchOptions) -> Result<PreparedLaunch> {
        self.prepare_embedded_launch(options).await
    }

    async fn launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle> {
        let prepared = self.try_prepare_launch(options).await?;
        self.try_launch_prepared_detached(options, prepared).await
    }

    async fn try_launch_prepared_detached(
        &self,
        options: &LaunchOptions,
        prepared: PreparedLaunch,
    ) -> Result<RunnerHandle> {
        if prepared.launch_id() != options.launch_id {
            return Err(RunnerError::StartFailed(
                "prepared launch does not match the requested generation".to_string(),
            ));
        }
        let EmbeddedPreparedLaunch {
            _preparation_slot,
            workflow,
            input,
        } = prepared.take()?;

        // The child compiler already read, hashed, compiled, and returned the
        // exact serialized component held by `workflow`; no parent artifact
        // reread is permitted at this boundary. Releasing preparation before
        // the run permit preserves the strict separation between compiler
        // pressure and active guest capacity.
        drop(_preparation_slot);

        // Capacity is a durable-dispatch decision, never a reason to park a
        // request, trigger worker, or wake worker on a semaphore waiter. Take
        // it before allocating per-run state so an unavailable runner leaves
        // neither a task-registry entry nor a run directory behind.
        let permit =
            Arc::clone(&self.run_permits)
                .try_acquire_owned()
                .map_err(|error| match error {
                    TryAcquireError::NoPermits => RunnerError::CapacityUnavailable,
                    TryAcquireError::Closed => {
                        RunnerError::Other("run semaphore closed".to_string())
                    }
                })?;
        self.run_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                options.launch_id.clone(),
                RunSlotEntry {
                    instance_id: options.instance_id.clone(),
                    taken_at: Instant::now(),
                },
            );
        self.runs_started.fetch_add(1, Ordering::Relaxed);
        let run_slot = RunSlot {
            _permit: permit,
            launch_id: options.launch_id.clone(),
            registry: Arc::clone(&self.run_slots),
            finished: Arc::clone(&self.runs_finished),
        };

        let env = self.merged_env(options);
        let cancel: CancelToken = Arc::new(AtomicBool::new(false));
        let task = Arc::new(InstanceTask {
            cancel: Arc::clone(&cancel),
            finished: AtomicBool::new(false),
            done: tokio::sync::Notify::new(),
        });
        self.tasks
            .lock()
            .expect("embedded runner task registry poisoned")
            .insert(options.launch_id.clone(), Arc::clone(&task));
        // Construct this before `tokio::spawn`, not inside the task body. A
        // runtime shutdown can drop an unpolled future immediately after the
        // map insertion; moving the guard into that future still retires the
        // exact map entry in its Drop implementation.
        let completion = TaskCompletionGuard {
            task: Arc::clone(&task),
            registry: Arc::clone(&self.tasks),
            launch_id: options.launch_id.clone(),
        };

        let metrics = Arc::new(tokio::sync::Mutex::new(ContainerMetrics::default()));

        // The monitor is defense in depth, not the only deadline owner. The
        // embedded component host needs the same finite active deadline so
        // its epoch/watchdog rings cover guest work after the start gate
        // opens. `Duration::MAX` overflows its monotonic HTTP deadline and
        // lets an otherwise healthy gated run panic before it can park.
        let spec = self.run_spec(
            options,
            env,
            // Durable prepared launches intentionally do not create a
            // per-run stderr file in the parent. See `prepare_embedded_launch`:
            // a blocked filesystem operation must never retain a preparation
            // or guest slot.
            None,
            options.timeout,
            Some(cancel),
            Some(input.clone()),
        );

        let executor = Arc::clone(&self.executor);
        let persistence = Arc::clone(&self.persistence);
        let metrics_for_task = Arc::clone(&metrics);
        let task_for_run = Arc::clone(&task);
        let instance_id = options.instance_id.clone();
        let launch_id = options.launch_id.clone();
        let start_gate = options.start_gate.clone();
        let start_confirmation = start_gate.as_ref().map(|gate| {
            Arc::new(GateWorkflowStartConfirmation { gate: gate.clone() })
                as Arc<dyn WorkflowStartConfirmation>
        });
        // Durable dispatchers promote the matching Core instance in the same
        // transaction that changes their queue row to `running`, before they
        // open `start_gate`. Direct callers retain the historical runner-side
        // promotion below.
        let supervisor_owns_lifecycle = start_gate.is_some();
        // The run slot was already claimed without waiting before any per-run
        // allocation. It moves into the task so every completion/error/panic
        // returns capacity and retires its occupancy timestamp together.
        tokio::spawn(async move {
            let _run_slot = run_slot;
            let _completion = completion;
            if let Some(gate) = start_gate {
                // The durable dispatcher may open the in-memory gate once it
                // owns the running generation. Do not clear the durable
                // marker here: Store/WASI setup below can still pause or
                // crash. Component host confirms exactly before
                // `instantiate_async`, the first guest-controlled boundary.
                match gate.wait().await {
                    StartGateOutcome::Opened => {}
                    StartGateOutcome::Cancelled
                    | StartGateOutcome::TimedOut
                    | StartGateOutcome::ConfirmationFailed => {
                        info!(
                            instance_id = %instance_id,
                            launch_id = %launch_id,
                            "Detached launch gate did not permit guest execution"
                        );
                        return;
                    }
                }
            }
            // A stop can win in the small interval after the dispatcher has
            // durably promoted this generation but before it opens the gate.
            // `Runner::stop` cannot hold the supervisor's in-memory gate, so
            // re-check the task-local cancellation fence here. This keeps a
            // cancelled handoff from loading (let alone invoking) guest code
            // if the supervisor subsequently opens its gate.
            if task_for_run.cancel.load(Ordering::SeqCst) {
                info!(
                    instance_id = %instance_id,
                    launch_id = %launch_id,
                    "Detached launch was cancelled before guest execution"
                );
                return;
            }
            if workflow.is_lifecycle_invoke(executor.engine()) {
                // The verified component and persisted input were both prepared
                // before the run permit. Only guest invocation remains after
                // the durable gate confirmation.
                if !supervisor_owns_lifecycle {
                    mark_running(persistence.as_ref(), &instance_id).await;
                }
                let run = executor
                    .execute_invoke_with_start_confirmation(
                        workflow.instance_pre(),
                        spec,
                        input,
                        start_confirmation.clone(),
                    )
                    .await;
                {
                    let mut guard = metrics_for_task.lock().await;
                    *guard = invoke_metrics_of(&run);
                }
                use runtara_component_host::InvokeExit;
                match &run.exit {
                    InvokeExit::Completed(_) => {
                        info!(instance_id = %instance_id, "Embedded workflow run completed");
                    }
                    InvokeExit::Suspended(wakes) => {
                        info!(instance_id = %instance_id, ?wakes, "Embedded workflow run suspended");
                        // Store-freeing durable sleep: the guest exited with a
                        // timed wake instead of blocking; park it so the wake
                        // scheduler relaunches at the deadline.
                        park_invoke_suspend(persistence.as_ref(), &instance_id, wakes).await;
                    }
                    InvokeExit::Failed(_) => {
                        warn!(instance_id = %instance_id, "Embedded workflow run returned error");
                    }
                    InvokeExit::Trapped { reason } => {
                        error!(instance_id = %instance_id, reason = %reason, "Embedded workflow run failed");
                    }
                    InvokeExit::Timeout => {
                        warn!(instance_id = %instance_id, "Embedded workflow run timed out");
                    }
                    InvokeExit::Cancelled => {
                        warn!(instance_id = %instance_id, "Embedded workflow run cancelled");
                    }
                }
                // A park is not an ending — the wake scheduler owns it from
                // here (and resolves a pending cancel itself).
                if !matches!(&run.exit, InvokeExit::Suspended(_)) {
                    enforce_unacked_cancel(&persistence, &instance_id).await;
                }
            } else if let Some(pre) = workflow.command() {
                // Generic non-workflow components retain their established
                // wasi:cli/run ABI. Generated direct workflows were rejected
                // during preparation if they lack lifecycle.invoke.
                if !supervisor_owns_lifecycle {
                    mark_running(persistence.as_ref(), &instance_id).await;
                }
                let run = executor
                    .execute_with_start_confirmation(pre, spec, start_confirmation.clone())
                    .await;
                {
                    let mut guard = metrics_for_task.lock().await;
                    *guard = metrics_of(&run);
                }
                match &run.exit {
                    WorkflowExit::Completed => {
                        info!(instance_id = %instance_id, "Embedded workflow run completed");
                    }
                    WorkflowExit::GuestError => {
                        warn!(instance_id = %instance_id, "Embedded workflow run returned error");
                    }
                    WorkflowExit::Failed { reason } => {
                        error!(instance_id = %instance_id, reason = %reason, "Embedded workflow run failed");
                    }
                    WorkflowExit::Timeout => {
                        warn!(instance_id = %instance_id, "Embedded workflow run timed out");
                    }
                    WorkflowExit::Cancelled => {
                        warn!(instance_id = %instance_id, "Embedded workflow run cancelled");
                    }
                }
                enforce_unacked_cancel(&persistence, &instance_id).await;
            } else {
                error!(
                    instance_id = %instance_id,
                    "Prepared generic component has no wasi:cli/run entrypoint"
                );
            }
        });

        info!(
            instance_id = %options.instance_id,
            wasm = %options.wasm_path.display(),
            "Launched embedded workflow run (detached)"
        );

        Ok(RunnerHandle {
            launch_id: options.launch_id.clone(),
            handle_id: format!("wasm_{}", options.launch_id),
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: chrono::Utc::now(),
            metrics: Some(metrics),
        })
    }

    async fn try_launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle> {
        // `launch_detached` itself now uses `try_acquire_owned`, so retaining
        // this explicit trait entry point makes durable-dispatch intent clear
        // while guaranteeing it never puts the dispatcher on a permit waiter.
        self.launch_detached(options).await
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        match self.task_of(&handle.launch_id) {
            Some(task) => !task.finished.load(Ordering::SeqCst),
            None => false,
        }
    }

    async fn wait_for_exit(&self, handle: &RunnerHandle, poll_interval: Duration) {
        loop {
            let Some(task) = self.task_of(&handle.launch_id) else {
                return;
            };
            if task.finished.load(Ordering::SeqCst) {
                return;
            }
            // The poll fallback covers the notify-before-wait race; the
            // notified() arm makes the common case prompt.
            tokio::select! {
                _ = task.done.notified() => {}
                _ = tokio::time::sleep(poll_interval.max(Duration::from_millis(50))) => {}
            }
        }
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
        if let Some(task) = self.task_of(&handle.launch_id) {
            info!(instance_id = %handle.instance_id, launch_id = %handle.launch_id, "Cancelling embedded workflow run");
            task.cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    async fn collect_result(
        &self,
        handle: &RunnerHandle,
    ) -> (Option<Value>, Option<String>, ContainerMetrics) {
        // Output is read from runtara-core by the container monitor, not from
        // files. collect_result only provides stderr for diagnostics.
        let stderr = common::load_stderr(
            &self.config.data_dir,
            &handle.tenant_id,
            &handle.instance_id,
            &handle.launch_id,
        )
        .await;

        let metrics = if let Some(metrics) = &handle.metrics {
            metrics.lock().await.clone()
        } else {
            ContainerMetrics::default()
        };

        (None, stderr, metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InstanceTask, RunSlot, RunSlotEntry, TaskCompletionGuard, TaskRegistry, compute_occupancy,
        remove_task_if_current,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Nothing running must be reported as "nothing running", not as "unknown".
    ///
    /// The distinction matters downstream: a consumer renders an absent
    /// occupancy as "not measured" and a present-but-zero one as idle, and
    /// collapsing the two invents an idle system that is actually unobserved.
    #[test]
    fn idle_runner_reports_zero_held_and_no_age() {
        let occ = compute_occupancy(16, 16, &HashMap::new(), Instant::now());
        assert_eq!(occ.limit, 16);
        assert_eq!(occ.held, 0);
        assert_eq!(occ.oldest_held_ms, None);
        assert_eq!(occ.oldest_instance_id, None);
    }

    /// `held` must follow the semaphore, not the bookkeeping map.
    ///
    /// A permit is taken before its acquisition time is recorded, so the map
    /// lags by that window. Were `held` counted from the map, a runner at its
    /// bound would briefly report headroom it does not have.
    #[test]
    fn held_comes_from_the_semaphore_not_the_slot_map() {
        let now = Instant::now();
        let mut slots = HashMap::new();
        slots.insert(
            "launch-1".to_string(),
            RunSlotEntry {
                instance_id: "only-one-recorded".to_string(),
                taken_at: now,
            },
        );

        // Eight permits gone, but only one has been stamped yet.
        let occ = compute_occupancy(8, 0, &slots, now);
        assert_eq!(occ.held, 8, "occupancy must not lag behind the semaphore");
        assert_eq!(
            occ.oldest_instance_id.as_deref(),
            Some("only-one-recorded"),
            "the age still comes from whatever has been recorded"
        );
    }

    /// The reported age must belong to the longest-held permit.
    ///
    /// This is the signal that separates a runner turning work over from one
    /// holding work that never leaves, so picking any other holder — the first
    /// inserted, or whatever the map happens to yield first — would report a
    /// stalled runner as healthy.
    #[test]
    fn age_belongs_to_the_oldest_holder() {
        let now = Instant::now();
        let mut slots = HashMap::new();
        slots.insert(
            "launch-recent".to_string(),
            RunSlotEntry {
                instance_id: "recent".to_string(),
                taken_at: now - Duration::from_secs(2),
            },
        );
        slots.insert(
            "launch-ancient".to_string(),
            RunSlotEntry {
                instance_id: "ancient".to_string(),
                taken_at: now - Duration::from_secs(2880),
            },
        );
        slots.insert(
            "launch-middling".to_string(),
            RunSlotEntry {
                instance_id: "middling".to_string(),
                taken_at: now - Duration::from_secs(45),
            },
        );

        let occ = compute_occupancy(8, 5, &slots, now);
        assert_eq!(occ.held, 3);
        assert_eq!(occ.oldest_instance_id.as_deref(), Some("ancient"));
        assert_eq!(
            occ.oldest_held_ms,
            Some(2_880_000),
            "48 minutes held is exactly the case this exists to surface"
        );
    }

    /// An over-subscribed reading must clamp rather than wrap.
    ///
    /// `held` is `limit - available` on unsigned values; a stale or racing read
    /// where available exceeds the limit would otherwise underflow into an
    /// enormous occupancy and paint a healthy runner as catastrophically full.
    #[test]
    fn available_above_limit_clamps_to_zero() {
        let occ = compute_occupancy(4, 9, &HashMap::new(), Instant::now());
        assert_eq!(occ.held, 0, "must saturate, never wrap");
    }

    /// Dropping the slot must release the permit *and* retire its timestamp.
    ///
    /// The pairing is the invariant: a permit returned without its entry
    /// removed leaves a timestamp that never ages out, and the runner would
    /// report a permanently stuck holder that finished long ago.
    #[tokio::test]
    async fn dropping_a_slot_releases_the_permit_and_forgets_its_age() {
        let permits = Arc::new(tokio::sync::Semaphore::new(2));
        let registry: Arc<Mutex<HashMap<String, RunSlotEntry>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let finished = Arc::new(std::sync::atomic::AtomicU64::new(0));

        {
            let permit = Arc::clone(&permits).acquire_owned().await.expect("acquire");
            registry.lock().expect("registry").insert(
                "launch-1".to_string(),
                RunSlotEntry {
                    instance_id: "inst-1".to_string(),
                    taken_at: Instant::now(),
                },
            );
            let _slot = RunSlot {
                _permit: permit,
                launch_id: "launch-1".to_string(),
                registry: Arc::clone(&registry),
                finished: Arc::clone(&finished),
            };

            assert_eq!(permits.available_permits(), 1);
            assert_eq!(registry.lock().expect("registry").len(), 1);
        }

        assert_eq!(
            permits.available_permits(),
            2,
            "the permit must return on drop"
        );
        assert!(
            registry.lock().expect("registry").is_empty(),
            "the acquisition time must retire with the permit"
        );
        assert_eq!(
            finished.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a released permit is a finished run; the two must not drift"
        );
    }

    /// A panicking run must not leave a phantom holder behind.
    ///
    /// Unwinding is exactly when bookkeeping is most likely to be skipped, and
    /// a leaked entry here is worse than none: it ages forever and would make
    /// every later reading report a stall that is not happening.
    #[tokio::test]
    async fn a_panicking_run_still_retires_its_slot() {
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let registry: Arc<Mutex<HashMap<String, RunSlotEntry>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let finished = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let permit = Arc::clone(&permits).acquire_owned().await.expect("acquire");
        registry.lock().expect("registry").insert(
            "launch-doomed".to_string(),
            RunSlotEntry {
                instance_id: "doomed".to_string(),
                taken_at: Instant::now(),
            },
        );
        let slot = RunSlot {
            _permit: permit,
            launch_id: "launch-doomed".to_string(),
            registry: Arc::clone(&registry),
            finished: Arc::clone(&finished),
        };

        let handle = tokio::spawn(async move {
            let _slot = slot;
            panic!("the guest blew up");
        });
        assert!(handle.await.is_err(), "the task must have panicked");

        assert_eq!(permits.available_permits(), 1);
        assert!(
            registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "drop runs while unwinding, so the entry must still be retired"
        );
        assert_eq!(
            finished.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a run that panicked still finished, and must be counted as such"
        );
    }

    #[test]
    fn stale_task_cleanup_cannot_remove_a_replacement_generation() {
        let registry: TaskRegistry = Arc::new(Mutex::new(HashMap::new()));
        let old = Arc::new(InstanceTask {
            cancel: Arc::new(AtomicBool::new(false)),
            finished: AtomicBool::new(false),
            done: tokio::sync::Notify::new(),
        });
        let replacement = Arc::new(InstanceTask {
            cancel: Arc::new(AtomicBool::new(false)),
            finished: AtomicBool::new(false),
            done: tokio::sync::Notify::new(),
        });

        registry
            .lock()
            .expect("registry")
            .insert("launch-current".to_string(), Arc::clone(&replacement));

        // This is the stop-then-immediate-resume race: an old task's deferred
        // cleanup reaches the map after a newer attempt owns the same logical
        // slot. Pointer comparison must leave the replacement visible.
        remove_task_if_current(&registry, "launch-current", &old);

        let current = registry
            .lock()
            .expect("registry")
            .get("launch-current")
            .cloned()
            .expect("replacement remains registered");
        assert!(Arc::ptr_eq(&current, &replacement));
    }

    #[tokio::test]
    async fn panicking_task_still_retires_its_runner_handle() {
        let registry: TaskRegistry = Arc::new(Mutex::new(HashMap::new()));
        let task = Arc::new(InstanceTask {
            cancel: Arc::new(AtomicBool::new(false)),
            finished: AtomicBool::new(false),
            done: tokio::sync::Notify::new(),
        });
        registry
            .lock()
            .expect("registry")
            .insert("launch-doomed".to_string(), Arc::clone(&task));
        let guard = TaskCompletionGuard {
            task: Arc::clone(&task),
            registry: Arc::clone(&registry),
            launch_id: "launch-doomed".to_string(),
        };

        let join = tokio::spawn(async move {
            let _guard = guard;
            panic!("runner task panicked before its manual cleanup tail");
        });
        assert!(join.await.is_err(), "test task must panic");
        assert!(task.finished.load(Ordering::SeqCst));
        assert!(
            registry.lock().expect("registry").is_empty(),
            "RAII cleanup must retire the exact task entry during unwind"
        );
    }

    #[test]
    fn unpolled_task_future_still_retires_its_runner_handle() {
        let registry: TaskRegistry = Arc::new(Mutex::new(HashMap::new()));
        let task = Arc::new(InstanceTask {
            cancel: Arc::new(AtomicBool::new(false)),
            finished: AtomicBool::new(false),
            done: tokio::sync::Notify::new(),
        });
        registry
            .lock()
            .expect("registry")
            .insert("launch-never-polled".to_string(), Arc::clone(&task));
        // This mirrors production: construct the guard before `tokio::spawn`
        // moves it into the task future. Dropping that future before its first
        // poll is what runtime shutdown does for a just-spawned task.
        let completion = TaskCompletionGuard {
            task: Arc::clone(&task),
            registry: Arc::clone(&registry),
            launch_id: "launch-never-polled".to_string(),
        };
        let never_polled = async move {
            let _completion = completion;
            std::future::pending::<()>().await;
        };

        drop(never_polled);

        assert!(task.finished.load(Ordering::SeqCst));
        assert!(
            registry.lock().expect("registry").is_empty(),
            "dropping an unpolled task future must retire the exact task entry"
        );
    }

    /// The advisory bound check must fire only when the setting cannot fit.
    ///
    /// It exists because raising `RUNTARA_MAX_CONCURRENT_RUNS` for throughput
    /// presents as a memory leak rather than as a setting: memory climbs, and
    /// the kernel kills the process minutes later, far from the change.
    #[test]
    fn run_bound_warning_tracks_host_memory() {
        // Whatever this host reports, a bound of one must fit and an absurd one
        // must not; the warning itself is a log line, so this pins the maths
        // that decides it rather than the emission.
        if let Some(total) = super::host_memory_bytes() {
            assert!(total > 0, "a readable MemTotal must be positive");
            let fits = super::OBSERVED_BYTES_PER_RUN <= total;
            assert!(fits, "one run must fit in any host this can run on");

            let absurd = (total / super::OBSERVED_BYTES_PER_RUN) as usize + 2;
            assert!(
                (absurd as u64).saturating_mul(super::OBSERVED_BYTES_PER_RUN) > total,
                "a bound past the host's memory must be recognised as too large"
            );
        }
        // Must not panic regardless of platform or how large the value is.
        super::warn_if_run_bound_exceeds_memory(1);
        super::warn_if_run_bound_exceeds_memory(usize::MAX);
    }

    use super::*;
    #[cfg(feature = "db-integration-tests")]
    use crate::test_support;
    use runtara_component_host::lifecycle::{SignalWait, WorkflowWake};

    #[test]
    fn earliest_wake_deadline_is_the_min_timed_wake() {
        // Re-invoke-on-ANY: the scheduler must relaunch at the EARLIEST wake.
        let wakes = vec![
            WorkflowWake::At(300),
            WorkflowWake::OnSignal(SignalWait {
                checkpoint_id: "sig".into(),
                deadline_ms: Some(120),
            }),
            WorkflowWake::OnResume,
        ];
        assert_eq!(earliest_wake_deadline_ms(&wakes), Some(120));
    }

    #[test]
    fn earliest_wake_deadline_is_none_when_all_deadline_less() {
        let wakes = vec![
            WorkflowWake::OnResume,
            WorkflowWake::OnSignal(SignalWait {
                checkpoint_id: "sig".into(),
                deadline_ms: None,
            }),
        ];
        assert_eq!(earliest_wake_deadline_ms(&wakes), None);
    }

    #[cfg(feature = "db-integration-tests")]
    async fn running_instance() -> (Arc<dyn Persistence>, String) {
        test_support::running_instance("park").await
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn park_stamps_suspended_and_sleep_until_for_a_timed_wake() {
        let (persistence, instance_id) = running_instance().await;
        let deadline_ms = 1_900_000_000_000u64; // a fixed absolute epoch-ms
        park_invoke_suspend(
            persistence.as_ref(),
            &instance_id,
            &[WorkflowWake::At(deadline_ms)],
        )
        .await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(inst.status, "suspended");
        assert_eq!(
            inst.sleep_until.map(|dt| dt.timestamp_millis() as u64),
            Some(deadline_ms),
            "sleep_until must be the wake deadline so the wake scan selects it"
        );
        assert_eq!(
            inst.termination_reason.as_deref(),
            Some("sleeping"),
            "a pure timed park is scheduler-woken, never signal-woken"
        );
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn park_leaves_a_deadline_less_suspend_untouched() {
        // on-resume (breakpoint/drain pause) already recorded suspended via its
        // ack; park must NOT stamp a premature sleep_until that would wake it.
        let (persistence, instance_id) = running_instance().await;
        park_invoke_suspend(
            persistence.as_ref(),
            &instance_id,
            &[WorkflowWake::OnResume],
        )
        .await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(inst.status, "running", "no timed wake => no status change");
        assert!(
            inst.sleep_until.is_none(),
            "no timed wake => no sleep_until stamp"
        );
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn park_marks_deadline_less_on_signal_suspended_without_sleep() {
        // A no-timeout wait: parked suspended so the custom-signal waker can
        // find it, but with NO sleep_until (the waker stamps it on arrival).
        let (persistence, instance_id) = running_instance().await;
        park_invoke_suspend(
            persistence.as_ref(),
            &instance_id,
            &[WorkflowWake::OnSignal(SignalWait {
                checkpoint_id: "wait-sig".into(),
                deadline_ms: None,
            })],
        )
        .await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(inst.status, "suspended", "on-signal parks as suspended");
        assert!(
            inst.sleep_until.is_none(),
            "a deadline-less on-signal wait relies on the waker, not sleep_until"
        );
        assert_eq!(
            inst.termination_reason.as_deref(),
            Some(WAITING_SIGNAL_TERMINATION),
            "the waker must be able to distinguish this park from a pause suspend"
        );
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn park_self_wakes_when_the_signal_beat_the_suspend_write() {
        // The lost-wakeup window: `handle_send_custom_signal` inserts the row and
        // calls `wake_suspended_on_signal`, which no-ops while the instance is
        // still `running` — which it is, right up until the write inside
        // `park_invoke_suspend`. For a wait with NO timeout there is no other
        // wake path, so without the re-check the instance would sit suspended
        // forever with its signal already in the table.
        let (persistence, instance_id) = running_instance().await;
        persistence
            .insert_custom_signal(&instance_id, "raced-sig", b"{}")
            .await
            .expect("insert signal");

        park_invoke_suspend(
            persistence.as_ref(),
            &instance_id,
            &[WorkflowWake::OnSignal(SignalWait {
                checkpoint_id: "raced-sig".into(),
                deadline_ms: None,
            })],
        )
        .await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(inst.status, "suspended");
        assert!(
            inst.sleep_until.is_some(),
            "a signal already present at park time must self-wake the instance, \
             not strand it waiting for a waker that already ran"
        );
        // Non-destructive: the replayed guest still observes the signal.
        assert!(
            persistence
                .take_pending_custom_signal(&instance_id, "raced-sig")
                .await
                .expect("read signal")
                .is_some(),
            "the self-wake must not consume the signal the replay needs"
        );
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn park_leaves_sleep_unset_when_no_signal_has_arrived_yet() {
        // The ordinary case must be untouched by the re-check above.
        let (persistence, instance_id) = running_instance().await;
        park_invoke_suspend(
            persistence.as_ref(),
            &instance_id,
            &[WorkflowWake::OnSignal(SignalWait {
                checkpoint_id: "quiet-sig".into(),
                deadline_ms: None,
            })],
        )
        .await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(inst.status, "suspended");
        assert!(
            inst.sleep_until.is_none(),
            "with no signal present the waker remains the sole wake path"
        );
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn park_never_overwrites_a_terminal_status() {
        // A malformed guest that reported terminal complete and THEN returned
        // outcome::suspended must not be resurrected (and must not be
        // scheduled for a wake) — the same if_running guard the event path's
        // suspend uses.
        let (persistence, instance_id) = running_instance().await;
        persistence
            .complete_instance(
                runtara_core::persistence::CompleteInstanceParams::new(&instance_id, "completed")
                    .if_running(),
            )
            .await
            .expect("complete");

        park_invoke_suspend(
            persistence.as_ref(),
            &instance_id,
            &[WorkflowWake::At(1_900_000_000_000u64)],
        )
        .await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(
            inst.status, "completed",
            "a terminal status must survive a late suspend return"
        );
        assert!(
            inst.sleep_until.is_none(),
            "a rejected park must not schedule a wake for a completed instance"
        );
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn park_stamps_on_signal_timeout_deadline_as_the_fallback() {
        let (persistence, instance_id) = running_instance().await;
        let deadline_ms = 1_950_000_000_000u64;
        park_invoke_suspend(
            persistence.as_ref(),
            &instance_id,
            &[WorkflowWake::OnSignal(SignalWait {
                checkpoint_id: "wait-sig".into(),
                deadline_ms: Some(deadline_ms),
            })],
        )
        .await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(inst.status, "suspended");
        assert_eq!(
            inst.sleep_until.map(|dt| dt.timestamp_millis() as u64),
            Some(deadline_ms),
            "an on-signal timeout is the fallback wake if the signal never arrives"
        );
    }

    /// A running instance on the real database.
    #[cfg(feature = "db-integration-tests")]
    async fn backstop_fixture() -> (Arc<dyn Persistence>, String) {
        test_support::running_instance("backstop").await
    }

    /// The floor under the host-side escalation: a guest that reported success
    /// while a cancel sat unacknowledged must still land on `cancelled`. This is
    /// what a workflow artifact with no poll site does, and it is the silent
    /// success the backstop exists to prevent.
    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn unacked_cancel_overrides_a_reported_completion() {
        let (persistence, instance_id) = backstop_fixture().await;
        persistence
            .insert_signal(instance_id.as_str(), "cancel", b"")
            .await
            .unwrap();
        persistence
            .complete_instance(runtara_core::persistence::CompleteInstanceParams::new(
                instance_id.as_str(),
                "completed",
            ))
            .await
            .unwrap();

        enforce_unacked_cancel(&persistence, instance_id.as_str()).await;

        assert_eq!(
            persistence
                .get_instance(instance_id.as_str())
                .await
                .unwrap()
                .unwrap()
                .status,
            "cancelled",
            "cancel wins the exit race: a stop was requested and not honoured"
        );
    }

    /// A backstop keyed on the row's mere presence would re-cancel a run whose
    /// guest handled the signal correctly and then finished. Pins both halves of
    /// the defence: the query filters acknowledged rows, and the caller checks
    /// `acknowledged_at` anyway.
    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn an_acknowledged_cancel_does_not_re_cancel_a_finished_run() {
        let (persistence, instance_id) = backstop_fixture().await;
        persistence
            .insert_signal(instance_id.as_str(), "cancel", b"")
            .await
            .unwrap();
        persistence
            .acknowledge_signal(instance_id.as_str())
            .await
            .unwrap();
        persistence
            .complete_instance(runtara_core::persistence::CompleteInstanceParams::new(
                instance_id.as_str(),
                "completed",
            ))
            .await
            .unwrap();

        enforce_unacked_cancel(&persistence, instance_id.as_str()).await;

        assert_eq!(
            persistence
                .get_instance(instance_id.as_str())
                .await
                .unwrap()
                .unwrap()
                .status,
            "completed",
            "an already-acknowledged cancel must not touch a finished run"
        );
    }

    /// A run nobody stopped is left entirely alone.
    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn a_run_with_no_cancel_is_untouched() {
        let (persistence, instance_id) = backstop_fixture().await;
        persistence
            .complete_instance(runtara_core::persistence::CompleteInstanceParams::new(
                instance_id.as_str(),
                "completed",
            ))
            .await
            .unwrap();

        enforce_unacked_cancel(&persistence, instance_id.as_str()).await;

        assert_eq!(
            persistence
                .get_instance(instance_id.as_str())
                .await
                .unwrap()
                .unwrap()
                .status,
            "completed"
        );
    }
}
