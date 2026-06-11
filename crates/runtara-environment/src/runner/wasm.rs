// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! WebAssembly runner using wasmtime.
//!
//! Launches WASM workflow binaries via the `wasmtime` CLI with WASI support.
//! Output is read from runtara-core persistence (the SDK reports completion/failure
//! via HTTP). No filesystem I/O is needed for input or output.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use runtara_core::persistence::Persistence;

use crate::runner::{
    CancelToken, ContainerMetrics, LaunchOptions, LaunchResult, Result, Runner, RunnerError,
    RunnerHandle,
};

/// Logging filter for the host-side `wasmtime` CLI process.
///
/// The guest gets its own `RUST_LOG` via `--env` below. This value is for the
/// CLI itself; without overriding inherited host env, `RUST_LOG=debug` or
/// `WASMTIME_LOG=debug` on the environment process makes Wasmtime/Cranelift
/// emit one compile/timing line per wasm function.
const WASMTIME_PROCESS_LOG_FILTER: &str = "warn";

fn configure_wasmtime_process_logging(cmd: &mut Command) {
    let filter = wasmtime_process_log_filter(std::env::var("RUNTARA_WASMTIME_LOG").ok().as_deref());
    cmd.env("RUST_LOG", &filter);
    cmd.env("WASMTIME_LOG", filter);
}

fn wasmtime_process_log_filter(override_filter: Option<&str>) -> String {
    override_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(WASMTIME_PROCESS_LOG_FILTER)
        .to_string()
}

fn merge_process_metrics(target: &mut ContainerMetrics, sample: ContainerMetrics) {
    if let Some(sample_peak) = sample.memory_peak_bytes {
        target.memory_peak_bytes = Some(
            target
                .memory_peak_bytes
                .map_or(sample_peak, |current_peak| current_peak.max(sample_peak)),
        );
    }
    if let Some(current) = sample.memory_current_bytes {
        target.memory_current_bytes = Some(current);
    }
    if let Some(cpu) = sample.cpu_usage_usec {
        target.cpu_usage_usec = Some(
            target
                .cpu_usage_usec
                .map_or(cpu, |current_cpu| current_cpu.max(cpu)),
        );
    }
    if let Some(user) = sample.cpu_user_usec {
        target.cpu_user_usec = Some(
            target
                .cpu_user_usec
                .map_or(user, |current_user| current_user.max(user)),
        );
    }
    if let Some(system) = sample.cpu_system_usec {
        target.cpu_system_usec = Some(
            target
                .cpu_system_usec
                .map_or(system, |current_system| current_system.max(system)),
        );
    }
}

fn sample_process_metrics(pid: u32) -> Option<ContainerMetrics> {
    #[cfg(target_os = "linux")]
    {
        sample_linux_proc_metrics(pid).or_else(|| sample_ps_rss_metrics(pid))
    }
    #[cfg(not(target_os = "linux"))]
    {
        sample_ps_rss_metrics(pid)
    }
}

#[cfg(target_os = "linux")]
fn sample_linux_proc_metrics(pid: u32) -> Option<ContainerMetrics> {
    let status = std::fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
    let (peak, current) = parse_proc_status_memory(&status);
    if peak.is_none() && current.is_none() {
        return None;
    }

    Some(ContainerMetrics {
        memory_peak_bytes: peak.or(current),
        memory_current_bytes: current,
        ..Default::default()
    })
}

#[cfg(target_os = "linux")]
fn parse_proc_status_memory(status: &str) -> (Option<u64>, Option<u64>) {
    let mut peak = None;
    let mut current = None;

    for line in status.lines() {
        if line.starts_with("VmHWM:") {
            peak = parse_proc_status_kb_value(line);
        } else if line.starts_with("VmRSS:") {
            current = parse_proc_status_kb_value(line);
        }
    }

    (peak, current)
}

