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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, info, warn};

use runtara_component_host::{
    EngineConfig, WorkflowExecutor, WorkflowExit, WorkflowLimits, WorkflowRunSpec, build_engine,
    spawn_epoch_ticker,
};
use runtara_core::persistence::Persistence;

use super::common::{self, WorkflowRunnerConfig};
use super::traits::{
    CancelToken, ContainerMetrics, LaunchOptions, LaunchResult, Result, Runner, RunnerError,
    RunnerHandle,
};

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
    /// Shared handler state for per-run [`PersistenceRuntimeHost`]s — the
    /// native runtime interface for HostImport-composed artifacts.
    handler_state: Arc<runtara_core::instance_handlers::InstanceHandlerState>,
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
        Ok(Self {
            config,
            limits: limits_from_env(),
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
            &options.runtara_core_addr,
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
        let runtime = Arc::new(crate::runtime_host::PersistenceRuntimeHost::new(
            Arc::clone(&self.handler_state),
            options.instance_id.clone(),
            debug_mode,
        ));
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
    /// `runtime.load-input` served the legacy guest. Fetched fresh on every
    /// launch so a woken instance re-reads the SAME stored bytes (never the
    /// relaunch request's placeholder input).
    async fn persisted_input(&self, instance_id: &str) -> Result<Vec<u8>> {
        let instance = self
            .persistence
            .get_instance(instance_id)
            .await
            .map_err(|e| RunnerError::StartFailed(format!("load instance input: {e:#}")))?
            .ok_or_else(|| RunnerError::StartFailed(format!("instance {instance_id} not found")))?;
        Ok(instance.input.unwrap_or_else(|| b"{}".to_vec()))
    }

    fn task_of(&self, instance_id: &str) -> Option<Arc<InstanceTask>> {
        self.tasks
            .lock()
            .expect("embedded runner task registry poisoned")
            .get(instance_id)
            .cloned()
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

/// Park an invoke-shaped instance that returned `outcome::suspended` with a
/// TIMED wake (the store-freeing durable-sleep path): stamp `status=suspended`
/// and `sleep_until=deadline` so the wake scheduler relaunches it. The guest
/// already persisted its resume checkpoint before exiting, so there is no
/// output/checkpoint work here.
///
/// Deadline-less suspends (`on-resume` from a breakpoint/drain-signal pause)
/// are NOT handled here: those already recorded `status=suspended` inline via
/// their host-import ack, and the drain path stamps its own restart wake —
/// touching `sleep_until` for them would wrongly schedule an immediate wake.
async fn park_invoke_suspend(
    persistence: &dyn Persistence,
    instance_id: &str,
    wakes: &[runtara_component_host::lifecycle::WorkflowWake],
) {
    let Some(deadline_ms) = earliest_wake_deadline_ms(wakes) else {
        return;
    };
    let Some(deadline) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(deadline_ms as i64)
    else {
        warn!(
            instance_id,
            deadline_ms, "Suspend deadline out of range; not parking"
        );
        return;
    };
    // status first, then sleep_until: the wake scan requires BOTH
    // `status='suspended'` AND `sleep_until IS NOT NULL`, so neither ordering
    // exposes a half-parked instance to a premature claim.
    if let Err(e) = persistence
        .update_instance_status(instance_id, "suspended", None)
        .await
    {
        warn!(instance_id, error = %e, "Failed to mark instance suspended after invoke suspend");
    }
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

    async fn run(
        &self,
        options: &LaunchOptions,
        cancel_token: Option<CancelToken>,
    ) -> Result<LaunchResult> {
        let start = std::time::Instant::now();

        let wasm_path = options.bundle_path.clone();
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
            let input = self.persisted_input(&options.instance_id).await?;
            // Run as `running` (see the detached path for why relaunches need
            // this) — no-op on the first-run path, which is already running.
            if let Err(e) = self
                .persistence
                .update_instance_status(&options.instance_id, "running", None)
                .await
            {
                warn!(instance_id = %options.instance_id, error = %e, "Failed to mark invoke instance running");
            }
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
        let wasm_path = options.bundle_path.clone();
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
        tokio::spawn(async move {
            match executor.load_instance_pre(&wasm_path).await {
                Ok(instance_pre) => {
                    if runtara_component_host::lifecycle::exports_lifecycle_invoke(
                        &instance_pre,
                        executor.engine(),
                    ) {
                        // Invoke-shaped artifact: input from persistence (the
                        // enriched stored envelope), terminal result in-band.
                        let input = match persistence.get_instance(&instance_id).await {
                            Ok(Some(instance)) => instance.input.unwrap_or_else(|| b"{}".to_vec()),
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
                        if let Err(e) = persistence
                            .update_instance_status(&instance_id, "running", None)
                            .await
                        {
                            warn!(instance_id = %instance_id, error = %e, "Failed to mark invoke instance running");
                        }
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
                    } else {
                        match runtara_component_host::WorkflowExecutor::load(&executor, &wasm_path)
                            .await
                        {
                            Ok(pre) => {
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
            wasm = %options.bundle_path.display(),
            "Launched embedded workflow run (detached)"
        );

        Ok(RunnerHandle {
            handle_id: format!("wasm_{}", options.instance_id),
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: chrono::Utc::now(),
            spawned_pid: None,
            child: None,
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

    async fn get_pid(&self, _handle: &RunnerHandle) -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_component_host::lifecycle::{SignalWait, WorkflowWake};
    use runtara_core::persistence::SqlitePersistence;

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

    async fn running_instance() -> (Arc<dyn Persistence>, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let persistence: Arc<dyn Persistence> = Arc::new(
            SqlitePersistence::from_path(dir.path().join("park.db"))
                .await
                .expect("sqlite persistence"),
        );
        let instance_id = "park-inst".to_string();
        persistence
            .register_instance(&instance_id, "park-tenant")
            .await
            .expect("register");
        persistence
            .update_instance_status(&instance_id, "running", None)
            .await
            .expect("mark running");
        (persistence, instance_id, dir)
    }

    #[tokio::test]
    async fn park_stamps_suspended_and_sleep_until_for_a_timed_wake() {
        let (persistence, instance_id, _dir) = running_instance().await;
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
    }

    #[tokio::test]
    async fn park_leaves_a_deadline_less_suspend_untouched() {
        // on-resume (breakpoint/drain pause) already recorded suspended via its
        // ack; park must NOT stamp a premature sleep_until that would wake it.
        let (persistence, instance_id, _dir) = running_instance().await;
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
}
