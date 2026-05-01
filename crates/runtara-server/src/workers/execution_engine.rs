//! Execution Engine
//!
//! Single source of truth for workflow execution. Responsible for:
//! - Queuing async executions onto the Valkey trigger stream.
//! - Running synchronous executions end-to-end via the Runtime client.
//! - Proxying execution status / list / stop / pause / resume calls through
//!   the Runtara Management SDK.
//!
//! Sync and async entrypoints are thin wrappers around this engine — see
//! `ExecutionEngine::queue`, `ExecutionEngine::run_sync`, and the various
//! status / lifecycle helpers.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use dashmap::DashMap;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use runtara_management_sdk::{InstanceStatus, ListInstancesOptions, ListInstancesOrder};

use crate::api::dto::executions::ExecutionFilters;
use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::dto::workflows::{
    PageWorkflowInstanceHistoryDto, ValidationErrorDto, WorkflowInstanceDto,
};
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;
use crate::api::repositories::workflows::{CompilationStatus, WorkflowRepository};
use crate::api::services::input_validation::{is_empty_schema, validate_inputs};
use crate::metrics::MetricsService;
use crate::runtime_client::RuntimeClient;
use crate::workers::CancellationHandle;
use crate::workers::runtara_dto::{
    ExecutionWithMetadata, enrich_pending_input, execution_status_to_runtara, runtara_info_to_dto,
    runtara_info_to_execution_with_metadata, runtara_instance_to_dto_with_info,
};

/// Result of workflow execution (native path; currently unused by the server).
#[derive(Debug)]
pub struct ExecutionResult {
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub duration_seconds: f64,
    /// Peak memory usage in MB (from cgroup metrics)
    pub max_memory_mb: Option<f64>,
    /// CPU usage in milliseconds (from cgroup metrics)
    pub cpu_usage_ms: Option<f64>,
}

/// Unified error surface for the execution engine.
///
/// This is a superset of the previous `ExecutionError` (engine), the
/// `ServiceError` in `api/services/executions.rs`, and the `ServiceError`
/// from `api/services/workflows.rs` that was reused by the sync path.
#[derive(Debug)]
#[allow(dead_code)] // A few variants are reserved for handler migrations.
pub enum ExecutionError {
    ValidationError(String),
    WorkflowValidationError {
        message: String,
        errors: Vec<ValidationErrorDto>,
    },
    NotFound(String),
    WorkflowNotFound(String),
    BinaryNotFound(String),
    CompilationFailed(String),
    CompilationTimeout(String),
    NotCompiled {
        workflow_id: String,
        version: i32,
        compilation_queued: bool,
    },
    BundlePreparationFailed(String),
    RuntimeError(String),
    DatabaseError(String),
    NotConnected(String),
}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            ExecutionError::WorkflowValidationError { message, .. } => {
                write!(f, "Workflow validation failed: {}", message)
            }
            ExecutionError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ExecutionError::WorkflowNotFound(msg) => write!(f, "Workflow not found: {}", msg),
            ExecutionError::BinaryNotFound(msg) => write!(f, "Binary not found: {}", msg),
            ExecutionError::CompilationFailed(msg) => write!(f, "Compilation failed: {}", msg),
            ExecutionError::CompilationTimeout(msg) => write!(f, "Compilation timeout: {}", msg),
            ExecutionError::NotCompiled {
                workflow_id,
                version,
                compilation_queued,
            } => {
                write!(
                    f,
                    "Workflow '{}' version {} not compiled (compilation queued: {})",
                    workflow_id, version, compilation_queued
                )
            }
            ExecutionError::BundlePreparationFailed(msg) => {
                write!(f, "Bundle preparation failed: {}", msg)
            }
            ExecutionError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            ExecutionError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            ExecutionError::NotConnected(msg) => write!(f, "Not connected: {}", msg),
        }
    }
}

impl std::error::Error for ExecutionError {}

