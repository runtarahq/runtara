// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct client for the embedded runtara-environment.
//!
//! runtara-environment is a library, and it runs in this process — same tokio
//! runtime, same connection pool, same `Arc<dyn Persistence>`. So this calls
//! [`runtara_environment::handlers`] as functions. There is no socket, no JSON
//! round trip, and nothing to connect to or reconnect to.
//!
//! What remains is the shape conversion the old HTTP client did after
//! deserializing: environment reports wire-shaped values (`*_ms` timestamps,
//! base64 bodies, statuses as strings) and [`crate::runtime_types`] holds the
//! richer forms the server's handlers want (`DateTime`, decoded JSON, enums).
//! Every method here is that mapping and nothing else.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use chrono::{TimeZone, Utc};
use runtara_environment::db;
use runtara_environment::handlers::{
    self, EnvironmentHandlerState, ResumeInstanceRequest, SendCustomSignalOutcome,
    SendSignalOutcome, StartInstanceRequest, StopInstanceRequest,
};
use runtara_environment::launch_queue::SINGLE_INSTANCE_ACTIVE;
use thiserror::Error;
use tracing::{debug, info, instrument};

use crate::runtime_types::{
    CheckpointSummary, EventSummary, GetTenantMetricsOptions, ImageSummary, InstanceInfo,
    InstanceStatus, InstanceSummary, ListCheckpointsOptions, ListCheckpointsResult,
    ListEventsOptions, ListEventsResult, ListImagesOptions, ListImagesResult, ListInstancesOptions,
    ListInstancesResult, ListStepSummariesOptions, ListStepSummariesResult, MetricsBucket,
    MetricsGranularity, RegisterImageResult, RegisterImageStreamOptions, ScopeInfo, SignalType,
    StartInstanceResult, StepStatus, StepSummary, StopInstanceOptions, TenantMetricsResult,
    TerminationReason,
};

/// Errors from a call into the embedded environment.
#[derive(Debug, Error)]
pub enum EnvironmentError {
    /// The instance does not exist.
    #[error("instance not found: {0}")]
    InstanceNotFound(String),

    /// The image does not exist.
    #[error("image not found: {0}")]
    ImageNotFound(String),

    /// A guarded trigger lost the durable workflow-wide launch race.
    #[error("single-instance workflow already has active work")]
    SingleInstanceActive,

    /// The caller supplied something the handler rejected.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The handler reported the operation failed.
    #[error("environment error [{code}]: {message}")]
    Failed {
        /// Stable code for the failing operation.
        code: String,
        /// Human-readable detail.
        message: String,
    },

    /// The handler itself errored.
    #[error(transparent)]
    Environment(#[from] runtara_environment::error::Error),
}

/// Result type for environment calls.
pub type Result<T> = std::result::Result<T, EnvironmentError>;

/// In-process client for the embedded environment.
#[derive(Clone)]
pub struct EnvironmentClient {
    state: Arc<EnvironmentHandlerState>,
}

impl std::fmt::Debug for EnvironmentClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentClient").finish_non_exhaustive()
    }
}

impl EnvironmentClient {
    /// Wrap the running environment's shared handler state.
    pub fn new(state: Arc<EnvironmentHandlerState>) -> Self {
        Self { state }
    }

    // =========================================================================
    // Instance operations
    // =========================================================================

