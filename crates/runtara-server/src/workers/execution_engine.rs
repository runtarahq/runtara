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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use dashmap::DashMap;
use serde_json::Value;
use sqlx::PgPool;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::runtime_types::{ListInstancesOptions, ListInstancesOrder};

use crate::api::dto::executions::ExecutionFilters;
use crate::api::dto::trigger_event::TriggerEvent;
use crate::api::dto::workflows::{
    PageWorkflowInstanceHistoryDto, ValidationErrorDto, WorkflowInstanceDto,
    validate_workflow_inputs,
};
use crate::api::repositories::trigger_stream::TriggerStreamPublisher;
use crate::api::repositories::workflows::{CompilationStatus, WorkflowRepository};
use crate::metrics::MetricsService;
use crate::product_events::{ActorType, EventSource, EventType, ProductEvent, ProductEventSink};
use crate::runtime_client::{RuntimeClient, RuntimeError};
use crate::workers::CancellationHandle;
use crate::workers::execution_outbox::{
    EnqueuedExecution, ExecutionOutbox, ExecutionOutboxError, source_idempotency_key,
};
use crate::workers::runtara_dto::{
    ExecutionWithMetadata, enrich_pending_input, execution_statuses_to_runtara, parse_image_id,
    runtara_info_to_dto, runtara_info_to_execution_with_metadata,
    runtara_instance_to_dto_with_info,
};
use runtara_environment::execution_timeout::ExecutionTimeoutSeconds;
use runtara_workflows::input_validation::validate_workflow_start_inputs;

/// Recover workflow identity from an artifact-qualified runtime image name.
///
/// The server database retains only the currently selected image ID for a
/// workflow version.  Older instances still retain their Environment image
/// name, whose readable prefix is durable provenance for listings after a
/// recompile.
fn workflow_info_from_image_name(image_name: &str) -> Option<(String, i32)> {
    let (workflow_id, version) = parse_image_id(image_name);
    (!workflow_id.is_empty() && version > 0).then_some((workflow_id, version))
}

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
    /// The workflow cannot run as authored — no steps, an unreachable entry
    /// point, and so on. Permanent until the graph is edited, and reported to
    /// the author rather than alerted on.
    WorkflowNotRunnable {
        workflow_id: String,
        version: i32,
        error: String,
    },
    /// A guarded trigger lost the durable workflow-wide launch race.
    SingleInstanceActive,
    RuntimeError(String),
    DatabaseError(String),
    NotConnected(String),
    /// Per-tenant entitlement gate denied the request
    /// (`maxConcurrentExecutions` or similar). Carries the full
    /// `EntitlementDenial` so the handler can render the standard
    /// `ENTITLEMENT_LIMIT_EXCEEDED` body via `denial.json_body()`.
    EntitlementDenied(crate::entitlement_error::EntitlementDenial),
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
            ExecutionError::WorkflowNotRunnable {
                workflow_id,
                version,
                error,
            } => {
                write!(
                    f,
                    "Workflow '{}' version {} cannot run: {}",
                    workflow_id, version, error
                )
            }
            ExecutionError::SingleInstanceActive => {
                write!(f, "single-instance workflow already has active work")
            }
            ExecutionError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            ExecutionError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            ExecutionError::NotConnected(msg) => write!(f, "Not connected: {}", msg),
            ExecutionError::EntitlementDenied(denial) => write!(f, "{}", denial.message()),
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
            ExecutionError::WorkflowNotRunnable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            ExecutionError::SingleInstanceActive => StatusCode::CONFLICT,
            ExecutionError::RuntimeError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ExecutionError::DatabaseError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ExecutionError::NotConnected(_) => StatusCode::SERVICE_UNAVAILABLE,
            ExecutionError::EntitlementDenied(_) => StatusCode::FORBIDDEN,
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
    Replay { original_instance_id: String },
}

/// Request to queue an async workflow execution onto the trigger stream.
pub struct QueueRequest<'a> {
    pub tenant_id: &'a str,
    pub workflow_id: &'a str,
    pub version: Option<i32>,
    pub inputs: Value,
    pub debug: bool,
    pub correlation_id: Option<String>,
    /// Optional stable source identity (for example an HTTP Idempotency-Key).
    /// When absent the generated/caller-supplied instance ID remains the
    /// idempotency identity for this one queue submission.
    pub idempotency_key: Option<String>,
    pub trigger_source: TriggerSource,
    /// Optional caller-provided identity for idempotent queueing. Environment
    /// deduplicates starts by instance ID, so retries can safely republish the
    /// trigger without creating a second execution history entry.
    pub instance_id: Option<Uuid>,
}

/// Result of queuing an execution.
#[derive(Debug)]
pub struct QueuedExecution {
    pub instance_id: Uuid,
    pub status: String,
    pub workflow_id: String,
    pub version: i32,
}

/// Outcome of submitting a detached execution to Environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetachedExecution {
    /// This call reserved the instance ID and launched the process.
    Started(String),
    /// Environment had already accepted this instance ID, so no process was
    /// launched by this replayed trigger delivery.
    Deduplicated(String),
}

