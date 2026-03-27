// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! WebAssembly runner using wasmtime.
//!
//! Launches WASM scenario binaries via the `wasmtime` CLI with WASI support.
//! Input/output is exchanged via files in the data directory (same as NativeRunner):
//! - Input: {DATA_DIR}/{tenant_id}/runs/{instance_id}/input.json
//! - Output: {DATA_DIR}/{tenant_id}/runs/{instance_id}/output.json

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use crate::runner::{
    CancelToken, ContainerMetrics, LaunchOptions, LaunchResult, Result, Runner, RunnerError,
    RunnerHandle,
};

/// WebAssembly runner configuration.
#[derive(Debug, Clone)]
pub struct WasmRunnerConfig {
    /// Path to the wasmtime binary.
    pub wasmtime_path: PathBuf,
    /// Data directory for instance I/O.
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
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false),
            connection_service_url: std::env::var("RUNTARA_CONNECTION_SERVICE_URL").ok(),
        }
    }
}

/// WebAssembly runner.
///
/// Executes WASM scenario binaries via wasmtime with WASI HTTP and network support.
/// The WASM module gets a preopened `/data` directory mapped to the host data directory
/// for reading input.json and writing output.json.
pub struct WasmRunner {
    config: WasmRunnerConfig,
}

impl WasmRunner {
    /// Create a new WASM runner.
    pub fn new(config: WasmRunnerConfig) -> Self {
        Self { config }
    }

    /// Create from environment variables.
    pub fn from_env() -> Self {
        Self::new(WasmRunnerConfig::from_env())
    }

