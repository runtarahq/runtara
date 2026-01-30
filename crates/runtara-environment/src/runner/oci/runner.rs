// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! OCI container runner implementation.
//!
//! Launches instance binaries via crun. Pure execution logic, no database access.
//! Input/output is exchanged via files in the data directory:
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

use super::bundle::{BundleConfig, BundleManager, NetworkMode};
use crate::runner::{
    CancelToken, ContainerMetrics, LaunchOptions, LaunchResult, Result, Runner, RunnerError,
    RunnerHandle,
};

/// Parse an env var into a bool with a sensible default.
fn parse_env_bool(var: &str, default: bool) -> bool {
    std::env::var(var)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

/// Parse network mode from environment variable
fn parse_network_mode(var: &str) -> NetworkMode {
    match std::env::var(var)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "host" => NetworkMode::Host,
        "none" | "isolated" => NetworkMode::None,
        _ => NetworkMode::Pasta, // Default to pasta networking (isolated with NAT)
    }
}

/// Parse DNS servers from environment variable.
/// Format: comma-separated list of IP addresses (e.g., "1.1.1.1,8.8.8.8")
/// This is needed on hosts with systemd-resolved where /etc/resolv.conf contains 127.0.0.53.
fn parse_dns_servers(var: &str) -> Vec<String> {
    std::env::var(var)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Get the host's IP address on the default route interface.
/// Note: With the new `pasta --config-net` approach, this is no longer needed
/// for address transformation, but kept for tests and potential future use.
#[allow(dead_code)]
fn get_host_ip() -> Option<String> {
    // Parse `ip route` output to find the source IP for the default route
    // Format: "default via 192.168.1.1 dev eth0 proto dhcp src 192.168.1.95 ..."
    let output = std::process::Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        // Look for "src" keyword and get the IP after it
        for (i, part) in parts.iter().enumerate() {
            if *part == "src" && i + 1 < parts.len() {
                return Some(parts[i + 1].to_string());
            }
        }
    }
    None
}

/// Transform address for pasta networking
///
/// IMPORTANT: When using pasta networking (NetworkMode::Pasta), the RUNTARA_CORE_ADDR
/// must be set to an IP address reachable from inside containers. Localhost addresses
/// (127.0.0.1) will NOT work because they refer to the container's own loopback, not
/// the host's.
///
/// Correct configuration for pasta networking:
///   RUNTARA_CORE_ADDR=192.168.1.100:8001  (host's actual IP)
///
/// Incorrect (won't work with pasta):
///   RUNTARA_CORE_ADDR=127.0.0.1:8001      (localhost - unreachable from container)
///
/// This function is kept for potential future use but currently returns the address
/// unchanged. Address configuration is the operator's responsibility.
#[allow(dead_code)]
fn transform_addr_for_pasta(addr: &str) -> String {
    addr.to_string()
}

async fn read_cgroup_value(path: &str) -> Option<u64> {
    tokio::fs::read_to_string(path)
        .await
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// OCI runner configuration
#[derive(Debug, Clone)]
pub struct OciRunnerConfig {
    /// Directory for OCI bundles
    pub bundles_dir: PathBuf,
    /// Data directory for instance I/O
    pub data_dir: PathBuf,
    /// Default execution timeout
    pub default_timeout: Duration,
    /// Whether to use systemd for cgroup management
    pub use_systemd_cgroup: bool,
    /// Bundle configuration
    pub bundle_config: BundleConfig,
    /// Skip TLS certificate verification (passed to instances)
    pub skip_cert_verification: bool,
    /// Connection service URL for fetching credentials at runtime (passed to instances)
    pub connection_service_url: Option<String>,
}

impl OciRunnerConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        // Convert to absolute path for OCI container mounts
        let data_dir_raw =
            PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string()));
        let data_dir = if data_dir_raw.is_absolute() {
            data_dir_raw
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&data_dir_raw))
                .unwrap_or(data_dir_raw)
        };
        let default_bundles_dir = data_dir.join("bundles");

        Self {
            bundles_dir: std::env::var("BUNDLES_DIR")
                .map(PathBuf::from)
                .unwrap_or(default_bundles_dir),
            data_dir,
            default_timeout: Duration::from_secs(
                std::env::var("EXECUTION_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(300),
            ),
            use_systemd_cgroup: parse_env_bool("USE_SYSTEMD_CGROUP", false),
            bundle_config: BundleConfig {
                network_mode: parse_network_mode("RUNTARA_NETWORK_MODE"),
                dns_servers: parse_dns_servers("RUNTARA_PASTA_DNS"),
                ..Default::default()
            },
            skip_cert_verification: parse_env_bool("RUNTARA_SKIP_CERT_VERIFICATION", false),
            connection_service_url: std::env::var("RUNTARA_CONNECTION_SERVICE_URL").ok(),
        }
    }
}