    /// Read one instance's full state.
    #[instrument(skip(self), fields(instance_id = %instance_id), level = "debug")]
    pub async fn get_instance_status(&self, instance_id: &str) -> Result<InstanceInfo> {
        debug!("Getting instance status");

        let json = handlers::handle_get_instance_status(&self.state, instance_id).await?;

        if !json.found {
            return Err(EnvironmentError::InstanceNotFound(instance_id.to_string()));
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

    /// Count a tenant's instances in the given statuses.
    #[instrument(skip(self), level = "debug")]
    pub async fn count_instances_by_status(
        &self,
        tenant_id: Option<&str>,
        statuses: &[String],
        ceiling: i64,
    ) -> Result<i64> {
        Ok(
            handlers::handle_count_instances_by_status(&self.state, tenant_id, statuses, ceiling)
                .await?,
        )
    }

    /// List instances with optional filtering.
    #[instrument(skip(self, options), level = "debug")]
    pub async fn list_instances(
        &self,
        options: ListInstancesOptions,
    ) -> Result<ListInstancesResult> {
        debug!("Listing instances");

        let result =
            handlers::handle_list_instances(&self.state, &list_instances_options(&options)).await?;

        Ok(ListInstancesResult {
            instances: result
                .instances
                .into_iter()
                .map(|inst| InstanceSummary {
                    instance_id: inst.instance_id,
                    tenant_id: inst.tenant_id,
                    image_id: inst.image_id.unwrap_or_default(),
                    image_name: inst.image_name.unwrap_or_default(),
                    status: instance_status_from_string(&inst.status),
                    created_at: ms_to_datetime(inst.created_at_ms),
                    started_at: opt_ms_to_datetime(inst.started_at_ms),
                    finished_at: opt_ms_to_datetime(inst.finished_at_ms),
                    has_error: inst.has_error,
                })
                .collect(),
            total_count: result.total_count as u32,
        })
    }

    /// Start a new instance.
    #[instrument(skip(self, options), fields(image_id = %options.image_id, tenant_id = %options.tenant_id))]
    pub(crate) async fn start_instance(
        &self,
        options: crate::runtime_types::StartInstanceOptions,
    ) -> Result<StartInstanceResult> {
        info!("Starting instance");

        let resp = handlers::handle_start_instance(
            &self.state,
            StartInstanceRequest {
                image_id: options.image_id,
                tenant_id: options.tenant_id,
                instance_id: options.instance_id,
                input: options.input,
                timeout_seconds: options.timeout_seconds.map(u64::from),
                env: options.env,
            },
        )
        .await?;

        if !resp.success && resp.error.as_deref() == Some(SINGLE_INSTANCE_ACTIVE) {
            return Err(EnvironmentError::SingleInstanceActive);
        }

        // A refusal naming a missing image is reported as such rather than as a
        // generic failure: callers retry the former by re-registering the image.
        if !resp.success
            && let Some(ref error) = resp.error
            && error.contains("not found")
        {
            return Err(EnvironmentError::ImageNotFound(error.clone()));
        }

        Ok(StartInstanceResult {
            success: resp.success,
            instance_id: resp.instance_id,
            deduplicated: resp.deduplicated,
            error: resp.error,
        })
    }

    /// Stop a running instance.
    #[instrument(skip(self, options), fields(instance_id = %options.instance_id))]
    pub async fn stop_instance(&self, options: StopInstanceOptions) -> Result<()> {
        info!(reason = %options.reason, "Stopping instance");

        let resp = handlers::handle_stop_instance(
            &self.state,
            StopInstanceRequest {
                instance_id: options.instance_id,
                reason: options.reason,
                grace_period_seconds: u64::from(options.grace_period_seconds),
            },
        )
        .await?;

        failed_unless(resp.success, "STOP_FAILED", resp.error)
    }

    /// Resume a suspended instance.
    #[instrument(skip(self), fields(instance_id = %instance_id))]
    pub async fn resume_instance(&self, instance_id: &str) -> Result<()> {
        info!("Resuming instance");

        let resp = handlers::handle_resume_instance(
            &self.state,
            ResumeInstanceRequest {
                instance_id: instance_id.to_string(),
            },
        )
        .await?;

        failed_unless(resp.success, "RESUME_FAILED", resp.error)
    }

    // =========================================================================
    // Signal operations
    // =========================================================================

    /// Send a lifecycle signal to an instance.
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

        match handlers::handle_send_signal(
            &self.state,
            instance_id,
            signal_str,
            payload_str.as_deref(),
        )
        .await?
        {
            SendSignalOutcome::Delivered => Ok(()),
            SendSignalOutcome::InstanceNotFound => {
                Err(EnvironmentError::InstanceNotFound(instance_id.to_string()))
            }
            SendSignalOutcome::NotSignalable { status } => Err(EnvironmentError::Failed {
                code: "SIGNAL_FAILED".to_string(),
                message: format!("Cannot send signal to instance in '{}' state", status),
            }),
            SendSignalOutcome::UnknownSignalType { signal_type } => Err(
                EnvironmentError::InvalidInput(format!("Unknown signal type: {}", signal_type)),
            ),
        }
    }

    /// Send a custom (workflow-defined) signal addressed to one checkpoint.
    #[instrument(skip(self, payload), fields(instance_id = %instance_id, signal_id = %signal_id))]
    pub async fn send_custom_signal(
        &self,
        instance_id: &str,
        signal_id: &str,
        payload: Option<&[u8]>,
    ) -> Result<()> {
        info!("Sending custom signal to instance");

        let payload_str = payload.map(|p| String::from_utf8_lossy(p).to_string());

        match handlers::handle_send_custom_signal(
            &self.state,
            instance_id,
            signal_id,
            payload_str.as_deref(),
        )
        .await?
        {
            SendCustomSignalOutcome::Delivered => Ok(()),
            SendCustomSignalOutcome::InstanceNotFound => {
                Err(EnvironmentError::InstanceNotFound(instance_id.to_string()))
            }
        }
    }

    // =========================================================================
    // Image operations
    // =========================================================================

    /// List images.
    #[instrument(skip(self, options), level = "debug")]
    pub async fn list_images(&self, options: ListImagesOptions) -> Result<ListImagesResult> {
        debug!("Listing images");

        let images = handlers::handle_list_images(
            &self.state,
            &handlers::ListImagesParams {
                tenant_id: options.tenant_id,
                name: None,
                limit: i64::from(options.limit),
                offset: i64::from(options.offset),
            },
        )
        .await?;

        let total_count = images.len() as u32;
        Ok(ListImagesResult {
            images: images.into_iter().map(image_summary).collect(),
            total_count,
        })
    }

    /// Look up one image by its tenant-scoped name.
    ///
    /// This goes through Environment's exact-name path rather than scanning a
    /// paginated image list. Compiled artifacts are immutable and accumulate
    /// over time, so a bounded list scan can otherwise miss a valid orphaned
    /// artifact after the first page.
    #[instrument(skip(self), fields(tenant_id = %tenant_id, name = %name), level = "debug")]
    pub async fn find_image_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<ImageSummary>> {
        debug!("Finding image by name");

        Ok(handlers::handle_list_images(
            &self.state,
            &handlers::ListImagesParams {
                tenant_id: Some(tenant_id.to_string()),
                name: Some(name.to_string()),
                limit: 1,
                offset: 0,
            },
        )
        .await?
        .into_iter()
        .next()
        .map(image_summary))
    }

    /// Get one image, scoped to a tenant.
    #[instrument(skip(self), fields(image_id = %image_id, tenant_id = %tenant_id), level = "debug")]
    pub async fn get_image(&self, image_id: &str, tenant_id: &str) -> Result<Option<ImageSummary>> {
        debug!("Getting image");

        Ok(
            handlers::handle_get_image(&self.state, image_id, Some(tenant_id))
                .await?
                .map(image_summary),
        )
    }

    /// Store an uploaded image artifact and register (or replace) its row.
    ///
    /// The reader is drained into memory first. Over HTTP this was a streaming
    /// multipart upload; in-process the bytes have to be materialized anyway to
    /// verify the checksum and write the file, so streaming bought nothing but
    /// a socket.
    #[instrument(skip(self, options, reader), fields(tenant_id = %options.tenant_id, name = %options.name))]
    pub async fn register_image_stream<R: tokio::io::AsyncRead + Unpin>(
        &self,
        options: RegisterImageStreamOptions,
        mut reader: R,
    ) -> Result<RegisterImageResult> {
        use tokio::io::AsyncReadExt;

        info!("Registering image");

        let mut binary = Vec::with_capacity(options.binary_size as usize);
        reader
            .read_to_end(&mut binary)
            .await
            .map_err(|e| EnvironmentError::Failed {
                code: "UPLOAD_ERROR".to_string(),
                message: format!("Failed to read image binary: {}", e),
            })?;

        // Checksum before anything is written, so a corrupted upload never
        // reaches the registry.
        if let Some(ref expected) = options.sha256 {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&binary);
            let actual = format!("{:x}", hasher.finalize());
            if &actual != expected {
                return Err(EnvironmentError::InvalidInput(format!(
                    "Checksum mismatch: expected {}, got {}",
                    expected, actual
                )));
            }
        }

        let image_id = handlers::handle_store_image(
            &self.state,
            handlers::StoreImageParams {
                tenant_id: options.tenant_id,
                name: options.name,
                description: options.description,
                metadata: options.metadata,
            },
            &binary,
        )
        .await
        .map_err(|e| match e {
            handlers::StoreImageError::Io(message) => EnvironmentError::Failed {
                code: "IO_ERROR".to_string(),
                message,
            },
            handlers::StoreImageError::Lookup(message)
            | handlers::StoreImageError::Register(message) => EnvironmentError::Failed {
                code: "REGISTER_IMAGE_ERROR".to_string(),
                message,
            },
        })?;

        Ok(RegisterImageResult {
            success: true,
            image_id,
            error: None,
        })
    }