#[cfg(target_os = "linux")]
fn parse_proc_status_kb_value(line: &str) -> Option<u64> {
    line.split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .map(|kb| kb.saturating_mul(1024))
}

fn sample_ps_rss_metrics(pid: u32) -> Option<ContainerMetrics> {
    let output = StdCommand::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let rss_kb = stdout
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())?;
    let rss_bytes = rss_kb.saturating_mul(1024);

    Some(ContainerMetrics {
        memory_peak_bytes: Some(rss_bytes),
        memory_current_bytes: Some(rss_bytes),
        ..Default::default()
    })
}

async fn sample_process_metrics_into(
    pid: u32,
    metrics: &Arc<tokio::sync::Mutex<ContainerMetrics>>,
) -> bool {
    let Some(sample) = sample_process_metrics(pid) else {
        return false;
    };

    let mut guard = metrics.lock().await;
    merge_process_metrics(&mut guard, sample);
    true
}

fn spawn_process_metrics_sampler(pid: u32, metrics: Arc<tokio::sync::Mutex<ContainerMetrics>>) {
    tokio::spawn(async move {
        loop {
            if !sample_process_metrics_into(pid, &metrics).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}

/// WebAssembly runner configuration.
#[derive(Debug, Clone)]
pub struct WasmRunnerConfig {
    /// Path to the wasmtime binary.
    pub wasmtime_path: PathBuf,
    /// Data directory for stderr capture.
    pub data_dir: PathBuf,
    /// Default execution timeout.
    pub default_timeout: Duration,
    /// Skip TLS certificate verification (passed to instances).
    pub skip_cert_verification: bool,
    /// Connection service URL for fetching credentials at runtime (passed to instances).
    pub connection_service_url: Option<String>,
}

impl WasmRunnerConfig {
    /// Create configuration from environment variables.
    ///
    /// - `WASMTIME_PATH`: path to the wasmtime binary (default: `wasmtime` in PATH,
    ///   falling back to `~/.wasmtime/bin/wasmtime`).
    /// - `DATA_DIR`: data directory for instance I/O (default: `.data`).
    /// - `EXECUTION_TIMEOUT_SECS`: default execution timeout in seconds (default: 300).
    /// - `RUNTARA_SKIP_CERT_VERIFICATION`: skip TLS cert verification (default: false).
    /// - `RUNTARA_CONNECTION_SERVICE_URL`: connection service URL (optional).
    pub fn from_env() -> Self {
        let wasmtime_path = std::env::var("WASMTIME_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                // Try ~/.wasmtime/bin/wasmtime if it exists, otherwise fall back to PATH
                if let Ok(home) = std::env::var("HOME") {
                    let home_path = PathBuf::from(home)
                        .join(".wasmtime")
                        .join("bin")
                        .join("wasmtime");
                    if home_path.exists() {
                        return home_path;
                    }
                }
                PathBuf::from("wasmtime")
            });

        let data_dir_raw =
            PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string()));
        let data_dir = if data_dir_raw.is_absolute() {
            data_dir_raw
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&data_dir_raw))
                .unwrap_or(data_dir_raw)
        };

        Self {
            wasmtime_path,
            data_dir,
            default_timeout: Duration::from_secs(
                std::env::var("EXECUTION_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(300),
            ),
            skip_cert_verification: std::env::var("RUNTARA_SKIP_CERT_VERIFICATION")
                .ok()
                .map(|v| crate::config::parse_bool_lenient(&v))
                .unwrap_or(false),
            connection_service_url: std::env::var("RUNTARA_CONNECTION_SERVICE_URL").ok(),
        }
    }
}

/// WebAssembly runner.
///
/// Executes WASM workflow binaries via wasmtime with WASI HTTP and network support.
/// Output is read from runtara-core persistence after process exit (the SDK reports
/// completion/failure via HTTP to runtara-core during execution).
pub struct WasmRunner {
    config: WasmRunnerConfig,
    persistence: Arc<dyn Persistence>,
}