#[derive(Debug, Clone)]
enum CgroupLocation {
    V2 {
        unified_path: String,
    },
    V1 {
        memory_path: Option<String>,
        cpu_path: Option<String>,
    },
}

/// OCI container runner using crun.
pub struct OciRunner {
    config: OciRunnerConfig,
    bundle_manager: BundleManager,
}

impl OciRunner {
    /// Create a new OCI runner
    pub fn new(config: OciRunnerConfig) -> Self {
        let bundle_manager =
            BundleManager::new(config.bundles_dir.clone(), config.bundle_config.clone());

        Self {
            config,
            bundle_manager,
        }
    }

    /// Create from environment variables
    pub fn from_env() -> Self {
        Self::new(OciRunnerConfig::from_env())
    }

    /// Get the bundle manager
    pub fn bundle_manager(&self) -> &BundleManager {
        &self.bundle_manager
    }

    /// Get the data directory
    pub fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }

    /// Build environment variables for container
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
        // Workspace directory for ephemeral file storage (inside container)
        env.insert(
            "RUNTARA_WORKSPACE_DIR".to_string(),
            "/data/workspace".to_string(),
        );

        // For pasta networking, transform localhost addresses to gateway address
        // so the container can reach the host via pasta's NAT
        let server_addr = if matches!(self.config.bundle_config.network_mode, NetworkMode::Pasta) {
            let transformed = transform_addr_for_pasta(runtara_core_addr);
            if transformed != runtara_core_addr {
                debug!(
                    original = %runtara_core_addr,
                    transformed = %transformed,
                    "Transformed server address for pasta networking"
                );
            }
            transformed
        } else {
            runtara_core_addr.to_string()
        };

        env.insert("RUNTARA_SERVER_ADDR".to_string(), server_addr);
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
        env
    }

    /// Generate container ID from instance ID
    fn container_id(&self, instance_id: &str) -> String {
        format!("runtara_{}", &instance_id[..8.min(instance_id.len())])
    }

    /// Store input in file for instance to read
    async fn store_input(&self, tenant_id: &str, instance_id: &str, input: &Value) -> Result<()> {
        fs::create_dir_all(&self.config.data_dir).await?;

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

        // Set run directory permissions to allow container (running as nobody) to write
        // This is needed because the container runs as UID 65534 (nobody)
        let (uid, _gid) = self.config.bundle_config.user;
        if uid != 0 {
            use std::os::unix::fs::PermissionsExt;
            // Make directory world-writable so container can write output.json
            std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o777))?;
            std::fs::set_permissions(&workspace_dir, std::fs::Permissions::from_mode(0o777))?;
        }

        let input_path = run_dir.join("input.json");
        let value = serde_json::to_string_pretty(input)?;
        fs::write(&input_path, &value).await?;

        debug!(instance_id = %instance_id, path = %input_path.display(), "Stored input to file");
        Ok(())
    }

    /// Load output from file (written by instance)
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

    /// Load error from error.json file
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

    // NOTE: Run directory cleanup is now handled by the CleanupWorker in cleanup_worker.rs
    // which runs periodically and removes directories older than 24 hours.
    // This prevents race conditions where cleanup happens before output.json can be read.

    /// Run crun container and wait for exit
    ///
    /// Uses the shared image bundle but with a per-instance config.json file.
    /// For Pasta network mode, pasta wraps crun to provide networking.
    async fn run_container(
        &self,
        bundle_path: &Path,
        config_path: &Path,
        instance_id: &str,
        cancel_token: Option<CancelToken>,
        timeout: Duration,
    ) -> (Result<()>, ContainerMetrics) {
        let container_id = self.container_id(instance_id);
        let use_pasta = matches!(self.config.bundle_config.network_mode, NetworkMode::Pasta);

        debug!(
            bundle_path = %bundle_path.display(),
            instance_id = %instance_id,
            container_id = %container_id,
            network_mode = ?self.config.bundle_config.network_mode,
            "Launching container"
        );

        // For pasta mode: pasta runs crun (pasta creates the namespace, crun runs inside it)
        // For other modes: just run crun directly
        let (mut child, stderr_handle) = if use_pasta {
            // Pasta wraps crun: `pasta -- crun run --bundle ... container_id`
            // This way pasta creates and configures the network namespace,
            // and crun runs inside that namespace.
            let mut cmd = Command::new("pasta");
            cmd.arg("--config-net"); // Configure the tap interface
            // Add custom DNS servers if configured (needed for systemd-resolved hosts)
            for dns in &self.config.bundle_config.dns_servers {
                cmd.arg("--dns").arg(dns);
            }
            cmd.arg("--");
            cmd.arg("crun");
            cmd.arg("run");
            if self.config.use_systemd_cgroup {
                cmd.arg("--systemd-cgroup");
            }
            cmd.arg("--bundle").arg(bundle_path);
            cmd.arg("--config").arg(config_path);
            cmd.arg(&container_id);
            cmd.stderr(std::process::Stdio::piped());

            match cmd.spawn() {
                Ok(mut c) => {
                    let stderr = c.stderr.take();
                    (c, stderr)
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        // Pasta not found - fall back to running without network isolation
                        warn!(
                            instance_id = %instance_id,
                            "pasta not found - falling back to host networking"
                        );
                        let mut cmd = Command::new("crun");
                        cmd.arg("run");
                        if self.config.use_systemd_cgroup {
                            cmd.arg("--systemd-cgroup");
                        }
                        cmd.arg("--bundle").arg(bundle_path);
                        cmd.arg("--config").arg(config_path);
                        cmd.arg(&container_id);
                        cmd.stderr(std::process::Stdio::piped());

                        match cmd.spawn() {
                            Ok(mut c) => {
                                let stderr = c.stderr.take();
                                (c, stderr)
                            }
                            Err(e) => {
                                return (Err(RunnerError::Io(e)), ContainerMetrics::default());
                            }
                        }
                    } else {
                        return (Err(RunnerError::Io(e)), ContainerMetrics::default());
                    }
                }
            }
        } else {
            // Direct run for Host and None modes
            let mut cmd = Command::new("crun");
            cmd.arg("run");
            if self.config.use_systemd_cgroup {
                cmd.arg("--systemd-cgroup");
            }
            cmd.arg("--bundle").arg(bundle_path);
            cmd.arg("--config").arg(config_path);
            cmd.arg(&container_id);
            cmd.stderr(std::process::Stdio::piped());

            match cmd.spawn() {
                Ok(mut c) => {
                    let stderr = c.stderr.take();
                    (c, stderr)
                }
                Err(e) => return (Err(RunnerError::Io(e)), ContainerMetrics::default()),
            }
        };

        // Wait for completion with timeout and cancellation check
        let result = self
            .wait_with_cancellation(
                &mut child,
                &container_id,
                cancel_token,
                timeout,
                stderr_handle,
            )
            .await;

        // Collect metrics BEFORE deleting the container
        let metrics = self.collect_container_metrics(&container_id).await;

        // Note: When using pasta, it wraps crun as a child process, so when crun exits,
        // pasta also exits automatically. No separate cleanup needed.

        // Always try to clean up container
        let _ = self.delete_container(&container_id).await;

        (result, metrics)
    }

    /// Wait for child process with timeout and cancellation support
    async fn wait_with_cancellation(
        &self,
        child: &mut tokio::process::Child,
        container_id: &str,
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
                warn!(container_id = %container_id, "Execution cancelled, killing container");
                let _ = self.kill_container(container_id).await;
                return Err(RunnerError::Cancelled);
            }

            // Check timeout
            if start.elapsed() > timeout_duration {
                warn!(container_id = %container_id, "Execution timed out, killing container");
                let _ = self.kill_container(container_id).await;
                return Err(RunnerError::Timeout);
            }

            // Try to get exit status (non-blocking)
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        info!(container_id = %container_id, "Container completed successfully");
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

                        error!(container_id = %container_id, exit_code = exit_code, stderr = %stderr, "Container failed");
                        return Err(RunnerError::ExitCode { exit_code, stderr });
                    }
                }
                Ok(None) => {
                    tokio::time::sleep(poll_interval).await;
                }
                Err(e) => {
                    error!(container_id = %container_id, error = %e, "Error waiting for container");
                    return Err(RunnerError::Io(e));
                }
            }
        }
    }

    /// Kill a running container
    async fn kill_container(&self, container_id: &str) -> Result<()> {
        let _ = Command::new("crun")
            .args(["kill", container_id, "SIGKILL"])
            .output()
            .await;
        Ok(())
    }

    /// Delete a container
    async fn delete_container(&self, container_id: &str) -> Result<()> {
        let _ = Command::new("crun")
            .args(["delete", "--force", container_id])
            .output()
            .await;
        Ok(())
    }

    /// Get the cgroup path(s) for a container from crun state
    async fn get_container_cgroup_paths(&self, container_id: &str) -> Option<CgroupLocation> {
        let output = Command::new("crun")
            .args(["state", container_id])
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let state: Value = serde_json::from_slice(&output.stdout).ok()?;
        let pid = state.get("pid")?.as_u64()?;
        let cgroup_info = tokio::fs::read_to_string(format!("/proc/{}/cgroup", pid))
            .await
            .ok()?;

        // cgroups v2 format: "0::/path/to/cgroup"
        for line in cgroup_info.lines() {
            if let Some(cgroup_path) = line.strip_prefix("0::") {
                return Some(CgroupLocation::V2 {
                    unified_path: format!("/sys/fs/cgroup{}", cgroup_path),
                });
            }
        }

        // cgroups v1 / hybrid
        let mut memory_path = None;
        let mut cpu_path = None;

        for line in cgroup_info.lines() {
            let mut parts = line.splitn(3, ':');
            let _hierarchy = parts.next();
            let controllers = match parts.next() {
                Some(c) => c,
                None => continue,
            };
            let path = parts.next().unwrap_or_default();

            for controller in controllers.split(',') {
                match controller {
                    "memory" => {
                        memory_path = Some(format!("/sys/fs/cgroup/{}{}", controllers, path));
                    }
                    "cpu" | "cpuacct" => {
                        if cpu_path.is_none() {
                            cpu_path = Some(format!("/sys/fs/cgroup/{}{}", controllers, path));
                        }
                    }
                    _ => {}
                }
            }
        }

        if memory_path.is_some() || cpu_path.is_some() {
            Some(CgroupLocation::V1 {
                memory_path,
                cpu_path,
            })
        } else {
            None
        }
    }

    /// Collect resource metrics from container cgroup
    async fn collect_container_metrics(&self, container_id: &str) -> ContainerMetrics {
        let mut metrics = ContainerMetrics::default();

        let Some(cgroup_paths) = self.get_container_cgroup_paths(container_id).await else {
            debug!(container_id = %container_id, "Could not determine cgroup path for metrics");
            return metrics;
        };

        match cgroup_paths {
            CgroupLocation::V2 { unified_path } => {
                if let Some(bytes) =
                    read_cgroup_value(&format!("{}/memory.peak", unified_path)).await
                {
                    metrics.memory_peak_bytes = Some(bytes);
                }
                if let Some(bytes) =
                    read_cgroup_value(&format!("{}/memory.current", unified_path)).await
                {
                    metrics.memory_current_bytes = Some(bytes);
                }

                if let Ok(content) =
                    tokio::fs::read_to_string(format!("{}/cpu.stat", unified_path)).await
                {
                    for line in content.lines() {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() == 2
                            && let Ok(value) = parts[1].parse::<u64>()
                        {
                            match parts[0] {
                                "usage_usec" => metrics.cpu_usage_usec = Some(value),
                                "user_usec" => metrics.cpu_user_usec = Some(value),
                                "system_usec" => metrics.cpu_system_usec = Some(value),
                                _ => {}
                            }
                        }
                    }
                }
            }
            CgroupLocation::V1 {
                memory_path,
                cpu_path,
            } => {
                if let Some(path) = memory_path {
                    if let Some(bytes) =
                        read_cgroup_value(&format!("{}/memory.max_usage_in_bytes", path)).await
                    {
                        metrics.memory_peak_bytes = Some(bytes);
                    }
                    if let Some(bytes) =
                        read_cgroup_value(&format!("{}/memory.usage_in_bytes", path)).await
                    {
                        metrics.memory_current_bytes = Some(bytes);
                    }
                }

                if let Some(path) = cpu_path
                    && let Some(usage_ns) =
                        read_cgroup_value(&format!("{}/cpuacct.usage", path)).await
                {
                    metrics.cpu_usage_usec = Some(usage_ns / 1_000);
                }
            }
        }

        info!(
            container_id = %container_id,
            memory_peak_mb = ?metrics.memory_peak_bytes.map(|b| b / 1024 / 1024),
            cpu_usage_ms = ?metrics.cpu_usage_usec.map(|u| u / 1000),
            "Collected container metrics"
        );

        metrics
    }

    /// Check if a container is running via crun state
    pub async fn is_container_running(&self, container_id: &str) -> bool {
        let output = Command::new("crun")
            .args(["state", container_id])
            .output()
            .await;

        match output {
            Ok(out) => {
                if !out.status.success() {
                    return false;
                }
                if let Ok(state) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
                    && let Some(status) = state.get("status").and_then(|v| v.as_str())
                {
                    return status == "running" || status == "created";
                }
                false
            }
            Err(_) => false,
        }
    }

    /// Get container PID from crun state
    pub async fn get_container_pid(&self, container_id: &str) -> Option<u32> {
        let output = Command::new("crun")
            .args(["state", container_id])
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let state: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        state.get("pid")?.as_u64().map(|p| p as u32)
    }
}