    // =========================================================================
    // Checkpoints, events, steps
    // =========================================================================

    /// List an instance's checkpoints.
    #[instrument(skip(self, options), fields(instance_id = %instance_id), level = "debug")]
    pub async fn list_checkpoints(
        &self,
        instance_id: &str,
        options: ListCheckpointsOptions,
    ) -> Result<ListCheckpointsResult> {
        debug!("Listing checkpoints");

        let limit = options.limit.unwrap_or(100);
        let offset = options.offset.unwrap_or(0);

        let result = handlers::handle_list_checkpoints(
            &self.state,
            instance_id,
            &handlers::ListCheckpointsParams {
                checkpoint_id: options.checkpoint_id,
                created_after: options.created_after,
                created_before: options.created_before,
                limit: i64::from(limit),
                offset: i64::from(offset),
            },
        )
        .await?;

        Ok(ListCheckpointsResult {
            checkpoints: result
                .checkpoints
                .into_iter()
                .map(|cp| CheckpointSummary {
                    checkpoint_id: cp.checkpoint_id,
                    instance_id: cp.instance_id,
                    created_at: ms_to_datetime(cp.created_at_ms),
                    data_size_bytes: cp.data_size_bytes,
                })
                .collect(),
            total_count: result.total_count as u32,
            limit,
            offset,
        })
    }

