// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! ManagementSdk client for interacting with runtara-environment over HTTP/JSON.
//!
//! This module provides the client for all management operations, targeting the
//! HTTP server defined in `runtara-environment/src/http_server.rs`.

use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use chrono::{TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, instrument};

use crate::config::SdkConfig;
use crate::error::{Result, SdkError};
use crate::types::{
    AgentInfo, CapabilityField, Checkpoint, CheckpointSummary, EventSummary,
    GetTenantMetricsOptions, HealthStatus, ImageSummary, InstanceInfo, InstanceStatus,
    InstanceSummary, ListCheckpointsOptions, ListCheckpointsResult, ListEventsOptions,
    ListEventsResult, ListImagesOptions, ListImagesResult, ListInstancesOptions,
    ListInstancesResult, ListStepSummariesOptions, ListStepSummariesResult, MetricsBucket,
    MetricsGranularity, RegisterImageOptions, RegisterImageResult, RegisterImageStreamOptions,
    RunnerType, ScopeInfo, SignalType, StartInstanceOptions, StartInstanceResult, StepStatus,
    StepSummary, StopInstanceOptions, TenantMetricsResult, TerminationReason,
    TestCapabilityOptions, TestCapabilityResult,
};

// ============================================================================
// Intermediate JSON response structs (match HTTP server's JSON format)
// ============================================================================

#[derive(Debug, Deserialize)]
struct HealthCheckJson {
    healthy: bool,
    version: String,
    #[serde(default)]
    uptime_ms: i64,
}

