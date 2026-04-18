// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native process runner.
//!
//! Launches instance binaries as plain child processes without container isolation.
//! Intended for development on platforms where OCI runtimes are not available (macOS, Windows).
//!
//! Input is provided via files; output is read from runtara-core persistence
//! (the SDK reports completion/failure via HTTP).

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use runtara_core::persistence::Persistence;

use crate::runner::{
    CancelToken, ContainerMetrics, LaunchOptions, LaunchResult, Result, Runner, RunnerError,
    RunnerHandle,
};

/// Native runner configuration.
#[derive(Debug, Clone)]
pub struct NativeRunnerConfig {
    /// Data directory for instance I/O
    pub data_dir: PathBuf,
    /// Default execution timeout
    pub default_timeout: Duration,
    /// Skip TLS certificate verification (passed to instances)
    pub skip_cert_verification: bool,
    /// Connection service URL for fetching credentials at runtime (passed to instances)
    pub connection_service_url: Option<String>,
}

impl NativeRunnerConfig {
    /// Create configuration from environment variables.
    pub fn from_env() -> Self {
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

/// Native process runner.
///
/// Launches scenario binaries as direct child processes. No container isolation,
/// no network namespaces, no filesystem restrictions. The binary runs with the
/// same permissions as the smo-runtime process.
///
/// Use this runner for local development on macOS/Windows where OCI runtimes
/// (crun, runc) are not available.
pub struct NativeRunner {
    config: NativeRunnerConfig,
    persistence: Arc<dyn Persistence>,
}

impl NativeRunner {
    /// Create a new native runner.
    pub fn new(config: NativeRunnerConfig, persistence: Arc<dyn Persistence>) -> Self {
        Self {
            config,
            persistence,
        }
    }

    /// Get the data directory.
    pub fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }

    /// Build environment variables for the scenario process.
    fn build_env(
        &self,
        instance_id: &str,
        tenant_id: &str,
        runtara_core_addr: &str,
        checkpoint_id: Option<&str>,
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
        // This allows scenarios to select the HTTP backend when configured.
        if let Ok(backend) = std::env::var("RUNTARA_SDK_BACKEND") {
            env.insert("RUNTARA_SDK_BACKEND".to_string(), backend);
        }
        if let Ok(url) = std::env::var("RUNTARA_HTTP_URL") {
            env.insert("RUNTARA_HTTP_URL".to_string(), url);
        }
        if let Ok(port) = std::env::var("RUNTARA_CORE_HTTP_PORT") {
            env.insert("RUNTARA_CORE_HTTP_PORT".to_string(), port);
        }

        // RUNTARA_OBJECT_MODEL_URL, RUNTARA_AGENT_SERVICE_URL, RUNTARA_HTTP_PROXY_URL
        // and RUNTARA_TENANT_ID now arrive via LaunchOptions.env (populated by
        // the caller from its typed config) and are merged into `env` by the
        // caller of build_env. The runner no longer reads them from its own
        // process environment.

        env
    }

    /// Resolve the binary path from the bundle path.
    ///
    /// OCI bundles store the binary at `{bundle_path}/rootfs/binary`.
    /// For native execution, we run it directly.
    fn resolve_binary_path(&self, bundle_path: &Path) -> PathBuf {
        bundle_path.join("rootfs").join("binary")
    }

    /// Ensure the run directory exists for stderr capture.
    async fn ensure_run_dir(&self, tenant_id: &str, instance_id: &str) -> Result<()> {
        let run_dir = self
            .config
            .data_dir
            .join(tenant_id)
            .join("runs")
            .join(instance_id);

        fs::create_dir_all(&run_dir).await?;

        debug!(instance_id = %instance_id, "Run directory created");
        Ok(())
    }

    /// Load output from runtara-core persistence.
    ///
    /// The SDK reports completion/failure to runtara-core via HTTP during execution.
    /// By the time the process exits, the instance record is already persisted.
    async fn load_output(&self, instance_id: &str) -> Result<Value> {
        match self.persistence.get_instance(instance_id).await {
            Ok(Some(inst)) => match inst.status.as_str() {
                "completed" => {
                    if let Some(output_bytes) = inst.output {
                        serde_json::from_slice(&output_bytes).map_err(|e| {
                            RunnerError::Other(format!("Failed to parse output: {}", e))
                        })
                    } else {
                        Ok(Value::Null)
                    }
                }
                "failed" => {
                    let error = inst.error.unwrap_or_else(|| "Unknown error".to_string());
                    Err(RunnerError::Other(error))
                }
                "cancelled" => Err(RunnerError::Cancelled),
                status => Err(RunnerError::Other(format!(
                    "Unexpected instance status after exit: {}",
                    status
                ))),
            },
            Ok(None) => Err(RunnerError::OutputNotFound(instance_id.to_string())),
            Err(e) => Err(RunnerError::Other(format!(
                "Failed to query instance status: {}",
                e
            ))),
        }
    }

    /// Load stderr from log file for diagnostics.
    async fn load_stderr(&self, tenant_id: &str, instance_id: &str) -> Option<String> {
        let stderr_path = self
            .config
            .data_dir
            .join(tenant_id)
            .join("runs")
            .join(instance_id)
            .join("stderr.log");

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
                    return Some(truncated);
                }
            }
        }

        None
    }

    /// Run process and wait for exit with timeout and cancellation.
    async fn run_process(
        &self,
        binary_path: &Path,
        env: &HashMap<String, String>,
        instance_id: &str,
        cancel_token: Option<CancelToken>,
        timeout: Duration,
    ) -> (Result<()>, ContainerMetrics) {
        debug!(
            binary = %binary_path.display(),
            instance_id = %instance_id,
            "Launching native process"
        );

        let mut cmd = Command::new(binary_path);

        // Set environment variables
        for (key, value) in env {
            cmd.env(key, value);
        }

        cmd.stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return (
                        Err(RunnerError::BinaryNotFound(
                            binary_path.display().to_string(),
                        )),
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

        // No cgroup metrics available for native processes
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
                warn!(instance_id = %instance_id, "Execution cancelled, killing process");
                let _ = child.kill().await;
                return Err(RunnerError::Cancelled);
            }

            // Check timeout
            if start.elapsed() > timeout_duration {
                warn!(instance_id = %instance_id, "Execution timed out, killing process");
                let _ = child.kill().await;
                return Err(RunnerError::Timeout);
            }

            // Try to get exit status (non-blocking)
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        info!(instance_id = %instance_id, "Process completed successfully");
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

                        error!(instance_id = %instance_id, exit_code = exit_code, stderr = %stderr, "Process failed");
                        return Err(RunnerError::ExitCode { exit_code, stderr });
                    }
                }
                Ok(None) => {
                    tokio::time::sleep(poll_interval).await;
                }
                Err(e) => {
                    error!(instance_id = %instance_id, error = %e, "Error waiting for process");
                    return Err(RunnerError::Io(e));
                }
            }
        }
    }

    /// Launch a detached process, returning a handle.
    async fn spawn_detached(
        &self,
        binary_path: &Path,
        env: &HashMap<String, String>,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<RunnerHandle> {
        let run_dir = self
            .config
            .data_dir
            .join(tenant_id)
            .join("runs")
            .join(instance_id);

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

        let mut cmd = Command::new(binary_path);

        for (key, value) in env {
            cmd.env(key, value);
        }

        cmd.stderr(std::process::Stdio::from(stderr_file))
            .stdout(std::process::Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RunnerError::BinaryNotFound(binary_path.display().to_string())
            } else {
                RunnerError::Io(e)
            }
        })?;

        let spawned_pid = child.id();

        info!(
            instance_id = %instance_id,
            pid = ?spawned_pid,
            binary = %binary_path.display(),
            "Launched native process (detached)"
        );

        Ok(RunnerHandle {
            handle_id: format!("native_{}", instance_id),
            instance_id: instance_id.to_string(),
            tenant_id: tenant_id.to_string(),
            started_at: chrono::Utc::now(),
            spawned_pid,
            child: None,
        })
    }
}