    /// List an instance's events.
    #[instrument(skip(self, options), fields(instance_id = %instance_id), level = "debug")]
    pub async fn list_events(
        &self,
        instance_id: &str,
        options: ListEventsOptions,
    ) -> Result<ListEventsResult> {
        use runtara_core::persistence::{EventSortOrder as CoreSort, ListEventsFilter};

        debug!("Listing events");

        let limit = options.limit.unwrap_or(100);
        let offset = options.offset.unwrap_or(0);

        // Event filters historically accept arbitrary strings. An unknown name
        // cannot match a domain event, so preserve the empty page response.
        let event_type = match options.event_type.as_deref() {
            Some(value) => match parse_event_type(value) {
                Some(event_type) => Some(event_type),
                None => {
                    return Ok(ListEventsResult {
                        events: Vec::new(),
                        total_count: 0,
                        limit,
                        offset,
                    });
                }
            },
            None => None,
        };

        let filter = ListEventsFilter {
            event_type,
            subtype: options.subtype,
            created_after: options.created_after,
            created_before: options.created_before,
            payload_contains: options.payload_contains,
            scope_id: options.scope_id,
            parent_scope_id: options.parent_scope_id,
            root_scopes_only: options.root_scopes_only,
            sort_order: match options.sort_order.map(|o| o.as_str()) {
                Some("asc") => CoreSort::Asc,
                _ => CoreSort::Desc,
            },
        };

        let result = handlers::handle_list_events(
            &self.state,
            instance_id,
            &filter,
            i64::from(limit),
            i64::from(offset),
        )
        .await?;

        Ok(ListEventsResult {
            events: result
                .events
                .into_iter()
                .map(|ev| EventSummary {
                    id: ev.id,
                    instance_id: ev.instance_id,
                    event_type: ev.event_type,
                    checkpoint_id: ev.checkpoint_id,
                    payload: ev.payload.as_deref().and_then(decode_base64_json),
                    created_at: ms_to_datetime(ev.created_at_ms),
                    subtype: ev.subtype,
                })
                .collect(),
            total_count: result.total_count as u32,
            limit,
            offset,
        })
    }