#[derive(Debug, Deserialize)]
struct InstanceStatusJson {
    found: bool,
    instance_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    tenant_id: Option<String>,
    #[serde(default)]
    image_id: Option<String>,
    #[serde(default)]
    image_name: Option<String>,
    #[serde(default)]
    checkpoint_id: Option<String>,
    #[serde(default)]
    created_at_ms: Option<i64>,
    #[serde(default)]
    started_at_ms: Option<i64>,
    #[serde(default)]
    finished_at_ms: Option<i64>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
    #[serde(default)]
    heartbeat_at_ms: Option<i64>,
    #[serde(default)]
    retry_count: Option<u32>,
    #[serde(default)]
    max_retries: Option<u32>,
    #[serde(default)]
    memory_peak_bytes: Option<u64>,
    #[serde(default)]
    cpu_usage_usec: Option<u64>,
    #[serde(default)]
    termination_reason: Option<String>,
    #[serde(default)]
    exit_code: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ListInstancesJson {
    instances: Vec<InstanceSummaryJson>,
    total_count: u32,
}

#[derive(Debug, Deserialize)]
struct InstanceSummaryJson {
    instance_id: String,
    tenant_id: String,
    #[serde(default)]
    image_id: Option<String>,
    status: String,
    created_at_ms: i64,
    #[serde(default)]
    started_at_ms: Option<i64>,
    #[serde(default)]
    finished_at_ms: Option<i64>,
    #[serde(default)]
    has_error: bool,
}

#[derive(Debug, Deserialize)]
struct StartInstanceJson {
    success: bool,
    #[serde(default)]
    instance_id: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SimpleSuccessJson {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegisterImageJson {
    success: bool,
    #[serde(default)]
    image_id: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListImagesJson {
    images: Vec<ImageSummaryJson>,
    #[serde(default)]
    total_count: u32,
}

#[derive(Debug, Deserialize)]
struct ImageSummaryJson {
    image_id: String,
    tenant_id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    runner_type: String,
    created_at_ms: i64,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GetImageJson {
    found: bool,
    #[serde(default)]
    image: Option<ImageSummaryJson>,
}

#[derive(Debug, Deserialize)]
struct ListCheckpointsJson {
    checkpoints: Vec<CheckpointSummaryJson>,
    total_count: u32,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Deserialize)]
struct CheckpointSummaryJson {
    checkpoint_id: String,
    instance_id: String,
    created_at_ms: i64,
    data_size_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct CheckpointDetailJson {
    found: bool,
    checkpoint_id: String,
    instance_id: String,
    created_at_ms: i64,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListEventsJson {
    events: Vec<EventSummaryJson>,
    total_count: u32,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Deserialize)]
struct EventSummaryJson {
    id: i64,
    instance_id: String,
    event_type: String,
    #[serde(default)]
    checkpoint_id: Option<String>,
    #[serde(default)]
    payload: Option<String>,
    created_at_ms: i64,
    #[serde(default)]
    subtype: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListStepSummariesJson {
    steps: Vec<StepSummaryJson>,
    total_count: u32,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Deserialize)]
struct StepSummaryJson {
    step_id: String,
    #[serde(default)]
    step_name: Option<String>,
    step_type: String,
    status: String,
    started_at_ms: i64,
    #[serde(default)]
    completed_at_ms: Option<i64>,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    inputs: Option<serde_json::Value>,
    #[serde(default)]
    outputs: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    scope_id: Option<String>,
    #[serde(default)]
    parent_scope_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScopeAncestorsJson {
    ancestors: Vec<ScopeInfoJson>,
}

#[derive(Debug, Deserialize)]
struct ScopeInfoJson {
    scope_id: String,
    #[serde(default)]
    parent_scope_id: Option<String>,
    step_id: String,
    #[serde(default)]
    step_name: Option<String>,
    step_type: String,
    #[serde(default)]
    index: Option<u32>,
    created_at_ms: i64,
}

#[derive(Debug, Deserialize)]
struct TenantMetricsJson {
    tenant_id: String,
    start_time_ms: i64,
    end_time_ms: i64,
    granularity: String,
    buckets: Vec<MetricsBucketJson>,
}

#[derive(Debug, Deserialize)]
struct MetricsBucketJson {
    bucket_time_ms: i64,
    invocation_count: i64,
    success_count: i64,
    failure_count: i64,
    cancelled_count: i64,
    #[serde(default)]
    avg_duration_ms: Option<f64>,
    #[serde(default)]
    min_duration_ms: Option<f64>,
    #[serde(default)]
    max_duration_ms: Option<f64>,
    #[serde(default)]
    avg_memory_bytes: Option<i64>,
    #[serde(default)]
    max_memory_bytes: Option<i64>,
    #[serde(default)]
    success_rate_percent: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct TestCapabilityJson {
    success: bool,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    execution_time_ms: u64,
}

#[derive(Debug, Deserialize)]
struct ListAgentsJson {
    agents: Vec<AgentInfo>,
}

#[derive(Debug, Deserialize)]
struct GetCapabilityJson {
    found: bool,
    #[serde(default)]
    inputs: Option<Vec<CapabilityField>>,
}

/// Error response body from the HTTP server.
#[derive(Debug, Deserialize)]
struct ErrorResponseJson {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

// ============================================================================
// Helper functions
// ============================================================================

fn runner_type_from_string(s: &str) -> RunnerType {
    match s.to_lowercase().as_str() {
        "native" | "1" => RunnerType::Native,
        "wasm" | "2" => RunnerType::Wasm,
        _ => RunnerType::Oci,
    }
}

fn runner_type_to_string(rt: RunnerType) -> &'static str {
    match rt {
        RunnerType::Oci => "oci",
        RunnerType::Native => "native",
        RunnerType::Wasm => "wasm",
    }
}

fn instance_status_from_string(s: &str) -> InstanceStatus {
    match s {
        "pending" => InstanceStatus::Pending,
        "running" => InstanceStatus::Running,
        "suspended" | "sleeping" => InstanceStatus::Suspended,
        "completed" => InstanceStatus::Completed,
        "failed" => InstanceStatus::Failed,
        "cancelled" => InstanceStatus::Cancelled,
        _ => InstanceStatus::Unknown,
    }
}

fn step_status_from_string(s: &str) -> StepStatus {
    match s {
        "completed" => StepStatus::Completed,
        "failed" => StepStatus::Failed,
        _ => StepStatus::Running,
    }
}

fn ms_to_datetime(ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

fn opt_ms_to_datetime(ms: Option<i64>) -> Option<chrono::DateTime<Utc>> {
    ms.and_then(|ms| Utc.timestamp_millis_opt(ms).single())
}

/// Decode a base64-encoded string to JSON Value, or None if empty/invalid.
fn decode_base64_json(encoded: &str) -> Option<serde_json::Value> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

// ============================================================================
// ManagementSdk
// ============================================================================

/// HTTP-based management SDK for interacting with runtara-environment.
///
/// Provides the same API as [`ManagementSdk`](crate::ManagementSdk) but uses
/// HTTP/JSON for communicating with runtara-environment.
pub struct ManagementSdk {
    client: Client,
    base_url: String,
    config: SdkConfig,
    connected: AtomicBool,
}

impl ManagementSdk {
    /// Create a new HTTP SDK with the given configuration.
    pub fn new(config: SdkConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .connect_timeout(config.connect_timeout)
            .build()
            .map_err(|e| SdkError::Connection(format!("Failed to create HTTP client: {}", e)))?;

        let base_url = format!("http://{}", config.server_addr);

        Ok(Self {
            client,
            base_url,
            config,
            connected: AtomicBool::new(false),
        })
    }

    /// Create an SDK from environment variables.
    pub fn from_env() -> Result<Self> {
        let config = SdkConfig::from_env()?;
        Self::new(config)
    }

    /// Create an SDK for localhost development.
    pub fn localhost() -> Result<Self> {
        Self::new(SdkConfig::localhost())
    }

    /// Connect to runtara-environment.
    ///
    /// For HTTP, this performs a health check to verify reachability.
    #[instrument(skip(self), level = "debug")]
    pub async fn connect(&self) -> Result<()> {
        self.health_check().await?;
        self.connected.store(true, Ordering::SeqCst);
        debug!("Connected to runtara-environment (HTTP)");
        Ok(())
    }

    /// Close the connection (no-op for HTTP).
    pub async fn close(&self) {
        self.connected.store(false, Ordering::SeqCst);
    }

    /// Check if connected.
    pub async fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Get the SDK configuration.
    pub fn config(&self) -> &SdkConfig {
        &self.config
    }

    // =========================================================================
    // Internal helpers
    // =========================================================================

    /// Build a full URL from a path.
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Parse an error response body from the server.
    async fn parse_error_response(resp: reqwest::Response) -> SdkError {
        let status = resp.status();
        match resp.json::<ErrorResponseJson>().await {
            Ok(err_body) => {
                let message = err_body
                    .error
                    .unwrap_or_else(|| format!("HTTP {} error", status));
                let code = err_body.code.unwrap_or_else(|| status.as_str().to_string());
                SdkError::Server { code, message }
            }
            Err(_) => SdkError::Server {
                code: status.as_str().to_string(),
                message: format!("HTTP {} error", status),
            },
        }
    }

    // =========================================================================
    // Health & Status
    // =========================================================================

    /// Check health of runtara-environment.
    #[instrument(skip(self), level = "debug")]
    pub async fn health_check(&self) -> Result<HealthStatus> {
        debug!("Performing health check");

        let resp = self.client.get(self.url("/api/v1/health")).send().await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: HealthCheckJson = resp.json().await?;

        // HTTP server doesn't return active_instances in health check, default to 0
        Ok(HealthStatus {
            healthy: json.healthy,
            version: json.version,
            uptime_ms: json.uptime_ms,
            active_instances: 0,
        })
    }

    // =========================================================================
    // Instance Management
    // =========================================================================

    /// Get status of a specific instance.
    #[instrument(skip(self), fields(instance_id = %instance_id), level = "debug")]
    pub async fn get_instance_status(&self, instance_id: &str) -> Result<InstanceInfo> {
        debug!("Getting instance status");

        let resp = self
            .client
            .get(self.url(&format!("/api/v1/instances/{}", instance_id)))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: InstanceStatusJson = resp.json().await?;

        if !json.found {
            return Err(SdkError::InstanceNotFound(instance_id.to_string()));
        }

        Ok(InstanceInfo {
            instance_id: json.instance_id,
            image_id: json.image_id.unwrap_or_default(),
            image_name: json.image_name.unwrap_or_default(),
            tenant_id: json.tenant_id.unwrap_or_default(),
            status: instance_status_from_string(json.status.as_deref().unwrap_or("unknown")),
            checkpoint_id: json.checkpoint_id,
            created_at: json
                .created_at_ms
                .map(ms_to_datetime)
                .unwrap_or_else(Utc::now),
            started_at: opt_ms_to_datetime(json.started_at_ms),
            finished_at: opt_ms_to_datetime(json.finished_at_ms),
            heartbeat_at: opt_ms_to_datetime(json.heartbeat_at_ms),
            input: json.input.as_deref().and_then(decode_base64_json),
            output: json.output.as_deref().and_then(decode_base64_json),
            error: json.error,
            stderr: json.stderr,
            retry_count: json.retry_count.unwrap_or(0),
            max_retries: json.max_retries.unwrap_or(0),
            memory_peak_bytes: json.memory_peak_bytes,
            cpu_usage_usec: json.cpu_usage_usec,
            termination_reason: json
                .termination_reason
                .and_then(|s| TerminationReason::from_str(&s)),
            exit_code: json.exit_code,
        })
    }

    /// List instances with optional filtering.
    #[instrument(skip(self, options), level = "debug")]
    pub async fn list_instances(
        &self,
        options: ListInstancesOptions,
    ) -> Result<ListInstancesResult> {
        debug!("Listing instances");

        let mut query: Vec<(String, String)> = Vec::new();

        if let Some(ref tenant_id) = options.tenant_id {
            query.push(("tenant_id".to_string(), tenant_id.clone()));
        }
        if let Some(status) = options.status {
            let status_str = match status {
                InstanceStatus::Pending => "pending",
                InstanceStatus::Running => "running",
                InstanceStatus::Suspended => "suspended",
                InstanceStatus::Completed => "completed",
                InstanceStatus::Failed => "failed",
                InstanceStatus::Cancelled => "cancelled",
                InstanceStatus::Unknown => "unknown",
            };
            query.push(("status".to_string(), status_str.to_string()));
        }
        if let Some(ref image_id) = options.image_id {
            query.push(("image_id".to_string(), image_id.clone()));
        }
        if let Some(ref prefix) = options.image_name_prefix {
            query.push(("image_name_prefix".to_string(), prefix.clone()));
        }
        if let Some(created_after) = options.created_after {
            query.push((
                "created_after_ms".to_string(),
                created_after.timestamp_millis().to_string(),
            ));
        }
        if let Some(created_before) = options.created_before {
            query.push((
                "created_before_ms".to_string(),
                created_before.timestamp_millis().to_string(),
            ));
        }
        if let Some(finished_after) = options.finished_after {
            query.push((
                "finished_after_ms".to_string(),
                finished_after.timestamp_millis().to_string(),
            ));
        }
        if let Some(finished_before) = options.finished_before {
            query.push((
                "finished_before_ms".to_string(),
                finished_before.timestamp_millis().to_string(),
            ));
        }
        if let Some(order_by) = options.order_by {
            query.push(("order_by".to_string(), order_by.as_str().to_string()));
        }
        query.push(("limit".to_string(), options.limit.to_string()));
        query.push(("offset".to_string(), options.offset.to_string()));

        let resp = self
            .client
            .get(self.url("/api/v1/instances"))
            .query(&query)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: ListInstancesJson = resp.json().await?;

        let instances = json
            .instances
            .into_iter()
            .map(|inst| InstanceSummary {
                instance_id: inst.instance_id,
                tenant_id: inst.tenant_id,
                image_id: inst.image_id.unwrap_or_default(),
                status: instance_status_from_string(&inst.status),
                created_at: ms_to_datetime(inst.created_at_ms),
                started_at: opt_ms_to_datetime(inst.started_at_ms),
                finished_at: opt_ms_to_datetime(inst.finished_at_ms),
                has_error: inst.has_error,
            })
            .collect();

        Ok(ListInstancesResult {
            instances,
            total_count: json.total_count,
        })
    }

    /// Start a new instance.
    #[instrument(skip(self, options), fields(image_id = %options.image_id, tenant_id = %options.tenant_id))]
    pub async fn start_instance(
        &self,
        options: StartInstanceOptions,
    ) -> Result<StartInstanceResult> {
        info!("Starting instance");

        let body = serde_json::json!({
            "image_id": options.image_id,
            "tenant_id": options.tenant_id,
            "instance_id": options.instance_id,
            "input": options.input,
            "timeout_seconds": options.timeout_seconds,
            "env": options.env,
        });

        let resp = self
            .client
            .post(self.url("/api/v1/instances"))
            .json(&body)
            .send()
            .await?;

        // Server returns 201 on success, 400 on failure — both have JSON body
        let json: StartInstanceJson = if resp.status().is_success() || resp.status().as_u16() == 400
        {
            resp.json().await?
        } else {
            return Err(Self::parse_error_response(resp).await);
        };

        if !json.success
            && let Some(ref error) = json.error
            && error.contains("not found")
        {
            return Err(SdkError::ImageNotFound(error.clone()));
        }

        Ok(StartInstanceResult {
            success: json.success,
            instance_id: json.instance_id.unwrap_or_default(),
            error: json.error,
        })
    }

    /// Stop a running instance.
    #[instrument(skip(self, options), fields(instance_id = %options.instance_id))]
    pub async fn stop_instance(&self, options: StopInstanceOptions) -> Result<()> {
        info!(reason = %options.reason, "Stopping instance");

        let body = serde_json::json!({
            "reason": options.reason,
            "grace_period_seconds": options.grace_period_seconds,
        });

        let resp = self
            .client
            .post(self.url(&format!("/api/v1/instances/{}/stop", options.instance_id)))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: SimpleSuccessJson = resp.json().await?;

        if !json.success {
            let error = json.error.unwrap_or_default();
            if error.contains("not found") {
                return Err(SdkError::InstanceNotFound(options.instance_id));
            }
            return Err(SdkError::Server {
                code: "STOP_FAILED".to_string(),
                message: error,
            });
        }
        Ok(())
    }

    /// Resume a suspended instance.
    #[instrument(skip(self), fields(instance_id = %instance_id))]
    pub async fn resume_instance(&self, instance_id: &str) -> Result<()> {
        info!("Resuming instance");

        let resp = self
            .client
            .post(self.url(&format!("/api/v1/instances/{}/resume", instance_id)))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: SimpleSuccessJson = resp.json().await?;

        if !json.success {
            let error = json.error.unwrap_or_default();
            if error.contains("not found") {
                return Err(SdkError::InstanceNotFound(instance_id.to_string()));
            }
            return Err(SdkError::Server {
                code: "RESUME_FAILED".to_string(),
                message: error,
            });
        }
        Ok(())
    }

    // =========================================================================
    // Image Management
    // =========================================================================

    /// Register a new image.
    #[instrument(skip(self, options), fields(tenant_id = %options.tenant_id, name = %options.name))]
    pub async fn register_image(
        &self,
        options: RegisterImageOptions,
    ) -> Result<RegisterImageResult> {
        info!(
            binary_size = options.binary.len(),
            runner_type = ?options.runner_type,
            "Registering image"
        );

        let binary_b64 = base64::engine::general_purpose::STANDARD.encode(&options.binary);

        let body = serde_json::json!({
            "tenant_id": options.tenant_id,
            "name": options.name,
            "description": options.description,
            "binary": binary_b64,
            "runner_type": runner_type_to_string(options.runner_type),
            "metadata": options.metadata,
        });

        let resp = self
            .client
            .post(self.url("/api/v1/images"))
            .json(&body)
            .send()
            .await?;

        let json: RegisterImageJson = if resp.status().is_success() || resp.status().as_u16() == 400
        {
            resp.json().await?
        } else {
            return Err(Self::parse_error_response(resp).await);
        };

        Ok(RegisterImageResult {
            success: json.success,
            image_id: json.image_id.unwrap_or_default(),
            error: json.error,
        })
    }

    /// Register a new image using streaming upload via multipart form.
    ///
    /// For HTTP, this uses multipart/form-data upload to the `/api/v1/images/upload` endpoint.
    #[instrument(skip(self, options, reader), fields(tenant_id = %options.tenant_id, name = %options.name, binary_size = options.binary_size))]
    pub async fn register_image_stream<R: tokio::io::AsyncRead + Unpin>(
        &self,
        options: RegisterImageStreamOptions,
        mut reader: R,
    ) -> Result<RegisterImageResult> {
        info!(
            binary_size = options.binary_size,
            runner_type = ?options.runner_type,
            "Registering image via streaming (HTTP multipart)"
        );

        // Read the entire stream into memory for multipart upload
        let mut binary_data = Vec::with_capacity(options.binary_size as usize);
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut binary_data).await?;

        let mut form = reqwest::multipart::Form::new()
            .text("tenant_id", options.tenant_id)
            .text("name", options.name);

        if let Some(description) = options.description {
            form = form.text("description", description);
        }

        form = form.text(
            "runner_type",
            runner_type_to_string(options.runner_type).to_string(),
        );

        if let Some(metadata) = options.metadata {
            form = form.text("metadata", serde_json::to_string(&metadata)?);
        }

        if let Some(sha256) = options.sha256 {
            form = form.text("sha256", sha256);
        }

        let binary_part = reqwest::multipart::Part::bytes(binary_data)
            .file_name("binary")
            .mime_str("application/octet-stream")
            .map_err(|e| SdkError::Connection(format!("Failed to set MIME type: {}", e)))?;
        form = form.part("binary", binary_part);

        let resp = self
            .client
            .post(self.url("/api/v1/images/upload"))
            .multipart(form)
            .send()
            .await?;

        let json: RegisterImageJson = if resp.status().is_success() || resp.status().as_u16() == 400
        {
            resp.json().await?
        } else {
            return Err(Self::parse_error_response(resp).await);
        };

        Ok(RegisterImageResult {
            success: json.success,
            image_id: json.image_id.unwrap_or_default(),
            error: json.error,
        })
    }

    /// List images with optional filtering.
    #[instrument(skip(self, options), level = "debug")]
    pub async fn list_images(&self, options: ListImagesOptions) -> Result<ListImagesResult> {
        debug!("Listing images");

        let mut query: Vec<(String, String)> = Vec::new();

        if let Some(ref tenant_id) = options.tenant_id {
            query.push(("tenant_id".to_string(), tenant_id.clone()));
        }
        query.push(("limit".to_string(), options.limit.to_string()));
        query.push(("offset".to_string(), options.offset.to_string()));

        let resp = self
            .client
            .get(self.url("/api/v1/images"))
            .query(&query)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: ListImagesJson = resp.json().await?;

        let images = json
            .images
            .into_iter()
            .map(|img| ImageSummary {
                image_id: img.image_id,
                tenant_id: img.tenant_id,
                name: img.name,
                description: img.description,
                runner_type: runner_type_from_string(&img.runner_type),
                created_at: ms_to_datetime(img.created_at_ms),
                metadata: img.metadata,
            })
            .collect();

        Ok(ListImagesResult {
            images,
            total_count: json.total_count,
        })
    }

    /// Get information about a specific image.
    #[instrument(skip(self), fields(image_id = %image_id, tenant_id = %tenant_id), level = "debug")]
    pub async fn get_image(&self, image_id: &str, tenant_id: &str) -> Result<Option<ImageSummary>> {
        debug!("Getting image");

        let resp = self
            .client
            .get(self.url(&format!("/api/v1/images/{}", image_id)))
            .query(&[("tenant_id", tenant_id)])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: GetImageJson = resp.json().await?;

        if !json.found {
            return Ok(None);
        }

        match json.image {
            Some(img) => Ok(Some(ImageSummary {
                image_id: img.image_id,
                tenant_id: img.tenant_id,
                name: img.name,
                description: img.description,
                runner_type: runner_type_from_string(&img.runner_type),
                created_at: ms_to_datetime(img.created_at_ms),
                metadata: img.metadata,
            })),
            None => Ok(None),
        }
    }

    /// Delete an image.
    #[instrument(skip(self), fields(image_id = %image_id, tenant_id = %tenant_id))]
    pub async fn delete_image(&self, image_id: &str, tenant_id: &str) -> Result<()> {
        info!("Deleting image");

        let resp = self
            .client
            .delete(self.url(&format!("/api/v1/images/{}", image_id)))
            .query(&[("tenant_id", tenant_id)])
            .send()
            .await?;

        if resp.status().as_u16() == 404 {
            return Err(SdkError::ImageNotFound(image_id.to_string()));
        }

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: SimpleSuccessJson = resp.json().await?;

        if !json.success {
            let error = json.error.unwrap_or_default();
            if error.contains("not found") {
                return Err(SdkError::ImageNotFound(image_id.to_string()));
            }
            return Err(SdkError::Server {
                code: "DELETE_FAILED".to_string(),
                message: error,
            });
        }
        Ok(())
    }

    // =========================================================================
    // Signal Operations
    // =========================================================================

    /// Send a signal to an instance.
    #[instrument(skip(self), fields(instance_id = %instance_id, signal = ?signal_type))]
    pub async fn send_signal(
        &self,
        instance_id: &str,
        signal_type: SignalType,
        payload: Option<&[u8]>,
    ) -> Result<()> {
        info!("Sending signal to instance");

        // Resume is handled via resume_instance()
        if signal_type == SignalType::Resume {
            return self.resume_instance(instance_id).await;
        }

        let signal_str = match signal_type {
            SignalType::Cancel => "cancel",
            SignalType::Pause => "pause",
            SignalType::Shutdown => "shutdown",
            SignalType::Resume => unreachable!(),
        };

        let payload_str = payload.map(|p| String::from_utf8_lossy(p).to_string());

        let body = serde_json::json!({
            "signal_type": signal_str,
            "payload": payload_str,
        });

        let resp = self
            .client
            .post(self.url(&format!("/api/v1/instances/{}/signals", instance_id)))
            .json(&body)
            .send()
            .await?;

        if resp.status().as_u16() == 404 {
            return Err(SdkError::InstanceNotFound(instance_id.to_string()));
        }

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: SimpleSuccessJson = resp.json().await?;

        if !json.success {
            let error = json.error.unwrap_or_default();
            if error.contains("not found") {
                return Err(SdkError::InstanceNotFound(instance_id.to_string()));
            }
            return Err(SdkError::Server {
                code: "SIGNAL_FAILED".to_string(),
                message: error,
            });
        }
        Ok(())
    }

    /// Send a cancel signal to an instance.
    pub async fn cancel_instance(&self, instance_id: &str, reason: Option<&str>) -> Result<()> {
        let payload = reason.map(|r| r.as_bytes());
        self.send_signal(instance_id, SignalType::Cancel, payload)
            .await
    }

    /// Send a pause signal to an instance.
    pub async fn pause_instance(&self, instance_id: &str) -> Result<()> {
        self.send_signal(instance_id, SignalType::Pause, None).await
    }

    /// Send a shutdown signal to an instance. The SDK is expected to suspend
    /// at the next checkpoint boundary so the instance can be resumed after
    /// server restart.
    pub async fn shutdown_instance(&self, instance_id: &str) -> Result<()> {
        self.send_signal(instance_id, SignalType::Shutdown, None)
            .await
    }

    /// Send a custom signal to a specific checkpoint/signal ID.
    #[instrument(skip(self, payload), fields(instance_id = %instance_id, signal_id = %signal_id))]
    pub async fn send_custom_signal(
        &self,
        instance_id: &str,
        signal_id: &str,
        payload: Option<&[u8]>,
    ) -> Result<()> {
        info!("Sending custom signal to instance");

        let payload_str = payload.map(|p| String::from_utf8_lossy(p).to_string());

        let body = serde_json::json!({
            "checkpoint_id": signal_id,
            "payload": payload_str,
        });

        let resp = self
            .client
            .post(self.url(&format!("/api/v1/instances/{}/signals/custom", instance_id)))
            .json(&body)
            .send()
            .await?;

        if resp.status().as_u16() == 404 {
            return Err(SdkError::InstanceNotFound(instance_id.to_string()));
        }

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: SimpleSuccessJson = resp.json().await?;

        if !json.success {
            let error = json.error.unwrap_or_default();
            if error.contains("not found") {
                return Err(SdkError::InstanceNotFound(instance_id.to_string()));
            }
            return Err(SdkError::Server {
                code: "CUSTOM_SIGNAL_FAILED".to_string(),
                message: error,
            });
        }
        Ok(())
    }

    // =========================================================================
    // Checkpoints
    // =========================================================================

    /// List checkpoints for an instance.
    #[instrument(skip(self, options), fields(instance_id = %instance_id), level = "debug")]
    pub async fn list_checkpoints(
        &self,
        instance_id: &str,
        options: ListCheckpointsOptions,
    ) -> Result<ListCheckpointsResult> {
        debug!("Listing checkpoints");

        let mut query: Vec<(String, String)> = Vec::new();

        if let Some(ref checkpoint_id) = options.checkpoint_id {
            query.push(("checkpoint_id".to_string(), checkpoint_id.clone()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        if let Some(offset) = options.offset {
            query.push(("offset".to_string(), offset.to_string()));
        }
        if let Some(created_after) = options.created_after {
            query.push((
                "created_after_ms".to_string(),
                created_after.timestamp_millis().to_string(),
            ));
        }
        if let Some(created_before) = options.created_before {
            query.push((
                "created_before_ms".to_string(),
                created_before.timestamp_millis().to_string(),
            ));
        }

        let resp = self
            .client
            .get(self.url(&format!("/api/v1/instances/{}/checkpoints", instance_id)))
            .query(&query)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: ListCheckpointsJson = resp.json().await?;

        let checkpoints = json
            .checkpoints
            .into_iter()
            .map(|cp| CheckpointSummary {
                checkpoint_id: cp.checkpoint_id,
                instance_id: cp.instance_id,
                created_at: ms_to_datetime(cp.created_at_ms),
                data_size_bytes: cp.data_size_bytes,
            })
            .collect();

        Ok(ListCheckpointsResult {
            checkpoints,
            total_count: json.total_count,
            limit: json.limit as u32,
            offset: json.offset as u32,
        })
    }

    /// Get a specific checkpoint with its full data.
    #[instrument(skip(self), fields(instance_id = %instance_id, checkpoint_id = %checkpoint_id), level = "debug")]
    pub async fn get_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<Checkpoint>> {
        debug!("Getting checkpoint");

        // Percent-encode the checkpoint_id since it may contain slashes
        let encoded_checkpoint_id = percent_encoding::utf8_percent_encode(
            checkpoint_id,
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();

        let resp = self
            .client
            .get(self.url(&format!(
                "/api/v1/instances/{}/checkpoints/{}",
                instance_id, encoded_checkpoint_id
            )))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: CheckpointDetailJson = resp.json().await?;

        if !json.found {
            return Ok(None);
        }

        // Decode base64 checkpoint data as JSON
        let data = match json.data {
            Some(encoded) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&encoded)
                    .map_err(|e| {
                        SdkError::UnexpectedResponse(format!(
                            "Failed to decode checkpoint data base64: {}",
                            e
                        ))
                    })?;
                if bytes.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_slice(&bytes).map_err(|e| {
                        SdkError::UnexpectedResponse(format!(
                            "Failed to parse checkpoint data as JSON: {}",
                            e
                        ))
                    })?
                }
            }
            None => serde_json::Value::Null,
        };

        Ok(Some(Checkpoint {
            checkpoint_id: json.checkpoint_id,
            instance_id: json.instance_id,
            created_at: ms_to_datetime(json.created_at_ms),
            data,
        }))
    }

    // =========================================================================
    // Events
    // =========================================================================

    /// List events for an instance with optional filtering.
    #[instrument(skip(self, options), fields(instance_id = %instance_id), level = "debug")]
    pub async fn list_events(
        &self,
        instance_id: &str,
        options: ListEventsOptions,
    ) -> Result<ListEventsResult> {
        debug!("Listing events");

        let mut query: Vec<(String, String)> = Vec::new();

        if let Some(ref event_type) = options.event_type {
            query.push(("event_type".to_string(), event_type.clone()));
        }
        if let Some(ref subtype) = options.subtype {
            query.push(("subtype".to_string(), subtype.clone()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        if let Some(offset) = options.offset {
            query.push(("offset".to_string(), offset.to_string()));
        }
        if let Some(created_after) = options.created_after {
            query.push((
                "created_after_ms".to_string(),
                created_after.timestamp_millis().to_string(),
            ));
        }
        if let Some(created_before) = options.created_before {
            query.push((
                "created_before_ms".to_string(),
                created_before.timestamp_millis().to_string(),
            ));
        }
        if let Some(ref payload_contains) = options.payload_contains {
            query.push(("payload_contains".to_string(), payload_contains.clone()));
        }
        if let Some(ref scope_id) = options.scope_id {
            query.push(("scope_id".to_string(), scope_id.clone()));
        }
        if let Some(ref parent_scope_id) = options.parent_scope_id {
            query.push(("parent_scope_id".to_string(), parent_scope_id.clone()));
        }
        if options.root_scopes_only {
            query.push(("root_scopes_only".to_string(), "true".to_string()));
        }
        if let Some(sort_order) = options.sort_order {
            query.push(("sort_order".to_string(), sort_order.as_str().to_string()));
        }

        let resp = self
            .client
            .get(self.url(&format!("/api/v1/instances/{}/events", instance_id)))
            .query(&query)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: ListEventsJson = resp.json().await?;

        let events = json
            .events
            .into_iter()
            .map(|ev| {
                // Decode base64 payload as JSON if present
                let payload = ev.payload.as_deref().and_then(decode_base64_json);

                EventSummary {
                    id: ev.id,
                    instance_id: ev.instance_id,
                    event_type: ev.event_type,
                    checkpoint_id: ev.checkpoint_id,
                    payload,
                    created_at: ms_to_datetime(ev.created_at_ms),
                    subtype: ev.subtype,
                }
            })
            .collect();

        Ok(ListEventsResult {
            events,
            total_count: json.total_count,
            limit: json.limit as u32,
            offset: json.offset as u32,
        })
    }

    /// Get the ancestors of a scope in the execution hierarchy.
    #[instrument(skip(self), fields(instance_id = %instance_id, scope_id = %scope_id), level = "debug")]
    pub async fn get_scope_ancestors(
        &self,
        instance_id: &str,
        scope_id: &str,
    ) -> Result<Vec<ScopeInfo>> {
        debug!("Getting scope ancestors");

        let resp = self
            .client
            .get(self.url(&format!(
                "/api/v1/instances/{}/scopes/{}/ancestors",
                instance_id, scope_id
            )))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: ScopeAncestorsJson = resp.json().await?;

        let ancestors = json
            .ancestors
            .into_iter()
            .map(|info| ScopeInfo {
                scope_id: info.scope_id,
                parent_scope_id: info.parent_scope_id,
                step_id: info.step_id,
                step_name: info.step_name,
                step_type: info.step_type,
                index: info.index,
                created_at: ms_to_datetime(info.created_at_ms),
            })
            .collect();

        Ok(ancestors)
    }

    /// List step summaries for an instance.
    #[instrument(skip(self, options), fields(instance_id = %instance_id), level = "debug")]
    pub async fn list_step_summaries(
        &self,
        instance_id: &str,
        options: ListStepSummariesOptions,
    ) -> Result<ListStepSummariesResult> {
        debug!("Listing step summaries");

        let mut query: Vec<(String, String)> = Vec::new();

        if let Some(status) = options.status {
            let status_str = match status {
                StepStatus::Running => "running",
                StepStatus::Completed => "completed",
                StepStatus::Failed => "failed",
            };
            query.push(("status".to_string(), status_str.to_string()));
        }
        if let Some(ref step_type) = options.step_type {
            query.push(("step_type".to_string(), step_type.clone()));
        }
        if let Some(ref scope_id) = options.scope_id {
            query.push(("scope_id".to_string(), scope_id.clone()));
        }
        if let Some(ref parent_scope_id) = options.parent_scope_id {
            query.push(("parent_scope_id".to_string(), parent_scope_id.clone()));
        }
        if options.root_scopes_only {
            query.push(("root_scopes_only".to_string(), "true".to_string()));
        }
        if let Some(sort_order) = options.sort_order {
            query.push(("sort_order".to_string(), sort_order.as_str().to_string()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        if let Some(offset) = options.offset {
            query.push(("offset".to_string(), offset.to_string()));
        }

        let resp = self
            .client
            .get(self.url(&format!("/api/v1/instances/{}/steps", instance_id)))
            .query(&query)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: ListStepSummariesJson = resp.json().await?;

        let steps = json
            .steps
            .into_iter()
            .map(|step| StepSummary {
                step_id: step.step_id,
                step_name: step.step_name,
                step_type: step.step_type,
                status: step_status_from_string(&step.status),
                started_at: ms_to_datetime(step.started_at_ms),
                completed_at: opt_ms_to_datetime(step.completed_at_ms),
                duration_ms: step.duration_ms,
                inputs: step.inputs,
                outputs: step.outputs,
                error: step.error,
                scope_id: step.scope_id,
                parent_scope_id: step.parent_scope_id,
            })
            .collect();

        Ok(ListStepSummariesResult {
            steps,
            total_count: json.total_count,
            limit: json.limit as u32,
            offset: json.offset as u32,
        })
    }

    // =========================================================================
    // Agent Testing
    // =========================================================================

    /// Test a single agent capability.
    #[instrument(skip(self, options), fields(agent_id = %options.agent_id, capability_id = %options.capability_id))]
    pub async fn test_capability(
        &self,
        options: TestCapabilityOptions,
    ) -> Result<TestCapabilityResult> {
        info!("Testing capability");

        let body = serde_json::json!({
            "tenant_id": options.tenant_id,
            "agent_id": options.agent_id,
            "capability_id": options.capability_id,
            "input": options.input,
            "connection": options.connection,
            "timeout_ms": options.timeout_ms,
        });

        let resp = self
            .client
            .post(self.url("/api/v1/agents/test"))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() && resp.status().as_u16() != 200 {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: TestCapabilityJson = resp.json().await?;

        Ok(TestCapabilityResult {
            success: json.success,
            output: json.output,
            error: json.error,
            execution_time_ms: json.execution_time_ms,
        })
    }

    /// List all available agents and their capabilities.
    #[instrument(skip(self), level = "debug")]
    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>> {
        debug!("Listing agents");

        let resp = self.client.get(self.url("/api/v1/agents")).send().await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: ListAgentsJson = resp.json().await?;
        Ok(json.agents)
    }

    /// Get details about a specific capability including its input schema.
    #[instrument(skip(self), fields(agent_id = %agent_id, capability_id = %capability_id), level = "debug")]
    pub async fn get_capability(
        &self,
        agent_id: &str,
        capability_id: &str,
    ) -> Result<Option<Vec<CapabilityField>>> {
        debug!("Getting capability details");

        let resp = self
            .client
            .get(self.url(&format!(
                "/api/v1/agents/{}/capabilities/{}",
                agent_id, capability_id
            )))
            .send()
            .await?;

        let status = resp.status();

        // 404 means not found
        if status.as_u16() == 404 {
            let json: GetCapabilityJson = resp.json().await?;
            if !json.found {
                return Ok(None);
            }
            // Should not reach here, but if found is true at 404, fall through
            return Ok(json.inputs);
        }

        if !status.is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: GetCapabilityJson = resp.json().await?;

        if !json.found {
            return Ok(None);
        }

        Ok(json.inputs)
    }

    // =========================================================================
    // Tenant Metrics
    // =========================================================================

    /// Get aggregated execution metrics for a tenant.
    #[instrument(skip(self, options), fields(tenant_id = %options.tenant_id), level = "debug")]
    pub async fn get_tenant_metrics(
        &self,
        options: GetTenantMetricsOptions,
    ) -> Result<TenantMetricsResult> {
        debug!("Getting tenant metrics");

        if options.tenant_id.is_empty() {
            return Err(SdkError::InvalidInput("tenant_id is required".to_string()));
        }

        let mut query: Vec<(String, String)> = Vec::new();

        if let Some(start_time) = options.start_time {
            query.push((
                "start_time_ms".to_string(),
                start_time.timestamp_millis().to_string(),
            ));
        }
        if let Some(end_time) = options.end_time {
            query.push((
                "end_time_ms".to_string(),
                end_time.timestamp_millis().to_string(),
            ));
        }
        if let Some(granularity) = options.granularity {
            let gran_str = match granularity {
                MetricsGranularity::Hourly => "hourly",
                MetricsGranularity::Daily => "daily",
            };
            query.push(("granularity".to_string(), gran_str.to_string()));
        }

        let resp = self
            .client
            .get(self.url(&format!("/api/v1/tenants/{}/metrics", options.tenant_id)))
            .query(&query)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(Self::parse_error_response(resp).await);
        }

        let json: TenantMetricsJson = resp.json().await?;

        let granularity = match json.granularity.as_str() {
            "daily" => MetricsGranularity::Daily,
            _ => MetricsGranularity::Hourly,
        };

        let buckets = json
            .buckets
            .into_iter()
            .map(|b| MetricsBucket {
                bucket_time: ms_to_datetime(b.bucket_time_ms),
                invocation_count: b.invocation_count,
                success_count: b.success_count,
                failure_count: b.failure_count,
                cancelled_count: b.cancelled_count,
                // Convert milliseconds to seconds for SDK API
                avg_duration_seconds: b.avg_duration_ms.map(|ms| ms / 1000.0),
                min_duration_seconds: b.min_duration_ms.map(|ms| ms / 1000.0),
                max_duration_seconds: b.max_duration_ms.map(|ms| ms / 1000.0),
                avg_memory_bytes: b.avg_memory_bytes,
                max_memory_bytes: b.max_memory_bytes,
                success_rate_percent: b.success_rate_percent,
            })
            .collect();

        Ok(TenantMetricsResult {
            tenant_id: json.tenant_id,
            start_time: ms_to_datetime(json.start_time_ms),
            end_time: ms_to_datetime(json.end_time_ms),
            granularity,
            buckets,
        })
    }

    // =========================================================================
    // Convenience Methods
    // =========================================================================

    /// Wait for an instance to reach a terminal state.
    #[instrument(skip(self), fields(instance_id = %instance_id), level = "debug")]
    pub async fn wait_for_completion(
        &self,
        instance_id: &str,
        poll_interval: std::time::Duration,
    ) -> Result<InstanceInfo> {
        loop {
            let info = self.get_instance_status(instance_id).await?;
            if info.status.is_terminal() {
                return Ok(info);
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Start an instance and wait for completion.
    pub async fn run_instance(
        &self,
        options: StartInstanceOptions,
        poll_interval: std::time::Duration,
    ) -> Result<InstanceInfo> {
        let result = self.start_instance(options).await?;
        if !result.success {
            return Err(SdkError::Server {
                code: "START_FAILED".to_string(),
                message: result.error.unwrap_or_else(|| "Unknown error".to_string()),
            });
        }

        self.wait_for_completion(&result.instance_id, poll_interval)
            .await
    }
}
