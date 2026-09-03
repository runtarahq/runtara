//! Runtime Client
//!
//! The server's view of workflow execution: start, stop, signal, and read back
//! instances, events and step summaries.
//!
//! runtara-environment runs in this process, so every call below reaches it
//! through [`EnvironmentClient`] as a direct function call. There is no socket
//! and nothing to connect to — a `RuntimeClient` exists exactly when the
//! embedded runtime does.

use std::sync::Arc;

use crate::environment_client::{EnvironmentClient, EnvironmentError};
use crate::runtime_types::{ListInstancesOptions, StartInstanceOptions};
use runtara_environment::execution_timeout::{ExecutionTimeoutPolicy, ExecutionTimeoutSeconds};
use runtara_environment::handlers::EnvironmentHandlerState;
use runtara_environment::launch_queue::SINGLE_INSTANCE_LAUNCH_ENV;
use serde_json::Value;

// Re-export types from the SDK for use by other modules
pub use crate::runtime_types::{
    GetTenantMetricsOptions, InstanceInfo, InstanceStatus, InstanceSummary, ListInstancesResult,
    MAX_METRIC_BUCKETS, MetricsBucket, MetricsGranularity, TenantMetricsResult, TerminationReason,
};

use thiserror::Error;
use tracing::{debug, info, warn};

use crate::observability::trace_context;

/// Errors that can occur when interacting with the runtime
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Instance start failed: {0}")]
    StartFailed(String),

    #[error("Image not found: {0}")]
    ImageNotFound(String),

    #[error("single-instance workflow already has active work")]
    SingleInstanceActive,

    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Timeout waiting for instance completion")]
    Timeout,

    #[error("SDK error: {0}")]
    SdkError(String),

    #[error("Invalid execution timeout: {0}")]
    InvalidExecutionTimeout(String),
}

/// Result of a workflow execution
#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<String>,
    /// Raw stderr output from the container (for debugging/logging).
    /// Separate from `error` to allow products to decide whether to show it to users.
    pub stderr: Option<String>,
    pub duration_ms: Option<u64>,
    /// Peak memory usage during execution (in bytes)
    pub memory_peak_bytes: Option<u64>,
    /// Total CPU time consumed (in microseconds)
    pub cpu_usage_usec: Option<u64>,
}

/// Result of submitting an instance start to runtara-environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartInstanceOutcome {
    pub instance_id: String,
    /// `true` when Environment had already accepted this idempotency key and
    /// deliberately did not launch another process.
    pub deduplicated: bool,
}

/// Terminal outcome of [`RuntimeClient::poll_until_terminal`]. Distinct from
/// [`ExecutionOutput`] (which collapses `Cancelled` into `success: false`) because callers
/// here need to tell "failed" and "cancelled" apart to emit the right product-analytics event.
#[derive(Debug)]
pub enum TerminalOutcome {
    Completed(ExecutionOutput),
    Failed(ExecutionOutput),
    Cancelled(ExecutionOutput),
    /// The instance was killed for exceeding its configured execution timeout (or missed
    /// heartbeats) — reported as `InstanceStatus::Failed` with `termination_reason` set to
    /// `Timeout`/`HeartbeatTimeout`. Distinct from a generic `Failed` so callers can tell
    /// "the platform cut this off" apart from "the workflow itself errored".
    TimedOut(ExecutionOutput),
    /// `max_wait` elapsed before the instance reached a terminal state. The instance is left
    /// running — this is an *observation* timeout, not an execution timeout.
    GaveUp,
    /// The instance parked durably: a `Delay`, or a `WaitForSignal` with no signal yet. It is
    /// not running and holds no runtime state; it resumes when its deadline passes or its
    /// signal arrives, which may be days away, or never.
    ///
    /// This ends the *observation*, not the run. Polling on would cost one query every
    /// `poll_interval` for the whole of `max_wait` per parked instance — with a 2 s interval
    /// and a 24 h cap that is ~43,000 queries each, and it is the dominant load on the
    /// database once parked instances accumulate. It also buys nothing: waking relaunches the
    /// instance as a fresh execution, which is observed on its own.
    Suspended,
}