impl WasmRunner {
    /// Create a new WASM runner.
    pub fn new(config: WasmRunnerConfig, persistence: Arc<dyn Persistence>) -> Self {
        Self {
            config,
            persistence,
        }
    }

    /// Get the data directory.
    pub fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }

    /// Resolve the WASM binary path.
    ///
    /// For WASM images, the path is the binary file directly (not an OCI bundle).
    fn resolve_wasm_path(&self, binary_path: &Path) -> PathBuf {
        binary_path.to_path_buf()
    }

    /// Build environment variables for the workflow process.
    fn build_env(
        &self,
        instance_id: &str,
        tenant_id: &str,
        runtara_core_addr: &str,
        checkpoint_id: Option<&str>,
    ) -> HashMap<String, String> {
        super::common::build_env(
            &self.config,
            instance_id,
            tenant_id,
            runtara_core_addr,
            checkpoint_id,
        )
    }

    /// Build the wasmtime command with all flags.
    fn build_command(&self, wasm_path: &Path, env: &HashMap<String, String>) -> Command {
        let mut cmd = Command::new(&self.config.wasmtime_path);
        configure_wasmtime_process_logging(&mut cmd);

        cmd.arg("run");

        // WASI configuration — HTTP networking only, no filesystem access
        cmd.arg("--wasi").arg("http");
        cmd.arg("--wasi").arg("inherit-network");
        cmd.arg("--wasi")
            .arg("http-outgoing-body-buffer-chunks=4096");
        cmd.arg("--wasi")
            .arg("http-outgoing-body-chunk-size=1048576");
        cmd.arg("--wasi").arg("max-resources=10000000");

        // Pass environment variables via --env flags
        for (key, value) in env {
            cmd.arg("--env").arg(format!("{}={}", key, value));
        }

        // The WASM module to execute
        cmd.arg(wasm_path);

        cmd.stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null());

        cmd
    }

    /// Create run directory for stderr capture.
    async fn ensure_run_dir(&self, tenant_id: &str, instance_id: &str) -> Result<()> {
        super::common::ensure_run_dir(&self.config.data_dir, tenant_id, instance_id).await
    }

    /// Load output from runtara-core persistence.
    ///
    /// The SDK reports completion/failure to runtara-core via HTTP during execution.
    /// By the time the process exits, the instance record is already persisted.
    async fn load_output(&self, instance_id: &str) -> Result<Value> {
        super::common::load_output(self.persistence.as_ref(), instance_id).await
    }

    /// Load stderr from log file for diagnostics.
    async fn load_stderr(&self, tenant_id: &str, instance_id: &str) -> Option<String> {
        super::common::load_stderr(&self.config.data_dir, tenant_id, instance_id).await
    }

    /// Get the run directory for an instance.
    fn run_dir(&self, tenant_id: &str, instance_id: &str) -> PathBuf {
        super::common::run_dir(&self.config.data_dir, tenant_id, instance_id)
    }

    /// Run wasmtime process and wait for exit with timeout and cancellation.
    async fn run_process(
        &self,
        wasm_path: &Path,
        env: &HashMap<String, String>,
        instance_id: &str,
        cancel_token: Option<CancelToken>,
        timeout: Duration,
    ) -> (Result<()>, ContainerMetrics) {
        debug!(
            wasm = %wasm_path.display(),
            instance_id = %instance_id,
            wasmtime = %self.config.wasmtime_path.display(),
            "Launching WASM process via wasmtime"
        );

        let mut cmd = self.build_command(wasm_path, env);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return (
                        Err(RunnerError::BinaryNotFound(format!(
                            "wasmtime not found at: {}",
                            self.config.wasmtime_path.display()
                        ))),
                        ContainerMetrics::default(),
                    );
                }
                return (Err(RunnerError::Io(e)), ContainerMetrics::default());
            }
        };

        let stderr_handle = child.stderr.take();
        let mut metrics = ContainerMetrics::default();
        if let Some(pid) = child.id()
            && let Some(sample) = sample_process_metrics(pid)
        {
            merge_process_metrics(&mut metrics, sample);
        }

        let result = self
            .wait_with_cancellation(
                &mut child,
                instance_id,
                cancel_token,
                timeout,
                stderr_handle,
                &mut metrics,
            )
            .await;

        (result, metrics)
    }

    /// Wait for child process with timeout and cancellation support.
    async fn wait_with_cancellation(
        &self,
        child: &mut tokio::process::Child,
        instance_id: &str,
        cancel_token: Option<CancelToken>,
        timeout_duration: Duration,
        stderr_handle: Option<tokio::process::ChildStderr>,
        metrics: &mut ContainerMetrics,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let poll_interval = Duration::from_millis(100);
        let start = std::time::Instant::now();

        loop {
            if let Some(pid) = child.id()
                && let Some(sample) = sample_process_metrics(pid)
            {
                merge_process_metrics(metrics, sample);
            }

            // Check cancellation
            if let Some(ref flag) = cancel_token
                && flag.load(Ordering::Relaxed)
            {
                warn!(instance_id = %instance_id, "WASM execution cancelled, killing wasmtime process");
                let _ = child.kill().await;
                return Err(RunnerError::Cancelled);
            }

            // Check timeout
            if start.elapsed() > timeout_duration {
                warn!(instance_id = %instance_id, "WASM execution timed out, killing wasmtime process");
                let _ = child.kill().await;
                return Err(RunnerError::Timeout);
            }

            // Try to get exit status (non-blocking)
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        info!(instance_id = %instance_id, "WASM process completed successfully");
                        return Ok(());
                    } else {
                        let exit_code = status.code().unwrap_or(-1);

                        let stderr = if let Some(mut handle) = stderr_handle {
                            let mut buf = String::new();
                            let _ = handle.read_to_string(&mut buf).await;
                            buf.trim().to_string()
                        } else {
                            String::new()
                        };

                        error!(instance_id = %instance_id, exit_code = exit_code, stderr = %stderr, "WASM process failed");
                        return Err(RunnerError::ExitCode { exit_code, stderr });
                    }
                }
                Ok(None) => {
                    tokio::time::sleep(poll_interval).await;
                }
                Err(e) => {
                    error!(instance_id = %instance_id, error = %e, "Error waiting for WASM process");
                    return Err(RunnerError::Io(e));
                }
            }
        }
    }

    /// Launch a detached wasmtime process, returning a handle.
    async fn spawn_detached(
        &self,
        wasm_path: &Path,
        env: &HashMap<String, String>,
        run_dir: &Path,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<RunnerHandle> {
        let log_path = run_dir.join("stderr.log");

        // Open stderr log file
        let stderr_file = match std::fs::File::create(&log_path) {
            Ok(f) => f,
            Err(e) => {
                warn!(
                    instance_id = %instance_id,
                    error = %e,
                    path = %log_path.display(),
                    "Failed to create stderr log file, using null"
                );
                std::fs::File::open("/dev/null")?
            }
        };

        let mut cmd = Command::new(&self.config.wasmtime_path);
        configure_wasmtime_process_logging(&mut cmd);

        cmd.arg("run");

        // WASI configuration — HTTP networking only, no filesystem access
        cmd.arg("--wasi").arg("http");
        cmd.arg("--wasi").arg("inherit-network");
        cmd.arg("--wasi")
            .arg("http-outgoing-body-buffer-chunks=4096");
        cmd.arg("--wasi")
            .arg("http-outgoing-body-chunk-size=1048576");
        cmd.arg("--wasi").arg("max-resources=10000000");

        // Pass environment variables
        for (key, value) in env {
            cmd.arg("--env").arg(format!("{}={}", key, value));
        }

        // The WASM module
        cmd.arg(wasm_path);

        cmd.stderr(std::process::Stdio::from(stderr_file))
            .stdout(std::process::Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RunnerError::BinaryNotFound(format!(
                    "wasmtime not found at: {}",
                    self.config.wasmtime_path.display()
                ))
            } else {
                RunnerError::Io(e)
            }
        })?;

        let spawned_pid = child.id();

        info!(
            instance_id = %instance_id,
            pid = ?spawned_pid,
            wasm = %wasm_path.display(),
            "Launched WASM process via wasmtime (detached)"
        );

        let child_handle = std::sync::Arc::new(tokio::sync::Mutex::new(Some(child)));
        let metrics = std::sync::Arc::new(tokio::sync::Mutex::new(ContainerMetrics::default()));
        if let Some(pid) = spawned_pid {
            spawn_process_metrics_sampler(pid, metrics.clone());
        }

        Ok(RunnerHandle {
            handle_id: format!("wasm_{}", instance_id),
            instance_id: instance_id.to_string(),
            tenant_id: tenant_id.to_string(),
            started_at: chrono::Utc::now(),
            spawned_pid,
            child: Some(child_handle),
            metrics: Some(metrics),
        })
    }
}

