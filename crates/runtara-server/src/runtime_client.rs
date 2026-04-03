//! Runtime Client
//!
//! Provides a client abstraction for executing workflows via the Runtara Management SDK.
//! This module bridges smo-runtime with the runtara-environment execution server.

use std::sync::Arc;

use runtara_management_sdk::{
    ListInstancesOptions, ManagementSdk, SdkConfig, StartInstanceOptions,
};
use serde_json::Value;

// Re-export types from the SDK for use by other modules
pub use runtara_management_sdk::{
    GetTenantMetricsOptions, InstanceInfo, InstanceStatus, InstanceSummary, ListInstancesResult,
    MetricsBucket, MetricsGranularity, TenantMetricsResult,
};

use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::observability::trace_context;

/// Errors that can occur when interacting with the runtime
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Instance start failed: {0}")]
    StartFailed(String),

    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Timeout waiting for instance completion")]
    Timeout,

    #[error("SDK error: {0}")]
    SdkError(String),
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

/// Configuration for the runtime client
#[derive(Debug, Clone)]
pub struct RuntimeClientConfig {
    /// Default timeout for operations in seconds
    pub default_timeout_secs: u32,
}

impl RuntimeClientConfig {
    /// Create configuration from environment variables
    ///
    /// Returns Some if RUNTARA_ENVIRONMENT_ADDR is set (required by the SDK).
    /// The SDK will read its own configuration via SdkConfig::from_env().
    pub fn from_env() -> Option<Self> {
        // Check if the environment is configured for runtara-environment
        // The actual address is read by SdkConfig::from_env() in connect()
        std::env::var("RUNTARA_ENVIRONMENT_ADDR").ok()?;

        let default_timeout_secs = std::env::var("RUNTARA_REQUEST_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|ms| ms / 1000)
            .unwrap_or(300);

        Some(Self {
            default_timeout_secs,
        })
    }
}

/// Client for executing workflows via the Runtara Management SDK
pub struct RuntimeClient {
    sdk: Arc<RwLock<Option<ManagementSdk>>>,
    config: RuntimeClientConfig,
}

impl RuntimeClient {
    /// Create a new runtime client with the given configuration
    pub fn new(config: RuntimeClientConfig) -> Self {
        Self {
            sdk: Arc::new(RwLock::new(None)),
            config,
        }
    }

    /// Create a runtime client for a specific server address
    ///
    /// This sets the RUNTARA_ENVIRONMENT_ADDR environment variable before creating
    /// the client, which is used by SdkConfig::from_env() during connect().
    pub fn with_address(addr: &str) -> Self {
        // SAFETY: This is called during initialization before any threads that
        // might read RUNTARA_ENVIRONMENT_ADDR are spawned.
        unsafe {
            std::env::set_var("RUNTARA_ENVIRONMENT_ADDR", addr);
        }

        Self::new(RuntimeClientConfig {
            default_timeout_secs: std::env::var("RUNTARA_REQUEST_TIMEOUT_MS")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .map(|ms| ms / 1000)
                .unwrap_or(300),
        })
    }

    /// Create a runtime client from environment variables
    pub fn from_env() -> Option<Self> {
        RuntimeClientConfig::from_env().map(Self::new)
    }

    /// Connect to the runtara-environment server
    pub async fn connect(&self) -> Result<(), RuntimeError> {
        let sdk_config =
            SdkConfig::from_env().map_err(|e| RuntimeError::ConnectionFailed(e.to_string()))?;

        let sdk = ManagementSdk::new(sdk_config)
            .map_err(|e| RuntimeError::ConnectionFailed(e.to_string()))?;

        sdk.connect()
            .await
            .map_err(|e| RuntimeError::ConnectionFailed(e.to_string()))?;

        *self.sdk.write().await = Some(sdk);

        info!("Connected to runtara-environment server");
        Ok(())
    }