impl DetachedExecution {
    pub fn instance_id(&self) -> &str {
        match self {
            Self::Started(instance_id) | Self::Deduplicated(instance_id) => instance_id,
        }
    }
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

fn is_runtime_instance_not_found(error: &crate::runtime_client::RuntimeError) -> bool {
    let message = error.to_string();
    message.to_ascii_lowercase().contains("not found") || message.contains("InstanceNotFound")
}

/// Execution engine — the single orchestrator for workflow execution.
pub struct ExecutionEngine {
    pool: PgPool,
    workflow_repo: Arc<WorkflowRepository>,
    runtime_client: Option<Arc<RuntimeClient>>,
    /// Kept in the constructor while callers migrate; stream publishing is
    /// owned by [`ExecutionOutbox`] and its relay, never by an intake source.
    #[allow(dead_code)]
    trigger_stream: Option<Arc<TriggerStreamPublisher>>,
    /// Durable source request + admission reservation writer.
    outbox: ExecutionOutbox,
    #[allow(dead_code)] // Reserved for future in-memory cancellation tracking.
    running_executions: Option<Arc<DashMap<Uuid, CancellationHandle>>>,
    /// Sink for product-analytics execution events.
    events: ProductEventSink,
    /// Short-lived cache of the per-tenant in-flight count used by the
    /// concurrency gate, so a burst of intake does not issue two status counts
    /// per accepted execution.
    concurrency_counts: Arc<DashMap<String, (Instant, u64)>>,
    /// Executions admitted by the gate that the cached count cannot see yet.
    ///
    /// The count is cached, and an accepted request does not appear in it until
    /// the instance exists and the entry is refreshed. Without this, a cap of
    /// one and a cached zero admits every request arriving inside the TTL —
    /// the overshoot is bounded by arrival rate, not by the limit. Counting
    /// admissions alongside the cached figure keeps the decision honest until
    /// a fresh count subsumes them.
    concurrency_reservations: Arc<DashMap<String, Arc<AtomicU64>>>,
    /// Held by whichever task is currently refreshing a tenant's count.
    ///
    /// Without this the TTL does not bound how often the count runs: while one
    /// refresh is in flight every other intake still sees an expired entry and
    /// starts its own. That is self-reinforcing, because each extra concurrent
    /// count makes the database slower, which widens the window, which admits
    /// more of them.
    concurrency_refresh: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Serializes the in-process active-runtime decision with a durable
    /// enqueue for one tenant. The database reservation remains authoritative
    /// across processes/restarts; this prevents a local cached-count race
    /// before the next background runtime refresh observes a launch.
    concurrency_admission: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Monotonic counters for the execution pipeline.
    ///
    /// Read by the analytics sampler; written only from the gate below, where
    /// the cost is one relaxed atomic add per intake.
    gauges: Arc<crate::workers::pipeline_gauges::PipelineGauges>,
}

/// How long an in-flight execution count stays usable for the concurrency gate.
///
/// Short enough that the cap still tracks reality, long enough that a
/// sustained intake burst collapses two counts per execution into two counts
/// per tick.
const CONCURRENCY_COUNT_TTL: Duration = Duration::from_millis(500);

/// One verified compilation result, carried unchanged to `start_instance`.
///
/// The image ID and tracking mode have to come from the same readiness read.
/// Re-reading the image after the mode was checked can pair a freshly tracked
/// row with an older artifact during a toggle or recompile.
struct ReadyLaunch {
    image_id: String,
    execution_timeout: Option<ExecutionTimeoutSeconds>,
    track_events: bool,
}

/// Admissions still unaccounted for after a fresh count landed.
///
/// `subsumed` is the counter as it stood when the query was issued: those
/// admissions have had time to become instances and are in the result, so they
/// are forgiven. Anything admitted while the query was in flight is not in that
/// result and has to stay counted, or a burst arriving during the query is
/// admitted twice over — once against the stale figure and again against the
/// fresh one.
fn retained_reservations(subsumed: u64, current: u64) -> u64 {
    current.saturating_sub(subsumed.min(current))
}

impl ExecutionEngine {
    /// Create a new execution engine.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        workflow_repo: Arc<WorkflowRepository>,
        runtime_client: Option<Arc<RuntimeClient>>,
        trigger_stream: Option<Arc<TriggerStreamPublisher>>,
        running_executions: Option<Arc<DashMap<Uuid, CancellationHandle>>>,
        events: ProductEventSink,
        gauges: Arc<crate::workers::pipeline_gauges::PipelineGauges>,
    ) -> Self {
        Self {
            pool: pool.clone(),
            workflow_repo,
            runtime_client,
            trigger_stream,
            outbox: ExecutionOutbox::new(pool.clone()),
            running_executions,
            events,
            concurrency_counts: Arc::new(DashMap::new()),
            concurrency_reservations: Arc::new(DashMap::new()),
            concurrency_refresh: Arc::new(DashMap::new()),
            concurrency_admission: Arc::new(DashMap::new()),
            gauges,
        }
    }

    /// The pipeline counters this engine writes to.
    pub fn gauges(&self) -> &Arc<crate::workers::pipeline_gauges::PipelineGauges> {
        &self.gauges
    }

    /// In-flight executions as the admission gate itself counts them.
    ///
    /// The same cached figure the gate decides on, so a viewer sees what the
    /// gate sees rather than a second opinion taken from a different query at
    /// a different moment — two numbers that disagree about the same thing are
    /// worse than one.
    ///
    /// Never queries: it reads the cache the gate maintains, so calling it on
    /// a sampler tick costs nothing and cannot slow intake.
    pub fn observed_in_flight(&self, tenant_id: &str, ceiling: u64) -> u64 {
        self.active_execution_count(tenant_id, ceiling)
    }

    /// Count the tenant's currently in-flight executions (Running + Pending)
    /// as reported by the runtime — the source of truth. Two cheap
    /// `total_count` lookups (limit 1) rather than fetching rows.
    ///
    /// SYN-433 Finding 1: we query the runtime instead of maintaining our own
    /// counter precisely because the runtime reflects real completion — an
    /// execution drops out of this count the instant it terminates, so the
    /// gate means "N concurrent", not "N started recently".
    fn active_execution_count(&self, tenant_id: &str, ceiling: u64) -> u64 {
        let reserved = self.reservations_for(tenant_id);
        let cached = self.concurrency_counts.get(tenant_id).map(|e| (e.0, e.1));

        // Refresh behind the caller, never in front of it. The count reads a
        // table that the same workload is churning, so its cost tracks dead
        // tuples rather than the answer's size; on the request path that makes
        // admission slower exactly when admissions are frequent. Answering from
        // the last count keeps the decision honest, because every admission
        // since it was taken is counted in `reserved` and added below.
        let stale = cached.is_none_or(|(at, _)| at.elapsed() >= CONCURRENCY_COUNT_TTL);
        if stale {
            self.spawn_count_refresh(tenant_id.to_string(), ceiling);
        }

        cached
            .map(|(_, count)| count)
            .unwrap_or(0)
            .saturating_add(reserved.load(Ordering::SeqCst))
    }

    /// Refresh one tenant's count in the background, at most one at a time.
    ///
    /// Without the guard the TTL does not bound how often the count runs: every
    /// caller past it would start another, and each concurrent count makes the
    /// database slower, which widens the window and starts more of them.
    fn spawn_count_refresh(&self, tenant_id: String, ceiling: u64) {
        let Some(client) = self.runtime_client.clone() else {
            return;
        };
        let refresh = self.refresh_lock_for(&tenant_id);
        let reserved = self.reservations_for(&tenant_id);
        let counts = Arc::clone(&self.concurrency_counts);

        tokio::spawn(async move {
            let Ok(_refreshing) = refresh.try_lock() else {
                return;
            };
            // Snapshot before the query, not after: anything admitted while it
            // is in flight is not in the result, so only the admissions this
            // count subsumes may be forgiven.
            let subsumed = reserved.load(Ordering::SeqCst);
            let statuses = vec!["running".to_string(), "pending".to_string()];
            match client
                .count_instances_by_status(&tenant_id, &statuses, ceiling.saturating_add(1))
                .await
            {
                Ok(count) => {
                    let retained = retained_reservations(subsumed, reserved.load(Ordering::SeqCst));
                    reserved.store(retained, Ordering::SeqCst);
                    counts.insert(tenant_id, (Instant::now(), count));
                }
                Err(e) => {
                    tracing::warn!(
                        tenant_id = %tenant_id,
                        error = %e,
                        "concurrency gate: background count failed, keeping the previous figure"
                    );
                }
            }
        });
    }

    /// The refresh mutex for a tenant, created on first use.
    fn refresh_lock_for(&self, tenant_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.concurrency_refresh
                .entry(tenant_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .value(),
        )
    }

    /// The admission counter for a tenant, created on first use.
    fn reservations_for(&self, tenant_id: &str) -> Arc<AtomicU64> {
        Arc::clone(
            self.concurrency_reservations
                .entry(tenant_id.to_string())
                .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                .value(),
        )
    }

    fn admission_lock_for(&self, tenant_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.concurrency_admission
                .entry(tenant_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .value(),
        )
    }

    fn effective_concurrency_cap(&self) -> u64 {
        let snapshot = crate::config::entitlements();
        crate::middleware::entitlement::effective_limit(
            crate::config::raw_max_concurrent_executions(),
            snapshot.limits.max_concurrent_executions,
        ) as u64
    }

    async fn concurrent_execution_decision(
        &self,
        tenant_id: &str,
    ) -> Result<(), crate::entitlement_error::EntitlementDenial> {
        let snapshot = crate::config::entitlements();
        let cap = self.effective_concurrency_cap();
        let active = self.active_execution_count(tenant_id, cap);
        crate::middleware::entitlement::concurrent_executions_decision(
            snapshot,
            active,
            crate::config::raw_max_concurrent_executions(),
        )
        .inspect_err(|denial| denial.audit_log(tenant_id))
    }