#[async_trait]
impl Runner for OciRunner {
    fn runner_type(&self) -> &'static str {
        "oci"
    }

    async fn run(
        &self,
        options: &LaunchOptions,
        cancel_token: Option<CancelToken>,
    ) -> Result<LaunchResult> {
        let start = std::time::Instant::now();

        // Check bundle exists (shared per-image)
        if !options.bundle_path.exists() {
            return Err(RunnerError::BundleNotFound(
                options.bundle_path.display().to_string(),
            ));
        }

        // Store input to file for the instance to read
        self.store_input(&options.tenant_id, &options.instance_id, &options.input)
            .await?;

        // Build environment variables for the instance
        let mut env = self.build_env(
            &options.instance_id,
            &options.tenant_id,
            &options.runtara_core_addr,
            options.checkpoint_id.as_deref(),
        );
        // Apply custom environment variables (override system vars)
        env.extend(options.env.clone());

        // Generate per-instance config.json in the run directory
        let run_dir = self
            .config
            .data_dir
            .join(&options.tenant_id)
            .join("runs")
            .join(&options.instance_id);
        let config_path = run_dir.join("config.json");
        self.bundle_manager
            .write_config_to_path(&config_path, &env, &run_dir, None)?;

        // Launch container and wait for completion (using shared bundle + per-instance config)
        let (result, metrics) = self
            .run_container(
                &options.bundle_path,
                &config_path,
                &options.instance_id,
                cancel_token,
                options.timeout,
            )
            .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        // NOTE: Run directory cleanup is handled by a separate background worker
        // after 24 hours, not immediately.
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
                        // Container exited but didn't produce output - check stderr for diagnostics
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
                // Container execution failed - get stderr for diagnostics
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
        // Check bundle exists (shared per-image)
        if !options.bundle_path.exists() {
            return Err(RunnerError::BundleNotFound(
                options.bundle_path.display().to_string(),
            ));
        }

        // Store input to file
        self.store_input(&options.tenant_id, &options.instance_id, &options.input)
            .await?;

        // Build environment variables
        let mut env = self.build_env(
            &options.instance_id,
            &options.tenant_id,
            &options.runtara_core_addr,
            options.checkpoint_id.as_deref(),
        );
        // Apply custom environment variables (override system vars)
        env.extend(options.env.clone());

        // Generate container ID
        let container_id = self.container_id(&options.instance_id);

        let now = chrono::Utc::now();

        // Build log file path for stderr redirection
        let run_dir = self
            .config
            .data_dir
            .join(&options.tenant_id)
            .join("runs")
            .join(&options.instance_id);
        let log_path = run_dir.join("stderr.log");
        let log_path_str = log_path.to_string_lossy().to_string();

        // Generate per-instance config.json in the run directory
        let config_path = run_dir.join("config.json");
        self.bundle_manager.write_config_to_path(
            &config_path,
            &env,
            &run_dir,
            Some(&log_path_str),
        )?;

        let use_pasta = matches!(self.config.bundle_config.network_mode, NetworkMode::Pasta);

        // Open stderr log file for the spawned process.
        // IMPORTANT: We redirect stderr to a file instead of using Stdio::piped() because
        // when the Child handle is dropped (after try_wait), the pipe's read end closes.
        // If pasta/crun then writes to stderr (e.g., "No routable interface for IPv6"),
        // it receives SIGPIPE and exits with code 101 before the container completes.
        // Using a file avoids this issue while still preserving stderr for debugging.
        let stderr_file = match std::fs::File::create(&log_path) {
            Ok(f) => f,
            Err(e) => {
                warn!(
                    instance_id = %options.instance_id,
                    error = %e,
                    path = %log_path.display(),
                    "Failed to create stderr log file, using null"
                );
                // Fall back to null if we can't create the file
                std::fs::File::open("/dev/null")?
            }
        };

        // Helper closure to read stderr from the log file for error messages
        let read_stderr_log = || -> String {
            std::fs::read_to_string(&log_path)
                .unwrap_or_default()
                .trim()
                .to_string()
        };

        // For pasta mode: pasta wraps crun (creates namespace, then crun runs inside)
        // For other modes: just run crun directly
        if use_pasta {
            // Pasta wraps crun: `pasta -- crun run --bundle ... container_id`
            let mut cmd = Command::new("pasta");
            cmd.arg("--config-net"); // Configure the tap interface
            // Add custom DNS servers if configured (needed for systemd-resolved hosts)
            for dns in &self.config.bundle_config.dns_servers {
                cmd.arg("--dns").arg(dns);
            }
            cmd.arg("--");
            cmd.arg("crun");
            cmd.arg("run");
            if self.config.use_systemd_cgroup {
                cmd.arg("--systemd-cgroup");
            }
            cmd.arg("--bundle")
                .arg(&options.bundle_path)
                .arg("--config")
                .arg(&config_path)
                .arg(&container_id)
                .stderr(std::process::Stdio::from(stderr_file.try_clone()?))
                .stdout(std::process::Stdio::null());

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        // Pasta not found - fall back to running without network isolation
                        warn!(
                            instance_id = %options.instance_id,
                            "pasta not found - falling back to host networking"
                        );
                        // Fall through to the else branch logic
                        let mut cmd = Command::new("crun");
                        cmd.arg("run");
                        if self.config.use_systemd_cgroup {
                            cmd.arg("--systemd-cgroup");
                        }
                        cmd.arg("--bundle")
                            .arg(&options.bundle_path)
                            .arg("--config")
                            .arg(&config_path)
                            .arg(&container_id)
                            .stderr(std::process::Stdio::from(stderr_file.try_clone()?))
                            .stdout(std::process::Stdio::null());

                        cmd.spawn()?
                    } else {
                        return Err(RunnerError::Io(e));
                    }
                }
            };

            // Check for immediate startup failures
            match child.try_wait() {
                Ok(Some(status)) if !status.success() => {
                    let scenario_error = self
                        .load_error(&options.tenant_id, &options.instance_id)
                        .await;

                    let error_msg = if let Some(err) = scenario_error {
                        err
                    } else {
                        let stderr_msg = read_stderr_log();
                        if stderr_msg.is_empty() {
                            format!("pasta/crun exited with status: {}", status)
                        } else {
                            format!("pasta/crun failed: {}", stderr_msg)
                        }
                    };

                    error!(
                        container_id = %container_id,
                        instance_id = %options.instance_id,
                        error = %error_msg,
                        "Container failed to start"
                    );
                    return Err(RunnerError::StartFailed(error_msg));
                }
                Ok(Some(_status)) => {
                    info!(
                        container_id = %container_id,
                        instance_id = %options.instance_id,
                        "Container completed immediately"
                    );
                }
                Ok(None) => {
                    info!(
                        container_id = %container_id,
                        instance_id = %options.instance_id,
                        bundle_path = %options.bundle_path.display(),
                        network_mode = "pasta",
                        "Launched container (detached) with pasta networking"
                    );
                }
                Err(e) => {
                    warn!(
                        container_id = %container_id,
                        instance_id = %options.instance_id,
                        error = %e,
                        "Could not check pasta/crun status"
                    );
                }
            }
        } else {
            // Spawn crun with shared bundle + per-instance config
            let mut cmd = Command::new("crun");
            cmd.arg("run");
            if self.config.use_systemd_cgroup {
                cmd.arg("--systemd-cgroup");
            }
            cmd.arg("--bundle")
                .arg(&options.bundle_path)
                .arg("--config")
                .arg(&config_path)
                .arg(&container_id)
                .stderr(std::process::Stdio::from(stderr_file))
                .stdout(std::process::Stdio::null());

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => {
                    return Err(RunnerError::Io(e));
                }
            };

            // Check for immediate startup failures (no delay needed)
            match child.try_wait() {
                Ok(Some(status)) if !status.success() => {
                    let scenario_error = self
                        .load_error(&options.tenant_id, &options.instance_id)
                        .await;

                    let error_msg = if let Some(err) = scenario_error {
                        err
                    } else {
                        let stderr_msg = read_stderr_log();
                        if stderr_msg.is_empty() {
                            format!("crun exited with status: {}", status)
                        } else {
                            format!("crun failed: {}", stderr_msg)
                        }
                    };

                    error!(
                        container_id = %container_id,
                        instance_id = %options.instance_id,
                        error = %error_msg,
                        "Container failed to start"
                    );
                    return Err(RunnerError::StartFailed(error_msg));
                }
                Ok(Some(_status)) => {
                    info!(
                        container_id = %container_id,
                        instance_id = %options.instance_id,
                        "Container completed immediately"
                    );
                }
                Ok(None) => {
                    info!(
                        container_id = %container_id,
                        instance_id = %options.instance_id,
                        bundle_path = %options.bundle_path.display(),
                        "Launched container (detached)"
                    );
                }
                Err(e) => {
                    warn!(
                        container_id = %container_id,
                        instance_id = %options.instance_id,
                        error = %e,
                        "Could not check crun status"
                    );
                }
            }
        }

        Ok(RunnerHandle {
            handle_id: container_id,
            instance_id: options.instance_id.clone(),
            tenant_id: options.tenant_id.clone(),
            started_at: now,
        })
    }

    async fn is_running(&self, handle: &RunnerHandle) -> bool {
        self.is_container_running(&handle.handle_id).await
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<()> {
        self.kill_container(&handle.handle_id).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.delete_container(&handle.handle_id).await?;
        Ok(())
    }

    async fn collect_result(
        &self,
        handle: &RunnerHandle,
    ) -> (Option<Value>, Option<String>, ContainerMetrics) {
        let metrics = self.collect_container_metrics(&handle.handle_id).await;
        let _ = self.delete_container(&handle.handle_id).await;
        let output = self
            .load_output(&handle.tenant_id, &handle.instance_id)
            .await
            .ok();
        let error = self
            .load_error(&handle.tenant_id, &handle.instance_id)
            .await;
        // NOTE: Run directory cleanup is handled by a separate background worker
        // after 24 hours, not immediately. This allows output.json to be read
        // by the container monitor's process_output function.
        (output, error, metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_transform_addr_for_pasta_no_transformation() {
        // With pasta --config-net, addresses are passed through unchanged
        // because pasta handles localhost translation automatically
        assert_eq!(transform_addr_for_pasta("127.0.0.1:8001"), "127.0.0.1:8001");
        assert_eq!(transform_addr_for_pasta("localhost:8001"), "localhost:8001");
        assert_eq!(
            transform_addr_for_pasta("192.168.1.100:8001"),
            "192.168.1.100:8001"
        );
        assert_eq!(transform_addr_for_pasta("10.0.0.1:9000"), "10.0.0.1:9000");
    }

    #[test]
    fn test_get_host_ip() {
        // This function should return a valid IP if network is configured
        // It's optional and may return None on some systems
        if let Some(host_ip) = get_host_ip() {
            // Verify it looks like an IP address (basic check)
            assert!(host_ip.contains('.'));
            assert!(!host_ip.contains("127.0.0.1")); // Should not be localhost
        }
    }

    #[tokio::test]
    async fn test_store_input_creates_workspace_directory() {
        // Create a temp directory for test data
        let temp_dir = TempDir::new().unwrap();

        // Create runner with test config
        let config = OciRunnerConfig {
            bundles_dir: temp_dir.path().join("bundles"),
            data_dir: temp_dir.path().to_path_buf(),
            default_timeout: Duration::from_secs(60),
            use_systemd_cgroup: false,
            bundle_config: BundleConfig::default(),
            skip_cert_verification: false,
            connection_service_url: None,
        };
        let runner = OciRunner::new(config);

        // Call store_input
        let tenant_id = "test-tenant";
        let instance_id = "test-instance";
        let input = serde_json::json!({"key": "value"});

        runner
            .store_input(tenant_id, instance_id, &input)
            .await
            .unwrap();

        // Verify run directory exists
        let run_dir = temp_dir
            .path()
            .join(tenant_id)
            .join("runs")
            .join(instance_id);
        assert!(run_dir.exists(), "Run directory should exist");

        // Verify workspace directory exists
        let workspace_dir = run_dir.join("workspace");
        assert!(
            workspace_dir.exists(),
            "Workspace directory should exist at {:?}",
            workspace_dir
        );
        assert!(workspace_dir.is_dir(), "Workspace should be a directory");

        // Verify input.json exists
        let input_path = run_dir.join("input.json");
        assert!(input_path.exists(), "input.json should exist");

        // Verify input.json content
        let content = std::fs::read_to_string(&input_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed, input);
    }

    #[tokio::test]
    async fn test_store_input_workspace_directory_is_writable() {
        // Create a temp directory for test data
        let temp_dir = TempDir::new().unwrap();

        // Create runner with non-root user config (simulates container user)
        let bundle_config = BundleConfig {
            user: (65534, 65534), // nobody user
            ..Default::default()
        };

        let config = OciRunnerConfig {
            bundles_dir: temp_dir.path().join("bundles"),
            data_dir: temp_dir.path().to_path_buf(),
            default_timeout: Duration::from_secs(60),
            use_systemd_cgroup: false,
            bundle_config,
            skip_cert_verification: false,
            connection_service_url: None,
        };
        let runner = OciRunner::new(config);

        // Call store_input
        let tenant_id = "test-tenant";
        let instance_id = "test-instance-2";
        let input = serde_json::json!({});

        runner
            .store_input(tenant_id, instance_id, &input)
            .await
            .unwrap();

        // Verify workspace directory has world-writable permissions
        let workspace_dir = temp_dir
            .path()
            .join(tenant_id)
            .join("runs")
            .join(instance_id)
            .join("workspace");

        assert!(workspace_dir.exists());

        // Check permissions (0o777 = world-writable)
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&workspace_dir).unwrap();
        let mode = metadata.permissions().mode();
        // Check that the directory is world-writable (last 3 bits are rwx for others)
        assert_eq!(
            mode & 0o777,
            0o777,
            "Workspace directory should be world-writable (0o777), got {:o}",
            mode & 0o777
        );
    }

    #[tokio::test]
    async fn test_store_input_preserves_existing_workspace_files() {
        // Create a temp directory for test data
        let temp_dir = TempDir::new().unwrap();

        let config = OciRunnerConfig {
            bundles_dir: temp_dir.path().join("bundles"),
            data_dir: temp_dir.path().to_path_buf(),
            default_timeout: Duration::from_secs(60),
            use_systemd_cgroup: false,
            bundle_config: BundleConfig::default(),
            skip_cert_verification: false,
            connection_service_url: None,
        };
        let runner = OciRunner::new(config);

        let tenant_id = "test-tenant";
        let instance_id = "test-instance-3";

        // First call to store_input
        runner
            .store_input(tenant_id, instance_id, &serde_json::json!({"step": 1}))
            .await
            .unwrap();

        // Write a file to workspace
        let workspace_dir = temp_dir
            .path()
            .join(tenant_id)
            .join("runs")
            .join(instance_id)
            .join("workspace");
        let test_file = workspace_dir.join("test_data.txt");
        std::fs::write(&test_file, "important data").unwrap();

        // Second call to store_input (simulates restart)
        runner
            .store_input(tenant_id, instance_id, &serde_json::json!({"step": 2}))
            .await
            .unwrap();

        // Verify the file still exists
        assert!(
            test_file.exists(),
            "Files in workspace should survive store_input calls"
        );
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "important data");

        // Verify input.json was updated
        let input_path = temp_dir
            .path()
            .join(tenant_id)
            .join("runs")
            .join(instance_id)
            .join("input.json");
        let input_content = std::fs::read_to_string(&input_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&input_content).unwrap();
        assert_eq!(parsed["step"], 2);
    }
}