    /// Ensure we're connected, connecting if necessary
    ///
    /// This provides lazy initialization - if the SDK hasn't been created yet,
    /// it will be created and connected. This allows operations to proceed even
    /// if the initial background connect hasn't completed yet.
    async fn ensure_connected(&self) -> Result<(), RuntimeError> {
        // Fast path: already connected
        let lock_start = std::time::Instant::now();
        debug!("RuntimeClient: acquiring read lock for connection check");
        let guard = self.sdk.read().await;
        let lock_duration = lock_start.elapsed();
        if lock_duration.as_millis() > 100 {
            warn!(
                lock_wait_ms = lock_duration.as_millis(),
                "RuntimeClient: slow read lock acquisition in ensure_connected"
            );
        }
        if guard.is_some() {
            drop(guard);
            return Ok(());
        }
        drop(guard);

        // Slow path: need to connect
        info!("SDK not connected, attempting to connect...");
        self.connect().await
    }

    /// Check if connected to the server
    pub async fn is_connected(&self) -> bool {
        if let Some(sdk) = self.sdk.read().await.as_ref() {
            sdk.is_connected().await
        } else {
            false
        }
    }

    /// Start a workflow instance
    ///
    /// # Arguments
    /// * `image_id` - The compiled workflow image ID (UUID from runtara-environment)
    /// * `tenant_id` - The tenant identifier
    /// * `scenario_id` - The scenario identifier (for tracing context)
    /// * `instance_id` - Optional custom instance ID
    /// * `input` - Input data for the workflow
    /// * `timeout_secs` - Optional timeout in seconds
    ///
    /// # Returns
    /// The instance ID of the started workflow
    #[allow(clippy::too_many_arguments)]
    pub async fn start_instance(
        &self,
        image_id: &str,
        tenant_id: &str,
        scenario_id: &str,
        instance_id: Option<String>,
        input: Option<Value>,
        timeout_secs: Option<u32>,
        debug: bool,
    ) -> Result<String, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

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

        // Always pass a timeout to runtara to avoid SDK's internal default (which may be too short)
        let effective_timeout = timeout_secs.unwrap_or(self.config.default_timeout_secs);
        options = options.with_timeout(effective_timeout);

        // Pass database URLs to scenario for object_model agent
        if let Ok(url) = std::env::var("OBJECT_STORE_DATABASE_URL") {
            options = options.with_env_var("OBJECT_STORE_DATABASE_URL", &url);
        } else if let Ok(url) = std::env::var("OBJECT_MODEL_DATABASE_URL") {
            // Fallback: use OBJECT_MODEL_DATABASE_URL if OBJECT_STORE is not set
            options = options.with_env_var("OBJECT_STORE_DATABASE_URL", &url);
        }

        // Pass OpenTelemetry configuration for distributed tracing (if enabled)
        if trace_context::is_otel_enabled() {
            // OTEL endpoint
            if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
                options = options.with_env_var("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint);
            }

            // Service name (standard OTEL var, derived from scenario_id)
            options =
                options.with_env_var("OTEL_SERVICE_NAME", format!("smo-scenario-{}", scenario_id));

            // Resource attributes - map vendor-specific vars (DD_*) to standard OTEL format
            if let Some(attrs) = trace_context::build_resource_attributes() {
                options = options.with_env_var("OTEL_RESOURCE_ATTRIBUTES", &attrs);
            }

            // W3C Trace Context (links scenario spans to parent)
            if let Some(traceparent) = trace_context::format_traceparent() {
                options = options.with_env_var("TRACEPARENT", &traceparent);
                debug!(traceparent = %traceparent, "Propagating trace context to scenario");
            }
        }

        // Scenario context (always pass these for correlation)
        options = options.with_env_var("SCENARIO_ID", scenario_id);
        options = options.with_env_var("TENANT_ID", tenant_id);
        options = options.with_env_var("INSTANCE_ID", &actual_instance_id);

        // Debug mode (pause at breakpoints)
        if debug {
            options = options.with_env_var("DEBUG_MODE", "true");
        }

