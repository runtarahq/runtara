// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runner trait definitions.
//!
//! Defines the abstract interface for instance runners.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use thiserror::Error;

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

/// Options for launching an instance.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
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
}

/// Handle for a launched instance (detached execution).
#[derive(Debug, Clone)]
pub struct RunnerHandle {
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