    /// Get the data directory.
    pub fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }

    /// Resolve the WASM binary path from the bundle path.
    ///
    /// OCI bundles store the binary at `{bundle_path}/rootfs/binary`.
    /// For WASM execution, we expect a `.wasm` file at the same location.
    fn resolve_wasm_path(&self, bundle_path: &Path) -> PathBuf {
        bundle_path.join("rootfs").join("binary")
    }

    /// Build environment variables for the scenario process.
    fn build_env(
        &self,
        instance_id: &str,
        tenant_id: &str,
        runtara_core_addr: &str,
        checkpoint_id: Option<&str>,
        run_dir: &Path,
    ) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("RUNTARA_INSTANCE_ID".to_string(), instance_id.to_string());
        env.insert("RUNTARA_TENANT_ID".to_string(), tenant_id.to_string());
        env.insert(
            "RUNTARA_HTTP_URL".to_string(),
            format!("http://{}", runtara_core_addr),
        );
        env.insert(
            "RUNTARA_SERVER_ADDR".to_string(),
            runtara_core_addr.to_string(),
        );

        // Workspace directory inside the WASM guest filesystem
        env.insert(
            "RUNTARA_WORKSPACE_DIR".to_string(),
            "/data/workspace".to_string(),
        );

        // Input path inside the WASM guest filesystem
        env.insert("SCENARIO_INPUT".to_string(), "/data/input.json".to_string());

        // Output path inside the WASM guest filesystem (for the guest to know where to write)
        env.insert(
            "SCENARIO_OUTPUT".to_string(),
            "/data/output.json".to_string(),
        );

        // Error output path inside the WASM guest filesystem
        env.insert("SCENARIO_ERROR".to_string(), "/data/error.json".to_string());

        if self.config.skip_cert_verification {
            env.insert(
                "RUNTARA_SKIP_CERT_VERIFICATION".to_string(),
                "true".to_string(),
            );
        }
        if let Some(cp_id) = checkpoint_id {
            env.insert("RUNTARA_CHECKPOINT_ID".to_string(), cp_id.to_string());
        }
        if let Some(ref url) = self.config.connection_service_url {
            env.insert("CONNECTION_SERVICE_URL".to_string(), url.clone());
        }

        // Forward SDK backend selection and HTTP URL if set in host environment.
        if let Ok(backend) = std::env::var("RUNTARA_SDK_BACKEND") {
            env.insert("RUNTARA_SDK_BACKEND".to_string(), backend);
        }
        if let Ok(url) = std::env::var("RUNTARA_HTTP_URL") {
            env.insert("RUNTARA_HTTP_URL".to_string(), url);
        }
        if let Ok(port) = std::env::var("RUNTARA_CORE_HTTP_PORT") {
            env.insert("RUNTARA_CORE_HTTP_PORT".to_string(), port);
        }

        // Forward object model internal API URL for smo-stdlib agents
        if let Ok(url) = std::env::var("RUNTARA_OBJECT_MODEL_URL") {
            env.insert("RUNTARA_OBJECT_MODEL_URL".to_string(), url);
        }

        // Forward tenant ID for internal API authentication
        if let Ok(tid) = std::env::var("RUNTARA_TENANT_ID") {
            env.insert("RUNTARA_TENANT_ID".to_string(), tid);
        }

        // Store the host run_dir path for reference (not visible to guest)
        let _ = run_dir;

        env
    }

    /// Build the wasmtime command with all flags.
    fn build_command(
        &self,
        wasm_path: &Path,
        env: &HashMap<String, String>,
        run_dir: &Path,
    ) -> Command {
        let mut cmd = Command::new(&self.config.wasmtime_path);

        cmd.arg("run");

        // WASI configuration
        cmd.arg("--wasi").arg("http");
        cmd.arg("--wasi").arg("inherit-network");

        // Preopened directory: map host run_dir to /data inside the guest
        cmd.arg("--dir")
            .arg(format!("/data::{}", run_dir.display()));

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

    /// Store input in file for instance to read.
    async fn store_input(&self, tenant_id: &str, instance_id: &str, input: &Value) -> Result<()> {
        let run_dir = self
            .config
            .data_dir
            .join(tenant_id)
            .join("runs")
            .join(instance_id);

        fs::create_dir_all(&run_dir).await?;

        // Create workspace directory for ephemeral file storage
        let workspace_dir = run_dir.join("workspace");
        fs::create_dir_all(&workspace_dir).await?;

        let input_path = run_dir.join("input.json");
        let value = serde_json::to_string_pretty(input)?;
        fs::write(&input_path, &value).await?;

        debug!(instance_id = %instance_id, path = %input_path.display(), "Stored input to file");
        Ok(())
    }

    /// Load output from file (written by instance).
    async fn load_output(&self, tenant_id: &str, instance_id: &str) -> Result<Value> {
        let output_path = self
            .config
            .data_dir
            .join(tenant_id)
            .join("runs")
            .join(instance_id)
            .join("output.json");

        match fs::read_to_string(&output_path).await {
            Ok(json) => {
                let output: Value = serde_json::from_str(&json)?;
                Ok(output)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(RunnerError::OutputNotFound(instance_id.to_string()))
            }
            Err(e) => Err(RunnerError::Io(e)),
        }
    }

    /// Load error from error.json or stderr.log file.
    async fn load_error(&self, tenant_id: &str, instance_id: &str) -> Option<String> {
        let run_dir = self
            .config
            .data_dir
            .join(tenant_id)
            .join("runs")
            .join(instance_id);

        // Try error.json first
        let error_path = run_dir.join("error.json");
        if let Ok(json) = fs::read_to_string(&error_path).await
            && let Ok(value) = serde_json::from_str::<Value>(&json)
            && let Some(error) = value.get("error").and_then(|e| e.as_str())
        {
            return Some(error.to_string());
        }

        // Fallback to stderr.log
        let stderr_path = run_dir.join("stderr.log");
        if let Ok(stderr_content) = fs::read_to_string(&stderr_path).await {
            let stderr_trimmed = stderr_content.trim();
            if !stderr_trimmed.is_empty() {
                let lines: Vec<&str> = stderr_trimmed
                    .lines()
                    .filter(|line| {
                        let line_lower = line.to_lowercase();
                        !line_lower.contains("warning:")
                            && !line_lower.starts_with("at ")
                            && !line.trim().is_empty()
                    })
                    .take(10)
                    .collect();

                if !lines.is_empty() {
                    let preview = lines.join("\n");
                    let truncated = if preview.len() > 2000 {
                        format!("{}...", &preview[..2000])
                    } else {
                        preview
                    };
                    return Some(format!("Execution failed:\n{}", truncated));
                }
            }
        }

        None
    }

    /// Get the run directory for an instance.
    fn run_dir(&self, tenant_id: &str, instance_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join(tenant_id)
            .join("runs")
            .join(instance_id)
    }

    /// Run wasmtime process and wait for exit with timeout and cancellation.
    async fn run_process(
        &self,
        wasm_path: &Path,
        env: &HashMap<String, String>,
        run_dir: &Path,
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

        let mut cmd = self.build_command(wasm_path, env, run_dir);

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

        let result = self
            .wait_with_cancellation(
                &mut child,
                instance_id,
                cancel_token,
                timeout,
                stderr_handle,
            )
            .await;

        // No cgroup metrics available for WASM processes
        (result, ContainerMetrics::default())
    }

    /// Wait for child process with timeout and cancellation support.
    async fn wait_with_cancellation(
        &self,
        child: &mut tokio::process::Child,
        instance_id: &str,
        cancel_token: Option<CancelToken>,
        timeout_duration: Duration,
        stderr_handle: Option<tokio::process::ChildStderr>,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let poll_interval = Duration::from_millis(100);
        let start = std::time::Instant::now();

        loop {
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

        cmd.arg("run");

        // WASI configuration
        cmd.arg("--wasi").arg("http");
        cmd.arg("--wasi").arg("inherit-network");

        // Preopened directory
        cmd.arg("--dir")
            .arg(format!("/data::{}", run_dir.display()));

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

        Ok(RunnerHandle {
            handle_id: format!("wasm_{}", instance_id),
            instance_id: instance_id.to_string(),
            tenant_id: tenant_id.to_string(),
            started_at: chrono::Utc::now(),
            spawned_pid,
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

        // Store input to file for the instance to read
        self.store_input(&options.tenant_id, &options.instance_id, &options.input)
            .await?;

        let run_dir = self.run_dir(&options.tenant_id, &options.instance_id);

        // Build environment variables
        let mut env = self.build_env(
            &options.instance_id,
            &options.tenant_id,
            &options.runtara_core_addr,
            options.checkpoint_id.as_deref(),
            &run_dir,
        );
        env.extend(options.env.clone());

        // Launch wasmtime and wait for completion
        let (result, metrics) = self
            .run_process(
                &wasm_path,
                &env,
                &run_dir,
                &options.instance_id,
                cancel_token,
                options.timeout,
            )
            .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(()) => {
                match self
                    .load_output(&options.tenant_id, &options.instance_id)
                    .await
                {
                    Ok(output) => Ok(LaunchResult {
                        instance_id: options.instance_id.clone(),
                        success: true,
                        output: Some(output),
                        error: None,
                        stderr: None,
                        duration_ms,
                        metrics,
                    }),
                    Err(e) => {
                        let stderr = self
                            .load_error(&options.tenant_id, &options.instance_id)
                            .await;
                        Ok(LaunchResult {
                            instance_id: options.instance_id.clone(),
                            success: false,
                            output: None,
                            error: Some(format!("Failed to load output: {}", e)),
                            stderr,
                            duration_ms,
                            metrics,
                        })
                    }
                }
            }
            Err(e) => {
                let stderr = self
                    .load_error(&options.tenant_id, &options.instance_id)
                    .await;
                let error_msg = match &stderr {
                    Some(msg) => msg.clone(),
                    None => e.to_string(),
                };
                Ok(LaunchResult {
                    instance_id: options.instance_id.clone(),
                    success: false,
                    output: None,
                    error: Some(error_msg),
                    stderr,
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

        // Store input to file
        self.store_input(&options.tenant_id, &options.instance_id, &options.input)
            .await?;

        let run_dir = self.run_dir(&options.tenant_id, &options.instance_id);

        // Build environment variables
        let mut env = self.build_env(
            &options.instance_id,
            &options.tenant_id,
            &options.runtara_core_addr,
            options.checkpoint_id.as_deref(),
            &run_dir,
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
        let output = self
            .load_output(&handle.tenant_id, &handle.instance_id)
            .await
            .ok();

        let error = if output.is_none() {
            self.load_error(&handle.tenant_id, &handle.instance_id)
                .await
        } else {
            None
        };

        // No cgroup metrics for WASM processes
        (output, error, ContainerMetrics::default())
    }

    async fn get_pid(&self, handle: &RunnerHandle) -> Option<u32> {
        handle.spawned_pid
    }
}