#[async_trait]
impl Runner for NativeRunner {
    fn runner_type(&self) -> &'static str {
        "native"
    }

    async fn run(
        &self,
        options: &LaunchOptions,
        cancel_token: Option<CancelToken>,
    ) -> Result<LaunchResult> {
        let start = std::time::Instant::now();

        let binary_path = self.resolve_binary_path(&options.bundle_path);
        if !binary_path.exists() {
            return Err(RunnerError::BinaryNotFound(
                binary_path.display().to_string(),
            ));
        }

        // Ensure run directory exists for stderr capture
        self.ensure_run_dir(&options.tenant_id, &options.instance_id)
            .await?;

        // Build environment variables
        let mut env = self.build_env(
            &options.instance_id,
            &options.tenant_id,
            &options.runtara_core_addr,
            options.checkpoint_id.as_deref(),
        );
        env.extend(options.env.clone());

        // Launch process and wait for completion
        let (result, metrics) = self
            .run_process(
                &binary_path,
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
        let binary_path = self.resolve_binary_path(&options.bundle_path);
        if !binary_path.exists() {
            return Err(RunnerError::BinaryNotFound(
                binary_path.display().to_string(),
            ));
        }

        // Ensure run directory exists for stderr capture
        self.ensure_run_dir(&options.tenant_id, &options.instance_id)
            .await?;

        // Build environment variables
        let mut env = self.build_env(
            &options.instance_id,
            &options.tenant_id,
            &options.runtara_core_addr,
            options.checkpoint_id.as_deref(),
        );
        env.extend(options.env.clone());

        self.spawn_detached(&binary_path, &env, &options.instance_id, &options.tenant_id)
            .await
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        if let Some(pid) = handle.spawned_pid {
            // Check if process is still alive via kill(pid, 0)
            use nix::sys::signal;
            use nix::unistd::Pid;
            signal::kill(Pid::from_raw(pid as i32), None).is_ok()
        } else {
            false
        }
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
        if let Some(pid) = handle.spawned_pid {
            debug!(pid = pid, instance_id = %handle.instance_id, "Killing native process");
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

        // No cgroup metrics for native processes
        (None, stderr, ContainerMetrics::default())
    }

    async fn get_pid(&self, handle: &RunnerHandle) -> Option<u32> {
        handle.spawned_pid
    }
}