    /// List an instance's per-step summaries.
    #[instrument(skip(self, options), fields(instance_id = %instance_id), level = "debug")]
    pub async fn list_step_summaries(
        &self,
        instance_id: &str,
        options: ListStepSummariesOptions,
    ) -> Result<ListStepSummariesResult> {
        use runtara_core::persistence::{
            EventSortOrder as CoreSort, ListPairedRecordsFilter,
            PairedRecordStatus as CoreStepStatus,
        };

        debug!("Listing step summaries");

        let limit = options.limit.unwrap_or(100);
        let offset = options.offset.unwrap_or(0);

        let filter = ListPairedRecordsFilter {
            sort_order: match options.sort_order.map(|o| o.as_str()) {
                Some("asc") => CoreSort::Asc,
                _ => CoreSort::Desc,
            },
            status: options.status.map(|s| match s {
                StepStatus::Running => CoreStepStatus::Running,
                StepStatus::Completed => CoreStepStatus::Completed,
                StepStatus::Failed => CoreStepStatus::Failed,
            }),
            kind: options.step_type,
            scope_id: options.scope_id,
            parent_scope_id: options.parent_scope_id,
            root_scopes_only: options.root_scopes_only,
            correlation_ids: options.step_ids.filter(|ids| !ids.is_empty()),
        };

        let result = handlers::handle_list_step_summaries(
            &self.state,
            instance_id,
            &filter,
            i64::from(limit),
            i64::from(offset),
        )
        .await?;

        Ok(ListStepSummariesResult {
            steps: result
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
                    launched_at_ms: step.launched_at_ms,
                    settled_at_ms: step.settled_at_ms,
                    inputs: step.inputs,
                    outputs: step.outputs,
                    error: step.error,
                    scope_id: step.scope_id,
                    parent_scope_id: step.parent_scope_id,
                })
                .collect(),
            total_count: result.total_count as u32,
            limit,
            offset,
        })
    }

    /// Walk a scope's ancestry, innermost first.
    #[instrument(skip(self), fields(instance_id = %instance_id, scope_id = %scope_id), level = "debug")]
    pub async fn get_scope_ancestors(
        &self,
        instance_id: &str,
        scope_id: &str,
    ) -> Result<Vec<ScopeInfo>> {
        debug!("Getting scope ancestors");

        Ok(
            handlers::handle_get_scope_ancestors(&self.state, instance_id, scope_id)
                .await?
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
                .collect(),
        )
    }

    /// Read a tenant's execution metrics, bucketed.
    #[instrument(skip(self, options), level = "debug")]
    pub async fn get_tenant_metrics(
        &self,
        options: GetTenantMetricsOptions,
    ) -> Result<TenantMetricsResult> {
        debug!("Getting tenant metrics");

        if options.tenant_id.is_empty() {
            return Err(EnvironmentError::InvalidInput(
                "tenant_id is required".to_string(),
            ));
        }

        // Same defaults the HTTP layer applied: a day ending now.
        let now = Utc::now();
        let end_time = options.end_time.unwrap_or(now);
        let start_time = options
            .start_time
            .unwrap_or(end_time - chrono::Duration::hours(24));
        let granularity = options.granularity.unwrap_or(MetricsGranularity::Hourly);

        let buckets = handlers::handle_get_tenant_metrics(
            &self.state,
            &db::TenantMetricsOptions {
                tenant_id: options.tenant_id.clone(),
                start_time,
                end_time,
                bucket_seconds: granularity.seconds(),
            },
        )
        .await?;

        Ok(TenantMetricsResult {
            tenant_id: options.tenant_id,
            start_time,
            end_time,
            granularity,
            buckets: buckets
                .into_iter()
                .map(|b| MetricsBucket {
                    bucket_time: ms_to_datetime(b.bucket_time_ms),
                    invocation_count: b.invocation_count,
                    success_count: b.success_count,
                    failure_count: b.failure_count,
                    cancelled_count: b.cancelled_count,
                    // Convert milliseconds to seconds for the server-side API
                    avg_duration_seconds: b.avg_duration_ms.map(|ms| ms / 1000.0),
                    min_duration_seconds: b.min_duration_ms.map(|ms| ms / 1000.0),
                    max_duration_seconds: b.max_duration_ms.map(|ms| ms / 1000.0),
                    avg_memory_bytes: b.avg_memory_bytes,
                    max_memory_bytes: b.max_memory_bytes,
                    success_rate_percent: b.success_rate_percent,
                })
                .collect(),
        })
    }
}