/// Configuration for the runtime client.
///
/// It deliberately contains the same policy that Environment receives at
/// startup. A server must not choose one timeout while saving a workflow and a
/// different one after that workflow wakes.
#[derive(Debug, Clone)]
pub struct RuntimeClientConfig {
    /// The bounded active-execution timeout policy.
    pub execution_timeout_policy: ExecutionTimeoutPolicy,
}

impl RuntimeClientConfig {
    /// Build a client configuration from the already-validated process policy.
    pub const fn new(execution_timeout_policy: ExecutionTimeoutPolicy) -> Self {
        Self {
            execution_timeout_policy,
        }
    }
}

/// Client for executing workflows on the embedded runtara-environment.
pub struct RuntimeClient {
    client: EnvironmentClient,
    config: RuntimeClientConfig,
}

/// Decide what an observed [`InstanceInfo`] means for [`RuntimeClient::poll_until_terminal`].
///
/// `Some(outcome)` ends the observation; `None` means "poll again". Split out of the poll loop
/// so the classification — in particular that a `Suspended` instance ends observation rather
/// than being polled for the rest of `max_wait` — is testable without a live environment.
fn classify_observed_status(info: InstanceInfo) -> Option<TerminalOutcome> {
    let duration_ms = info.started_at.and_then(|start| {
        info.finished_at
            .map(|end| (end - start).num_milliseconds() as u64)
    });

    match info.status {
        InstanceStatus::Completed => Some(TerminalOutcome::Completed(ExecutionOutput {
            success: true,
            output: info.output,
            error: None,
            stderr: info.stderr,
            duration_ms,
            memory_peak_bytes: info.memory_peak_bytes,
            cpu_usage_usec: info.cpu_usage_usec,
        })),
        InstanceStatus::Failed => {
            let output = ExecutionOutput {
                success: false,
                output: info.output,
                error: info.error,
                stderr: info.stderr,
                duration_ms,
                memory_peak_bytes: info.memory_peak_bytes,
                cpu_usage_usec: info.cpu_usage_usec,
            };
            Some(match info.termination_reason {
                Some(TerminationReason::Timeout | TerminationReason::HeartbeatTimeout) => {
                    TerminalOutcome::TimedOut(output)
                }
                _ => TerminalOutcome::Failed(output),
            })
        }
        InstanceStatus::Cancelled => Some(TerminalOutcome::Cancelled(ExecutionOutput {
            success: false,
            output: info.output,
            error: info
                .error
                .or_else(|| Some("Instance was cancelled".to_string())),
            stderr: info.stderr,
            duration_ms,
            memory_peak_bytes: info.memory_peak_bytes,
            cpu_usage_usec: info.cpu_usage_usec,
        })),
        // Durably parked — stop observing. See `TerminalOutcome::Suspended`.
        InstanceStatus::Suspended => Some(TerminalOutcome::Suspended),
        // Still in progress, keep waiting.
        InstanceStatus::Pending | InstanceStatus::Running => None,
        InstanceStatus::Unknown => {
            warn!(
                instance_id = %info.instance_id,
                "Instance status unknown, continuing to wait"
            );
            None
        }
    }
}

impl RuntimeClient {
    /// Create a client over the embedded environment's shared handler state.
    pub fn new(state: Arc<EnvironmentHandlerState>, config: RuntimeClientConfig) -> Self {
        Self {
            client: EnvironmentClient::new(state),
            config,
        }
    }

    /// Resolve an optional workflow timeout through the one process-wide
    /// policy. ExecutionEngine calls this before both sync and async starts so
    /// a stored definition cannot bypass a stricter deployment maximum.
    pub fn resolve_execution_timeout(
        &self,
        timeout: Option<ExecutionTimeoutSeconds>,
    ) -> Result<ExecutionTimeoutSeconds, RuntimeError> {
        self.config
            .execution_timeout_policy
            .resolve(timeout)
            .map_err(|error| RuntimeError::InvalidExecutionTimeout(error.to_string()))
    }

