// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! ManagementSdk client for interacting with runtara-environment.

use chrono::{TimeZone, Utc};
use tracing::{debug, info, instrument};

use runtara_protocol::client::{RuntaraClient, RuntaraClientConfig};
use runtara_protocol::environment_proto::{
    DeleteImageRequest, GetCapabilityRequest, GetCheckpointRequest, GetImageRequest,
    GetInstanceStatusRequest, GetScopeAncestorsRequest, GetTenantMetricsRequest,
    HealthCheckRequest, ListAgentsRequest, ListCheckpointsRequest, ListEventsRequest,
    ListImagesRequest, ListInstancesRequest, ListStepSummariesRequest, RegisterImageRequest,
    RegisterImageStreamStart, ResumeInstanceRequest, RpcRequest, RpcResponse,
    SendCustomSignalRequest, SendSignalRequest, StartInstanceRequest, StopInstanceRequest,
    TestCapabilityRequest, rpc_request::Request, rpc_response::Response,
};
use runtara_protocol::frame::{Frame, write_frame};
use tokio::io::AsyncRead;

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
    StepSummary, StopInstanceOptions, TenantMetricsResult, TestCapabilityOptions,
    TestCapabilityResult,
};

/// High-level SDK for managing runtara-environment instances and images.
///
/// This client wraps the low-level QUIC protocol and provides ergonomic methods
/// for management operations like health checks, starting/stopping instances,
/// managing images, and sending signals.
///
/// The Management SDK talks ONLY to runtara-environment. Environment handles
/// all image registry and instance lifecycle operations. For signals (pause, cancel),
/// Environment proxies the request to runtara-core internally.
pub struct ManagementSdk {
    client: RuntaraClient,
    config: SdkConfig,
}

impl ManagementSdk {
    /// Create a new SDK with the given configuration.
    pub fn new(config: SdkConfig) -> Result<Self> {
        let client_config = RuntaraClientConfig {
            server_addr: config.server_addr,
            server_name: config.server_name.clone(),
            enable_0rtt: true,
            dangerous_skip_cert_verification: config.skip_cert_verification,
            keep_alive_interval_ms: 10_000,
            idle_timeout_ms: config.request_timeout.as_millis() as u64,
            connect_timeout_ms: config.connect_timeout.as_millis() as u64,
        };

        let client = RuntaraClient::new(client_config)?;

        Ok(Self { client, config })
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
    #[instrument(skip(self))]
    pub async fn connect(&self) -> Result<()> {
        self.client.connect().await?;
        info!("Connected to runtara-environment");
        Ok(())
    }

    /// Close the connection.
    pub async fn close(&self) {
        self.client.close().await;
    }

    /// Check if connected.
    pub async fn is_connected(&self) -> bool {
        self.client.is_connected().await
    }

    /// Get the SDK configuration.
    pub fn config(&self) -> &SdkConfig {
        &self.config
    }

    // =========================================================================
    // Internal helpers
    // =========================================================================

    /// Send a request and receive a response.
    async fn send_request(&self, request: Request) -> Result<Response> {
        let rpc_request = RpcRequest {
            request: Some(request),
        };

        let rpc_response: RpcResponse = self.client.request(&rpc_request).await?;

        match rpc_response.response {
            Some(Response::Error(err)) => Err(SdkError::Server {
                code: err.code,
                message: err.message,
            }),
            Some(response) => Ok(response),
            None => Err(SdkError::UnexpectedResponse(
                "empty response from server".to_string(),
            )),
        }
    }

    // =========================================================================
    // Health & Status
    // =========================================================================

    /// Check health of runtara-environment.
    #[instrument(skip(self))]
    pub async fn health_check(&self) -> Result<HealthStatus> {
        debug!("Performing health check");

        let response = self
            .send_request(Request::HealthCheck(HealthCheckRequest {}))
            .await?;

        match response {
            Response::HealthCheck(resp) => Ok(HealthStatus {
                healthy: resp.healthy,
                version: resp.version,
                uptime_ms: resp.uptime_ms,
                active_instances: resp.active_instances,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected HealthCheckResponse".to_string(),
            )),
        }
    }

    // =========================================================================
    // Instance Management
    // =========================================================================

    /// Get status of a specific instance.
    #[instrument(skip(self), fields(instance_id = %instance_id))]
    pub async fn get_instance_status(&self, instance_id: &str) -> Result<InstanceInfo> {
        debug!("Getting instance status");

        let response = self
            .send_request(Request::GetInstanceStatus(GetInstanceStatusRequest {
                instance_id: instance_id.to_string(),
            }))
            .await?;

        match response {
            Response::GetInstanceStatus(resp) => {
                // Check if instance was not found using explicit `found` field
                if !resp.found {
                    return Err(SdkError::InstanceNotFound(instance_id.to_string()));
                }

                Ok(InstanceInfo {
                    instance_id: resp.instance_id,
                    image_id: resp.image_id,
                    image_name: resp.image_name,
                    tenant_id: resp.tenant_id,
                    status: InstanceStatus::from(resp.status),
                    checkpoint_id: resp.checkpoint_id,
                    created_at: Utc
                        .timestamp_millis_opt(resp.created_at_ms)
                        .single()
                        .unwrap_or_else(Utc::now),
                    started_at: resp
                        .started_at_ms
                        .and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                    finished_at: resp
                        .finished_at_ms
                        .and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                    heartbeat_at: resp
                        .heartbeat_at_ms
                        .and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                    input: resp
                        .input
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok()),
                    output: resp
                        .output
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok()),
                    error: resp.error,
                    stderr: resp.stderr,
                    retry_count: resp.retry_count,
                    max_retries: resp.max_retries,
                    memory_peak_bytes: resp.memory_peak_bytes,
                    cpu_usage_usec: resp.cpu_usage_usec,
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetInstanceStatusResponse".to_string(),
            )),
        }
    }