    /// Enforce `maxConcurrentExecutions` at intake. Returns the
    /// `EntitlementDenial` to surface when the tenant's live active-instance
    /// count is at/over the effective cap; `Ok(())` otherwise.
    ///
    /// Applies whether or not the tenant carries a `maxConcurrentExecutions`
    /// entitlement: with no tenant cap the infra bound
    /// (`MAX_CONCURRENT_EXECUTIONS`) is still enforced, which is the only thing
    /// standing between an untiered deployment and unbounded intake. Fails
    /// **open** if the count query errors: a transient runtime/count failure
    /// should not wedge execution.
    ///
    /// The count is cached for [`CONCURRENCY_COUNT_TTL`] so a high intake rate
    /// does not issue two status counts per accepted execution. The cap is a
    /// backstop against runaway intake, not a precise quota, and the runtime
    /// count is itself a moment-in-time reading — a sub-second staleness window
    /// cannot let intake exceed the cap by more than one TTL's worth of work.
    pub(crate) async fn check_concurrency_gate(
        &self,
        tenant_id: &str,
    ) -> Result<(), crate::entitlement_error::EntitlementDenial> {
        // Counted here rather than at either call site, so the identity
        // `offered == accepted + denied` cannot drift as call sites are added.
        self.gauges.record_offered();
        // No early return when the tenant has no `maxConcurrentExecutions`:
        // the infra cap (`MAX_CONCURRENT_EXECUTIONS`, default cores x 32) has
        // to stand on its own, or an untiered deployment has no admission
        // control at all — it will accept executions until it runs out of file
        // descriptors or memory. `concurrent_executions_decision` already
        // composes the two with `effective_limit`, so passing through with the
        // entitlement unset simply applies the infra bound.
        //
        // Safe against large sleeping populations: `active_execution_count`
        // counts only Running + Pending, so suspended instances (which are not
        // terminal, but are also not consuming a slot) never count against the
        // cap.
        // Resolve the cap before counting so the count can stop there. The
        // gate only needs to know whether the cap is reached, and an unbounded
        // count gets slower exactly as a backlog builds.
        let decision = self.concurrent_execution_decision(tenant_id).await;

        // Record the admission so the next caller sees it even though the
        // cached count still cannot.
        if decision.is_ok() {
            self.gauges.record_accepted();
            self.reservations_for(tenant_id)
                .fetch_add(1, Ordering::SeqCst);
        } else {
            // The refusal rate had no counter at all before this: the denial
            // was returned to the caller and observed nowhere, so the first
            // question anyone asks about failing intake was unanswerable.
            self.gauges.record_denied();
        }
        decision
    }

    /// The only asynchronous source admission path. It first returns an
    /// existing idempotent request, then commits the source reservation and
    /// outbox record before any relay touches Valkey.
    ///
    /// The returned request ID is carried by the relay in the stream entry and
    /// is the handoff token for P0.1's launch queue. Stream delivery never
    /// releases the source reservation: it remains held until the durable
    /// launch parks, terminalizes, expires, or is cancelled.
    pub async fn enqueue_trigger_event(
        &self,
        tenant_id: &str,
        event: TriggerEvent,
        idempotency_key: String,
    ) -> Result<EnqueuedExecution, ExecutionError> {
        if let Some(existing) = self
            .outbox
            .find_by_idempotency(tenant_id, &idempotency_key)
            .await
            .map_err(map_outbox_error)?
        {
            return Ok(existing);
        }

        let admission_lock = self.admission_lock_for(tenant_id);
        let _admission_guard = admission_lock.lock().await;

        // A waiter can have committed the same key while this task waited for
        // the local active-count lock. Do not consume a second reservation.
        if let Some(existing) = self
            .outbox
            .find_by_idempotency(tenant_id, &idempotency_key)
            .await
            .map_err(map_outbox_error)?
        {
            return Ok(existing);
        }

        self.gauges.record_offered();
        if let Err(denial) = self.concurrent_execution_decision(tenant_id).await {
            self.gauges.record_denied();
            return Err(ExecutionError::EntitlementDenied(denial));
        }

        let cap = self.effective_concurrency_cap();
        match self
            .outbox
            .enqueue(tenant_id, &event, &idempotency_key, cap)
            .await
        {
            Ok(enqueued) if enqueued.duplicate => Ok(enqueued),
            Ok(enqueued) => {
                // Preserve the active-runtime gate's short handoff protection
                // until the runtime count refresh subsumes this accepted
                // request. The durable DB reservation independently protects
                // a restart or a Valkey outage before relay delivery.
                self.gauges.record_accepted();
                self.reservations_for(tenant_id)
                    .fetch_add(1, Ordering::SeqCst);
                Ok(enqueued)
            }
            Err(ExecutionOutboxError::AdmissionFull { limit }) => {
                self.gauges.record_denied();
                let denial = crate::entitlement_error::EntitlementDenial::LimitExceeded {
                    limit: "maxConcurrentExecutions",
                    maximum: limit,
                };
                denial.audit_log(tenant_id);
                Err(ExecutionError::EntitlementDenied(denial))
            }
            Err(error) => {
                // The intake response is a refusal, not an accepted request;
                // retain the pipeline identity while surfacing the real DB or
                // validation failure to the caller.
                self.gauges.record_denied();
                Err(map_outbox_error(error))
            }
        }
    }

    /// Lifecycle hook for P0.1: release the durable source reservation once a
    /// launch reaches a terminal/suspended handoff. It is idempotent so the
    /// lifecycle callback and the crash-recovery reconciler can race safely.
    pub async fn release_durable_admission_for_instance(
        &self,
        tenant_id: &str,
        instance_id: &str,
        reason: &str,
    ) -> Result<bool, ExecutionError> {
        self.outbox
            .release_admission_for_instance(tenant_id, instance_id, reason)
            .await
            .map_err(map_outbox_error)
    }

    /// Check if the runtime client is available.
    #[allow(dead_code)]
    pub fn has_runtime(&self) -> bool {
        self.runtime_client.is_some()
    }

    async fn workflow_id_for_instance_image(
        &self,
        tenant_id: &str,
        image_name: &str,
        image_id: &str,
    ) -> Result<String, ExecutionError> {
        let parsed_workflow_id = if image_name.trim().is_empty() {
            None
        } else {
            let (workflow_id, _) = parse_image_id(image_name);
            (!workflow_id.trim().is_empty()).then_some(workflow_id)
        };

        if image_name.contains(':')
            && let Some(workflow_id) = parsed_workflow_id.clone()
        {
            return Ok(workflow_id);
        }

        if !image_id.trim().is_empty() {
            let image_ids = vec![image_id.to_string()];
            let workflow_info = self
                .workflow_repo
                .get_workflow_info_by_image_ids(tenant_id, &image_ids)
                .await
                .map_err(|e| {
                    ExecutionError::DatabaseError(format!(
                        "Failed to look up workflow for original instance image: {}",
                        e
                    ))
                })?;

            if let Some((workflow_id, _, _)) = workflow_info.get(image_id)
                && !workflow_id.trim().is_empty()
            {
                return Ok(workflow_id.clone());
            }
        }

        if let Some(workflow_id) = parsed_workflow_id {
            return Ok(workflow_id);
        }

        Err(ExecutionError::NotFound(
            "Workflow for original instance image not found".to_string(),
        ))
    }

    // =========================================================================
    // Async queuing
    // =========================================================================