/// `Ok(())` when the handler reported success, otherwise the failure it named.
fn failed_unless(success: bool, code: &str, error: Option<String>) -> Result<()> {
    if success {
        return Ok(());
    }
    Err(EnvironmentError::Failed {
        code: code.to_string(),
        message: error.unwrap_or_else(|| "Unknown error".to_string()),
    })
}

fn image_summary(img: runtara_environment::handlers::ImageSummary) -> ImageSummary {
    ImageSummary {
        image_id: img.image_id,
        tenant_id: img.tenant_id,
        name: img.name,
        description: img.description,
        created_at: ms_to_datetime(img.created_at_ms),
        metadata: img.metadata,
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

/// Translate the caller's filter into environment's own options.
///
/// An empty status list means "no filter", not "match nothing" — the same
/// normalization the query-string form used to do on the way in.
fn list_instances_options(options: &ListInstancesOptions) -> db::ListInstancesOptions {
    db::ListInstancesOptions {
        tenant_id: options.tenant_id.clone(),
        statuses: (!options.statuses.is_empty()).then(|| {
            options
                .statuses
                .iter()
                .map(|status| status.as_str().to_string())
                .collect()
        }),
        image_id: options.image_id.clone(),
        image_name_prefix: options.image_name_prefix.clone(),
        created_after: options.created_after,
        created_before: options.created_before,
        finished_after: options.finished_after,
        finished_before: options.finished_before,
        order_by: options.order_by.map(|o| o.as_str().to_string()),
        limit: i64::from(options.limit),
        offset: i64::from(options.offset),
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

/// Keeps `HashMap` in the signature list honest for callers building env maps.
pub type EnvMap = HashMap<String, String>;

fn parse_event_type(value: &str) -> Option<runtara_core::domain::EventType> {
    use runtara_core::domain::EventType;
    match value {
        "started" => Some(EventType::Started),
        "progress" => Some(EventType::Progress),
        "heartbeat" => Some(EventType::Heartbeat),
        "completed" => Some(EventType::Completed),
        "failed" => Some(EventType::Failed),
        "suspended" => Some(EventType::Suspended),
        "custom" => Some(EventType::Custom),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::domain::EventType;
    use runtara_core::persistence::{EventRecord, Persistence, memory::InMemoryPersistence};
    use runtara_environment::runner::MockRunner;

    #[tokio::test]
    async fn event_filters_keep_wire_names_and_unknown_names_match_nothing() {
        let persistence = Arc::new(InMemoryPersistence::new());
        let events = [
            ("started", EventType::Started),
            ("progress", EventType::Progress),
            ("heartbeat", EventType::Heartbeat),
            ("completed", EventType::Completed),
            ("failed", EventType::Failed),
            ("suspended", EventType::Suspended),
            ("custom", EventType::Custom),
        ];
        for (_, event_type) in events {
            persistence
                .insert_event(&EventRecord {
                    id: None,
                    instance_id: "event-filter-test".into(),
                    event_type,
                    checkpoint_id: None,
                    payload: None,
                    created_at: Utc::now(),
                    subtype: None,
                })
                .await
                .unwrap();
        }
        // Event reads must use the injected persistence, with no environment I/O.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://localhost:1/unused")
            .unwrap();
        let client = EnvironmentClient::new(Arc::new(EnvironmentHandlerState::new(
            pool,
            persistence,
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )));
        for (name, _) in events {
            let page = client
                .list_events(
                    "event-filter-test",
                    ListEventsOptions::new().with_event_type(name),
                )
                .await
                .unwrap();
            assert_eq!(page.total_count, 1, "{name}");
            assert_eq!(page.events.len(), 1, "{name}");
            assert_eq!(page.events[0].event_type, name);
        }
        let all = client
            .list_events("event-filter-test", ListEventsOptions::new())
            .await
            .unwrap();
        assert_eq!(all.total_count, 7);
        for name in ["unknown", "CUSTOM", ""] {
            let page = client
                .list_events(
                    "event-filter-test",
                    ListEventsOptions::new()
                        .with_event_type(name)
                        .with_limit(12)
                        .with_offset(3),
                )
                .await
                .unwrap();
            assert!(page.events.is_empty(), "{name}");
            assert_eq!(page.total_count, 0);
            assert_eq!((page.limit, page.offset), (12, 3));
        }
    }
}