    /// List instances with optional filtering.
    #[instrument(skip(self))]
    pub async fn list_instances(
        &self,
        options: ListInstancesOptions,
    ) -> Result<ListInstancesResult> {
        debug!("Listing instances");

        let response = self
            .send_request(Request::ListInstances(ListInstancesRequest {
                tenant_id: options.tenant_id,
                status: options.status.map(i32::from),
                limit: options.limit,
                offset: options.offset,
                image_id: options.image_id,
                created_after_ms: options.created_after.map(|t| t.timestamp_millis()),
                created_before_ms: options.created_before.map(|t| t.timestamp_millis()),
                finished_after_ms: options.finished_after.map(|t| t.timestamp_millis()),
                finished_before_ms: options.finished_before.map(|t| t.timestamp_millis()),
                order_by: options.order_by.map(|o| o.as_str().to_string()),
                image_name_prefix: options.image_name_prefix,
            }))
            .await?;

        match response {
            Response::ListInstances(resp) => {
                let instances = resp
                    .instances
                    .into_iter()
                    .map(|inst| InstanceSummary {
                        instance_id: inst.instance_id,
                        tenant_id: inst.tenant_id,
                        image_id: inst.image_id,
                        status: InstanceStatus::from(inst.status),
                        created_at: Utc
                            .timestamp_millis_opt(inst.created_at_ms)
                            .single()
                            .unwrap_or_else(Utc::now),
                        started_at: inst
                            .started_at_ms
                            .and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                        finished_at: inst
                            .finished_at_ms
                            .and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                        has_error: inst.has_error,
                    })
                    .collect();

                Ok(ListInstancesResult {
                    instances,
                    total_count: resp.total_count,
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected ListInstancesResponse".to_string(),
            )),
        }
    }

    /// Start a new instance.
    #[instrument(skip(self, options), fields(image_id = %options.image_id, tenant_id = %options.tenant_id))]
    pub async fn start_instance(
        &self,
        options: StartInstanceOptions,
    ) -> Result<StartInstanceResult> {
        info!("Starting instance");

        let input_bytes = match &options.input {
            Some(value) => serde_json::to_vec(value)?,
            None => Vec::new(),
        };

        let response = self
            .send_request(Request::StartInstance(StartInstanceRequest {
                image_id: options.image_id,
                tenant_id: options.tenant_id,
                instance_id: options.instance_id,
                input: input_bytes,
                timeout_seconds: options.timeout_seconds,
                env: options.env,
            }))
            .await?;

        match response {
            Response::StartInstance(resp) => {
                if !resp.success && !resp.error.is_empty() {
                    // Check for specific error types
                    if resp.error.contains("not found") {
                        return Err(SdkError::ImageNotFound(resp.error));
                    }
                }

                Ok(StartInstanceResult {
                    success: resp.success,
                    instance_id: resp.instance_id,
                    error: if resp.error.is_empty() {
                        None
                    } else {
                        Some(resp.error)
                    },
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected StartInstanceResponse".to_string(),
            )),
        }
    }

    /// Stop a running instance.
    #[instrument(skip(self, options), fields(instance_id = %options.instance_id))]
    pub async fn stop_instance(&self, options: StopInstanceOptions) -> Result<()> {
        info!(reason = %options.reason, "Stopping instance");

        let response = self
            .send_request(Request::StopInstance(StopInstanceRequest {
                instance_id: options.instance_id.clone(),
                grace_period_seconds: options.grace_period_seconds,
                reason: options.reason,
            }))
            .await?;

        match response {
            Response::StopInstance(resp) => {
                if !resp.success {
                    if resp.error.contains("not found") {
                        return Err(SdkError::InstanceNotFound(options.instance_id));
                    }
                    return Err(SdkError::Server {
                        code: "STOP_FAILED".to_string(),
                        message: resp.error,
                    });
                }
                Ok(())
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected StopInstanceResponse".to_string(),
            )),
        }
    }

    /// Resume a suspended instance.
    ///
    /// This relaunches an instance that was paused via signal or is waiting after durable sleep.
    #[instrument(skip(self), fields(instance_id = %instance_id))]
    pub async fn resume_instance(&self, instance_id: &str) -> Result<()> {
        info!("Resuming instance");

        let response = self
            .send_request(Request::ResumeInstance(ResumeInstanceRequest {
                instance_id: instance_id.to_string(),
            }))
            .await?;

        match response {
            Response::ResumeInstance(resp) => {
                if !resp.success {
                    if resp.error.contains("not found") {
                        return Err(SdkError::InstanceNotFound(instance_id.to_string()));
                    }
                    return Err(SdkError::Server {
                        code: "RESUME_FAILED".to_string(),
                        message: resp.error,
                    });
                }
                Ok(())
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected ResumeInstanceResponse".to_string(),
            )),
        }
    }