        let result = sdk
            .start_instance(options)
            .await
            .map_err(|e| RuntimeError::StartFailed(e.to_string()))?;

        if !result.success {
            return Err(RuntimeError::StartFailed(
                result.error.unwrap_or_else(|| "Unknown error".to_string()),
            ));
        }

        info!(
            instance_id = %result.instance_id,
            image_id = %image_id,
            scenario_id = %scenario_id,
            tenant_id = %tenant_id,
            "Started workflow instance"
        );

        Ok(result.instance_id)
    }

    /// Get the status of a workflow instance
    pub async fn get_instance_status(
        &self,
        instance_id: &str,
    ) -> Result<InstanceStatus, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

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
    /// * `timeout_secs` - Maximum time to wait (default from config)
    pub async fn wait_for_completion(
        &self,
        instance_id: &str,
        poll_interval_ms: Option<u64>,
        timeout_secs: Option<u32>,
    ) -> Result<ExecutionOutput, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        let poll_interval = std::time::Duration::from_millis(poll_interval_ms.unwrap_or(10));
        let timeout = std::time::Duration::from_secs(
            timeout_secs.unwrap_or(self.config.default_timeout_secs) as u64,
        );
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
                // Drop the SDK guard before cancelling to avoid lock issues
                drop(sdk_guard);

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

    /// Execute a workflow synchronously (start and wait for completion)
    ///
    /// This is a convenience method that combines `start_instance` and `wait_for_completion`.
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_sync(
        &self,
        image_id: &str,
        tenant_id: &str,
        scenario_id: &str,
        instance_id: Option<String>,
        input: Option<Value>,
        timeout_secs: Option<u32>,
        debug: bool,
    ) -> Result<ExecutionOutput, RuntimeError> {
        let started_id = self
            .start_instance(
                image_id,
                tenant_id,
                scenario_id,
                instance_id,
                input,
                timeout_secs,
                debug,
            )
            .await?;

        self.wait_for_completion(&started_id, None, timeout_secs)
            .await
    }

    /// Stop a running workflow instance
    pub async fn stop_instance(&self, instance_id: &str) -> Result<(), RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        let options = runtara_management_sdk::StopInstanceOptions::new(instance_id)
            .with_grace_period(5)
            .with_reason("Stopped by smo-runtime");