    /// Start a workflow instance
    ///
    /// # Arguments
    /// * `image_id` - The compiled workflow image ID (UUID from runtara-environment)
    /// * `tenant_id` - The tenant identifier
    /// * `workflow_id` - The workflow identifier (for tracing context)
    /// * `instance_id` - Optional custom instance ID
    /// * `input` - Input data for the workflow
    /// * `timeout` - Optional validated active-execution deadline
    /// * `single_instance` - Enforce the durable workflow-wide launch lease
    ///
    /// # Returns
    /// The instance ID of the started workflow
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start_instance(
        &self,
        image_id: &str,
        tenant_id: &str,
        workflow_id: &str,
        instance_id: Option<String>,
        input: Option<Value>,
        timeout: Option<ExecutionTimeoutSeconds>,
        debug: bool,
        single_instance: bool,
    ) -> Result<StartInstanceOutcome, RuntimeError> {
        let sdk = &self.client;

        let mut options = StartInstanceOptions::new(image_id, tenant_id);

        // Store instance_id for later use in env vars
        let actual_instance_id = if let Some(ref id) = instance_id {
            options = options.with_instance_id(id);
            id.clone()
        } else {
            // Generate a UUID if not provided (so we can pass it as env var)
            let generated_id = uuid::Uuid::new_v4().to_string();
            options = options.with_instance_id(&generated_id);
            generated_id
        };

        if let Some(inp) = input {
            options = options.with_input(inp);
        }

        // Always pass a timeout to Environment. It owns the same policy, but
        // resolving it here keeps the synchronous observer and the active
        // runner on exactly the same deadline.
        let effective_timeout = self.resolve_execution_timeout(timeout)?;
        options = options.with_timeout(effective_timeout.as_secs());

        // Pass OpenTelemetry configuration for distributed tracing (if enabled)
        if trace_context::is_otel_enabled() {
            // OTEL endpoint
            if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
                options = options.with_env_var("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint);
            }

            // Service name (standard OTEL var, derived from workflow_id)
            options = options.with_env_var(
                "OTEL_SERVICE_NAME",
                format!("runtara-workflow-{}", workflow_id),
            );

            // Resource attributes - map vendor-specific vars (DD_*) to standard OTEL format
            if let Some(attrs) = trace_context::build_resource_attributes() {
                options = options.with_env_var("OTEL_RESOURCE_ATTRIBUTES", &attrs);
            }

            // W3C Trace Context (links workflow spans to parent)
            if let Some(traceparent) = trace_context::format_traceparent() {
                options = options.with_env_var("TRACEPARENT", &traceparent);
                debug!(traceparent = %traceparent, "Propagating trace context to workflow");
            }
        }

        // Workflow context (always pass these for correlation)
        options = options.with_env_var("WORKFLOW_ID", workflow_id);
        options = options.with_env_var("TENANT_ID", tenant_id);
        options = options.with_env_var("INSTANCE_ID", &actual_instance_id);

        // Server-side service URLs and tenant ID forwarded to the guest. These
        // were previously read by runners via env::var against the host process
        // environment (set by an unsafe env::set_var in server startup) —
        // passing them explicitly through StartInstanceOptions removes that
        // race-prone pattern and makes the workflow ABI typed.
        let server_config = crate::config::get();
        options = options.with_env_var("RUNTARA_TENANT_ID", &server_config.tenant_id);
        options = options.with_env_var("RUNTARA_HTTP_PROXY_URL", &server_config.http_proxy_url);
        options = options.with_env_var("RUNTARA_OBJECT_MODEL_URL", &server_config.object_model_url);
        options = options.with_env_var(
            "RUNTARA_AGENT_SERVICE_URL",
            &server_config.agent_service_url,
        );

        // Debug mode (pause at breakpoints)
        if debug {
            options = options.with_env_var("DEBUG_MODE", "true");
        }
        if single_instance {
            options = options.with_env_var(SINGLE_INSTANCE_LAUNCH_ENV, "true");
        }

        let result = sdk.start_instance(options).await.map_err(|e| match e {
            EnvironmentError::ImageNotFound(message) => RuntimeError::ImageNotFound(message),
            EnvironmentError::SingleInstanceActive => RuntimeError::SingleInstanceActive,
            other => RuntimeError::StartFailed(other.to_string()),
        })?;

        if !result.success {
            return Err(RuntimeError::StartFailed(
                result.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }

        info!(
            instance_id = %result.instance_id,
            image_id = %image_id,
            workflow_id = %workflow_id,
            tenant_id = %tenant_id,
            deduplicated = result.deduplicated,
            "Instance start accepted"
        );

        Ok(StartInstanceOutcome {
            instance_id: result.instance_id,
            deduplicated: result.deduplicated,
        })
    }

    /// Get the status of a workflow instance
    pub async fn get_instance_status(
        &self,
        instance_id: &str,
    ) -> Result<InstanceStatus, RuntimeError> {
        let sdk = &self.client;

        let info = sdk
            .get_instance_status(instance_id)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        Ok(info.status)
    }

    /// Wait for a workflow instance to complete and return its output
    ///
    /// # Arguments
    /// * `instance_id` - The instance to wait for
    /// * `poll_interval_ms` - How often to check status (default 10ms)
    /// * `timeout` - Maximum time to wait (default from the shared policy)
    pub async fn wait_for_completion(
        &self,
        instance_id: &str,
        poll_interval_ms: Option<u64>,
        timeout: Option<ExecutionTimeoutSeconds>,
    ) -> Result<ExecutionOutput, RuntimeError> {
        let sdk = &self.client;

        let poll_interval = std::time::Duration::from_millis(poll_interval_ms.unwrap_or(10));
        let timeout = self.resolve_execution_timeout(timeout)?.as_duration();
        let start_time = std::time::Instant::now();

        loop {
            let info = sdk
                .get_instance_status(instance_id)
                .await
                .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

            match info.status {
                InstanceStatus::Completed => {
                    let duration_ms = info.started_at.and_then(|start| {
                        info.finished_at
                            .map(|end| (end - start).num_milliseconds() as u64)
                    });

                    return Ok(ExecutionOutput {
                        success: true,
                        output: info.output,
                        error: None,
                        stderr: info.stderr,
                        duration_ms,
                        memory_peak_bytes: info.memory_peak_bytes,
                        cpu_usage_usec: info.cpu_usage_usec,
                    });
                }
                InstanceStatus::Failed => {
                    let duration_ms = info.started_at.and_then(|start| {
                        info.finished_at
                            .map(|end| (end - start).num_milliseconds() as u64)
                    });

                    return Ok(ExecutionOutput {
                        success: false,
                        output: info.output,
                        error: info.error,
                        stderr: info.stderr,
                        duration_ms,
                        memory_peak_bytes: info.memory_peak_bytes,
                        cpu_usage_usec: info.cpu_usage_usec,
                    });
                }
                InstanceStatus::Cancelled => {
                    return Ok(ExecutionOutput {
                        success: false,
                        output: None,
                        error: Some("Instance was cancelled".to_string()),
                        stderr: info.stderr,
                        duration_ms: None,
                        memory_peak_bytes: info.memory_peak_bytes,
                        cpu_usage_usec: info.cpu_usage_usec,
                    });
                }
                InstanceStatus::Pending | InstanceStatus::Running | InstanceStatus::Suspended => {
                    // Still in progress, continue waiting
                }
                InstanceStatus::Unknown => {
                    warn!(instance_id = %instance_id, "Instance status unknown, continuing to wait");
                }
            }

            if start_time.elapsed() > timeout {
                // Attempt to cancel the running instance
                warn!(
                    instance_id = %instance_id,
                    timeout_secs = timeout.as_secs(),
                    "Execution timed out, cancelling instance"
                );
                if let Err(e) = self.cancel_instance(instance_id).await {
                    warn!(
                        instance_id = %instance_id,
                        error = %e,
                        "Failed to cancel instance after timeout"
                    );
                }

                return Err(RuntimeError::Timeout);
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Poll for a terminal instance status, purely to observe the outcome.
    ///
    /// Unlike [`wait_for_completion`](Self::wait_for_completion), this NEVER cancels the
    /// instance when `max_wait` elapses — it just stops watching. Detached/async executions are
    /// explicitly allowed to run long; a background analytics observer must not be the thing
    /// that kills one just because it took a while.
    pub async fn poll_until_terminal(
        &self,
        instance_id: &str,
        poll_interval: std::time::Duration,
        max_wait: std::time::Duration,
    ) -> Result<TerminalOutcome, RuntimeError> {
        let start = std::time::Instant::now();
        loop {
            let info = self.get_instance_info(instance_id).await?;
            if let Some(outcome) = classify_observed_status(info) {
                return Ok(outcome);
            }

            if start.elapsed() > max_wait {
                warn!(
                    instance_id = %instance_id,
                    max_wait_secs = max_wait.as_secs(),
                    "Gave up observing instance for product-analytics; instance left running"
                );
                return Ok(TerminalOutcome::GaveUp);
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Stop a running workflow instance
    pub async fn stop_instance(&self, instance_id: &str) -> Result<(), RuntimeError> {
        let sdk = &self.client;

        let options = crate::runtime_types::StopInstanceOptions::new(instance_id)
            .with_grace_period(5)
            .with_reason("Stopped by runtara-server");

        sdk.stop_instance(options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        info!(instance_id = %instance_id, "Stopped workflow instance");
        Ok(())
    }

    /// Count a tenant's instances in the given statuses.
    ///
    /// The admission gate wants a number. Asking for it via a list-with-limit-1
    /// also ran the paginated list query, whose rows were then thrown away.
    pub async fn count_instances_by_status(
        &self,
        tenant_id: &str,
        statuses: &[String],
        ceiling: u64,
    ) -> Result<u64, RuntimeError> {
        let count = self
            .client
            .count_instances_by_status(
                Some(tenant_id),
                statuses,
                i64::try_from(ceiling).unwrap_or(i64::MAX),
            )
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// List running instances for a tenant (simple API)
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant to list instances for
    /// * `status_filter` - Optional status filter (e.g., Running, Pending)
    /// * `limit` - Maximum number of instances to return
    pub async fn list_instances(
        &self,
        tenant_id: &str,
        status_filter: Option<InstanceStatus>,
        limit: u32,
    ) -> Result<Vec<InstanceSummary>, RuntimeError> {
        let sdk = &self.client;

        let mut options = ListInstancesOptions::new()
            .with_tenant_id(tenant_id)
            .with_limit(limit);

        if let Some(status) = status_filter {
            options = options.with_status(status);
        }

        let result = sdk
            .list_instances(options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        Ok(result.instances)
    }

    /// List instances with full filtering options
    ///
    /// Returns instances matching the provided filters with pagination info.
    pub async fn list_instances_with_options(
        &self,
        options: ListInstancesOptions,
    ) -> Result<ListInstancesResult, RuntimeError> {
        let sdk = &self.client;

        sdk.list_instances(options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Get detailed instance info including output and error
    pub async fn get_instance_info(&self, instance_id: &str) -> Result<InstanceInfo, RuntimeError> {
        let sdk = &self.client;

        sdk.get_instance_status(instance_id)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Cancel a running workflow instance
    pub async fn cancel_instance(&self, instance_id: &str) -> Result<(), RuntimeError> {
        let sdk = &self.client;

        sdk.send_signal(instance_id, crate::runtime_types::SignalType::Cancel, None)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        debug!(instance_id = %instance_id, "Sent cancel signal to workflow instance");
        Ok(())
    }

    /// Write a `Shutdown` signal for an in-flight execution. Unlike
    /// [`Self::cancel_instance`], the SDK treats this as a graceful suspend:
    /// it checkpoints and exits so the instance can be resumed post-restart.
    ///
    /// Accepts any identifier (UUID or string) — the management SDK speaks strings.
    pub async fn signal_shutdown(&self, execution_id: uuid::Uuid) -> Result<(), RuntimeError> {
        let sdk = &self.client;

        sdk.send_signal(
            &execution_id.to_string(),
            crate::runtime_types::SignalType::Shutdown,
            None,
        )
        .await
        .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        debug!(execution_id = %execution_id, "Sent shutdown signal to workflow instance");
        Ok(())
    }

    /// Pause a running workflow instance
    ///
    /// Sends a pause signal to the instance. The instance will checkpoint its state
    /// and suspend execution until resumed.
    pub async fn pause_instance(&self, instance_id: &str) -> Result<(), RuntimeError> {
        let sdk = &self.client;

        sdk.send_signal(instance_id, crate::runtime_types::SignalType::Pause, None)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        info!(instance_id = %instance_id, "Sent pause signal to workflow instance");
        Ok(())
    }

    /// Resume a paused workflow instance
    ///
    /// Triggers the instance to resume execution from its last checkpoint.
    /// This uses the ResumeInstance request which relaunches the workflow process.
    pub async fn resume_instance(&self, instance_id: &str) -> Result<(), RuntimeError> {
        let sdk = &self.client;

        // Use resume_instance() which sends ResumeInstance request to relaunch the workflow
        // Note: send_signal(Resume) only stores a signal which won't work since the process exited
        sdk.resume_instance(instance_id)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        info!(instance_id = %instance_id, "Resumed workflow instance");
        Ok(())
    }

    /// Send a custom signal to a workflow instance.
    ///
    /// Used for human-in-the-loop interactions where an AI Agent step is waiting
    /// for external input via WaitForSignal. The signal_id must match exactly
    /// what the workflow is polling for.
    pub async fn send_custom_signal(
        &self,
        instance_id: &str,
        signal_id: &str,
        payload: Option<&[u8]>,
    ) -> Result<(), RuntimeError> {
        let sdk = &self.client;

        sdk.send_custom_signal(instance_id, signal_id, payload)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        info!(instance_id = %instance_id, signal_id = %signal_id, "Sent custom signal to workflow instance");
        Ok(())
    }

    /// Get image info by image ID
    ///
    /// Returns image details including the human-readable name (legacy
    /// `workflow_id:version` or artifact-qualified
    /// `workflow_id:version@fingerprint`).
    pub async fn get_image(
        &self,
        image_id: &str,
        tenant_id: &str,
    ) -> Result<Option<crate::runtime_types::ImageSummary>, RuntimeError> {
        let sdk = &self.client;

        sdk.get_image(image_id, tenant_id)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// List images for a tenant
    ///
    /// Returns a list of images registered for the given tenant.
    pub async fn list_images(
        &self,
        tenant_id: &str,
        limit: u32,
    ) -> Result<crate::runtime_types::ListImagesResult, RuntimeError> {
        let sdk = &self.client;

        let options = crate::runtime_types::ListImagesOptions::new()
            .with_tenant_id(tenant_id)
            .with_limit(limit);

        sdk.list_images(options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Find an image by name for a tenant
    ///
    /// Returns the image_id if found, None otherwise.
    pub async fn find_image_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<String>, RuntimeError> {
        Ok(self
            .find_image_by_name_summary(tenant_id, name)
            .await?
            .map(|image| image.image_id))
    }

    /// Find an image by name for a tenant and return the full summary.
    pub async fn find_image_by_name_summary(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<crate::runtime_types::ImageSummary>, RuntimeError> {
        self.client
            .find_image_by_name(tenant_id, name)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Register an image using streaming upload
    ///
    /// This method streams the binary data directly from a reader, avoiding the need
    /// to hold the entire binary in memory.
    pub async fn register_image_stream<R: tokio::io::AsyncRead + Unpin>(
        &self,
        options: crate::runtime_types::RegisterImageStreamOptions,
        reader: R,
    ) -> Result<crate::runtime_types::RegisterImageResult, RuntimeError> {
        let total_start = std::time::Instant::now();
        info!("RuntimeClient: register_image_stream starting");

        let sdk = &self.client;

        let upload_start = std::time::Instant::now();
        let result = sdk
            .register_image_stream(options, reader)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()));

        let upload_duration = upload_start.elapsed();
        let total_duration = total_start.elapsed();

        match &result {
            Ok(r) => info!(
                upload_ms = upload_duration.as_millis(),
                total_ms = total_duration.as_millis(),
                image_id = %r.image_id,
                "RuntimeClient: register_image_stream completed successfully"
            ),
            Err(e) => warn!(
                upload_ms = upload_duration.as_millis(),
                total_ms = total_duration.as_millis(),
                error = %e,
                "RuntimeClient: register_image_stream failed"
            ),
        }

        result
    }

    /// List checkpoints for an instance
    ///
    /// Returns checkpoint summaries for the specified instance, ordered by creation time.
    pub async fn list_checkpoints(
        &self,
        instance_id: &str,
        limit: Option<u32>,
    ) -> Result<crate::runtime_types::ListCheckpointsResult, RuntimeError> {
        let sdk = &self.client;

        let mut options = crate::runtime_types::ListCheckpointsOptions::new();
        if let Some(l) = limit {
            options = options.with_limit(l);
        }

        sdk.list_checkpoints(instance_id, options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// List events for an instance with optional filtering
    ///
    /// Returns events for the specified instance, including debug step events when
    /// the workflow was compiled with track_events enabled.
    ///
    /// # Arguments
    /// * `instance_id` - The instance to list events for
    /// * `options` - Optional filtering options (event_type, subtype, limit, etc.)
    pub async fn list_events(
        &self,
        instance_id: &str,
        options: Option<crate::runtime_types::ListEventsOptions>,
    ) -> Result<crate::runtime_types::ListEventsResult, RuntimeError> {
        let sdk = &self.client;

        let opts = options.unwrap_or_default();

        sdk.list_events(instance_id, opts)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// List step summaries for an instance with optional filtering
    ///
    /// Returns unified step records with paired start/end events. Each step appears
    /// once with its complete lifecycle information (inputs, outputs, duration, status).
    ///
    /// # Arguments
    /// * `instance_id` - The instance to list step summaries for
    /// * `options` - Optional filtering options (status, step_type, scope_id, limit, etc.)
    pub async fn list_step_summaries(
        &self,
        instance_id: &str,
        options: Option<crate::runtime_types::ListStepSummariesOptions>,
    ) -> Result<crate::runtime_types::ListStepSummariesResult, RuntimeError> {
        let sdk = &self.client;

        let opts = options.unwrap_or_default();

        sdk.list_step_summaries(instance_id, opts)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Get ancestor scopes for a given scope ID
    ///
    /// Returns the chain of parent scopes from the given scope up to the root,
    /// useful for reconstructing the call stack in hierarchical step execution
    /// (Split/While/EmbedWorkflow).
    pub async fn get_scope_ancestors(
        &self,
        instance_id: &str,
        scope_id: &str,
    ) -> Result<Vec<crate::runtime_types::ScopeInfo>, RuntimeError> {
        let sdk = &self.client;

        sdk.get_scope_ancestors(instance_id, scope_id)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Get aggregated execution metrics for a tenant
    ///
    /// Returns time-bucketed metrics including invocation counts, success rates,
    /// duration statistics, and memory usage across all instances for the tenant.
    ///
    /// # Arguments
    /// * `options` - Options including tenant_id, time range, and granularity
    pub async fn get_tenant_metrics(
        &self,
        options: GetTenantMetricsOptions,
    ) -> Result<TenantMetricsResult, RuntimeError> {
        let sdk = &self.client;

        sdk.get_tenant_metrics(options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }
}

/// Build a legacy human-readable image-name prefix.
///
/// Compiled workflow artifacts append an opaque `@` fingerprint to this form
/// so a recompile cannot replace the binary used by an already-selected image
/// UUID. This helper remains for legacy callers that only need the stable
/// `{workflow_id}:{version}` prefix.
///
/// **IMPORTANT**: This is a name, NOT the ID for execution!
/// When executing, you must use the UUID returned from `register_image_stream`.
/// The UUID is stored in `workflow_compilations.registered_image_id`.
pub fn build_image_name(workflow_id: &str, version: u32) -> String {
    format!("{}:{}", workflow_id, version)
}

#[cfg(test)]
mod classify_observed_status_tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};

    fn info(status: InstanceStatus) -> InstanceInfo {
        let created = Utc::now();
        InstanceInfo {
            instance_id: "inst-1".to_string(),
            image_id: "img-1".to_string(),
            image_name: "wf:1".to_string(),
            tenant_id: "tenant-1".to_string(),
            status,
            checkpoint_id: None,
            created_at: created,
            started_at: Some(created),
            finished_at: Some(created + ChronoDuration::milliseconds(1_500)),
            input: None,
            output: None,
            error: None,
            stderr: None,
            retry_count: 0,
            max_retries: 0,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
            termination_reason: None,
            exit_code: None,
        }
    }

    /// The regression this function exists for: a durably parked instance ends the
    /// observation. Returning `None` here would poll it once per `poll_interval` for the
    /// whole of `max_wait` — ~43,000 queries per instance at the caller's 2 s / 24 h.
    #[test]
    fn suspended_ends_observation() {
        assert!(matches!(
            classify_observed_status(info(InstanceStatus::Suspended)),
            Some(TerminalOutcome::Suspended)
        ));
    }

    #[test]
    fn in_progress_states_poll_again() {
        for status in [InstanceStatus::Pending, InstanceStatus::Running] {
            assert!(
                classify_observed_status(info(status)).is_none(),
                "{status:?} should keep polling"
            );
        }
    }

    #[test]
    fn unknown_polls_again() {
        assert!(classify_observed_status(info(InstanceStatus::Unknown)).is_none());
    }

    #[test]
    fn completed_carries_output_and_duration() {
        let mut i = info(InstanceStatus::Completed);
        i.output = Some(serde_json::json!({"done": true}));
        match classify_observed_status(i) {
            Some(TerminalOutcome::Completed(o)) => {
                assert!(o.success);
                assert_eq!(o.duration_ms, Some(1_500));
                assert_eq!(o.output, Some(serde_json::json!({"done": true})));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn failed_is_distinguished_from_platform_timeout() {
        let mut plain = info(InstanceStatus::Failed);
        plain.error = Some("boom".to_string());
        assert!(matches!(
            classify_observed_status(plain),
            Some(TerminalOutcome::Failed(_))
        ));

        for reason in [
            TerminationReason::Timeout,
            TerminationReason::HeartbeatTimeout,
        ] {
            let mut timed_out = info(InstanceStatus::Failed);
            timed_out.termination_reason = Some(reason);
            assert!(matches!(
                classify_observed_status(timed_out),
                Some(TerminalOutcome::TimedOut(_))
            ));
        }
    }

    #[test]
    fn cancelled_gets_a_default_error() {
        match classify_observed_status(info(InstanceStatus::Cancelled)) {
            Some(TerminalOutcome::Cancelled(o)) => {
                assert!(!o.success);
                assert_eq!(o.error.as_deref(), Some("Instance was cancelled"));
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod execution_timeout_tests {
    use super::*;

    #[test]
    fn runtime_client_config_carries_the_policy_unchanged() {
        let policy = ExecutionTimeoutPolicy::new(120, 300).unwrap();
        let config = RuntimeClientConfig::new(policy);

        assert_eq!(config.execution_timeout_policy, policy);
    }
}