    /// Queue a workflow execution through the durable source outbox.
    ///
    /// Validates the workflow exists, validates inputs against the workflow's
    /// input schema (if non-empty), then commits a `TriggerEvent` and its
    /// admission reservation before the relay publishes it for the trigger
    /// worker to pick up.
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

        // 3. Validate canonical inputs and inputs.data against input schema
        let validated_inputs =
            validate_workflow_start_inputs(req.inputs.clone(), &workflow.input_schema)
                .map_err(|e| ExecutionError::ValidationError(e.message))?;

        // 4. Carry the workflow's tracking mode into the queued event. The
        // launch path rereads it from the ready artifact, because a queued
        // request is not itself evidence that a run ever started.
        let track_events = workflow.track_events;

        // 5. Generate instance ID. It is also the durable idempotency identity
        // for retries that originate from this queue request.
        let instance_id = req.instance_id.unwrap_or_else(Uuid::new_v4);

        // 6. Build TriggerEvent appropriate to the source.
        //
        // Sessions, chat, webhooks, and cron-originated requests that go
        // through the engine share the `http_api` factory. Replay keeps its
        // own source metadata so runtime history can distinguish it.
        let source_name = match &req.trigger_source {
            TriggerSource::HttpApi => "http-api",
            TriggerSource::Session => "session",
            TriggerSource::Chat => "chat",
            TriggerSource::Webhook => "webhook",
            TriggerSource::Cron => "cron",
            TriggerSource::Replay { .. } => "replay",
        };
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
                validated_inputs,
                track_events,
                req.correlation_id,
                req.debug,
            ),
            TriggerSource::Replay {
                original_instance_id,
            } => TriggerEvent::replay(
                instance_id.to_string(),
                req.tenant_id.to_string(),
                req.workflow_id.to_string(),
                Some(version),
                validated_inputs,
                track_events,
                original_instance_id,
                req.debug,
            ),
        };

        // 7. Atomically reserve admission + record the source request + write
        // its relay outbox row. A Valkey outage now leaves a durable pending
        // request instead of losing an already-accepted execution.
        let idempotency_key = req
            .idempotency_key
            .unwrap_or_else(|| source_idempotency_key(source_name, &instance_id.to_string()));
        let enqueued = self
            .enqueue_trigger_event(req.tenant_id, event, idempotency_key)
            .await
            .inspect_err(|error| {
                if let ExecutionError::EntitlementDenied(denial) = &error {
                    crate::product_events::emit_quota_exceeded(
                        &self.events,
                        ProductEvent::new(EventType::QuotaExceeded)
                            .no_user_actor("execution_engine", ActorType::System)
                            .resource(req.workflow_id, "workflow")
                            .source(EventSource::Worker),
                        denial,
                    );
                }
            })?;

        // A retry carrying an HTTP idempotency key can find a request whose
        // original instance ID differs from the freshly generated local UUID.
        // Return the durable identity so clients can observe/cancel the
        // execution they actually retried rather than a phantom UUID.
        let instance_id = Uuid::parse_str(&enqueued.instance_id).map_err(|error| {
            ExecutionError::DatabaseError(format!(
                "durable execution request stored an invalid instance ID '{}': {error}",
                enqueued.instance_id
            ))
        })?;

        info!(
            instance_id = %instance_id,
            workflow_id = %req.workflow_id,
            version = version,
            "Queued durable execution request"
        );

        Ok(QueuedExecution {
            instance_id,
            status: "queued".to_string(),
            workflow_id: req.workflow_id.to_string(),
            version,
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

        let validated_inputs =
            validate_workflow_start_inputs(req.inputs.clone(), &workflow.input_schema)
                .map_err(|e| ExecutionError::ValidationError(e.message))?;

        // 3. Block on compilation readiness (delegated to compilation worker).
        // Keep the image ID and tracking mode from this one verified snapshot:
        // another database read could otherwise pair the mode of one artifact
        // with the ID of another while a toggle is recompiling.
        let ReadyLaunch {
            image_id,
            execution_timeout,
            track_events,
        } = self
            .wait_for_compilation_blocking(req.tenant_id, req.workflow_id, version)
            .await?;

        // 5.5. Per-tenant maxConcurrentExecutions gate (SYN-433 Finding 1).
        // Same runtime-count gate as the async path — the running instance
        // this call is about to create counts against the tenant's live
        // Running + Pending total, and the runtime drops it from that total
        // the moment it finishes (which for a sync run is when the
        // `execute_sync` call below returns). No bookkeeping to release.
        self.check_concurrency_gate(req.tenant_id)
            .await
            .map_err(|denial| {
                crate::product_events::emit_quota_exceeded(
                    &self.events,
                    ProductEvent::new(EventType::QuotaExceeded)
                        .no_user_actor("execution_engine", ActorType::System)
                        .resource(req.workflow_id, "workflow")
                        .source(EventSource::Worker),
                    &denial,
                );
                ExecutionError::EntitlementDenied(denial)
            })?;

        // Product analytics: a synchronous execution is starting. Engine-layer — no user
        // context — so it's a no-user, `worker`-source event.
        self.events.emit(
            ProductEvent::new(EventType::ExecutionStarted)
                .no_user_actor("execution_engine", ActorType::System)
                .resource(req.workflow_id, "workflow")
                .source(EventSource::Worker)
                .properties(serde_json::json!({ "version": version, "sync": true })),
        );

        let execution_timeout = runtime_client
            .resolve_execution_timeout(execution_timeout)
            .map_err(|error| ExecutionError::ValidationError(error.to_string()))?;

        // 6. Start via the runtime client, then wait for completion. Do not
        // use RuntimeClient::execute_sync here: its combined return value
        // hides the accepted start, so a run that later times out would never
        // reach the pipeline counters despite having actually launched.
        let execution_result = match runtime_client
            .start_instance(
                &image_id,
                req.tenant_id,
                req.workflow_id,
                None, // auto-generate instance id
                Some(validated_inputs),
                Some(execution_timeout),
                false,
                false,
            )
            .await
        {
            Ok(start) => {
                record_new_runtime_start(&self.gauges, track_events, start.deduplicated);
                runtime_client
                    .wait_for_completion(&start.instance_id, None, Some(execution_timeout))
                    .await
            }
            Err(error) => Err(error),
        };

        let total_duration = total_start.elapsed().as_secs_f64();

        // 7. Metrics + result shaping
        let metrics_service = MetricsService::new(self.workflow_repo.pool().clone());

        match execution_result {
            Ok(result) => {
                let execution_duration_secs = result
                    .duration_ms
                    .map(|ms| ms as f64 / 1000.0)
                    .unwrap_or(0.0);
                let max_memory_mb = result
                    .memory_peak_bytes
                    .map(|bytes| bytes as f64 / (1024.0 * 1024.0));

                let _ = metrics_service
                    .record_execution_completion(
                        req.tenant_id,
                        req.workflow_id,
                        version,
                        result.success,
                        execution_duration_secs,
                        max_memory_mb,
                    )
                    .await;

                // Product analytics: terminal outcome for this sync execution.
                self.events.emit(
                    ProductEvent::new(if result.success {
                        EventType::ExecutionCompleted
                    } else {
                        EventType::ExecutionFailed
                    })
                    .no_user_actor("execution_engine", ActorType::System)
                    .resource(req.workflow_id, "workflow")
                    .source(EventSource::Worker)
                    .properties(serde_json::json!({
                        "version": version,
                        "duration_ms": result.duration_ms,
                        "success": result.success,
                        "error": result.error,
                    })),
                );

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
                        max_memory_mb: max_memory_mb.unwrap_or(0.0),
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

                // Product analytics: the sync run failed to execute.
                self.events.emit(
                    ProductEvent::new(EventType::ExecutionFailed)
                        .no_user_actor("execution_engine", ActorType::System)
                        .resource(req.workflow_id, "workflow")
                        .source(EventSource::Worker)
                        .properties(serde_json::json!({
                            "version": version,
                            "error": error_message,
                            "duration_ms": (total_duration * 1000.0) as u64,
                        })),
                );

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
    pub async fn execute_detached(
        &self,
        event: &TriggerEvent,
    ) -> Result<DetachedExecution, ExecutionError> {
        self.execute_detached_with_scope(event, false).await
    }

    /// Atomically enforce `single_instance` through Environment's durable
    /// workflow-scoped launch lease.
    ///
    /// The scope is persisted with the launch row, and Environment serializes
    /// admission in PostgreSQL. It therefore survives worker/server restarts
    /// and is released by the same durable transition that parks or
    /// terminalizes its generation. A suspended approval intentionally has no
    /// active lease.
    ///
    /// `None` means another active or in-flight instance already owns the
    /// workflow-wide single-instance slot. The caller should ACK the trigger
    /// as a deliberate skip.
    pub(crate) async fn execute_single_instance_detached(
        &self,
        event: &TriggerEvent,
    ) -> Result<Option<DetachedExecution>, ExecutionError> {
        match self.execute_detached_with_scope(event, true).await {
            Ok(launch) => Ok(Some(launch)),
            Err(ExecutionError::SingleInstanceActive) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Run one durable Environment start and emit the detached execution
    /// analytics only after the start is accepted.
    async fn execute_detached_with_scope(
        &self,
        event: &TriggerEvent,
        single_instance: bool,
    ) -> Result<DetachedExecution, ExecutionError> {
        let result = self.execute_detached_inner(event, single_instance).await;

        // Product analytics: an async execution started. No user context survives into the
        // worker, so attribute to the firing trigger when present, else the system. `source`
        // is `worker` (engine-emitted).
        if matches!(result, Ok(DetachedExecution::Started(_))) {
            let actor = match event.trigger_id() {
                Some(trigger_id) => ProductEvent::new(EventType::ExecutionStarted)
                    .no_user_actor(trigger_id, ActorType::Trigger),
                None => ProductEvent::new(EventType::ExecutionStarted)
                    .no_user_actor("execution_engine", ActorType::System),
            };
            self.events.emit(
                actor
                    .resource(&event.workflow_id, "workflow")
                    .source(EventSource::Worker)
                    .properties(serde_json::json!({
                        "instance_id": event.instance_id,
                        "trigger_type": event.trigger_type(),
                    })),
            );
        }

        // Product analytics: unlike the sync path, this call returns before the instance
        // finishes, so the terminal event has to be observed in the background. Spawn a
        // best-effort watcher — never blocks/fails `execute_detached` itself, and never
        // cancels the instance (see `RuntimeClient::poll_until_terminal` doc comment).
        if let (Ok(DetachedExecution::Started(instance_id)), Some(runtime_client)) =
            (&result, self.runtime_client.clone())
        {
            let events = self.events.clone();
            let workflow_id = event.workflow_id.clone();
            let version = event.version;
            let instance_id = instance_id.clone();
            let trigger_id = event.trigger_id().map(|s| s.to_string());
            const ANALYTICS_POLL_INTERVAL: Duration = Duration::from_secs(2);
            tokio::spawn(async move {
                // Wait one interval before the first look. The run was started
                // microseconds ago, so an immediate poll almost never sees a
                // terminal state, and each poll is the two-join instance read.
                // Skipping it takes a workflow that starts and parks from two
                // of those reads down to one; anything that really did finish
                // in under an interval is simply observed an interval later,
                // which for an analytics event is not a meaningful delay.
                tokio::time::sleep(ANALYTICS_POLL_INTERVAL).await;
                let outcome = runtime_client
                    .poll_until_terminal(
                        &instance_id,
                        ANALYTICS_POLL_INTERVAL,
                        Duration::from_secs(24 * 3600),
                    )
                    .await;
                let (event_type, output) = match outcome {
                    Ok(crate::runtime_client::TerminalOutcome::Completed(o)) => {
                        (EventType::ExecutionCompleted, o)
                    }
                    Ok(crate::runtime_client::TerminalOutcome::Failed(o)) => {
                        (EventType::ExecutionFailed, o)
                    }
                    Ok(crate::runtime_client::TerminalOutcome::Cancelled(o)) => {
                        (EventType::ExecutionCancelled, o)
                    }
                    Ok(crate::runtime_client::TerminalOutcome::TimedOut(o)) => {
                        (EventType::ExecutionTimeout, o)
                    }
                    Ok(crate::runtime_client::TerminalOutcome::GaveUp)
                    | Ok(crate::runtime_client::TerminalOutcome::Suspended) => return,
                    Err(e) => {
                        warn!(
                            instance_id = %instance_id,
                            error = %e,
                            "product events: failed to observe async execution outcome"
                        );
                        return;
                    }
                };
                let actor = match trigger_id {
                    Some(trigger_id) => {
                        ProductEvent::new(event_type).no_user_actor(trigger_id, ActorType::Trigger)
                    }
                    None => ProductEvent::new(event_type)
                        .no_user_actor("execution_engine", ActorType::System),
                };
                events.emit(
                    actor
                        .resource(&workflow_id, "workflow")
                        .source(EventSource::Worker)
                        .properties(serde_json::json!({
                            "version": version,
                            "duration_ms": output.duration_ms,
                            "success": output.success,
                            "error": output.error,
                        })),
                );
            });
        }

        result
    }

    /// Inner implementation of execute_detached
    async fn execute_detached_inner(
        &self,
        event: &TriggerEvent,
        single_instance: bool,
    ) -> Result<DetachedExecution, ExecutionError> {
        let runtime_client = self.runtime_client.as_ref().ok_or_else(|| {
            ExecutionError::NotConnected("Runtime client not configured".to_string())
        })?;

        // Resolve the version and check compilation together. Both live in the
        // server database and hang off the same workflow, so an unversioned
        // event costs one statement rather than a read of `workflows` followed
        // by a read of its definition. The readiness check reads that
        // definition anyway, so the image, timeout, and tracking mode ride
        // along as one verified artifact snapshot.
        let (version, ready) = self
            .ensure_compiled(&event.tenant_id, &event.workflow_id, event.version)
            .await?;
        let execution_timeout = runtime_client
            .resolve_execution_timeout(ready.execution_timeout)
            .map_err(|error| ExecutionError::ValidationError(error.to_string()))?;
        let track_events = ready.track_events;
        let image_id = ready.image_id;

        // Inputs are already in canonical format {"data": {...}, "variables": {...}}
        // from the API layer - inject _workflow_id for cache key isolation
        let workflow_input = inject_workflow_id(event.inputs.clone(), &event.workflow_id);

        // Start instance (non-blocking)
        let start = match runtime_client
            .start_instance(
                &image_id,
                &event.tenant_id,
                &event.workflow_id,
                Some(event.instance_id.clone()),
                Some(workflow_input),
                Some(execution_timeout),
                event.debug,
                single_instance,
            )
            .await
        {
            Ok(start) => {
                record_new_runtime_start(&self.gauges, track_events, start.deduplicated);
                start
            }
            Err(RuntimeError::ImageNotFound(error)) => {
                tracing::warn!(
                    tenant_id = %event.tenant_id,
                    workflow_id = %event.workflow_id,
                    version = version,
                    image_id = %image_id,
                    error,
                    "Image or image artifact missing in Environment; forcing recompilation"
                );

                // The local compilation record and Environment image metadata
                // can both outlive the on-disk artifact. Invalidate only the
                // exact image this launch selected: another compile may have
                // attached a newer immutable image for the same version while
                // this old launch was in flight.
                let _ = sqlx::query(
                    "DELETE FROM workflow_compilations \
                     WHERE tenant_id = $1 AND workflow_id = $2 AND version = $3 \
                       AND registered_image_id = $4",
                )
                .bind(&event.tenant_id)
                .bind(&event.workflow_id)
                .bind(version)
                .bind(&image_id)
                .execute(&self.pool)
                .await;

                let compilation_queued =
                    if let Some(valkey_config) = crate::valkey::ValkeyConfig::from_env() {
                        crate::workers::compilation_worker::enqueue_compilation(
                            &valkey_config.connection_url(),
                            &event.tenant_id,
                            &event.workflow_id,
                            version,
                            true,
                        )
                        .await
                        .unwrap_or(false)
                    } else {
                        false
                    };

                return Err(ExecutionError::NotCompiled {
                    workflow_id: event.workflow_id.clone(),
                    version,
                    compilation_queued,
                });
            }
            Err(RuntimeError::SingleInstanceActive) => {
                return Err(ExecutionError::SingleInstanceActive);
            }
            Err(e) => return Err(ExecutionError::RuntimeError(e.to_string())),
        };

        let outcome = if start.deduplicated {
            info!(
                instance_id = %start.instance_id,
                workflow_id = %event.workflow_id,
                version,
                "Detached instance start was deduplicated"
            );
            DetachedExecution::Deduplicated(start.instance_id)
        } else {
            info!(
                instance_id = %start.instance_id,
                workflow_id = %event.workflow_id,
                version,
                "Started instance in detached mode"
            );
            DetachedExecution::Started(start.instance_id)
        };

        Ok(outcome)
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
        // Stamping `last_run` here rather than at webhook intake keeps it one
        // statement instead of two, and makes it mean what it says: the row is
        // touched when the run is actually being started, not when the event
        // was queued.
        let result = sqlx::query!(
            r#"
            UPDATE invocation_trigger
            SET last_run = NOW()
            WHERE id = $1
            RETURNING single_instance
            "#,
            trigger_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ExecutionError::DatabaseError(format!("Failed to get trigger: {}", e)))?;

        Ok(result.map(|r| r.single_instance))
    }

    // =========================================================================
    // Execution read API (proxy to Runtara)
    // =========================================================================

    /// Get execution results by instance ID (proxies to runtara-environment).
    pub async fn get_execution(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<WorkflowInstanceDto, ExecutionError> {
        let client = self.require_runtime_client()?;

        let info = match client.get_instance_info(instance_id).await {
            Ok(info) => info,
            Err(e) if is_runtime_instance_not_found(&e) => {
                if let Some(instance) = self
                    .find_recent_execution_summary(tenant_id, instance_id, None)
                    .await?
                {
                    warn!(
                        tenant_id = %tenant_id,
                        instance_id = %instance_id,
                        "Runtime detail lookup returned not found; returning execution list summary"
                    );
                    return Ok(instance);
                }
                return Err(ExecutionError::NotFound(format!(
                    "Instance '{}' not found",
                    instance_id
                )));
            }
            Err(e) => {
                return Err(ExecutionError::DatabaseError(format!(
                    "Failed to get instance from Runtara: {}",
                    e
                )));
            }
        };

        if info.tenant_id != tenant_id {
            warn!(
                instance_id = %instance_id,
                requested_tenant_id = %tenant_id,
                instance_tenant_id = %info.tenant_id,
                "Execution lookup denied because instance tenant does not match request tenant"
            );
            return Err(ExecutionError::NotFound(format!(
                "Instance '{}' not found",
                instance_id
            )));
        }

        Ok(runtara_info_to_dto(info))
    }

    async fn find_recent_execution_summary(
        &self,
        tenant_id: &str,
        instance_id: &str,
        workflow_id: Option<&str>,
    ) -> Result<Option<WorkflowInstanceDto>, ExecutionError> {
        const PAGE_SIZE: u32 = 100;
        const MAX_PAGES: u32 = 10;

        let client = self.require_runtime_client()?;

        for page in 0..MAX_PAGES {
            let mut options = ListInstancesOptions::new()
                .with_tenant_id(tenant_id)
                .with_limit(PAGE_SIZE)
                .with_offset(page * PAGE_SIZE)
                .with_order_by(ListInstancesOrder::CreatedAtDesc);

            if let Some(workflow_id) = workflow_id {
                options = options.with_image_name_prefix(format!("{}:", workflow_id));
            }

            let result = client
                .list_instances_with_options(options)
                .await
                .map_err(|e| {
                    ExecutionError::DatabaseError(format!(
                        "Failed to query Runtara for execution fallback: {}",
                        e
                    ))
                })?;

            let total_count = result.total_count;
            let page_len = result.instances.len() as u32;

            if let Some(instance) = result
                .instances
                .into_iter()
                .find(|instance| instance.instance_id == instance_id)
            {
                let image_ids = vec![instance.image_id.clone()];
                let workflow_info = self
                    .workflow_repo
                    .get_workflow_info_by_image_ids(tenant_id, &image_ids)
                    .await
                    .map_err(|e| {
                        ExecutionError::DatabaseError(format!(
                            "Failed to fetch workflow info for execution fallback: {}",
                            e
                        ))
                    })?;

                let (resolved_workflow_id, version, workflow_name) =
                    match workflow_info.get(&instance.image_id) {
                        Some((sid, version, name)) => {
                            let name = (!name.is_empty()).then_some(name.clone());
                            (sid.clone(), *version, name)
                        }
                        None => {
                            let (workflow_id, version) = workflow_info_from_image_name(
                                &instance.image_name,
                            )
                            .unwrap_or_else(|| (workflow_id.unwrap_or_default().to_string(), 0));
                            (workflow_id, version, None)
                        }
                    };

                return Ok(Some(runtara_instance_to_dto_with_info(
                    instance,
                    resolved_workflow_id,
                    version,
                    workflow_name,
                )));
            }

            if page_len == 0 || (page + 1) * PAGE_SIZE >= total_count {
                break;
            }
        }

        Ok(None)
    }

    /// Replay a previous instance by queuing the latest version of the same
    /// workflow with the original instance inputs.
    pub async fn replay(
        &self,
        tenant_id: &str,
        original_instance_id: &str,
    ) -> Result<QueuedExecution, ExecutionError> {
        let _ = Uuid::parse_str(original_instance_id).map_err(|_| {
            ExecutionError::ValidationError(
                "Invalid instance ID format. Instance ID must be a valid UUID".to_string(),
            )
        })?;

        let client = self.require_runtime_client()?;
        let info = client
            .get_instance_info(original_instance_id)
            .await
            .map_err(|e| {
                let error_str = e.to_string();
                warn!(
                    instance_id = %original_instance_id,
                    tenant_id = %tenant_id,
                    error = %error_str,
                    "Failed to get original instance info for replay"
                );
                if error_str.contains("not found") || error_str.contains("InstanceNotFound") {
                    ExecutionError::NotFound(format!(
                        "Instance '{}' not found",
                        original_instance_id
                    ))
                } else {
                    ExecutionError::DatabaseError(format!(
                        "Failed to get instance from Runtara: {}",
                        e
                    ))
                }
            })?;

        if info.tenant_id != tenant_id {
            warn!(
                instance_id = %original_instance_id,
                requested_tenant_id = %tenant_id,
                instance_tenant_id = %info.tenant_id,
                "Replay denied because instance tenant does not match request tenant"
            );
            return Err(ExecutionError::NotFound(format!(
                "Instance '{}' not found",
                original_instance_id
            )));
        }

        let workflow_id = self
            .workflow_id_for_instance_image(tenant_id, &info.image_name, &info.image_id)
            .await?;
        let latest_version = self
            .workflow_repo
            .get_latest_version(tenant_id, &workflow_id)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!(
                    "Failed to get latest workflow version for replay: {}",
                    e
                ))
            })?
            .ok_or_else(|| {
                ExecutionError::NotFound(format!("Workflow '{}' not found", workflow_id))
            })?;

        if latest_version <= 0 {
            return Err(ExecutionError::NotFound(format!(
                "Workflow '{}' has no versions",
                workflow_id
            )));
        }

        let original_inputs = info.input.ok_or_else(|| {
            ExecutionError::ValidationError(format!(
                "Original instance '{}' does not have stored inputs and cannot be replayed",
                original_instance_id
            ))
        })?;
        let validated_inputs = validate_workflow_inputs(original_inputs)
            .map_err(|e| ExecutionError::ValidationError(e.message))?;

        self.queue(QueueRequest {
            tenant_id,
            workflow_id: &workflow_id,
            version: Some(latest_version),
            inputs: validated_inputs,
            debug: false,
            correlation_id: None,
            idempotency_key: None,
            trigger_source: TriggerSource::Replay {
                original_instance_id: original_instance_id.to_string(),
            },
            instance_id: None,
        })
        .await
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
        let page = crate::api::utils::pagination::normalize_page(page);
        let size = size.unwrap_or(10).clamp(1, 100);

        let client = self.require_runtime_client()?;

        // Image names retain the `{workflow_id}:` prefix, with an optional
        // artifact fingerprint after the version.
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
                let version = version_info
                    .get(&inst.image_id)
                    .copied()
                    .or_else(|| {
                        workflow_info_from_image_name(&inst.image_name)
                            .filter(|(parsed_workflow_id, _)| parsed_workflow_id == workflow_id)
                            .map(|(_, version)| version)
                    })
                    .unwrap_or(0);
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
        let page = crate::api::utils::pagination::normalize_page(page);
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

        if let Some(ref statuses) = filters.statuses {
            let runtara_statuses = execution_statuses_to_runtara(statuses);
            if !runtara_statuses.is_empty() {
                options = options.with_statuses(runtara_statuses);
            }
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
            .chain(
                result
                    .instances
                    .iter()
                    .filter(|instance| !workflow_info.contains_key(&instance.image_id))
                    .filter_map(|instance| {
                        workflow_info_from_image_name(&instance.image_name)
                            .map(|(workflow_id, _)| workflow_id)
                    }),
            )
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
                    .unwrap_or_else(|| {
                        let (workflow_id, version) =
                            workflow_info_from_image_name(&inst.image_name)
                                .unwrap_or_else(|| (String::new(), 0));
                        let workflow_name = workflow_names.get(&workflow_id).cloned();
                        (workflow_id, version, workflow_name)
                    });

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

    /// Ensure the workflow is compiled (non-blocking: queues compilation if
    /// needed and returns `NotCompiled` for the caller to retry).
    ///
    /// Returns the ready artifact's ID, `executionTimeoutSeconds`, and tracking
    /// mode together so the launch cannot mix fields from different reads.
    async fn ensure_compiled(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: Option<i32>,
    ) -> Result<(i32, ReadyLaunch), ExecutionError> {
        let (resolved, status) = self
            .workflow_repo
            .ensure_compilation_ready(tenant_id, workflow_id, version)
            .await
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to check compilation: {}", e))
            })?;
        let version = resolved.or(version).ok_or_else(|| {
            ExecutionError::WorkflowNotFound(format!(
                "No runnable version for workflow '{workflow_id}'"
            ))
        })?;

        if let CompilationStatus::Ready {
            registered_image_id,
            execution_timeout,
            track_events,
            ..
        } = status
        {
            return Ok((
                version,
                ReadyLaunch {
                    image_id: registered_image_id,
                    execution_timeout,
                    track_events,
                },
            ));
        }

        // A terminal failure will repeat for as long as the definition is
        // unchanged, so surface it as a permanent error instead of queueing a
        // compilation the caller would retry until it exhausted its budget.
        if let CompilationStatus::Failed {
            error,
            terminal: true,
            authoring,
        } = &status
        {
            return Err(if *authoring {
                ExecutionError::WorkflowNotRunnable {
                    workflow_id: workflow_id.to_string(),
                    version,
                    error: error.clone(),
                }
            } else {
                ExecutionError::CompilationFailed(format!(
                    "Workflow '{}' version {} failed to compile: {}",
                    workflow_id, version, error
                ))
            });
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
    /// path), returning one ready-artifact launch snapshot.
    /// Delegates the actual wait to `compilation_worker::wait_for_compilation`.
    async fn wait_for_compilation_blocking(
        &self,
        tenant_id: &str,
        workflow_id: &str,
        version: i32,
    ) -> Result<ReadyLaunch, ExecutionError> {
        let status = self
            .workflow_repo
            .ensure_compilation_ready(tenant_id, workflow_id, Some(version))
            .await
            .map(|(_, status)| status)
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to check compilation: {}", e))
            })?;
        if let CompilationStatus::Ready {
            registered_image_id,
            execution_timeout,
            track_events,
            ..
        } = status
        {
            return Ok(ReadyLaunch {
                image_id: registered_image_id,
                execution_timeout,
                track_events,
            });
        }

        // Waiting cannot help a failure that will reproduce on the unchanged
        // definition - report it now rather than after the compile timeout.
        if let CompilationStatus::Failed {
            error,
            terminal: true,
            authoring,
        } = &status
        {
            return Err(if *authoring {
                ExecutionError::WorkflowNotRunnable {
                    workflow_id: workflow_id.to_string(),
                    version,
                    error: error.clone(),
                }
            } else {
                ExecutionError::CompilationFailed(format!(
                    "Workflow '{}' version {} failed to compile: {}",
                    workflow_id, version, error
                ))
            });
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
            .ensure_compilation_ready(tenant_id, workflow_id, Some(version))
            .await
            .map(|(_, status)| status)
            .map_err(|e| {
                ExecutionError::DatabaseError(format!("Failed to check compilation: {}", e))
            })?;
        match status_after {
            CompilationStatus::Ready {
                registered_image_id,
                execution_timeout,
                track_events,
                ..
            } => Ok(ReadyLaunch {
                image_id: registered_image_id,
                execution_timeout,
                track_events,
            }),
            // The compilation we waited on recorded why it failed; report that
            // rather than the generic missing-binary message.
            CompilationStatus::Failed { error, .. } => {
                Err(ExecutionError::CompilationFailed(format!(
                    "Workflow '{}' version {} failed to compile: {}",
                    workflow_id, version, error
                )))
            }
            _ => Err(ExecutionError::CompilationFailed(format!(
                "Compilation for workflow '{}' version {} completed but binary not found.",
                workflow_id, version
            ))),
        }
    }
}

fn map_outbox_error(error: ExecutionOutboxError) -> ExecutionError {
    match error {
        ExecutionOutboxError::AdmissionFull { limit } => ExecutionError::EntitlementDenied(
            crate::entitlement_error::EntitlementDenial::LimitExceeded {
                limit: "maxConcurrentExecutions",
                maximum: limit,
            },
        ),
        ExecutionOutboxError::InvalidIdempotencyKey | ExecutionOutboxError::TenantMismatch => {
            ExecutionError::ValidationError(error.to_string())
        }
        ExecutionOutboxError::Serialization(_) | ExecutionOutboxError::Database(_) => {
            ExecutionError::DatabaseError(error.to_string())
        }
    }
}

/// Record a runtime start only after Environment confirms it launched a new
/// instance. A deduplicated request reuses an already-accepted launch, so it
/// must not make a second run or step-capability claim visible in the pipeline.
fn record_new_runtime_start(
    gauges: &crate::workers::pipeline_gauges::PipelineGauges,
    track_events: bool,
    deduplicated: bool,
) {
    if deduplicated {
        return;
    }

    gauges.record_started();
    if track_events {
        gauges.record_tracked_start();
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn qualified_image_name_preserves_historical_workflow_identity() {
        assert_eq!(
            workflow_info_from_image_name("workflow-a:7@artifact-fingerprint"),
            Some(("workflow-a".to_string(), 7))
        );
        assert_eq!(workflow_info_from_image_name("not-a-workflow"), None);
    }

    #[test]
    fn only_a_new_tracked_runtime_start_makes_steps_measurable() {
        let gauges = crate::workers::pipeline_gauges::PipelineGauges::new();

        // An untracked run is still a real start, but does not make a zero
        // steps reading meaningful.
        record_new_runtime_start(&gauges, false, false);
        assert_eq!(gauges.totals().started, 1);
        assert_eq!(gauges.totals().tracked_starts, 0);

        // A retry that Environment deduplicates has not launched another run.
        record_new_runtime_start(&gauges, true, true);
        assert_eq!(gauges.totals().started, 1);
        assert_eq!(gauges.totals().tracked_starts, 0);

        record_new_runtime_start(&gauges, true, false);
        assert_eq!(gauges.totals().started, 2);
        assert_eq!(gauges.totals().tracked_starts, 1);
    }

    #[test]
    fn every_runtime_launch_path_records_its_confirmed_start() {
        // A unit test of `record_new_runtime_start` alone cannot catch a
        // perfectly good recorder with no production caller. Keep this guard
        // outside the production source slice so its own assertion cannot
        // satisfy it.
        // `include_str!` resolves a relative literal from this module's
        // directory; unlike `file!()`, it does not prefix that directory a
        // second time when Cargo invokes the test from another working dir.
        let source = include_str!("execution_engine.rs");
        let production = source
            .split("\n#[cfg(test)]")
            .next()
            .expect("source has a production section");
        let call = "record_new_runtime_start(&self.gauges, track_events, start.deduplicated)";

        assert_eq!(
            production.matches(call).count(),
            2,
            "sync and detached launch paths must each record their confirmed start"
        );

        let sync_start = production
            .find("pub async fn run_sync")
            .expect("sync launch path exists");
        let detached_start = production
            .find("async fn execute_detached_inner")
            .expect("detached launch path exists");
        let sync = &production[sync_start..detached_start];
        let recorded = sync.find(call).expect("sync path records the start");
        let waited = sync
            .find(".wait_for_completion(")
            .expect("sync path waits for completion");
        assert!(
            recorded < waited,
            "sync path must record a successful launch before its completion wait"
        );
    }

    /// The gate must answer from memory, never from a query.
    ///
    /// The count reads a table the same workload is churning, so its cost
    /// tracks dead tuples rather than the size of the answer: measured at 15ms
    /// standalone, it reached 11s per call under load and took 94% of all
    /// database time. On the request path that makes admission slowest exactly
    /// when admissions are most frequent, so the caller takes the last count
    /// and a refresh happens behind it.
    #[tokio::test]
    async fn the_gate_answers_from_memory_without_querying() {
        // No runtime client, so any attempt to refresh in-band would have to
        // fail or hang rather than silently succeed.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused/unused")
            .expect("lazy pool");
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let engine = ExecutionEngine::new(
            pool.clone(),
            Arc::new(WorkflowRepository::new(pool)),
            None,
            None,
            None,
            ProductEventSink::new(tx),
            crate::workers::pipeline_gauges::PipelineGauges::new(),
        );
        let tenant = format!("tenant-{}", uuid::Uuid::new_v4());

        // Deliberately stale, so the TTL cannot be what serves this.
        engine.concurrency_counts.insert(
            tenant.clone(),
            (
                Instant::now() - CONCURRENCY_COUNT_TTL - Duration::from_secs(1),
                7,
            ),
        );
        engine.reservations_for(&tenant).store(2, Ordering::SeqCst);

        assert_eq!(
            engine.active_execution_count(&tenant, 100),
            9,
            "expected the last count (7) plus the admissions since (2); a stale \
             entry must be answered from, not waited on"
        );

        // And with nothing cached at all it still answers rather than querying,
        // counting only what this process has admitted.
        let fresh = format!("tenant-{}", uuid::Uuid::new_v4());
        engine.reservations_for(&fresh).store(3, Ordering::SeqCst);
        assert_eq!(engine.active_execution_count(&fresh, 100), 3);
    }

    /// Admissions the gate has made but the cached count cannot see yet must
    /// keep counting, or a burst inside one TTL is admitted against a figure
    /// that predates all of it — with a cap of one and a cached zero, every
    /// request arriving in that window gets in.
    #[test]
    fn a_refresh_only_forgives_the_admissions_it_could_see() {
        use super::retained_reservations;

        // Nothing admitted during the query: the fresh count covers them all.
        assert_eq!(retained_reservations(3, 3), 0);
        assert_eq!(retained_reservations(0, 0), 0);

        // Two more arrived while the query was in flight. They are not in the
        // result, so they must survive it — forgiving them would let the same
        // burst be admitted twice, once per count.
        assert_eq!(retained_reservations(3, 5), 2);
        assert_eq!(retained_reservations(0, 4), 4);

        // A concurrent refresh may already have cleared the counter; never
        // underflow back into a huge number.
        assert_eq!(retained_reservations(5, 2), 0);
        assert_eq!(retained_reservations(u64::MAX, 1), 0);
    }

    /// The concurrency-count TTL must collapse a burst without letting the
    /// admission gate reason from an old runtime count for an age.
    #[test]
    fn concurrency_count_cache_has_a_sane_window() {
        use super::CONCURRENCY_COUNT_TTL;
        use std::time::Duration;

        // Long enough to collapse a burst.
        assert!(CONCURRENCY_COUNT_TTL >= Duration::from_millis(100));

        // Short enough to stay honest. The concurrency count is a backstop
        // against runaway intake, not a quota, but it still has to track
        // reality.
        assert!(CONCURRENCY_COUNT_TTL <= Duration::from_secs(2));
    }
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
    fn test_execution_error_single_instance_active_is_a_conflict() {
        let error = ExecutionError::SingleInstanceActive;
        assert_eq!(
            format!("{error}"),
            "single-instance workflow already has active work"
        );
        assert_eq!(error.http_status(), StatusCode::CONFLICT);
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

    #[test]
    fn detached_execution_preserves_started_and_deduplicated_instance_ids() {
        let started = DetachedExecution::Started("started-id".into());
        let deduplicated = DetachedExecution::Deduplicated("deduplicated-id".into());

        assert_eq!(started.instance_id(), "started-id");
        assert_eq!(deduplicated.instance_id(), "deduplicated-id");
        assert_ne!(started, deduplicated);
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