    // =========================================================================
    // Image Management
    // =========================================================================

    /// Register a new image.
    ///
    /// This uploads a compiled binary to runtara-environment, which will:
    /// 1. Store the binary on disk
    /// 2. Create an OCI bundle (for OCI runner type)
    /// 3. Register the image in the database
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

        let metadata_bytes = match &options.metadata {
            Some(value) => Some(serde_json::to_vec(value)?),
            None => None,
        };

        let response = self
            .send_request(Request::RegisterImage(RegisterImageRequest {
                tenant_id: options.tenant_id,
                name: options.name,
                description: options.description,
                binary: options.binary,
                runner_type: i32::from(options.runner_type),
                metadata: metadata_bytes,
            }))
            .await?;

        match response {
            Response::RegisterImage(resp) => Ok(RegisterImageResult {
                success: resp.success,
                image_id: resp.image_id,
                error: if resp.error.is_empty() {
                    None
                } else {
                    Some(resp.error)
                },
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected RegisterImageResponse".to_string(),
            )),
        }
    }

    /// Register a new image using streaming upload.
    ///
    /// This method streams the binary data directly from a reader, avoiding the need
    /// to hold the entire binary in memory. Use this for large binaries.
    #[instrument(skip(self, options, reader), fields(tenant_id = %options.tenant_id, name = %options.name, binary_size = options.binary_size))]
    pub async fn register_image_stream<R: AsyncRead + Unpin>(
        &self,
        options: RegisterImageStreamOptions,
        mut reader: R,
    ) -> Result<RegisterImageResult> {
        info!(
            binary_size = options.binary_size,
            runner_type = ?options.runner_type,
            "Registering image via streaming"
        );

        let metadata_bytes = match &options.metadata {
            Some(value) => Some(serde_json::to_vec(value)?),
            None => None,
        };

        // 1. Open a raw QUIC stream
        let (mut send, mut recv) = self.client.open_raw_stream().await?;

        // 2. Send the start frame with metadata
        let start_request = RpcRequest {
            request: Some(Request::RegisterImageStream(RegisterImageStreamStart {
                tenant_id: options.tenant_id,
                name: options.name,
                description: options.description,
                binary_size: options.binary_size,
                runner_type: i32::from(options.runner_type),
                metadata: metadata_bytes,
                sha256: options.sha256,
            })),
        };

        let frame = Frame::request(&start_request)?;
        write_frame(&mut send, &frame).await?;

        // 3. Stream the binary data
        let mut buf = [0u8; 64 * 1024]; // 64KB chunks
        let mut total_sent = 0u64;

        loop {
            use tokio::io::AsyncReadExt;
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            send.write_all(&buf[..n]).await?;
            total_sent += n as u64;
        }

        debug!(total_sent, "Finished streaming binary data");

        // 4. Signal end of data
        send.finish()?;

        // 5. Read the response
        let response_frame = runtara_protocol::frame::read_frame(&mut recv).await?;
        let rpc_response: RpcResponse = response_frame.decode()?;

        match rpc_response.response {
            Some(Response::Error(err)) => Err(SdkError::Server {
                code: err.code,
                message: err.message,
            }),
            Some(Response::RegisterImage(resp)) => Ok(RegisterImageResult {
                success: resp.success,
                image_id: resp.image_id,
                error: if resp.error.is_empty() {
                    None
                } else {
                    Some(resp.error)
                },
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected RegisterImageResponse".to_string(),
            )),
        }
    }

    /// List images with optional filtering.
    #[instrument(skip(self))]
    pub async fn list_images(&self, options: ListImagesOptions) -> Result<ListImagesResult> {
        debug!("Listing images");

        let response = self
            .send_request(Request::ListImages(ListImagesRequest {
                tenant_id: options.tenant_id,
                limit: options.limit,
                offset: options.offset,
            }))
            .await?;

        match response {
            Response::ListImages(resp) => {
                let images = resp
                    .images
                    .into_iter()
                    .map(|img| ImageSummary {
                        image_id: img.image_id,
                        tenant_id: img.tenant_id,
                        name: img.name,
                        description: img.description,
                        runner_type: runner_type_from_i32(img.runner_type),
                        created_at: Utc
                            .timestamp_millis_opt(img.created_at_ms)
                            .single()
                            .unwrap_or_else(Utc::now),
                    })
                    .collect();

                Ok(ListImagesResult {
                    images,
                    total_count: resp.total_count,
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected ListImagesResponse".to_string(),
            )),
        }
    }

    /// Get information about a specific image.
    #[instrument(skip(self), fields(image_id = %image_id, tenant_id = %tenant_id))]
    pub async fn get_image(&self, image_id: &str, tenant_id: &str) -> Result<Option<ImageSummary>> {
        debug!("Getting image");

        let response = self
            .send_request(Request::GetImage(GetImageRequest {
                image_id: image_id.to_string(),
                tenant_id: tenant_id.to_string(),
            }))
            .await?;

        match response {
            Response::GetImage(resp) => {
                if !resp.found {
                    return Ok(None);
                }
                match resp.image {
                    Some(img) => Ok(Some(ImageSummary {
                        image_id: img.image_id,
                        tenant_id: img.tenant_id,
                        name: img.name,
                        description: img.description,
                        runner_type: runner_type_from_i32(img.runner_type),
                        created_at: Utc
                            .timestamp_millis_opt(img.created_at_ms)
                            .single()
                            .unwrap_or_else(Utc::now),
                    })),
                    None => Ok(None),
                }
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetImageResponse".to_string(),
            )),
        }
    }

    /// Delete an image.
    #[instrument(skip(self), fields(image_id = %image_id, tenant_id = %tenant_id))]
    pub async fn delete_image(&self, image_id: &str, tenant_id: &str) -> Result<()> {
        info!("Deleting image");

        let response = self
            .send_request(Request::DeleteImage(DeleteImageRequest {
                image_id: image_id.to_string(),
                tenant_id: tenant_id.to_string(),
            }))
            .await?;

        match response {
            Response::DeleteImage(resp) => {
                if !resp.success {
                    if resp.error.contains("not found") {
                        return Err(SdkError::ImageNotFound(image_id.to_string()));
                    }
                    return Err(SdkError::Server {
                        code: "DELETE_FAILED".to_string(),
                        message: resp.error,
                    });
                }
                Ok(())
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected DeleteImageResponse".to_string(),
            )),
        }
    }

    // =========================================================================
    // Signal Operations
    // =========================================================================

    /// Send a signal to an instance.
    ///
    /// Note: Environment proxies this to runtara-core.
    #[instrument(skip(self), fields(instance_id = %instance_id, signal = ?signal_type))]
    pub async fn send_signal(
        &self,
        instance_id: &str,
        signal_type: SignalType,
        payload: Option<&[u8]>,
    ) -> Result<()> {
        info!("Sending signal to instance");

        // Note: Environment only supports Cancel and Pause signals (it proxies to Core)
        // Resume is handled via resume_instance() which relaunches the container
        if signal_type == SignalType::Resume {
            return self.resume_instance(instance_id).await;
        }

        let response = self
            .send_request(Request::SendSignal(SendSignalRequest {
                instance_id: instance_id.to_string(),
                signal_type: i32::from(signal_type),
                payload: payload.unwrap_or(&[]).to_vec(),
            }))
            .await?;

        match response {
            Response::SendSignal(resp) => {
                if !resp.success {
                    if resp.error.contains("not found") {
                        return Err(SdkError::InstanceNotFound(instance_id.to_string()));
                    }
                    return Err(SdkError::Server {
                        code: "SIGNAL_FAILED".to_string(),
                        message: resp.error,
                    });
                }
                Ok(())
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected SendSignalResponse".to_string(),
            )),
        }
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

    /// Send a custom signal to a specific checkpoint/signal ID.
    ///
    /// This is used to resume WaitForSignal steps in workflows.
    /// The signal_id must match exactly what the workflow is waiting for.
    ///
    /// # Arguments
    /// * `instance_id` - The instance waiting for the signal
    /// * `signal_id` - The checkpoint/signal ID (from on_wait callback)
    /// * `payload` - Optional payload data (typically JSON)
    ///
    /// # Example
    /// ```ignore
    /// // Approve a workflow waiting for manager approval
    /// sdk.send_custom_signal(
    ///     "inst-abc123",
    ///     "inst-abc123/root/approval_step/",
    ///     Some(r#"{"approved": true, "approver": "manager@example.com"}"#.as_bytes()),
    /// ).await?;
    /// ```
    #[instrument(skip(self, payload), fields(instance_id = %instance_id, signal_id = %signal_id))]
    pub async fn send_custom_signal(
        &self,
        instance_id: &str,
        signal_id: &str,
        payload: Option<&[u8]>,
    ) -> Result<()> {
        info!("Sending custom signal to instance");

        let response = self
            .send_request(Request::SendCustomSignal(SendCustomSignalRequest {
                instance_id: instance_id.to_string(),
                checkpoint_id: signal_id.to_string(),
                payload: payload.unwrap_or(&[]).to_vec(),
            }))
            .await?;

        match response {
            Response::SendCustomSignal(resp) => {
                if !resp.success {
                    if resp.error.contains("not found") {
                        return Err(SdkError::InstanceNotFound(instance_id.to_string()));
                    }
                    return Err(SdkError::Server {
                        code: "CUSTOM_SIGNAL_FAILED".to_string(),
                        message: resp.error,
                    });
                }
                Ok(())
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected SendCustomSignalResponse".to_string(),
            )),
        }
    }

    // =========================================================================
    // Checkpoints
    // =========================================================================

    /// List checkpoints for an instance.
    ///
    /// Returns a paginated list of checkpoint summaries for the specified instance.
    /// Checkpoints are ordered by creation time (newest first).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = sdk.list_checkpoints(
    ///     "instance-123",
    ///     ListCheckpointsOptions::new().with_limit(10)
    /// ).await?;
    ///
    /// for checkpoint in result.checkpoints {
    ///     println!("{}: {} bytes", checkpoint.checkpoint_id, checkpoint.data_size_bytes);
    /// }
    /// ```
    #[instrument(skip(self, options), fields(instance_id = %instance_id))]
    pub async fn list_checkpoints(
        &self,
        instance_id: &str,
        options: ListCheckpointsOptions,
    ) -> Result<ListCheckpointsResult> {
        debug!("Listing checkpoints");

        let response = self
            .send_request(Request::ListCheckpoints(ListCheckpointsRequest {
                instance_id: instance_id.to_string(),
                checkpoint_id: options.checkpoint_id,
                limit: options.limit,
                offset: options.offset,
                created_after_ms: options.created_after.map(|t| t.timestamp_millis()),
                created_before_ms: options.created_before.map(|t| t.timestamp_millis()),
            }))
            .await?;

        match response {
            Response::ListCheckpoints(resp) => {
                let checkpoints = resp
                    .checkpoints
                    .into_iter()
                    .map(|cp| CheckpointSummary {
                        checkpoint_id: cp.checkpoint_id,
                        instance_id: cp.instance_id,
                        created_at: Utc
                            .timestamp_millis_opt(cp.created_at_ms)
                            .single()
                            .unwrap_or_else(Utc::now),
                        data_size_bytes: cp.data_size_bytes,
                    })
                    .collect();

                Ok(ListCheckpointsResult {
                    checkpoints,
                    total_count: resp.total_count,
                    limit: resp.limit,
                    offset: resp.offset,
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected ListCheckpointsResponse".to_string(),
            )),
        }
    }

    /// Get a specific checkpoint with its full data.
    ///
    /// Returns the checkpoint data parsed as JSON, or None if the checkpoint doesn't exist.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(checkpoint) = sdk.get_checkpoint("instance-123", "step-1").await? {
    ///     println!("Checkpoint data: {:?}", checkpoint.data);
    /// }
    /// ```
    #[instrument(skip(self), fields(instance_id = %instance_id, checkpoint_id = %checkpoint_id))]
    pub async fn get_checkpoint(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<Checkpoint>> {
        debug!("Getting checkpoint");

        let response = self
            .send_request(Request::GetCheckpoint(GetCheckpointRequest {
                instance_id: instance_id.to_string(),
                checkpoint_id: checkpoint_id.to_string(),
            }))
            .await?;

        match response {
            Response::GetCheckpoint(resp) => {
                if !resp.found {
                    return Ok(None);
                }

                // Parse checkpoint data as JSON
                let data: serde_json::Value = if resp.data.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_slice(&resp.data).map_err(|e| {
                        SdkError::UnexpectedResponse(format!(
                            "Failed to parse checkpoint data as JSON: {}",
                            e
                        ))
                    })?
                };

                Ok(Some(Checkpoint {
                    checkpoint_id: resp.checkpoint_id,
                    instance_id: resp.instance_id,
                    created_at: Utc
                        .timestamp_millis_opt(resp.created_at_ms)
                        .single()
                        .unwrap_or_else(Utc::now),
                    data,
                }))
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetCheckpointResponse".to_string(),
            )),
        }
    }

    // =========================================================================
    // Events
    // =========================================================================

    /// List events for an instance with optional filtering.
    ///
    /// Returns a paginated list of events for the specified instance.
    /// Events include debug step events, workflow logs, and lifecycle events.
    /// Supports filtering by event type, subtype, time range, and full-text search in payload.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // List all debug events for an instance
    /// let result = sdk.list_events(
    ///     "instance-123",
    ///     ListEventsOptions::new()
    ///         .with_subtype("step_debug_start")
    ///         .with_limit(50)
    /// ).await?;
    ///
    /// for event in result.events {
    ///     println!("{}: {} - {:?}", event.id, event.subtype.unwrap_or_default(), event.payload);
    /// }
    ///
    /// // Search for events containing specific text in payload
    /// let result = sdk.list_events(
    ///     "instance-123",
    ///     ListEventsOptions::new()
    ///         .with_payload_contains("error")
    /// ).await?;
    /// ```
    #[instrument(skip(self, options), fields(instance_id = %instance_id))]
    pub async fn list_events(
        &self,
        instance_id: &str,
        options: ListEventsOptions,
    ) -> Result<ListEventsResult> {
        debug!("Listing events");

        let response = self
            .send_request(Request::ListEvents(ListEventsRequest {
                instance_id: instance_id.to_string(),
                event_type: options.event_type,
                subtype: options.subtype,
                limit: options.limit,
                offset: options.offset,
                created_after_ms: options.created_after.map(|t| t.timestamp_millis()),
                created_before_ms: options.created_before.map(|t| t.timestamp_millis()),
                payload_contains: options.payload_contains,
                scope_id: options.scope_id,
                parent_scope_id: options.parent_scope_id,
                root_scopes_only: options.root_scopes_only,
                sort_order: options.sort_order.map(|s| s.as_str().to_string()),
            }))
            .await?;

        match response {
            Response::ListEvents(resp) => {
                let events = resp
                    .events
                    .into_iter()
                    .map(|ev| {
                        // Parse payload bytes as JSON if present
                        let payload = ev.payload.and_then(|bytes| {
                            if bytes.is_empty() {
                                None
                            } else {
                                serde_json::from_slice(&bytes).ok()
                            }
                        });

                        EventSummary {
                            id: ev.id,
                            instance_id: ev.instance_id,
                            event_type: ev.event_type,
                            checkpoint_id: ev.checkpoint_id,
                            payload,
                            created_at: Utc
                                .timestamp_millis_opt(ev.created_at_ms)
                                .single()
                                .unwrap_or_else(Utc::now),
                            subtype: ev.subtype,
                        }
                    })
                    .collect();

                Ok(ListEventsResult {
                    events,
                    total_count: resp.total_count,
                    limit: resp.limit,
                    offset: resp.offset,
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected ListEventsResponse".to_string(),
            )),
        }
    }

    /// Get the ancestors of a scope in the execution hierarchy.
    ///
    /// Returns a list of `ScopeInfo` starting from the requested scope and walking
    /// up through parent scopes to the root. This is useful for reconstructing
    /// the call stack at any point in a workflow execution.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let ancestors = sdk.get_scope_ancestors("instance-123", "sc_split-orders_0_while-retry_2").await?;
    /// for scope in &ancestors {
    ///     println!("{}:{} (index {:?})", scope.step_type, scope.step_id, scope.index);
    /// }
    /// // Output:
    /// // While:while-retry (index Some(2))
    /// // Split:split-orders (index Some(0))
    /// ```
    #[instrument(skip(self), fields(instance_id = %instance_id, scope_id = %scope_id))]
    pub async fn get_scope_ancestors(
        &self,
        instance_id: &str,
        scope_id: &str,
    ) -> Result<Vec<ScopeInfo>> {
        debug!("Getting scope ancestors");

        let response = self
            .send_request(Request::GetScopeAncestors(GetScopeAncestorsRequest {
                instance_id: instance_id.to_string(),
                scope_id: scope_id.to_string(),
            }))
            .await?;

        match response {
            Response::GetScopeAncestors(resp) => {
                let ancestors = resp
                    .ancestors
                    .into_iter()
                    .map(|info| ScopeInfo {
                        scope_id: info.scope_id,
                        parent_scope_id: info.parent_scope_id,
                        step_id: info.step_id,
                        step_name: info.step_name,
                        step_type: info.step_type,
                        index: info.index,
                        created_at: Utc
                            .timestamp_millis_opt(info.created_at_ms)
                            .single()
                            .unwrap_or_else(Utc::now),
                    })
                    .collect();

                Ok(ancestors)
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetScopeAncestorsResponse".to_string(),
            )),
        }
    }

    /// List step summaries for an instance.
    ///
    /// Returns paired step events as unified records with status, duration, and I/O.
    /// This solves pagination issues where start/end events may appear on different
    /// pages, and provides accurate counts (100 steps = 100 records, not 200 events).
    ///
    /// # Example
    ///
    /// ```ignore
    /// use runtara_management_sdk::{ListStepSummariesOptions, StepStatus};
    ///
    /// // List all steps
    /// let result = sdk.list_step_summaries(
    ///     "instance-123",
    ///     ListStepSummariesOptions::new()
    /// ).await?;
    ///
    /// // Filter by status
    /// let failed_steps = sdk.list_step_summaries(
    ///     "instance-123",
    ///     ListStepSummariesOptions::new().with_status(StepStatus::Failed)
    /// ).await?;
    ///
    /// for step in &failed_steps.steps {
    ///     println!("{}: {:?} ({}ms)", step.step_id, step.status, step.duration_ms.unwrap_or(0));
    /// }
    /// ```
    #[instrument(skip(self, options), fields(instance_id = %instance_id))]
    pub async fn list_step_summaries(
        &self,
        instance_id: &str,
        options: ListStepSummariesOptions,
    ) -> Result<ListStepSummariesResult> {
        debug!("Listing step summaries");

        let response = self
            .send_request(Request::ListStepSummaries(ListStepSummariesRequest {
                instance_id: instance_id.to_string(),
                status: options.status.map(|s| {
                    match s {
                        StepStatus::Running => "running",
                        StepStatus::Completed => "completed",
                        StepStatus::Failed => "failed",
                    }
                    .to_string()
                }),
                step_type: options.step_type,
                scope_id: options.scope_id,
                parent_scope_id: options.parent_scope_id,
                root_scopes_only: options.root_scopes_only,
                sort_order: options.sort_order.map(|s| s.as_str().to_string()),
                limit: options.limit,
                offset: options.offset,
            }))
            .await?;

        match response {
            Response::ListStepSummaries(resp) => {
                let steps = resp
                    .steps
                    .into_iter()
                    .map(|step| {
                        // Parse payload bytes as JSON if present
                        let inputs = step.inputs.and_then(|bytes| {
                            if bytes.is_empty() {
                                None
                            } else {
                                serde_json::from_slice(&bytes).ok()
                            }
                        });
                        let outputs = step.outputs.and_then(|bytes| {
                            if bytes.is_empty() {
                                None
                            } else {
                                serde_json::from_slice(&bytes).ok()
                            }
                        });
                        let error = step.error.and_then(|bytes| {
                            if bytes.is_empty() {
                                None
                            } else {
                                serde_json::from_slice(&bytes).ok()
                            }
                        });

                        StepSummary {
                            step_id: step.step_id,
                            step_name: step.step_name,
                            step_type: step.step_type,
                            status: StepStatus::from(step.status),
                            started_at: Utc
                                .timestamp_millis_opt(step.started_at_ms)
                                .single()
                                .unwrap_or_else(Utc::now),
                            completed_at: step
                                .completed_at_ms
                                .and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                            duration_ms: step.duration_ms,
                            inputs,
                            outputs,
                            error,
                            scope_id: step.scope_id,
                            parent_scope_id: step.parent_scope_id,
                        }
                    })
                    .collect();

                Ok(ListStepSummariesResult {
                    steps,
                    total_count: resp.total_count,
                    limit: resp.limit,
                    offset: resp.offset,
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected ListStepSummariesResponse".to_string(),
            )),
        }
    }

    // =========================================================================
    // Agent Testing
    // =========================================================================

    /// Test a single agent capability.
    ///
    /// This executes the capability with the provided input and optional connection,
    /// running inside an OCI container (same environment as production workflows).
    /// Useful for validating agent behavior before deploying workflows.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = sdk.test_capability(
    ///     TestCapabilityOptions::new("tenant-1", "utils", "random-double", json!({}))
    /// ).await?;
    ///
    /// if result.success {
    ///     println!("Output: {:?}", result.output);
    /// }
    /// ```
    #[instrument(skip(self, options), fields(agent_id = %options.agent_id, capability_id = %options.capability_id))]
    pub async fn test_capability(
        &self,
        options: TestCapabilityOptions,
    ) -> Result<TestCapabilityResult> {
        info!("Testing capability");

        let input_bytes = serde_json::to_vec(&options.input)?;
        let connection_bytes = match &options.connection {
            Some(conn) => Some(serde_json::to_vec(conn)?),
            None => None,
        };

        let response = self
            .send_request(Request::TestCapability(TestCapabilityRequest {
                tenant_id: options.tenant_id,
                agent_id: options.agent_id,
                capability_id: options.capability_id,
                input: input_bytes,
                connection: connection_bytes,
                timeout_ms: options.timeout_ms,
            }))
            .await?;

        match response {
            Response::TestCapability(resp) => {
                let output = if resp.success && !resp.output.is_empty() {
                    serde_json::from_slice(&resp.output).ok()
                } else {
                    None
                };

                Ok(TestCapabilityResult {
                    success: resp.success,
                    output,
                    error: resp.error,
                    execution_time_ms: resp.execution_time_ms,
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected TestCapabilityResponse".to_string(),
            )),
        }
    }

    /// List all available agents and their capabilities.
    ///
    /// Returns metadata about all registered agents, including their capabilities
    /// and input schemas. This runs in-process (no OCI container needed).
    #[instrument(skip(self))]
    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>> {
        debug!("Listing agents");

        let response = self
            .send_request(Request::ListAgents(ListAgentsRequest { tenant_id: None }))
            .await?;

        match response {
            Response::ListAgents(resp) => {
                let agents: Vec<AgentInfo> =
                    serde_json::from_slice(&resp.agents_json).map_err(|e| {
                        SdkError::UnexpectedResponse(format!("Failed to parse agents: {}", e))
                    })?;
                Ok(agents)
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected ListAgentsResponse".to_string(),
            )),
        }
    }

    /// Get details about a specific capability including its input schema.
    ///
    /// Returns the input field definitions for the specified capability,
    /// or None if the capability is not found.
    #[instrument(skip(self), fields(agent_id = %agent_id, capability_id = %capability_id))]
    pub async fn get_capability(
        &self,
        agent_id: &str,
        capability_id: &str,
    ) -> Result<Option<Vec<CapabilityField>>> {
        debug!("Getting capability details");

        let response = self
            .send_request(Request::GetCapability(GetCapabilityRequest {
                agent_id: agent_id.to_string(),
                capability_id: capability_id.to_string(),
            }))
            .await?;

        match response {
            Response::GetCapability(resp) => {
                if !resp.found {
                    return Ok(None);
                }
                let inputs: Vec<CapabilityField> = serde_json::from_slice(&resp.inputs_json)
                    .map_err(|e| {
                        SdkError::UnexpectedResponse(format!("Failed to parse inputs: {}", e))
                    })?;
                Ok(Some(inputs))
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetCapabilityResponse".to_string(),
            )),
        }
    }

    // =========================================================================
    // Tenant Metrics
    // =========================================================================

    /// Get aggregated execution metrics for a tenant.
    ///
    /// Returns time-bucketed metrics including invocation counts, success rates,
    /// duration statistics, and memory usage across all instances for the tenant.
    ///
    /// # Arguments
    ///
    /// * `options` - Options including tenant_id, time range, and granularity
    ///
    /// # Example
    ///
    /// ```ignore
    /// use chrono::{Duration, Utc};
    ///
    /// // Get last 7 days of daily metrics
    /// let result = sdk.get_tenant_metrics(
    ///     GetTenantMetricsOptions::new("tenant-1")
    ///         .with_start_time(Utc::now() - Duration::days(7))
    ///         .with_granularity(MetricsGranularity::Daily)
    /// ).await?;
    ///
    /// for bucket in result.buckets {
    ///     println!("{}: {} invocations, {:.1}% success rate",
    ///         bucket.bucket_time.format("%Y-%m-%d"),
    ///         bucket.invocation_count,
    ///         bucket.success_rate_percent.unwrap_or(0.0)
    ///     );
    /// }
    /// ```
    #[instrument(skip(self), fields(tenant_id = %options.tenant_id))]
    pub async fn get_tenant_metrics(
        &self,
        options: GetTenantMetricsOptions,
    ) -> Result<TenantMetricsResult> {
        debug!("Getting tenant metrics");

        if options.tenant_id.is_empty() {
            return Err(SdkError::InvalidInput("tenant_id is required".to_string()));
        }

        let request = GetTenantMetricsRequest {
            tenant_id: options.tenant_id.clone(),
            start_time_ms: options.start_time.map(|t| t.timestamp_millis()),
            end_time_ms: options.end_time.map(|t| t.timestamp_millis()),
            granularity: options.granularity.map(i32::from),
        };

        let response = self
            .send_request(Request::GetTenantMetrics(request))
            .await?;

        match response {
            Response::GetTenantMetrics(resp) => {
                let buckets = resp
                    .buckets
                    .into_iter()
                    .map(|b| MetricsBucket {
                        bucket_time: Utc
                            .timestamp_millis_opt(b.bucket_time_ms)
                            .single()
                            .unwrap_or_else(Utc::now),
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
                    tenant_id: resp.tenant_id,
                    start_time: Utc
                        .timestamp_millis_opt(resp.start_time_ms)
                        .single()
                        .unwrap_or_else(Utc::now),
                    end_time: Utc
                        .timestamp_millis_opt(resp.end_time_ms)
                        .single()
                        .unwrap_or_else(Utc::now),
                    granularity: MetricsGranularity::from(resp.granularity),
                    buckets,
                })
            }
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetTenantMetricsResponse".to_string(),
            )),
        }
    }

    // =========================================================================
    // Convenience Methods
    // =========================================================================

    /// Wait for an instance to reach a terminal state.
    ///
    /// Returns the final instance info once it reaches Completed, Failed, or Cancelled.
    #[instrument(skip(self), fields(instance_id = %instance_id))]
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