impl ExecutionError {
    /// Default HTTP status mapping for this error.
    ///
    /// Handlers are free to override this when they want a more specific
    /// status (e.g. `503 SERVICE_UNAVAILABLE` for `NotConnected`). Unless a
    /// handler opts out, this is the recommended mapping.
    pub fn http_status(&self) -> StatusCode {
        match self {
            ExecutionError::ValidationError(_) => StatusCode::BAD_REQUEST,
            ExecutionError::WorkflowValidationError { .. } => StatusCode::BAD_REQUEST,
            ExecutionError::NotFound(_) => StatusCode::NOT_FOUND,
            ExecutionError::WorkflowNotFound(_) => StatusCode::NOT_FOUND,
            ExecutionError::BinaryNotFound(_) => StatusCode::NOT_FOUND,
            ExecutionError::CompilationFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ExecutionError::CompilationTimeout(_) => StatusCode::GATEWAY_TIMEOUT,
            ExecutionError::NotCompiled { .. } => StatusCode::CONFLICT,
            ExecutionError::BundlePreparationFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ExecutionError::RuntimeError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ExecutionError::DatabaseError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ExecutionError::NotConnected(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

/// Trigger source classification used to dispatch to the matching
/// `TriggerEvent::*` factory.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Future trigger sources carried for parity with existing factories.
pub enum TriggerSource {
    HttpApi,
    Session,
    Chat,
    Webhook,
    Cron,
}

/// Request to queue an async workflow execution onto the trigger stream.
pub struct QueueRequest<'a> {
    pub tenant_id: &'a str,
    pub workflow_id: &'a str,
    pub version: Option<i32>,
    pub inputs: Value,
    pub debug: bool,
    pub correlation_id: Option<String>,
    pub trigger_source: TriggerSource,
}

/// Result of queuing an execution.
#[derive(Debug)]
pub struct QueuedExecution {
    pub instance_id: Uuid,
    pub status: String,
}

/// Request for synchronous execution via `ExecutionEngine::run_sync`.
pub struct SyncRequest<'a> {
    pub tenant_id: &'a str,
    pub workflow_id: &'a str,
    pub version: Option<i32>,
    pub inputs: Value,
}

/// Metrics reported for a synchronous execution.
#[derive(Debug, Clone)]
pub struct SyncExecutionMetrics {
    pub execution_duration_seconds: f64,
    pub max_memory_mb: f64,
    pub total_duration_seconds: f64,
}

/// Result of `ExecutionEngine::run_sync` — the full synchronous execution
/// output. Handlers adapt this into their own wire types.
#[derive(Debug, Clone)]
pub struct SyncExecution {
    pub success: bool,
    pub outputs: Value,
    pub error: Option<String>,
    pub stderr: Option<String>,
    pub metrics: SyncExecutionMetrics,
}

/// Result of `ExecutionEngine::stop`.
#[derive(Debug)]
#[allow(dead_code)]
pub enum StopOutcome {
    AlreadyStopped { status: String },
    Stopped { previous_status: String },
}

/// Result of `ExecutionEngine::pause`.
#[derive(Debug)]
#[allow(dead_code)]
pub enum PauseOutcome {
    Paused { previous_status: String },
    AlreadyPaused,
    NotPausable { status: String },
}

/// Result of `ExecutionEngine::resume`.
#[derive(Debug)]
#[allow(dead_code)]
pub enum ResumeOutcome {
    Resumed { previous_status: String },
    AlreadyRunning,
    NotResumable { status: String },
}

/// Inject `_workflow_id` into the inputs' variables to ensure cache key isolation.
///
/// This prevents cache key collisions when different workflows have EmbedWorkflow steps
/// with the same step_id calling the same child workflow. The workflow_id becomes part
/// of the cache key prefix, ensuring each workflow's child executions are isolated.
fn inject_workflow_id(inputs: Value, workflow_id: &str) -> Value {
    let mut inputs = inputs;
    if let Some(obj) = inputs.as_object_mut() {
        // Get or create variables object
        let variables = obj
            .entry("variables")
            .or_insert_with(|| serde_json::json!({}));

        if let Some(vars_obj) = variables.as_object_mut() {
            vars_obj.insert(
                "_workflow_id".to_string(),
                serde_json::Value::String(workflow_id.to_string()),
            );
        }
    }
    inputs
}

/// Execution engine — the single orchestrator for workflow execution.
pub struct ExecutionEngine {
    pool: PgPool,
    workflow_repo: Arc<WorkflowRepository>,
    runtime_client: Option<Arc<RuntimeClient>>,
    trigger_stream: Option<Arc<TriggerStreamPublisher>>,
    #[allow(dead_code)] // Reserved for future in-memory cancellation tracking.
    running_executions: Option<Arc<DashMap<Uuid, CancellationHandle>>>,
    /// Tracks workflows currently starting (prevents single_instance races).
    starting_workflows: Arc<Mutex<HashSet<(String, String)>>>, // (tenant_id, workflow_id)
}

impl ExecutionEngine {
    /// Create a new execution engine.
    pub fn new(
        pool: PgPool,
        workflow_repo: Arc<WorkflowRepository>,
        runtime_client: Option<Arc<RuntimeClient>>,
        trigger_stream: Option<Arc<TriggerStreamPublisher>>,
        running_executions: Option<Arc<DashMap<Uuid, CancellationHandle>>>,
    ) -> Self {
        Self {
            pool,
            workflow_repo,
            runtime_client,
            trigger_stream,
            running_executions,
            starting_workflows: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Check if the runtime client is available.
    #[allow(dead_code)]
    pub fn has_runtime(&self) -> bool {
        self.runtime_client.is_some()
    }

    // =========================================================================
    // Async queuing
    // =========================================================================

    /// Queue a workflow execution onto the Valkey trigger stream.
    ///
    /// Validates the workflow exists, validates inputs against the workflow's
    /// input schema (if non-empty), then publishes a `TriggerEvent` for the
    /// trigger worker to pick up.
    pub async fn queue(&self, req: QueueRequest<'_>) -> Result<QueuedExecution, ExecutionError> {
        // 1. Resolve version
        let version = self
            .resolve_version(req.tenant_id, req.workflow_id, req.version)
            .await?;

        // 2. Get workflow for input schema
        let workflow = self
            .workflow_repo
            .get_by_id(req.tenant_id, req.workflow_id, Some(version))
            .await
            .map_err(|e| ExecutionError::DatabaseError(format!("Failed to get workflow: {}", e)))?
            .ok_or_else(|| {
                ExecutionError::NotFound(format!(
                    "Workflow '{}' version {} not found",
                    req.workflow_id, version
                ))
            })?;

        // 3. Validate inputs.data against input schema
        if !is_empty_schema(&workflow.input_schema) {
            let data_to_validate = req
                .inputs
                .get("data")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            validate_inputs(&data_to_validate, &workflow.input_schema).map_err(|e| {
                ExecutionError::ValidationError(format!("Input validation failed: {}", e))
            })?;
        }

        // 4. Get track_events (already have it from workflow)
        let track_events = workflow.track_events;

        // 5. Require trigger stream
        let trigger_stream = self.trigger_stream.as_ref().ok_or_else(|| {
            ExecutionError::NotConnected(
                "Valkey trigger stream not configured. Cannot queue execution.".to_string(),
            )
        })?;

        // 6. Generate instance ID
        let instance_id = Uuid::new_v4();

        // 7. Build TriggerEvent appropriate to the source.
        //
        // Only HttpApi-flavoured events are produced today — sessions, chat,
        // webhooks, and cron-originated requests that go through the engine
        // share the `http_api` factory. Specialised factories
        // (`http_event`, `cron`) remain available for callers that already
        // have structured trigger metadata; see `TriggerEvent::*`.
        let event = match req.trigger_source {
            TriggerSource::HttpApi
            | TriggerSource::Session
            | TriggerSource::Chat
            | TriggerSource::Webhook
            | TriggerSource::Cron => TriggerEvent::http_api(
                instance_id.to_string(),
                req.tenant_id.to_string(),
                req.workflow_id.to_string(),
                Some(version),
                req.inputs,
                track_events,
                req.correlation_id,
                req.debug,
            ),
        };

        // 8. Publish to stream
        trigger_stream
            .publish(req.tenant_id, &event)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to publish to trigger stream: {}", e))
            })?;

        info!(
            instance_id = %instance_id,
            workflow_id = %req.workflow_id,
            version = version,
            "Published execution to trigger stream"
        );

        Ok(QueuedExecution {
            instance_id,
            status: "queued".to_string(),
        })
    }

    // =========================================================================
    // Synchronous execution (http-sync path)
    // =========================================================================

    /// Run a workflow synchronously, returning the full execution output.
    ///
    /// Blocks on compilation via `compilation_worker::wait_for_compilation`
    /// (max 5 minutes). Then starts an instance and waits for completion
    /// via `RuntimeClient::execute_sync`. Records metrics (including
    /// failures) before returning.
    #[instrument(skip(self, req), fields(tenant_id = %req.tenant_id, workflow_id = %req.workflow_id))]
    pub async fn run_sync(&self, req: SyncRequest<'_>) -> Result<SyncExecution, ExecutionError> {
        let total_start = Instant::now();
        let runtime_client = self.runtime_client.as_ref().ok_or_else(|| {
            ExecutionError::NotConnected("Runtime client not configured".to_string())
        })?;

        // 1. Resolve + 2. validate + cache workflow for track_events / schema
        let version = self
            .resolve_version(req.tenant_id, req.workflow_id, req.version)
            .await?;
        let workflow = self
            .workflow_repo
            .get_by_id(req.tenant_id, req.workflow_id, Some(version))
            .await
            .map_err(|e| ExecutionError::DatabaseError(format!("Failed to get workflow: {}", e)))?
            .ok_or_else(|| {
                ExecutionError::NotFound(format!(
                    "Workflow '{}' version {} not found",
                    req.workflow_id, version
                ))
            })?;

        if !is_empty_schema(&workflow.input_schema) {
            let data_to_validate = req
                .inputs
                .get("data")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            validate_inputs(&data_to_validate, &workflow.input_schema).map_err(|e| {
                ExecutionError::ValidationError(format!("Input validation failed: {}", e))
            })?;
        }

        // 3. Block on compilation readiness (delegated to compilation worker)
        self.wait_for_compilation_blocking(req.tenant_id, req.workflow_id, version)
            .await?;

        // 4. Execution timeout
        let execution_timeout = self
            .workflow_repo
            .get_execution_timeout(req.tenant_id, req.workflow_id, version)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to get execution timeout: {}", e))
            })?;

        // 5. Image ID
        let image_id = self
            .workflow_repo
            .get_registered_image_id(req.tenant_id, req.workflow_id, version)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to get registered image ID: {}", e))
            })?
            .ok_or_else(|| {
                ExecutionError::NotFound(format!(
                    "Workflow '{}' version {} not registered with runtara-environment. Recompile it.",
                    req.workflow_id, version
                ))
            })?;

        // 6. Execute via runtime client (no debug for sync executions)
        let execution_result = runtime_client
            .execute_sync(
                &image_id,
                req.tenant_id,
                req.workflow_id,
                None, // auto-generate instance id
                Some(req.inputs),
                execution_timeout.map(|s| s as u32),
                false,
            )
            .await;

        let total_duration = total_start.elapsed().as_secs_f64();

        // 7. Metrics + result shaping
        let metrics_service = MetricsService::new(self.workflow_repo.pool().clone());

        match execution_result {
            Ok(result) => {
                let execution_duration_secs = result
                    .duration_ms
                    .map(|ms| ms as f64 / 1000.0)
                    .unwrap_or(0.0);

                let _ = metrics_service
                    .record_execution_completion(
                        req.tenant_id,
                        req.workflow_id,
                        version,
                        result.success,
                        execution_duration_secs,
                        None,
                    )
                    .await;

                info!(
                    tenant_id = req.tenant_id,
                    workflow_id = req.workflow_id,
                    version = version,
                    execution_duration_seconds = execution_duration_secs,
                    total_duration_seconds = total_duration,
                    "Synchronous execution completed"
                );

                Ok(SyncExecution {
                    success: result.success,
                    outputs: result.output.unwrap_or(Value::Null),
                    error: result.error,
                    stderr: result.stderr,
                    metrics: SyncExecutionMetrics {
                        execution_duration_seconds: execution_duration_secs,
                        max_memory_mb: 0.0, // Not available via SDK
                        total_duration_seconds: total_duration,
                    },
                })
            }
            Err(e) => {
                let error_message = e.to_string();

                let _ = metrics_service
                    .record_execution_completion(
                        req.tenant_id,
                        req.workflow_id,
                        version,
                        false,
                        total_duration,
                        None,
                    )
                    .await;

                info!(
                    tenant_id = req.tenant_id,
                    workflow_id = req.workflow_id,
                    version = version,
                    error = error_message.as_str(),
                    total_duration_seconds = total_duration,
                    "Synchronous execution failed"
                );

                Ok(SyncExecution {
                    success: false,
                    outputs: Value::Null,
                    error: Some(error_message),
                    stderr: None,
                    metrics: SyncExecutionMetrics {
                        execution_duration_seconds: 0.0,
                        max_memory_mb: 0.0,
                        total_duration_seconds: total_duration,
                    },
                })
            }
        }
    }

    // =========================================================================
    // Async detached execution (trigger worker path)
    // =========================================================================

    /// Execute a workflow in fire-and-forget mode for distributed execution.
    ///
    /// 1. Ensures the workflow is compiled
    /// 2. Starts an instance via the Management SDK (non-blocking)
    /// 3. Returns immediately without waiting for completion
    ///
    /// The instance will run on the runtara-environment server.
    /// Use `get_instance_status` to poll for completion.
    #[instrument(skip(self, event), fields(instance_id = %event.instance_id, workflow_id = %event.workflow_id))]
    pub async fn execute_detached(&self, event: &TriggerEvent) -> Result<String, ExecutionError> {
        // Mark workflow as starting (for single_instance race condition prevention)
        let workflow_key = (event.tenant_id.clone(), event.workflow_id.clone());
        {
            let mut starting = self.starting_workflows.lock().await;
            starting.insert(workflow_key.clone());
        }

        // Execute and clean up starting_workflows on completion (success or error)
        let result = self.execute_detached_inner(event).await;

        // Keep the workflow in starting_workflows for a grace period to prevent race conditions
        // This ensures the database record has time to be created before we allow another instance
        let starting_workflows = self.starting_workflows.clone();
        let key = workflow_key.clone();
        tokio::spawn(async move {
            // Wait for DB record creation (execution typically takes 100-500ms to register)
            tokio::time::sleep(Duration::from_millis(1000)).await;
            let mut starting = starting_workflows.lock().await;
            starting.remove(&key);
        });

        result
    }

    /// Inner implementation of execute_detached
    async fn execute_detached_inner(&self, event: &TriggerEvent) -> Result<String, ExecutionError> {
        let runtime_client = self.runtime_client.as_ref().ok_or_else(|| {
            ExecutionError::NotConnected("Runtime client not configured".to_string())
        })?;

        // Resolve version if not specified
        let version = match event.version {
            Some(v) => v,
            None => {
                self.resolve_version(&event.tenant_id, &event.workflow_id, None)
                    .await?
            }
        };

        // Fetch execution timeout from executionGraph (if set)
        let execution_timeout_secs = self
            .workflow_repo
            .get_execution_timeout(&event.tenant_id, &event.workflow_id, version)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to get execution timeout: {}", e))
            })?
            .map(|secs| secs as u32)
            .unwrap_or(3600); // Default 1 hour timeout

        // Ensure workflow is compiled (non-blocking: returns NotCompiled for retry)
        self.ensure_compiled(&event.tenant_id, &event.workflow_id, version)
            .await?;

        // Inputs are already in canonical format {"data": {...}, "variables": {...}}
        // from the API layer - inject _workflow_id for cache key isolation
        let workflow_input = inject_workflow_id(event.inputs.clone(), &event.workflow_id);

        // Get the registered image ID (UUID returned from runtara-environment)
        let image_id = self
            .get_registered_image_id(&event.tenant_id, &event.workflow_id, version)
            .await?;

        // Start instance (non-blocking)
        let started_id = match runtime_client
            .start_instance(
                &image_id,
                &event.tenant_id,
                &event.workflow_id,
                Some(event.instance_id.clone()),
                Some(workflow_input),
                Some(execution_timeout_secs),
                event.debug,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                let error_str = e.to_string();
                // Check if this is a stale image reference (image was purged from runtara-environment)
                if error_str.contains("image not found")
                    || error_str.contains("Image") && error_str.contains("not found")
                {
                    tracing::warn!(
                        tenant_id = %event.tenant_id,
                        workflow_id = %event.workflow_id,
                        version = version,
                        image_id = %image_id,
                        "Image not found in runtara-environment - deleting stale compilation record for recompilation"
                    );

                    // Delete the stale compilation record so it will be recompiled
                    let _ = sqlx::query!(
                        "DELETE FROM workflow_compilations WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3",
                        &event.tenant_id,
                        &event.workflow_id,
                        version
                    )
                    .execute(&self.pool)
                    .await;

                    // Return NotCompiled to trigger recompilation queue
                    return Err(ExecutionError::NotCompiled {
                        workflow_id: event.workflow_id.clone(),
                        version,
                        compilation_queued: false,
                    });
                }
                return Err(ExecutionError::RuntimeError(error_str));
            }
        };

        info!(
            instance_id = %started_id,
            workflow_id = %event.workflow_id,
            version = version,
            "Started instance in detached mode"
        );

        Ok(started_id)
    }

    // =========================================================================
    // Trigger + single_instance helpers
    // =========================================================================

    /// Check if a trigger has `single_instance` mode enabled.
    ///
    /// Returns `Some(true)` if enabled, `Some(false)` if disabled, or `None`
    /// if the trigger doesn't exist.
    pub async fn get_trigger_single_instance(
        &self,
        trigger_id: &str,
    ) -> Result<Option<bool>, ExecutionError> {
        let result = sqlx::query!(
            r#"
            SELECT single_instance
            FROM invocation_trigger
            WHERE id = $1
            "#,
            trigger_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ExecutionError::DatabaseError(format!("Failed to get trigger: {}", e)))?;

        Ok(result.map(|r| r.single_instance))
    }

    /// Check if there's a running instance of a workflow.
    ///
    /// Returns `true` if at least one running instance exists for the workflow.
    /// Used for `single_instance` trigger enforcement.
    ///
    /// Checks both:
    /// 1. In-memory `starting_workflows` set (for instances being launched)
    /// 2. Runtara Management SDK for running instances
    pub async fn has_running_instance(
        &self,
        tenant_id: &str,
        workflow_id: &str,
    ) -> Result<bool, ExecutionError> {
        {
            let starting = self.starting_workflows.lock().await;
            if starting.contains(&(tenant_id.to_string(), workflow_id.to_string())) {
                return Ok(true);
            }
        }

        let runtime_client = match self.runtime_client.as_ref() {
            Some(client) => client,
            None => {
                // If no runtime client, we can't check runtara - assume no running instances
                return Ok(false);
            }
        };

        let result = runtime_client
            .list_instances_with_options(
                ListInstancesOptions::new()
                    .with_image_name_prefix(format!("{}:", workflow_id))
                    .with_status(InstanceStatus::Running)
                    .with_limit(1),
            )
            .await
            .map_err(|e| {
                ExecutionError::RuntimeError(format!("Failed to check running instances: {}", e))
            })?;

        Ok(!result.instances.is_empty())
    }

    // =========================================================================
    // Execution read API (proxy to Runtara)
    // =========================================================================

    /// Get execution results by instance ID (proxies to runtara-environment).
    pub async fn get_execution(
        &self,
        instance_id: &str,
    ) -> Result<WorkflowInstanceDto, ExecutionError> {
        let client = self.require_runtime_client()?;

        let info = client.get_instance_info(instance_id).await.map_err(|e| {
            if e.to_string().contains("not found") {
                ExecutionError::NotFound(format!("Instance '{}' not found", instance_id))
            } else {
                ExecutionError::DatabaseError(format!("Failed to get instance from Runtara: {}", e))
            }
        })?;

        Ok(runtara_info_to_dto(info))
    }

    /// Get an execution enriched with workflow metadata.
    pub async fn get_execution_with_metadata(
        &self,
        workflow_id: &str,
        instance_id: &str,
        tenant_id: &str,
    ) -> Result<ExecutionWithMetadata, ExecutionError> {
        let _ = Uuid::parse_str(instance_id).map_err(|_| {
            ExecutionError::ValidationError(
                "Invalid instance ID format. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        let client = self.require_runtime_client()?;

        let info = client.get_instance_info(instance_id).await.map_err(|e| {
            let error_str = e.to_string();
            warn!(
                instance_id = %instance_id,
                workflow_id = %workflow_id,
                error = %error_str,
                "Failed to get instance info from Runtara"
            );
            if error_str.contains("not found") || error_str.contains("InstanceNotFound") {
                ExecutionError::NotFound(format!(
                    "Instance '{}' not found for workflow '{}'",
                    instance_id, workflow_id
                ))
            } else {
                ExecutionError::DatabaseError(format!("Failed to get instance from Runtara: {}", e))
            }
        })?;

        // Verify the instance belongs to the expected workflow by checking image_name
        let expected_prefix = format!("{}:", workflow_id);
        debug!(
            instance_id = %instance_id,
            image_name = %info.image_name,
            image_id = %info.image_id,
            expected_prefix = %expected_prefix,
            "Checking instance workflow match"
        );
        if !info.image_name.starts_with(&expected_prefix) {
            warn!(
                instance_id = %instance_id,
                image_name = %info.image_name,
                expected_prefix = %expected_prefix,
                "Instance image_name does not match expected workflow prefix"
            );
            return Err(ExecutionError::NotFound(format!(
                "Instance '{}' not found for workflow '{}'",
                instance_id, workflow_id
            )));
        }

        let workflow = self
            .workflow_repo
            .get_by_id(tenant_id, workflow_id, None)
            .await
            .map_err(|e| ExecutionError::DatabaseError(format!("Failed to get workflow: {}", e)))?;

        let (workflow_name, workflow_description) = match workflow {
            Some(s) => (Some(s.name), Some(s.description)),
            None => (None, None),
        };

        let mut result =
            runtara_info_to_execution_with_metadata(info, workflow_name, workflow_description);
        enrich_pending_input(std::slice::from_mut(&mut result.instance), client).await;

        Ok(result)
    }

    /// List executions for a specific workflow (with pagination).
    pub async fn list_executions(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        page: Option<i32>,
        size: Option<i32>,
    ) -> Result<PageWorkflowInstanceHistoryDto, ExecutionError> {
        let page = page.unwrap_or(0).max(0);
        let size = size.unwrap_or(10).clamp(1, 100);

        let client = self.require_runtime_client()?;

        // Image names follow pattern: {workflow_id}:{version}
        let image_name_prefix = format!("{}:", workflow_id);

        let options = ListInstancesOptions::new()
            .with_tenant_id(tenant_id)
            .with_image_name_prefix(&image_name_prefix)
            .with_limit(size as u32)
            .with_offset((page * size) as u32);

        debug!(
            tenant_id = %tenant_id,
            workflow_id = %workflow_id,
            image_name_prefix = %image_name_prefix,
            page = page,
            size = size,
            "Listing executions from Runtara"
        );

        let result = client
            .list_instances_with_options(options)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to query Runtara: {}", e))
            })?;

        // Fetch the workflow name directly (we already know the workflow id)
        let workflow_name = match self
            .workflow_repo
            .get_workflow_names_bulk(tenant_id, &[workflow_id.to_string()])
            .await
        {
            Ok(names) => names
                .get(workflow_id)
                .map(|(name, _)| name.clone())
                .filter(|n| !n.is_empty()),
            Err(e) => {
                warn!(
                    tenant_id = %tenant_id,
                    workflow_id = %workflow_id,
                    error = %e,
                    "Failed to fetch workflow name"
                );
                None
            }
        };

        // Collect unique image IDs to look up version info
        let image_ids: Vec<String> = result
            .instances
            .iter()
            .map(|inst| inst.image_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let version_info: std::collections::HashMap<String, i32> = if !image_ids.is_empty() {
            match self
                .workflow_repo
                .get_workflow_info_by_image_ids(tenant_id, &image_ids)
                .await
            {
                Ok(info) => info.into_iter().map(|(k, (_, ver, _))| (k, ver)).collect(),
                Err(e) => {
                    warn!(
                        tenant_id = %tenant_id,
                        workflow_id = %workflow_id,
                        error = %e,
                        "Failed to fetch version info for executions list"
                    );
                    std::collections::HashMap::new()
                }
            }
        } else {
            std::collections::HashMap::new()
        };

        let mut instances: Vec<WorkflowInstanceDto> = result
            .instances
            .into_iter()
            .map(|inst| {
                let version = version_info.get(&inst.image_id).copied().unwrap_or(0);
                runtara_instance_to_dto_with_info(
                    inst,
                    workflow_id.to_string(),
                    version,
                    workflow_name.clone(),
                )
            })
            .collect();

        enrich_pending_input(&mut instances, client).await;

        let total_elements = result.total_count as i64;
        let total_pages = if total_elements == 0 {
            0
        } else {
            ((total_elements as f64) / (size as f64)).ceil() as i32
        };
        let number_of_elements = instances.len() as i32;

        Ok(PageWorkflowInstanceHistoryDto {
            content: instances,
            total_pages,
            total_elements,
            size,
            number: page,
            first: page == 0,
            last: page >= total_pages.max(1) - 1,
            number_of_elements,
        })
    }

    /// List all executions across all workflows with filtering, sorting, and pagination.
    pub async fn list_all_executions(
        &self,
        tenant_id: &str,
        page: Option<i32>,
        size: Option<i32>,
        filters: ExecutionFilters,
    ) -> Result<PageWorkflowInstanceHistoryDto, ExecutionError> {
        let page = page.unwrap_or(0).max(0);
        let size = size.unwrap_or(20).clamp(1, 100);

        let client = self.require_runtime_client()?;

        let mut options = ListInstancesOptions::new()
            .with_tenant_id(tenant_id)
            .with_limit(size as u32)
            .with_offset((page * size) as u32);

        if let Some(ref workflow_id) = filters.workflow_id {
            let image_name_prefix = format!("{}:", workflow_id);
            options = options.with_image_name_prefix(&image_name_prefix);
        }

        if let Some(ref statuses) = filters.statuses
            && let Some(first_status) = statuses.first()
            && let Some(runtara_status) = execution_status_to_runtara(first_status)
        {
            options = options.with_status(runtara_status);
        }

        if let Some(created_from) = filters.created_from {
            options = options.with_created_after(created_from);
        }
        if let Some(created_to) = filters.created_to {
            options = options.with_created_before(created_to);
        }
        if let Some(completed_from) = filters.completed_from {
            options = options.with_finished_after(completed_from);
        }
        if let Some(completed_to) = filters.completed_to {
            options = options.with_finished_before(completed_to);
        }

        let order = match (filters.sort_by.as_str(), filters.sort_order.as_str()) {
            ("created_at", "ASC") => ListInstancesOrder::CreatedAtAsc,
            ("created_at", "DESC") => ListInstancesOrder::CreatedAtDesc,
            ("completed_at", "ASC") => ListInstancesOrder::FinishedAtAsc,
            ("completed_at", "DESC") => ListInstancesOrder::FinishedAtDesc,
            (_, "ASC") => ListInstancesOrder::FinishedAtAsc,
            _ => ListInstancesOrder::FinishedAtDesc,
        };
        options = options.with_order_by(order);

        debug!(
            tenant_id = %tenant_id,
            page = page,
            size = size,
            workflow_id_filter = ?filters.workflow_id,
            status_filter = ?filters.statuses,
            created_from = ?filters.created_from,
            created_to = ?filters.created_to,
            completed_from = ?filters.completed_from,
            completed_to = ?filters.completed_to,
            "Listing all executions from Runtara"
        );

        let result = client
            .list_instances_with_options(options)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to query Runtara: {}", e))
            })?;

        let image_ids: Vec<String> = result
            .instances
            .iter()
            .map(|inst| inst.image_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let workflow_info: std::collections::HashMap<String, (String, i32, String)> =
            if !image_ids.is_empty() {
                match self
                    .workflow_repo
                    .get_workflow_info_by_image_ids(tenant_id, &image_ids)
                    .await
                {
                    Ok(info) => info,
                    Err(e) => {
                        warn!(
                            tenant_id = %tenant_id,
                            error = %e,
                            "Failed to fetch workflow info for executions list"
                        );
                        std::collections::HashMap::new()
                    }
                }
            } else {
                std::collections::HashMap::new()
            };

        let workflow_ids_needing_names: Vec<String> = workflow_info
            .values()
            .filter(|(_, _, name)| name.is_empty())
            .map(|(sid, _, _)| sid.clone())
            .filter(|sid| !sid.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let workflow_names: std::collections::HashMap<String, String> =
            if !workflow_ids_needing_names.is_empty() {
                match self
                    .workflow_repo
                    .get_workflow_names_bulk(tenant_id, &workflow_ids_needing_names)
                    .await
                {
                    Ok(names) => names
                        .into_iter()
                        .filter(|(_, (name, _))| !name.is_empty())
                        .map(|(sid, (name, _))| (sid, name))
                        .collect(),
                    Err(e) => {
                        warn!(
                            tenant_id = %tenant_id,
                            error = %e,
                            "Failed to fetch workflow names"
                        );
                        std::collections::HashMap::new()
                    }
                }
            } else {
                std::collections::HashMap::new()
            };

        let mut instances: Vec<WorkflowInstanceDto> = result
            .instances
            .into_iter()
            .map(|inst| {
                let (workflow_id, version, workflow_name) = workflow_info
                    .get(&inst.image_id)
                    .map(|(sid, ver, name)| {
                        let final_name = if name.is_empty() {
                            workflow_names.get(sid).cloned()
                        } else {
                            Some(name.clone())
                        };
                        (sid.clone(), *ver, final_name)
                    })
                    .unwrap_or_else(|| (String::new(), 0, None));

                runtara_instance_to_dto_with_info(inst, workflow_id, version, workflow_name)
            })
            .collect();

        enrich_pending_input(&mut instances, client).await;

        let total_elements = result.total_count as i64;
        let total_pages = if total_elements == 0 {
            0
        } else {
            ((total_elements as f64) / (size as f64)).ceil() as i32
        };
        let number_of_elements = instances.len() as i32;

        Ok(PageWorkflowInstanceHistoryDto {
            content: instances,
            total_pages,
            total_elements,
            size,
            number: page,
            first: page == 0,
            last: page >= total_pages.max(1) - 1,
            number_of_elements,
        })
    }

    // =========================================================================
    // Lifecycle control (stop / pause / resume)
    // =========================================================================

    /// Stop a running instance.
    pub async fn stop(&self, instance_id: &str) -> Result<StopOutcome, ExecutionError> {
        let _ = Uuid::parse_str(instance_id).map_err(|_| {
            ExecutionError::ValidationError(
                "Invalid instance ID. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        let client = self.require_runtime_client()?;

        let runtara_status = client
            .get_instance_status(instance_id)
            .await
            .map_err(|e| ExecutionError::NotFound(format!("Instance not found: {}", e)))?;

        let status_str = format!("{:?}", runtara_status).to_lowercase();

        if matches!(
            runtara_status,
            crate::runtime_client::InstanceStatus::Completed
                | crate::runtime_client::InstanceStatus::Failed
                | crate::runtime_client::InstanceStatus::Cancelled
        ) {
            return Ok(StopOutcome::AlreadyStopped { status: status_str });
        }

        client.cancel_instance(instance_id).await.map_err(|e| {
            ExecutionError::DatabaseError(format!("Failed to cancel instance: {}", e))
        })?;

        if matches!(
            runtara_status,
            crate::runtime_client::InstanceStatus::Suspended
        ) && let Err(e) = client.resume_instance(instance_id).await
        {
            warn!(
                instance_id = %instance_id,
                error = %e,
                "Failed to resume suspended instance for cancellation"
            );
        }

        info!(
            instance_id = %instance_id,
            previous_status = %status_str,
            "Cancelled instance via runtara-environment"
        );

        Ok(StopOutcome::Stopped {
            previous_status: status_str,
        })
    }

    /// Pause a running workflow instance.
    pub async fn pause(&self, instance_id: &str) -> Result<PauseOutcome, ExecutionError> {
        let _ = Uuid::parse_str(instance_id).map_err(|_| {
            ExecutionError::ValidationError(
                "Invalid instance ID. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        let client = self.require_runtime_client()?;

        let runtara_status = client
            .get_instance_status(instance_id)
            .await
            .map_err(|e| ExecutionError::NotFound(format!("Instance not found: {}", e)))?;

        let status_str = format!("{:?}", runtara_status).to_lowercase();

        match status_str.as_str() {
            "suspended" => Ok(PauseOutcome::AlreadyPaused),
            "running" => {
                client.pause_instance(instance_id).await.map_err(|e| {
                    ExecutionError::DatabaseError(format!("Failed to send pause signal: {}", e))
                })?;

                info!(
                    instance_id = %instance_id,
                    "Sent pause signal to instance"
                );

                Ok(PauseOutcome::Paused {
                    previous_status: status_str,
                })
            }
            _ => Ok(PauseOutcome::NotPausable { status: status_str }),
        }
    }

    /// Resume a paused/suspended workflow instance.
    pub async fn resume(&self, instance_id: &str) -> Result<ResumeOutcome, ExecutionError> {
        let _ = Uuid::parse_str(instance_id).map_err(|_| {
            ExecutionError::ValidationError(
                "Invalid instance ID. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        let client = self.require_runtime_client()?;

        let runtara_status = client
            .get_instance_status(instance_id)
            .await
            .map_err(|e| ExecutionError::NotFound(format!("Instance not found: {}", e)))?;

        let status_str = format!("{:?}", runtara_status).to_lowercase();

        match status_str.as_str() {
            "running" => Ok(ResumeOutcome::AlreadyRunning),
            "suspended" | "failed" | "cancelled" => {
                client.resume_instance(instance_id).await.map_err(|e| {
                    ExecutionError::DatabaseError(format!("Failed to send resume signal: {}", e))
                })?;

                info!(
                    instance_id = %instance_id,
                    previous_status = %status_str,
                    "Sent resume signal to instance"
                );

                Ok(ResumeOutcome::Resumed {
                    previous_status: status_str,
                })
            }
            _ => Ok(ResumeOutcome::NotResumable { status: status_str }),
        }
    }

    // =========================================================================
    // Internal helpers
    // =========================================================================

    fn require_runtime_client(&self) -> Result<&Arc<RuntimeClient>, ExecutionError> {
        self.runtime_client.as_ref().ok_or_else(|| {
            ExecutionError::NotConnected(
                "Runtime client not configured. Cannot reach runtara-environment.".to_string(),
            )
        })
    }

    /// Resolve an explicit or current/latest version for a workflow.
    async fn resolve_version(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: Option<i32>,
    ) -> Result<i32, ExecutionError> {
        match version {
            Some(v) if v > 0 => Ok(v),
            Some(_) => Err(ExecutionError::NotFound(format!(
                "Workflow '{}' has no versions",
                workflow_id
            ))),
            None => {
                let resolved = self
                    .workflow_repo
                    .get_current_or_latest_version(tenant_id, workflow_id)
                    .await
                    .map_err(|e| {
                        ExecutionError::DatabaseError(format!(
                            "Failed to get current version: {}",
                            e
                        ))
                    })?
                    .ok_or_else(|| {
                        ExecutionError::NotFound(format!("Workflow '{}' not found", workflow_id))
                    })?;

                if resolved == 0 {
                    return Err(ExecutionError::NotFound(format!(
                        "Workflow '{}' has no versions",
                        workflow_id
                    )));
                }

                Ok(resolved)
            }
        }
    }

    /// Get the registered image ID for a compiled workflow.
    async fn get_registered_image_id(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<String, ExecutionError> {
        self.workflow_repo
            .get_registered_image_id(tenant_id, workflow_id, version)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to get registered image ID: {}", e))
            })?
            .ok_or_else(|| {
                ExecutionError::BinaryNotFound(format!(
                    "Workflow '{}' version {} not registered with runtara-environment. Recompile it.",
                    workflow_id, version
                ))
            })
    }

    /// Ensure the workflow is compiled (non-blocking: queues compilation if
    /// needed and returns `NotCompiled` for the caller to retry).
    async fn ensure_compiled(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<(), ExecutionError> {
        let status = self
            .workflow_repo
            .ensure_compilation_ready(tenant_id, workflow_id, version)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to check compilation: {}", e))
            })?;

        if matches!(status, CompilationStatus::Ready { .. }) {
            return Ok(());
        }

        // Not compiled - queue compilation if not already pending
        let compilation_queued =
            if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
                let redis_url = valkey_config.connection_url();

                let is_pending = crate::workers::compilation_worker::is_compilation_pending(
                    &redis_url,
                    tenant_id,
                    workflow_id,
                    version,
                )
                .await
                .unwrap_or(false);

                if is_pending {
                    info!(
                        tenant_id = %tenant_id,
                        workflow_id = %workflow_id,
                        version = version,
                        "Compilation already pending, returning NotCompiled for retry"
                    );
                    false
                } else {
                    info!(
                        tenant_id = %tenant_id,
                        workflow_id = %workflow_id,
                        version = version,
                        "Workflow not compiled, queueing compilation..."
                    );

                    match crate::workers::compilation_worker::enqueue_compilation(
                        &redis_url,
                        tenant_id,
                        workflow_id,
                        version,
                        false,
                    )
                    .await
                    {
                        Ok(queued) => {
                            info!(
                                tenant_id = %tenant_id,
                                workflow_id = %workflow_id,
                                version = version,
                                queued = queued,
                                "Compilation queued, returning NotCompiled for retry"
                            );
                            queued
                        }
                        Err(e) => {
                            tracing::warn!(
                                tenant_id = %tenant_id,
                                workflow_id = %workflow_id,
                                version = version,
                                error = %e,
                                "Failed to queue compilation"
                            );
                            false
                        }
                    }
                }
            } else {
                tracing::warn!(
                    tenant_id = %tenant_id,
                    workflow_id = %workflow_id,
                    version = version,
                    "Valkey not configured, cannot queue compilation"
                );
                false
            };

        Err(ExecutionError::NotCompiled {
            workflow_id: workflow_id.to_string(),
            version,
            compilation_queued,
        })
    }

    /// Block until compilation completes (used by the synchronous execution
    /// path). Delegates the actual wait to
    /// `compilation_worker::wait_for_compilation`.
    async fn wait_for_compilation_blocking(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<(), ExecutionError> {
        let status = self
            .workflow_repo
            .ensure_compilation_ready(tenant_id, workflow_id, version)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to check compilation: {}", e))
            })?;
        if matches!(status, CompilationStatus::Ready { .. }) {
            return Ok(());
        }

        let valkey_config = match crate::valkey::ValkeyConfig::from_env() {
            Some(v) => v,
            None => {
                return Err(ExecutionError::NotFound(format!(
                    "Workflow '{}' version {} not compiled and Valkey is not configured for auto-compilation.",
                    workflow_id, version
                )));
            }
        };
        let redis_url = valkey_config.connection_url();

        let is_pending = crate::workers::compilation_worker::is_compilation_pending(
            &redis_url,
            tenant_id,
            workflow_id,
            version,
        )
        .await
        .unwrap_or(false);

        if is_pending {
            info!(
                tenant_id = %tenant_id,
                workflow_id = %workflow_id,
                version = version,
                "Compilation pending, waiting for it to complete..."
            );
        } else {
            info!(
                tenant_id = %tenant_id,
                workflow_id = %workflow_id,
                version = version,
                "Workflow not compiled, queueing compilation..."
            );
            match crate::workers::compilation_worker::enqueue_compilation(
                &redis_url,
                tenant_id,
                workflow_id,
                version,
                false,
            )
            .await
            {
                Ok(_) => {
                    info!(
                        tenant_id = %tenant_id,
                        workflow_id = %workflow_id,
                        version = version,
                        "Compilation queued, waiting for it to complete..."
                    );
                }
                Err(e) => {
                    return Err(ExecutionError::CompilationFailed(format!(
                        "Failed to queue compilation for workflow '{}' version {}: {}",
                        workflow_id, version, e
                    )));
                }
            }
        }

        // Delegate the actual blocking wait
        let timeout = Duration::from_secs(300);
        let completed = crate::workers::compilation_worker::wait_for_compilation(
            &redis_url,
            tenant_id,
            workflow_id,
            version,
            timeout,
        )
        .await
        .unwrap_or(false);

        if !completed {
            return Err(ExecutionError::CompilationTimeout(format!(
                "Compilation for workflow '{}' version {} timed out after 5 minutes.",
                workflow_id, version
            )));
        }

        let status_after = self
            .workflow_repo
            .ensure_compilation_ready(tenant_id, workflow_id, version)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to check compilation: {}", e))
            })?;
        if matches!(status_after, CompilationStatus::Ready { .. }) {
            Ok(())
        } else {
            Err(ExecutionError::CompilationFailed(format!(
                "Compilation for workflow '{}' version {} completed but binary not found.",
                workflow_id, version
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // =========================================================================
    // ExecutionError Display tests
    // =========================================================================

    #[test]
    fn test_execution_error_display_workflow_not_found() {
        let error = ExecutionError::WorkflowNotFound("test-workflow".to_string());
        assert_eq!(format!("{}", error), "Workflow not found: test-workflow");
    }

    #[test]
    fn test_execution_error_display_not_compiled() {
        let error = ExecutionError::NotCompiled {
            workflow_id: "test-workflow".to_string(),
            version: 5,
            compilation_queued: true,
        };
        let display = format!("{}", error);
        assert!(display.contains("test-workflow"));
        assert!(display.contains("5"));
        assert!(display.contains("true"));
    }

    #[test]
    fn test_execution_error_display_compilation_failed() {
        let error = ExecutionError::CompilationFailed("syntax error".to_string());
        assert_eq!(format!("{}", error), "Compilation failed: syntax error");
    }

    #[test]
    fn test_execution_error_http_status_validation() {
        assert_eq!(
            ExecutionError::ValidationError("bad".to_string()).http_status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn test_execution_error_http_status_not_compiled() {
        let err = ExecutionError::NotCompiled {
            workflow_id: "s".into(),
            version: 1,
            compilation_queued: false,
        };
        assert_eq!(err.http_status(), StatusCode::CONFLICT);
    }

    #[test]
    fn test_execution_error_http_status_compilation_timeout() {
        assert_eq!(
            ExecutionError::CompilationTimeout("slow".to_string()).http_status(),
            StatusCode::GATEWAY_TIMEOUT
        );
    }

    #[test]
    fn test_execution_error_http_status_not_connected() {
        assert_eq!(
            ExecutionError::NotConnected("no conn".to_string()).http_status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    // =========================================================================
    // ExecutionResult tests
    // =========================================================================

    #[test]
    fn test_execution_result_success() {
        let result = ExecutionResult {
            success: true,
            output: Some(json!({"result": 42})),
            error: None,
            duration_seconds: 1.5,
            max_memory_mb: Some(128.0),
            cpu_usage_ms: Some(500.0),
        };

        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.output.unwrap()["result"], 42);
    }

    #[test]
    fn test_execution_result_failure() {
        let result = ExecutionResult {
            success: false,
            output: None,
            error: Some("Something went wrong".to_string()),
            duration_seconds: 0.1,
            max_memory_mb: None,
            cpu_usage_ms: None,
        };

        assert!(!result.success);
        assert!(result.output.is_none());
        assert_eq!(result.error.unwrap(), "Something went wrong");
    }
}