#[async_trait]
impl Runner for WasmRunner {
    fn runner_type(&self) -> &'static str {
        "wasm"
    }

    async fn run(
        &self,
        options: &LaunchOptions,
        cancel_token: Option<CancelToken>,
    ) -> Result<LaunchResult> {
        let start = std::time::Instant::now();

        let wasm_path = self.resolve_wasm_path(&options.bundle_path);
        if !wasm_path.exists() {
            return Err(RunnerError::BinaryNotFound(wasm_path.display().to_string()));
        }

        // Build environment variables
        let mut env = self.build_env(
            &options.instance_id,
            &options.tenant_id,
            &options.runtara_core_addr,
            options.checkpoint_id.as_deref(),
        );
        env.extend(options.env.clone());

        // Launch wasmtime and wait for completion
        let (result, metrics) = self
            .run_process(
                &wasm_path,
                &env,
                &options.instance_id,
                cancel_token,
                options.timeout,
            )
            .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(()) => {
                // Process exited successfully — read output from runtara-core persistence.
                // The SDK's completed() call persists before process exit (synchronous HTTP).
                match self.load_output(&options.instance_id).await {
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
                // Process failed — check if the SDK reported an error to runtara-core
                let error_msg = match self.load_output(&options.instance_id).await {
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
        let wasm_path = self.resolve_wasm_path(&options.bundle_path);
        if !wasm_path.exists() {
            return Err(RunnerError::BinaryNotFound(wasm_path.display().to_string()));
        }

        // Create run directory for stderr capture
        self.ensure_run_dir(&options.tenant_id, &options.instance_id)
            .await?;

        let run_dir = self.run_dir(&options.tenant_id, &options.instance_id);

        // Build environment variables
        let mut env = self.build_env(
            &options.instance_id,
            &options.tenant_id,
            &options.runtara_core_addr,
            options.checkpoint_id.as_deref(),
        );
        env.extend(options.env.clone());

        self.spawn_detached(
            &wasm_path,
            &env,
            &run_dir,
            &options.instance_id,
            &options.tenant_id,
        )
        .await
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        if let Some(pid) = handle.spawned_pid {
            // Check if wasmtime process is still alive via kill(pid, 0)
            use nix::sys::signal;
            use nix::unistd::Pid;
            signal::kill(Pid::from_raw(pid as i32), None).is_ok()
        } else {
            false
        }
    }

    async fn wait_for_exit(&self, handle: &RunnerHandle, poll_interval: Duration) {
        // Prefer waiting on the owned Child handle: this blocks until the
        // wasmtime process has fully exited (and stdio has been flushed),
        // which is what the monitor needs before reading SDK-written status.
        //
        // tokio::process::Child::wait is cancel-safe, so dropping this future
        // when the surrounding select! fires the timeout branch is safe.
        // No other code locks `handle.child` after the monitor starts.
        if let Some(child_arc) = handle.child.clone() {
            let mut guard = child_arc.lock().await;
            if let Some(child) = guard.as_mut() {
                match child.wait().await {
                    Ok(status) if status.success() => {
                        debug!(instance_id = %handle.instance_id, "WASM child process exited cleanly");
                    }
                    Ok(status) => {
                        // A non-zero / signalled exit is how a direct workflow
                        // surfaces a hard crash (e.g. `run` returning `Err`, a
                        // trap, or an OOM kill). Record the raw code/signal so the
                        // cause is diagnosable — the workflow component world has
                        // no `wasi:cli/stderr`, so the guest emits no other trace.
                        use std::os::unix::process::ExitStatusExt;
                        warn!(
                            instance_id = %handle.instance_id,
                            code = ?status.code(),
                            signal = ?status.signal(),
                            "WASM child process exited non-zero"
                        );
                    }
                    Err(e) => {
                        warn!(instance_id = %handle.instance_id, error = %e, "WASM child wait() failed");
                    }
                }
            }
            *guard = None;
            return;
        }
        while self.is_running(handle).await {
            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
        if let Some(pid) = handle.spawned_pid {
            debug!(pid = pid, instance_id = %handle.instance_id, "Killing wasmtime process");
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;
            let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
        }
        Ok(())
    }

    async fn collect_result(
        &self,
        handle: &RunnerHandle,
    ) -> (Option<Value>, Option<String>, ContainerMetrics) {
        // Output is read from runtara-core by the container monitor, not from files.
        // collect_result only provides stderr for diagnostics.
        let stderr = self
            .load_stderr(&handle.tenant_id, &handle.instance_id)
            .await;

        let metrics = if let Some(metrics) = &handle.metrics {
            metrics.lock().await.clone()
        } else {
            ContainerMetrics::default()
        };

        (None, stderr, metrics)
    }

    async fn get_pid(&self, handle: &RunnerHandle) -> Option<u32> {
        handle.spawned_pid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasmtime_process_log_filter_defaults_to_warn() {
        assert_eq!(wasmtime_process_log_filter(None), "warn");
        assert_eq!(wasmtime_process_log_filter(Some("   ")), "warn");
    }

    #[test]
    fn wasmtime_process_log_filter_accepts_explicit_override() {
        assert_eq!(wasmtime_process_log_filter(Some("debug")), "debug");
        assert_eq!(
            wasmtime_process_log_filter(Some("wasmtime=trace,cranelift_codegen=trace")),
            "wasmtime=trace,cranelift_codegen=trace"
        );
    }

    #[test]
    fn merge_process_metrics_keeps_memory_peak() {
        let mut metrics = ContainerMetrics {
            memory_peak_bytes: Some(1024),
            memory_current_bytes: Some(1024),
            ..Default::default()
        };

        merge_process_metrics(
            &mut metrics,
            ContainerMetrics {
                memory_peak_bytes: Some(512),
                memory_current_bytes: Some(512),
                ..Default::default()
            },
        );

        assert_eq!(metrics.memory_peak_bytes, Some(1024));
        assert_eq!(metrics.memory_current_bytes, Some(512));

        merge_process_metrics(
            &mut metrics,
            ContainerMetrics {
                memory_peak_bytes: Some(2048),
                memory_current_bytes: Some(2048),
                ..Default::default()
            },
        );

        assert_eq!(metrics.memory_peak_bytes, Some(2048));
        assert_eq!(metrics.memory_current_bytes, Some(2048));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_proc_status_memory_reads_hwm_and_rss() {
        let status = "\
Name:\twasmtime\n\
VmHWM:\t   1234 kB\n\
VmRSS:\t    456 kB\n";

        let (peak, current) = parse_proc_status_memory(status);

        assert_eq!(peak, Some(1234 * 1024));
        assert_eq!(current, Some(456 * 1024));
    }
}