/// Convert i32 runner type to RunnerType enum.
fn runner_type_from_i32(value: i32) -> RunnerType {
    match value {
        0 => RunnerType::Oci,
        1 => RunnerType::Native,
        2 => RunnerType::Wasm,
        _ => RunnerType::Oci, // Default to OCI
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_status_conversion() {
        assert_eq!(InstanceStatus::from(0), InstanceStatus::Unknown);
        assert_eq!(InstanceStatus::from(1), InstanceStatus::Pending);
        assert_eq!(InstanceStatus::from(2), InstanceStatus::Running);
        assert_eq!(InstanceStatus::from(3), InstanceStatus::Suspended);
        assert_eq!(InstanceStatus::from(4), InstanceStatus::Completed);
        assert_eq!(InstanceStatus::from(5), InstanceStatus::Failed);
        assert_eq!(InstanceStatus::from(6), InstanceStatus::Cancelled);
    }

    #[test]
    fn test_signal_type_conversion() {
        assert_eq!(i32::from(SignalType::Cancel), 0);
        assert_eq!(i32::from(SignalType::Pause), 1);
        assert_eq!(i32::from(SignalType::Resume), 2);
    }

    #[test]
    fn test_runner_type_from_i32() {
        assert_eq!(runner_type_from_i32(0), RunnerType::Oci);
        assert_eq!(runner_type_from_i32(1), RunnerType::Native);
        assert_eq!(runner_type_from_i32(2), RunnerType::Wasm);
        assert_eq!(runner_type_from_i32(99), RunnerType::Oci); // Unknown defaults to OCI
    }
}
