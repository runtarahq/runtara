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
        let metrics = metrics_of(&run);
        let result = exit_to_result(&run.exit);
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
        let metrics_for_task = Arc::clone(&metrics);
        let task_for_run = Arc::clone(&task);
        let registry = Arc::clone(&self.tasks);
        let instance_id = options.instance_id.clone();
        tokio::spawn(async move {
            match executor.load(&wasm_path).await {
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
