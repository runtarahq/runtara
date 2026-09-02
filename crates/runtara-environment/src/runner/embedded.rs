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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use runtara_component_host::{
    EngineConfig, WorkflowExecutor, WorkflowExit, WorkflowLimits, WorkflowRunSpec, build_engine,
    spawn_epoch_ticker,
};
use runtara_core::instance_handlers::{
    InstanceHandlerState, SignalAck, SignalType, handle_signal_ack,
};
use runtara_core::persistence::Persistence;

use super::common::{self, WorkflowRunnerConfig};
use super::traits::{
    CancelToken, ContainerMetrics, LaunchOptions, LaunchResult, Result, Runner, RunnerError,
    RunnerHandle, RunnerOccupancy,
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

/// Per-instance bookkeeping for detached runs.
struct InstanceTask {
    cancel: CancelToken,
    finished: AtomicBool,
    done: tokio::sync::Notify,
}

type TaskRegistry = Arc<Mutex<HashMap<String, Arc<InstanceTask>>>>;

/// In-process workflow runner backed by an embedded wasmtime engine.
pub struct EmbeddedWasmRunner {
    config: WorkflowRunnerConfig,
    limits: WorkflowLimits,
    persistence: Arc<dyn Persistence>,
    executor: Arc<WorkflowExecutor>,
    tasks: TaskRegistry,
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

/// Acquisition times of the run permits currently held, keyed by instance.
type RunSlotRegistry = Arc<Mutex<HashMap<String, Instant>>>;

/// A held run permit, tied to the moment it was taken.
///
/// The permit alone would bound concurrency perfectly well; this wrapper exists
/// only so that releasing it also retires the acquisition time. Pairing them in
/// one value is what makes the two impossible to drift apart — there is no path
/// that returns a permit without also dropping this.
struct RunSlot {
    /// Dropped with the struct; that release is the whole point of the field.
    _permit: tokio::sync::OwnedSemaphorePermit,
    instance_id: String,
    registry: RunSlotRegistry,
    /// Bumped as the permit returns, so the count of finished runs cannot drift
    /// from the count of released permits.
    finished: Arc<AtomicU64>,
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
    slots: &HashMap<String, Instant>,
    now: Instant,
) -> RunnerOccupancy {
    // Deliberately not `slots.len()`. A permit is taken before its acquisition
    // time is recorded, so during that window the map under-reports; the
    // semaphore never does. The map's only job is answering "how old is the
    // oldest", where a momentarily missing entry costs nothing.
    let held = limit.saturating_sub(available);
    let oldest = slots.iter().min_by_key(|(_, taken)| **taken);
    RunnerOccupancy {
        limit: limit as u64,
        held: held as u64,
        oldest_held_ms: oldest
            .map(|(_, taken)| now.saturating_duration_since(*taken).as_millis() as u64),
        oldest_instance_id: oldest.map(|(id, _)| id.clone()),
        // Filled in by the caller, which owns the lifetime counters; this
        // function is about a single instant's occupancy.
        runs_started: 0,
        runs_finished: 0,
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
        slots.remove(&self.instance_id);
        self.finished.fetch_add(1, Ordering::Relaxed);
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
        Ok(Self {
            config,
            limits: limits_from_env(),
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

    fn merged_env(&self, options: &LaunchOptions) -> HashMap<String, String> {
        let mut env = common::build_env(
            &self.config,
            &options.instance_id,
            &options.tenant_id,
            options.checkpoint_id.as_deref(),
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
        .with_prepersisted_input(options.prepersisted_input.clone());
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

    /// The instance's persisted (enriched) input envelope — what
    /// `runtime.load-input` served the legacy guest. Errors if the instance is
    /// gone; the detached path logs and falls back instead.
    async fn persisted_input(&self, options: &LaunchOptions) -> Result<Vec<u8>> {
        resolve_run_input(
            self.persistence.as_ref(),
            &options.instance_id,
            options.prepersisted_input.clone(),
        )
        .await
        .map_err(|e| RunnerError::StartFailed(format!("load instance input: {e:#}")))?
        .ok_or_else(|| {
            RunnerError::StartFailed(format!("instance {} not found", options.instance_id))
        })
    }

    fn task_of(&self, instance_id: &str) -> Option<Arc<InstanceTask>> {
        self.tasks
            .lock()
            .expect("embedded runner task registry poisoned")
            .get(instance_id)
            .cloned()
    }
}

/// The enriched input envelope a run starts from.
///
/// `prepersisted` is [`LaunchOptions::prepersisted_input`]: the first-start
/// path passes the bytes it has just written so the launch does not read back
/// its own write. Every other path passes `None` and gets the stored envelope,
/// which is what makes a woken instance resume on its real input instead of the
/// relaunch request's placeholder.
///
/// Both launch paths — synchronous `run` and the spawned `launch_detached` —
/// go through here on purpose. They used to each fetch the input themselves,
/// and the duplicate was how the detached path came to ignore this field.
/// `Ok(None)` means no such instance; callers differ on how loudly to fail.
async fn resolve_run_input(
    persistence: &dyn Persistence,
    instance_id: &str,
    prepersisted: Option<Vec<u8>>,
) -> std::result::Result<Option<Vec<u8>>, runtara_core::error::CoreError> {
    if let Some(input) = prepersisted {
        return Ok(Some(input));
    }
    Ok(persistence
        .get_instance(instance_id)
        .await?
        .map(|instance| instance.input.unwrap_or_else(|| b"{}".to_vec())))
}

/// How many guests may execute concurrently in this process.
///
/// Each live guest holds a wasmtime store, so this is a memory bound before it
/// is a throughput one: on a small host, letting a fast producer stack runs
/// exhausts the machine and the process is killed, which is a far worse outcome
/// than queueing. `RUNTARA_MAX_CONCURRENT_RUNS`, defaulting to four per core.
fn max_concurrent_runs() -> usize {
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

/// Map a finished embedded run to the `Result<()>` shape `WasmRunner`'s
/// process-exit path produces, so the surrounding `LaunchResult` logic stays
/// identical between runners.
fn exit_to_result(exit: &WorkflowExit) -> std::result::Result<(), RunnerError> {
    match exit {
        WorkflowExit::Completed => Ok(()),
        WorkflowExit::GuestError => Err(RunnerError::ExitCode {
            exit_code: 1,
            stderr: String::new(),
        }),
        WorkflowExit::Failed { reason } => Err(RunnerError::ExitCode {
            exit_code: 1,
            stderr: reason.clone(),
        }),
        WorkflowExit::Timeout => Err(RunnerError::Timeout),
        WorkflowExit::Cancelled => Err(RunnerError::Cancelled),
    }
}

/// Map an invoke-shaped run to the same `Result<()>` shape. A suspension is a
/// clean exit (the suspended status was recorded host-side by the signal
/// ack), exactly as the legacy run path's Ok-exit-with-DB-suspended was; a
/// Failed outcome mirrors GuestError (the error was recorded additively via
/// runtime.fail, so `load_output` surfaces it downstream unchanged).
fn invoke_exit_to_result(
    exit: &runtara_component_host::InvokeExit,
) -> std::result::Result<(), RunnerError> {
    use runtara_component_host::InvokeExit;
    match exit {
        InvokeExit::Completed(_) | InvokeExit::Suspended(_) => Ok(()),
        InvokeExit::Failed(_) => Err(RunnerError::ExitCode {
            exit_code: 1,
            stderr: String::new(),
        }),
        InvokeExit::Trapped { reason } => Err(RunnerError::ExitCode {
            exit_code: 1,
            stderr: reason.clone(),
        }),
        InvokeExit::Timeout => Err(RunnerError::Timeout),
        InvokeExit::Cancelled => Err(RunnerError::Cancelled),
    }
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

    async fn run(
        &self,
        options: &LaunchOptions,
        cancel_token: Option<CancelToken>,
    ) -> Result<LaunchResult> {
        let start = std::time::Instant::now();

        let wasm_path = options.wasm_path.clone();
        if !wasm_path.exists() {
            return Err(RunnerError::BinaryNotFound(wasm_path.display().to_string()));
        }

        let env = self.merged_env(options);
        let instance_pre = self
            .executor
            .load_instance_pre(&wasm_path)
            .await
            .map_err(|e| RunnerError::StartFailed(format!("{e:#}")))?;

        // Dual-ABI dispatch: an invoke-shaped artifact runs through the
        // in-band entry (input fetched from persistence — the enriched
        // stored envelope, first run AND wake alike); a legacy artifact
        // keeps the wasi:cli/run path unchanged.
        let (metrics, result) = if runtara_component_host::lifecycle::exports_lifecycle_invoke(
            &instance_pre,
            self.executor.engine(),
        ) {
            let input = self.persisted_input(options).await?;
            // Run as `running` (see the detached path for why relaunches need
            // this) — no-op on the first-run path, which is already running.
            mark_running(self.persistence.as_ref(), &options.instance_id).await;
            let run = self
                .executor
                .execute_invoke(
                    &instance_pre,
                    self.run_spec(options, env, None, options.timeout, cancel_token),
                    input,
                )
                .await;
            // A store-freeing suspend has no output yet — park it and report a
            // clean, non-terminal result rather than letting `load_output` fail.
            if let runtara_component_host::InvokeExit::Suspended(wakes) = &run.exit {
                park_invoke_suspend(self.persistence.as_ref(), &options.instance_id, wakes).await;
                return Ok(LaunchResult {
                    instance_id: options.instance_id.clone(),
                    success: true,
                    output: None,
                    error: None,
                    stderr: None,
                    duration_ms: start.elapsed().as_millis() as u64,
                    metrics: invoke_metrics_of(&run),
                });
            }
            (invoke_metrics_of(&run), invoke_exit_to_result(&run.exit))
        } else {
            let pre = self
                .executor
                .load(&wasm_path)
                .await
                .map_err(|e| RunnerError::StartFailed(format!("{e:#}")))?;
            let run = self
                .executor
                .execute(
                    &pre,
                    self.run_spec(options, env, None, options.timeout, cancel_token),
                )
                .await;
            (metrics_of(&run), exit_to_result(&run.exit))
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(()) => {
                match common::load_output(self.persistence.as_ref(), &options.instance_id).await {
                    Ok(output) => Ok(LaunchResult {
                        instance_id: options.instance_id.clone(),
                        success: true,
                        output: Some(output),
                        error: None,
                        stderr: None,
                        duration_ms,
                        metrics,
                    }),
                    Err(e) => Ok(LaunchResult {
                        instance_id: options.instance_id.clone(),
                        success: false,
                        output: None,
                        error: Some(format!("Failed to load output: {}", e)),
                        stderr: None,
                        duration_ms,
                        metrics,
                    }),
                }
            }
            Err(e) => {
                // Prefer the SDK-reported error from runtara-core when present.
                let error_msg = match common::load_output(
                    self.persistence.as_ref(),
                    &options.instance_id,
                )
                .await
                {
                    Err(RunnerError::Other(msg)) => msg,
                    _ => e.to_string(),
                };
                Ok(LaunchResult {
                    instance_id: options.instance_id.clone(),
                    success: false,
                    output: None,
                    error: Some(error_msg),
                    stderr: None,
                    duration_ms,
                    metrics,
                })
            }
        }
    }

    async fn launch_detached(&self, options: &LaunchOptions) -> Result<RunnerHandle> {
        let wasm_path = options.wasm_path.clone();
        if !wasm_path.exists() {
            return Err(RunnerError::BinaryNotFound(wasm_path.display().to_string()));
        }

        common::ensure_run_dir(
            &self.config.data_dir,
            &options.tenant_id,
            &options.instance_id,
        )
        .await?;
        let run_dir = common::run_dir(
            &self.config.data_dir,
            &options.tenant_id,
            &options.instance_id,
        );
        let log_path = run_dir.join("stderr.log");
        let stderr_file = match std::fs::File::create(&log_path) {
            Ok(f) => Some(f),
            Err(e) => {
                warn!(
                    instance_id = %options.instance_id,
                    error = %e,
                    path = %log_path.display(),
                    "Failed to create stderr log file"
                );
                None
            }
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
            .insert(options.instance_id.clone(), Arc::clone(&task));

        let metrics = Arc::new(tokio::sync::Mutex::new(ContainerMetrics::default()));

        // Timeout is enforced by the container monitor via `stop()`, exactly
        // as it is for the detached CLI runner (which spawns with no timeout
        // of its own). MAX keeps the internal rings cancel-only.
        let spec = self.run_spec(options, env, stderr_file, Duration::MAX, Some(cancel));

        let executor = Arc::clone(&self.executor);
        let persistence = Arc::clone(&self.persistence);
        let metrics_for_task = Arc::clone(&metrics);
        let task_for_run = Arc::clone(&task);
        let registry = Arc::clone(&self.tasks);
        let instance_id = options.instance_id.clone();
        // See `LaunchOptions::prepersisted_input`: set only by the first-start
        // path, with the bytes it has just written. A wake or resume leaves it
        // None and still reads the stored envelope below.
        let prepersisted_input = options.prepersisted_input.clone();
        // Take the slot BEFORE spawning, so what is bounded is the number of
        // runs outstanding rather than the number executing. Acquiring inside
        // the task bounds neither: every caller still gets a task immediately
        // and only then queues, so each one holds its spec, its input bytes and
        // its handles for as long as the queue is long. Waking a large parked
        // population is exactly that shape - the wake scheduler's own limit
        // frees as soon as the task is spawned, so it launches as fast as it
        // can read batches - and 20k queued runs cost about a gigabyte, which
        // killed the process the in-task acquire was added to protect.
        //
        // The caller waits here instead. The instance is already registered and
        // durable, so this delays the run rather than losing it, and it gives
        // the wake scheduler and trigger worker the backpressure their own
        // concurrency limits are supposed to express.
        let permit = Arc::clone(&self.run_permits)
            .acquire_owned()
            .await
            .expect("run semaphore closed");
        // Stamp the acquisition before the task is spawned, so the age covers
        // the whole time the permit is held rather than starting once the task
        // happens to be scheduled.
        self.run_slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(options.instance_id.clone(), Instant::now());
        self.runs_started.fetch_add(1, Ordering::Relaxed);
        let run_slot = RunSlot {
            _permit: permit,
            instance_id: options.instance_id.clone(),
            registry: Arc::clone(&self.run_slots),
            finished: Arc::clone(&self.runs_finished),
        };
        tokio::spawn(async move {
            let _run_slot = run_slot;
            match executor.load_instance_pre(&wasm_path).await {
                Ok(instance_pre) => {
                    if runtara_component_host::lifecycle::exports_lifecycle_invoke(
                        &instance_pre,
                        executor.engine(),
                    ) {
                        // Invoke-shaped artifact: input is the enriched stored
                        // envelope, terminal result in-band. The first-start
                        // path hands those bytes over directly rather than
                        // making this read back what it just wrote; every other
                        // path goes to the store, so a woken instance still gets
                        // its real input and never a relaunch placeholder.
                        let input = match resolve_run_input(
                            persistence.as_ref(),
                            &instance_id,
                            prepersisted_input,
                        )
                        .await
                        {
                            Ok(Some(input)) => input,
                            Ok(None) => {
                                error!(instance_id = %instance_id, "Instance not found for invoke launch");
                                b"{}".to_vec()
                            }
                            Err(e) => {
                                error!(instance_id = %instance_id, error = %e, "Failed to load instance input");
                                b"{}".to_vec()
                            }
                        };
                        // Ensure the run executes as `running`. The first-run
                        // launch also sets this after `launch_detached` returns,
                        // but a wake-scheduler relaunch (`wake_instance`) does
                        // NOT — and a guest that completes while still marked
                        // `suspended` would have its `if_running`-guarded
                        // terminal event silently dropped. Set it here so BOTH
                        // paths run as `running` before the guest starts.
                        mark_running(persistence.as_ref(), &instance_id).await;
                        let run = executor.execute_invoke(&instance_pre, spec, input).await;
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
                                // Store-freeing durable sleep: the guest exited
                                // with a timed wake instead of blocking; park it
                                // so the wake scheduler relaunches at the
                                // deadline. (A deadline-less on-resume was
                                // already recorded suspended by its ack.)
                                park_invoke_suspend(persistence.as_ref(), &instance_id, wakes)
                                    .await;
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
                        // A park is not an ending — the wake scheduler owns it
                        // from here (and resolves a pending cancel itself).
                        if !matches!(&run.exit, InvokeExit::Suspended(_)) {
                            enforce_unacked_cancel(&persistence, &instance_id).await;
                        }
                    } else {
                        match runtara_component_host::WorkflowExecutor::load(&executor, &wasm_path)
                            .await
                        {
                            Ok(pre) => {
                                // Same reason the invoke branch does it: the
                                // guest must be `running` before it starts, or
                                // a terminal event it reports is dropped by the
                                // `if_running` guard. Doing it on both branches
                                // is what lets the launching caller stop
                                // stamping it a second time after the fact.
                                mark_running(persistence.as_ref(), &instance_id).await;
                                let run = executor.execute(&pre, spec).await;
                                {
                                    let mut guard = metrics_for_task.lock().await;
                                    *guard = metrics_of(&run);
                                }
                                match &run.exit {
                                    WorkflowExit::Completed => {
                                        info!(instance_id = %instance_id, "Embedded workflow run completed");
                                    }
                                    WorkflowExit::GuestError => {
                                        // Failure details were reported to runtara-core
                                        // by the SDK before run() returned.
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
                            }
                            Err(e) => {
                                error!(
                                    instance_id = %instance_id,
                                    error = format!("{e:#}"),
                                    "Failed to load workflow component"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(
                        instance_id = %instance_id,
                        error = format!("{e:#}"),
                        "Failed to load workflow component"
                    );
                }
            }
            task_for_run.finished.store(true, Ordering::SeqCst);
            // Self-cleanup keeps the registry leak-free even when the monitor
            // takes the timeout path and never calls collect_result.
            registry
                .lock()
                .expect("embedded runner task registry poisoned")
                .remove(&instance_id);
            task_for_run.done.notify_waiters();
        });

        info!(
            instance_id = %options.instance_id,
            wasm = %options.wasm_path.display(),
            "Launched embedded workflow run (detached)"
        );

        Ok(RunnerHandle {
            handle_id: format!("wasm_{}", options.instance_id),
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: chrono::Utc::now(),
            metrics: Some(metrics),
        })
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        match self.task_of(&handle.instance_id) {
            Some(task) => !task.finished.load(Ordering::SeqCst),
            None => false,
        }
    }

    async fn wait_for_exit(&self, handle: &RunnerHandle, poll_interval: Duration) {
        loop {
            let Some(task) = self.task_of(&handle.instance_id) else {
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
        if let Some(task) = self.task_of(&handle.instance_id) {
            info!(instance_id = %handle.instance_id, "Cancelling embedded workflow run");
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
    use super::{RunSlot, compute_occupancy};
    use std::collections::HashMap;
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
        slots.insert("only-one-recorded".to_string(), now);

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
        slots.insert("recent".to_string(), now - Duration::from_secs(2));
        slots.insert("ancient".to_string(), now - Duration::from_secs(2880));
        slots.insert("middling".to_string(), now - Duration::from_secs(45));

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
        let registry: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let finished = Arc::new(std::sync::atomic::AtomicU64::new(0));

        {
            let permit = Arc::clone(&permits).acquire_owned().await.expect("acquire");
            registry
                .lock()
                .expect("registry")
                .insert("inst-1".to_string(), Instant::now());
            let _slot = RunSlot {
                _permit: permit,
                instance_id: "inst-1".to_string(),
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
        let registry: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let finished = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let permit = Arc::clone(&permits).acquire_owned().await.expect("acquire");
        registry
            .lock()
            .expect("registry")
            .insert("doomed".to_string(), Instant::now());
        let slot = RunSlot {
            _permit: permit,
            instance_id: "doomed".to_string(),
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