        sdk.stop_instance(options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        info!(instance_id = %instance_id, "Stopped workflow instance");
        Ok(())
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
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

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
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        sdk.list_instances(options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Get detailed instance info including output and error
    pub async fn get_instance_info(&self, instance_id: &str) -> Result<InstanceInfo, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        sdk.get_instance_status(instance_id)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Cancel a running workflow instance
    pub async fn cancel_instance(&self, instance_id: &str) -> Result<(), RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        sdk.send_signal(
            instance_id,
            runtara_management_sdk::SignalType::Cancel,
            None,
        )
        .await
        .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        debug!(instance_id = %instance_id, "Sent cancel signal to workflow instance");
        Ok(())
    }

    /// Pause a running workflow instance
    ///
    /// Sends a pause signal to the instance. The instance will checkpoint its state
    /// and suspend execution until resumed.
    pub async fn pause_instance(&self, instance_id: &str) -> Result<(), RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        sdk.send_signal(instance_id, runtara_management_sdk::SignalType::Pause, None)
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
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

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
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        sdk.send_custom_signal(instance_id, signal_id, payload)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))?;

        info!(instance_id = %instance_id, signal_id = %signal_id, "Sent custom signal to workflow instance");
        Ok(())
    }

    /// Close the connection
    pub async fn close(&self) {
        let mut sdk_guard = self.sdk.write().await;
        if let Some(sdk) = sdk_guard.take() {
            sdk.close().await;
        }
    }

    /// Get image info by image ID
    ///
    /// Returns image details including the human-readable name (format: scenario_id:version).
    pub async fn get_image(
        &self,
        image_id: &str,
        tenant_id: &str,
    ) -> Result<Option<runtara_management_sdk::ImageSummary>, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

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
    ) -> Result<runtara_management_sdk::ListImagesResult, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        let options = runtara_management_sdk::ListImagesOptions::new()
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
        // List all images and find by name
        // Note: This could be optimized with server-side filtering if the SDK supports it
        let result = self.list_images(tenant_id, 1000).await?;

        for image in result.images {
            if image.name == name {
                return Ok(Some(image.image_id));
            }
        }

        Ok(None)
    }

    /// Register an image using streaming upload
    ///
    /// This method streams the binary data directly from a reader, avoiding the need
    /// to hold the entire binary in memory.
    pub async fn register_image_stream<R: tokio::io::AsyncRead + Unpin>(
        &self,
        options: runtara_management_sdk::RegisterImageStreamOptions,
        reader: R,
    ) -> Result<runtara_management_sdk::RegisterImageResult, RuntimeError> {
        let total_start = std::time::Instant::now();
        info!("RuntimeClient: register_image_stream starting");

        self.ensure_connected().await?;

        let lock_start = std::time::Instant::now();
        info!("RuntimeClient: register_image_stream acquiring read lock for streaming upload");
        let sdk_guard = self.sdk.read().await;
        let lock_duration = lock_start.elapsed();
        info!(
            lock_wait_ms = lock_duration.as_millis(),
            "RuntimeClient: register_image_stream read lock acquired, starting upload"
        );

        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

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
                lock_held_ms = (total_duration - lock_duration).as_millis(),
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

        // Explicitly drop the lock guard and log
        drop(sdk_guard);
        debug!("RuntimeClient: register_image_stream read lock released");

        result
    }

    /// List checkpoints for an instance
    ///
    /// Returns checkpoint summaries for the specified instance, ordered by creation time.
    pub async fn list_checkpoints(
        &self,
        instance_id: &str,
        limit: Option<u32>,
    ) -> Result<runtara_management_sdk::ListCheckpointsResult, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        let mut options = runtara_management_sdk::ListCheckpointsOptions::new();
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
    /// the scenario was compiled with track_events enabled.
    ///
    /// # Arguments
    /// * `instance_id` - The instance to list events for
    /// * `options` - Optional filtering options (event_type, subtype, limit, etc.)
    pub async fn list_events(
        &self,
        instance_id: &str,
        options: Option<runtara_management_sdk::ListEventsOptions>,
    ) -> Result<runtara_management_sdk::ListEventsResult, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

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
        options: Option<runtara_management_sdk::ListStepSummariesOptions>,
    ) -> Result<runtara_management_sdk::ListStepSummariesResult, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        let opts = options.unwrap_or_default();

        sdk.list_step_summaries(instance_id, opts)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }

    /// Get ancestor scopes for a given scope ID
    ///
    /// Returns the chain of parent scopes from the given scope up to the root,
    /// useful for reconstructing the call stack in hierarchical step execution
    /// (Split/While/StartScenario).
    pub async fn get_scope_ancestors(
        &self,
        instance_id: &str,
        scope_id: &str,
    ) -> Result<Vec<runtara_management_sdk::ScopeInfo>, RuntimeError> {
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

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
        self.ensure_connected().await?;
        let sdk_guard = self.sdk.read().await;
        let sdk = sdk_guard
            .as_ref()
            .ok_or_else(|| RuntimeError::ConnectionFailed("Not connected".to_string()))?;

        sdk.get_tenant_metrics(options)
            .await
            .map_err(|e| RuntimeError::SdkError(e.to_string()))
    }
}

/// Build a human-readable image name for registration
///
/// This returns the name used when registering images with runtara-environment.
/// Format: {scenario_id}:{version}
///
/// **IMPORTANT**: This is the NAME for registration, NOT the ID for execution!
/// When executing, you must use the UUID returned from `register_image_stream`.
/// The UUID is stored in `scenario_compilations.registered_image_id`.
pub fn build_image_name(scenario_id: &str, version: u32) -> String {
    format!("{}:{}", scenario_id, version)
}
