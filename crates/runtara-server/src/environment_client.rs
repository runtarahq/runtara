// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct client for the embedded runtara-environment.
//!
//! runtara-environment is a library, and it runs in this process — same tokio
//! runtime, same connection pool, same `Arc<dyn Persistence>`. So this calls
//! [`runtara_environment::handlers`] as functions. There is no socket, no JSON
//! round trip, and nothing to connect to or reconnect to.
//!
//! What remains is a vocabulary translation, not a shape conversion.
//! Environment answers in `runtara-core`'s terms — `DateTime<Utc>`, the bytes
//! it stored, core's own status enums — and [`crate::runtime_types`] holds the
//! forms the server's handlers speak. Nothing round-trips through epoch
//! milliseconds, base64 or status strings any more; the mappings left here are
//! total, so no reading of a stored row can fall through one.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use runtara_environment::handlers::{
    self, EnvironmentHandlerState, ResumeInstanceRequest, SendCustomSignalOutcome,
    SendSignalOutcome, StartInstanceRequest, StartRejection, StopInstanceRequest,
};
use runtara_environment::image_registry::{Image, ImageFilter, ImageRegistry};
use runtara_environment::instance_repository::{self, InstanceRepository};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

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

    /// The registry that owns the `images` table.
    ///
    /// Image reads go straight to it. They used to pass through a pair of
    /// handlers that only re-shaped the row, which is a layer worth having when
    /// something has to cross a socket and not when the caller is linked
    /// against the same crate.
    fn image_registry(&self) -> ImageRegistry {
        self.state.images()
    }

    /// The repository that owns the `instances` row.
    fn instances(&self) -> InstanceRepository {
        self.state.instances()
    }

    // =========================================================================
    // Instance operations
    // =========================================================================

    /// Read one instance's full state.
    #[instrument(skip(self), fields(instance_id = %instance_id), level = "debug")]
    pub async fn get_instance_status(&self, instance_id: &str) -> Result<InstanceInfo> {
        debug!("Getting instance status");

        let Some(inst) = self.instances().detail(instance_id).await? else {
            return Err(EnvironmentError::InstanceNotFound(instance_id.to_string()));
        };

        Ok(InstanceInfo {
            instance_id: inst.instance_id,
            image_id: inst.image_id.unwrap_or_default(),
            image_name: inst.image_name.unwrap_or_default(),
            tenant_id: inst.tenant_id,
            status: instance_status_from_core(inst.status),
            checkpoint_id: inst.checkpoint_id,
            created_at: inst.created_at,
            started_at: inst.started_at,
            finished_at: inst.finished_at,
            input: decode_json_body(inst.input.as_deref(), instance_id, "input"),
            output: decode_json_body(inst.output.as_deref(), instance_id, "output"),
            error: inst.error,
            stderr: inst.stderr,
            retry_count: inst.retry_count,
            max_retries: inst.max_retries,
            memory_peak_bytes: inst.memory_peak_bytes,
            cpu_usage_usec: inst.cpu_usage_usec,
            termination_reason: inst
                .termination_reason
                .and_then(|s| TerminationReason::from_str(&s)),
            exit_code: inst.exit_code,
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
        Ok(self
            .instances()
            .count_by_status(tenant_id, statuses, ceiling)
            .await?)
    }

    /// List instances with optional filtering.
    #[instrument(skip(self, options), level = "debug")]
    pub async fn list_instances(
        &self,
        options: ListInstancesOptions,
    ) -> Result<ListInstancesResult> {
        debug!("Listing instances");

        let result = self
            .instances()
            .list(&list_instances_options(&options))
            .await?;

        Ok(ListInstancesResult {
            instances: result
                .instances
                .into_iter()
                .map(|inst| InstanceSummary {
                    instance_id: inst.instance_id,
                    tenant_id: inst.tenant_id,
                    image_id: inst.image_id.unwrap_or_default(),
                    image_name: inst.image_name.unwrap_or_default(),
                    status: instance_status_from_core(inst.status),
                    created_at: inst.created_at,
                    started_at: inst.started_at,
                    finished_at: inst.finished_at,
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

        // Environment reports a typed refusal, so the category is read from
        // the variant rather than recovered by searching the message. Matching
        // on the text used to fold a database failure that happened to say
        // "not found" into ImageNotFound, whose handler deletes the workflow's
        // compilation record and forces a rebuild.
        match resp.rejection {
            None => Ok(StartInstanceResult {
                success: true,
                instance_id: resp.instance_id,
                deduplicated: resp.deduplicated,
                error: None,
            }),
            Some(StartRejection::SingleInstanceActive) => {
                Err(EnvironmentError::SingleInstanceActive)
            }
            // Both are repaired by registering the image again, which is what
            // the caller's ImageNotFound handler does.
            Some(rejection @ StartRejection::ImageNotFound { .. })
            | Some(rejection @ StartRejection::ImageNotRunnable { .. }) => {
                Err(EnvironmentError::ImageNotFound(rejection.to_string()))
            }
            Some(rejection) => Ok(StartInstanceResult {
                success: false,
                instance_id: resp.instance_id,
                deduplicated: resp.deduplicated,
                error: Some(rejection.to_string()),
            }),
        }
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

        let signal_str = match signal_type {
            SignalType::Cancel => "cancel",
            SignalType::Pause => "pause",
            SignalType::Shutdown => "shutdown",
        };

        match handlers::handle_send_signal(&self.state, instance_id, signal_str, payload).await? {
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
    #[instrument(skip(self, payload), fields(instance_id = %instance_id, checkpoint_id = %checkpoint_id))]
    pub async fn send_custom_signal(
        &self,
        instance_id: &str,
        checkpoint_id: &str,
        payload: Option<&[u8]>,
    ) -> Result<String> {
        info!("Sending custom signal to instance");

        match handlers::handle_send_custom_signal(&self.state, instance_id, checkpoint_id, payload)
            .await?
        {
            SendCustomSignalOutcome::Delivered { signal_id } => Ok(signal_id),
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

        let images = self
            .image_registry()
            .list_filtered(&ImageFilter {
                tenant_id: options.tenant_id,
                name: None,
                limit: i64::from(options.limit),
                offset: i64::from(options.offset),
            })
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

        Ok(self
            .image_registry()
            .list_filtered(&ImageFilter {
                tenant_id: Some(tenant_id.to_string()),
                name: Some(name.to_string()),
                limit: 1,
                offset: 0,
            })
            .await?
            .into_iter()
            .next()
            .map(image_summary))
    }

    /// Whether an image's registered artifact is still on disk.
    ///
    /// The compile path reuses an immutable artifact on the strength of its
    /// row; this is how it checks the file is actually there before doing so.
    #[instrument(skip(self), fields(image_id = %image_id), level = "debug")]
    pub async fn image_artifact_present(&self, image_id: &str) -> Result<bool> {
        Ok(self.image_registry().artifact_present(image_id).await?)
    }

    /// Get one image, scoped to a tenant.
    #[instrument(skip(self), fields(image_id = %image_id, tenant_id = %tenant_id), level = "debug")]
    pub async fn get_image(&self, image_id: &str, tenant_id: &str) -> Result<Option<ImageSummary>> {
        debug!("Getting image");

        Ok(self
            .image_registry()
            .get_scoped(image_id, Some(tenant_id))
            .await?
            .map(image_summary))
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
                    created_at: cp.created_at,
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
                    payload: decode_json_body(ev.payload.as_deref(), instance_id, "event payload"),
                    created_at: ev.created_at,
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
                    status: step_status_from_core(step.status),
                    started_at: step.started_at,
                    completed_at: step.completed_at,
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
                    created_at: info.created_at,
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
            &handlers::TenantMetricsOptions {
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
                    bucket_time: b.bucket_time,
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

/// The image fields the server reports, dropping the ones only environment
/// uses (where the artifact sits on disk, when its row was last rewritten).
fn image_summary(img: Image) -> ImageSummary {
    ImageSummary {
        image_id: img.image_id,
        tenant_id: img.tenant_id,
        name: img.name,
        description: img.description,
        created_at: img.created_at,
        metadata: img.metadata,
    }
}

/// Core's lifecycle status in the server's vocabulary.
///
/// Total, so nothing falls through. The string version this replaces carried
/// two arms that could never be taken: `instance_status` is a six-label
/// Postgres enum declared in `001_initial_schema.sql` and never altered since,
/// so the `"sleeping"` alias (a `termination_reason` label, a different column)
/// and the `_ => Unknown` catch-all were both unreachable. `Unknown` stays in
/// the server's own enum for callers that model "not observed yet"; no
/// conversion produces it from a stored row.
fn instance_status_from_core(status: runtara_core::domain::InstanceStatus) -> InstanceStatus {
    use runtara_core::domain::InstanceStatus as Core;
    match status {
        Core::Pending => InstanceStatus::Pending,
        Core::Running => InstanceStatus::Running,
        Core::Suspended => InstanceStatus::Suspended,
        Core::Completed => InstanceStatus::Completed,
        Core::Failed => InstanceStatus::Failed,
        Core::Cancelled => InstanceStatus::Cancelled,
    }
}

/// Translate the caller's filter into environment's own options.
///
/// An empty status list means "no filter", not "match nothing" — the same
/// normalization the query-string form used to do on the way in.
fn list_instances_options(
    options: &ListInstancesOptions,
) -> instance_repository::ListInstancesOptions {
    instance_repository::ListInstancesOptions {
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

fn step_status_from_core(status: runtara_core::persistence::PairedRecordStatus) -> StepStatus {
    use runtara_core::persistence::PairedRecordStatus as Core;
    match status {
        Core::Running => StepStatus::Running,
        Core::Completed => StepStatus::Completed,
        Core::Failed => StepStatus::Failed,
    }
}

/// Parse a stored body as JSON, or `None` when there is nothing to parse.
///
/// The server's types model these as `Option<Value>`, so a body that is not
/// JSON has to come back as `None` — which is also what "no body at all" looks
/// like. That collapse used to happen behind a bare `.ok()`, silently: a
/// workflow whose output failed to parse was indistinguishable from one that
/// produced no output, with nothing anywhere to say so. It still collapses, but
/// it says so first.
fn decode_json_body(
    bytes: Option<&[u8]>,
    instance_id: &str,
    what: &str,
) -> Option<serde_json::Value> {
    let bytes = bytes?;
    if bytes.is_empty() {
        return None;
    }
    match serde_json::from_slice(bytes) {
        Ok(value) => Some(value),
        Err(error) => {
            warn!(
                instance_id = %instance_id,
                body = what,
                byte_len = bytes.len(),
                %error,
                "Stored body is not JSON; reporting it as absent"
            );
            None
        }
    }
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

    /// A signal payload must reach the store byte for byte.
    ///
    /// This path used to run the bytes through `String::from_utf8_lossy` and
    /// back, because the handler took `Option<&str>`. Every caller happens to
    /// pass `serde_json::to_vec`, so it was lossless in practice — but any byte
    /// sequence that is not valid UTF-8 was silently rewritten to U+FFFD on the
    /// way through, and nothing in the types said so.
    #[tokio::test]
    async fn a_signal_payload_is_stored_byte_for_byte() {
        let persistence = Arc::new(InMemoryPersistence::new());
        persistence
            .register_instance("signal-bytes", "tenant-1")
            .await
            .unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://localhost:1/unused")
            .unwrap();
        let client = EnvironmentClient::new(Arc::new(EnvironmentHandlerState::new(
            pool,
            persistence.clone(),
            Arc::new(MockRunner::new()),
            std::env::temp_dir(),
        )));

        // Lone continuation bytes and an interior NUL: not valid UTF-8, so
        // `from_utf8_lossy` would substitute replacement characters here.
        let payload: Vec<u8> = vec![0xff, 0xfe, 0x00, 0x01, 0x80, b'{'];
        assert!(
            std::str::from_utf8(&payload).is_err(),
            "the fixture must be invalid UTF-8 or it proves nothing"
        );

        client
            .send_custom_signal("signal-bytes", "cp-1", Some(&payload))
            .await
            .expect("send custom signal");

        let stored = persistence
            .get_custom_signal("signal-bytes", "cp-1")
            .await
            .expect("read back")
            .expect("a sent signal is retained");
        assert_eq!(
            stored.payload.as_deref(),
            Some(payload.as_slice()),
            "the payload must arrive as it was sent, not as lossy UTF-8"
        );
    }

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
