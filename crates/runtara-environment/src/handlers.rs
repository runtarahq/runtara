// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Environment protocol handlers.
//!
//! Handles requests from Management SDK and proxies to Core when needed.

use chrono::{DateTime, Utc};
use runtara_core::domain::InstanceStatus as CoreInstanceStatus;

use serde_json::Value;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{debug, error, info, instrument, warn};

use runtara_core::persistence::{CompleteInstanceParams, PairedRecordStatus, Persistence};

use crate::container_registry::ContainerRegistry;
use crate::db;
use crate::error::Result;
use crate::execution_timeout::ExecutionTimeoutPolicy;
use crate::image_registry::{ImageBuilder, ImageRegistry, require_current_workflow_entrypoint};
use crate::instance_repository::InstanceRepository;
use crate::launch_dispatcher::{DEFAULT_LAUNCH_QUEUE_TIMEOUT, LaunchLifecycleObservers};
use crate::launch_queue::{
    CancelOutcome, EnqueueOutcome, EnqueueRequest, InitialLaunchOutcome, InitialLaunchRequest,
    LaunchKind, LaunchRepository, LaunchState, SINGLE_INSTANCE_ACTIVE, SINGLE_INSTANCE_LAUNCH_ENV,
};
use crate::runner::{Runner, RunnerHandle, StartGate, StartGateOutcome};

/// Shared drain state for the environment runtime.
///
/// When `is_draining()` returns true:
/// - `spawn_container_monitor` exits its poll loop early.
/// - The crash branch of `spawn_container_monitor` writes `status=suspended +
///   termination_reason="shutdown_requested"` instead of `failed + crashed`,
///   because an in-flight instance dying during drain is a graceful outcome.
/// - `HeartbeatMonitor` pauses scanning so it doesn't mark an in-progress
///   instance as failed while we're waiting for it to checkpoint.
#[derive(Debug, Default, Clone)]
pub struct DrainController {
    inner: Arc<DrainInner>,
}

#[derive(Debug, Default)]
struct DrainInner {
    flag: AtomicBool,
}

impl DrainController {
    /// Create a new drain controller in the not-draining state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark draining as active. Idempotent.
    pub fn set(&self) {
        self.inner.flag.store(true, Ordering::SeqCst);
    }

    /// Returns `true` if drain has been requested.
    pub fn is_draining(&self) -> bool {
        self.inner.flag.load(Ordering::SeqCst)
    }
}

/// Convert a path to absolute if it's relative.
///
/// This is critical for paths stored in DB (like `binary_path`) - they must be
/// absolute so the runner can find them regardless of the current working
/// directory at launch time.
fn ensure_absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    }
}

/// Keep the last `max` characters of `s`, prefixing an ellipsis when truncated.
/// Used to bound a crash reason folded into the instance error; the host writes
/// the failure reason at the tail of stderr, so the tail is the relevant part.
/// Char-based (not byte-based) so it never splits a UTF-8 boundary.
fn tail_chars(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let mut out = String::from("…");
    out.extend(&chars[chars.len() - max..]);
    out
}

/// Shared state for environment handlers.
///
/// Contains database connection, runner, and configuration shared across all handlers.
pub struct EnvironmentHandlerState {
    /// PostgreSQL connection pool (for Environment-specific tables: images, containers, etc.).
    pub pool: PgPool,
    /// Core persistence layer (for instance lifecycle, checkpoints, signals).
    ///
    /// Everything core reasons about — status, termination, sleep — changes
    /// through here and nowhere else. Two deliberate exceptions write the
    /// `instances` row directly, both for columns core never reads: the
    /// forensic ones a run leaves behind and the crash-loop counters, which
    /// [`crate::instance_repository::InstanceRepository`] owns; and the
    /// pending-instance insert inside
    /// [`crate::launch_queue::LaunchRepository::claim_initial`], which shares a
    /// transaction with the launch row on purpose.
    pub persistence: Arc<dyn Persistence>,
    /// When the server started (for uptime calculation).
    pub start_time: std::time::Instant,
    /// Server version string.
    pub version: String,
    /// Runner for launching instances.
    pub runner: Arc<dyn Runner>,
    /// Data directory for images and instance I/O.
    pub data_dir: PathBuf,
    /// Request timeout for database operations.
    pub request_timeout: Duration,
    /// Bounded active-execution timeout policy shared with the server.
    pub execution_timeout_policy: ExecutionTimeoutPolicy,
    /// Drain signal observed by container monitors and workers.
    pub drain: DrainController,
    /// Wakes the durable dispatcher after a source commits a launch row.
    pub launch_notifier: Arc<tokio::sync::Notify>,
    /// Optional server-side admission-release hook installed after startup.
    pub lifecycle_observers: LaunchLifecycleObservers,
}

/// Default request timeout for database operations (30 seconds).
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A monitor that already knows guest execution never started must not become
/// another unbounded waiter when PostgreSQL is unhealthy. The durable marked
/// generation remains recoverable by the expiry scan after this local budget.
const START_GATE_MONITOR_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

impl EnvironmentHandlerState {
    /// Create a new environment handler state.
    ///
    /// # Arguments
    ///
    /// * `pool` - PostgreSQL pool for Environment-specific queries (reads with JOINs)
    /// * `persistence` - Core persistence layer for all instance write operations
    /// * `runner` - Container runner for launching instances
    /// * `data_dir` - Data directory for images and instance I/O
    pub fn new(
        pool: PgPool,
        persistence: Arc<dyn Persistence>,
        runner: Arc<dyn Runner>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            pool,
            persistence,
            start_time: std::time::Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            runner,
            data_dir: ensure_absolute_path(data_dir),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            execution_timeout_policy: ExecutionTimeoutPolicy::default(),
            drain: DrainController::new(),
            launch_notifier: Arc::new(tokio::sync::Notify::new()),
            lifecycle_observers: LaunchLifecycleObservers::default(),
        }
    }

    /// Set the request timeout for database operations.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set the bounded active-execution timeout policy.
    pub fn with_execution_timeout_policy(mut self, policy: ExecutionTimeoutPolicy) -> Self {
        self.execution_timeout_policy = policy;
        self
    }

    /// Attach an externally-managed drain controller.
    pub fn with_drain(mut self, drain: DrainController) -> Self {
        self.drain = drain;
        self
    }

    /// Attach a shared dispatcher wake notification and lifecycle observer
    /// holder created by the embedding runtime.
    pub fn with_launch_control(
        mut self,
        launch_notifier: Arc<tokio::sync::Notify>,
        lifecycle_observers: LaunchLifecycleObservers,
    ) -> Self {
        self.launch_notifier = launch_notifier;
        self.lifecycle_observers = lifecycle_observers;
        self
    }

    /// Get the server uptime in milliseconds.
    pub fn uptime_ms(&self) -> i64 {
        self.start_time.elapsed().as_millis() as i64
    }
}

// ============================================================================
// Start Instance
// ============================================================================

/// Request to start a new instance.
pub struct StartInstanceRequest {
    /// Image ID to create instance from.
    pub image_id: String,
    /// Tenant ID for multi-tenancy isolation.
    pub tenant_id: String,
    /// Optional instance ID (generated if not provided).
    pub instance_id: Option<String>,
    /// Optional input data for the instance.
    pub input: Option<serde_json::Value>,
    /// Optional execution timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Custom environment variables (override system vars).
    pub env: std::collections::HashMap<String, String>,
}

/// Response from starting an instance.
pub struct StartInstanceResponse {
    /// Instance ID (assigned or generated). Empty on a rejection.
    pub instance_id: String,
    /// Whether an earlier request had already reserved this exact instance.
    /// A deduplicated response never launches another process.
    pub deduplicated: bool,
    /// Why the start was refused, or `None` when it was accepted.
    pub rejection: Option<StartRejection>,
}

impl StartInstanceResponse {
    /// The start was accepted and a launch generation exists.
    pub fn accepted(instance_id: impl Into<String>, deduplicated: bool) -> Self {
        Self {
            instance_id: instance_id.into(),
            deduplicated,
            rejection: None,
        }
    }

    /// The start was refused. No instance id is reported: the caller asked for
    /// work that will not happen, and returning an id invites treating the
    /// refusal as a launch.
    pub fn rejected(rejection: StartRejection) -> Self {
        Self {
            instance_id: String::new(),
            deduplicated: false,
            rejection: Some(rejection),
        }
    }

    /// Whether a launch was accepted.
    pub fn is_accepted(&self) -> bool {
        self.rejection.is_none()
    }
}

/// Why Environment refused to start an instance.
///
/// These used to be a `success: bool` beside a free-text `error`, which left
/// the caller recovering the category by searching the message for "not found".
/// Two unrelated refusals then collapsed into one: a genuinely missing image,
/// and a database failure whose text happened to contain the phrase. The
/// server reacts to a missing image by deleting the workflow's compilation
/// record and forcing a rebuild, so a transient database error was able to
/// discard a perfectly good compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartRejection {
    /// The request itself was malformed; retrying it unchanged cannot work.
    InvalidRequest(String),

    /// No image with this id is visible to this tenant.
    ///
    /// Deliberately covers "no such image" and "the image belongs to another
    /// tenant" alike. Telling those apart would confirm the existence of an
    /// image the caller is not entitled to see, so the refusal is identical.
    ImageNotFound {
        /// The image the caller asked for.
        image_id: String,
    },

    /// The image row exists but cannot be launched: its artifact is missing
    /// from disk, or it does not export the current lifecycle entrypoint.
    ///
    /// Grouped with [`Self::ImageNotFound`] by callers on purpose — both are
    /// repaired by registering the image again, and neither is retryable as-is.
    ImageNotRunnable {
        /// The image the caller asked for.
        image_id: String,
        /// What was wrong with it.
        detail: String,
    },

    /// A guarded trigger lost the durable workflow-wide launch race. Not a
    /// failure: the workflow is already doing the work that was asked for.
    SingleInstanceActive,

    /// The instance id is taken by a row this request does not own, or by one
    /// with no active launch generation.
    InstanceAlreadyExists {
        /// The contested instance id.
        instance_id: String,
    },

    /// Environment could not carry the start through — a database, queue or
    /// persistence failure. Unlike every refusal above, the same request may
    /// succeed on a retry, so a caller must not treat it as a bad image.
    Internal(String),
}

impl std::fmt::Display for StartRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(detail) => write!(formatter, "{detail}"),
            Self::ImageNotFound { image_id } => {
                write!(formatter, "Image '{image_id}' not found")
            }
            Self::ImageNotRunnable { image_id, detail } => {
                write!(formatter, "Image '{image_id}' is not runnable: {detail}")
            }
            Self::SingleInstanceActive => write!(formatter, "{SINGLE_INSTANCE_ACTIVE}"),
            Self::InstanceAlreadyExists { instance_id } => {
                write!(formatter, "Instance '{instance_id}' already exists")
            }
            Self::Internal(detail) => write!(formatter, "Failed to create instance: {detail}"),
        }
    }
}

async fn existing_start_response(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    tenant_id: &str,
    image_id: &str,
) -> Result<Option<StartInstanceResponse>> {
    let Some(existing) = state.persistence.get_instance_meta(instance_id).await? else {
        return Ok(None);
    };

    if existing.tenant_id != tenant_id {
        warn!(
            instance_id,
            requested_tenant_id = tenant_id,
            existing_tenant_id = %existing.tenant_id,
            "Rejecting reuse of an instance ID owned by another tenant"
        );
        return Ok(Some(StartInstanceResponse::rejected(
            StartRejection::InstanceAlreadyExists {
                instance_id: instance_id.to_string(),
            },
        )));
    }

    match InstanceRepository::new(state.pool.clone())
        .image_binding(instance_id)
        .await?
        .map(|binding| binding.image_id)
    {
        Some(existing_image_id) if existing_image_id == image_id => {}
        Some(existing_image_id) => {
            warn!(
                instance_id,
                requested_image_id = image_id,
                existing_image_id,
                "Rejecting reuse of an instance ID for a different image"
            );
            return Ok(Some(StartInstanceResponse::rejected(
                StartRejection::InstanceAlreadyExists {
                    instance_id: instance_id.to_string(),
                },
            )));
        }
        None => {
            // An instance row without an image association is an incomplete or
            // self-registered instance, not proof that this exact start request
            // was accepted.
            warn!(
                instance_id,
                requested_image_id = image_id,
                "Rejecting reuse of an instance ID without an image association"
            );
            return Ok(Some(StartInstanceResponse::rejected(
                StartRejection::InstanceAlreadyExists {
                    instance_id: instance_id.to_string(),
                },
            )));
        }
    }

    info!(
        instance_id,
        tenant_id,
        image_id,
        status = ?existing.status,
        "Instance start already accepted; returning deduplicated response"
    );
    Ok(Some(StartInstanceResponse::accepted(instance_id, true)))
}

/// Enrich instance input for storage (display/audit purposes):
/// 1. Merge default variable values from image metadata (fill missing only)
/// 2. Strip system variables (prefixed with `_`)
///
/// This ensures the stored input reflects what the workflow actually receives,
/// while hiding internal runtime variables from API users.
pub fn enrich_input_for_storage(
    mut input: serde_json::Value,
    image: &crate::image_registry::Image,
) -> serde_json::Value {
    // Merge defaults from image metadata (if available)
    if let Some(ref metadata) = image.metadata
        && let Some(default_vars) = metadata.get("variables").and_then(|v| v.as_object())
    {
        let input_obj = input
            .as_object_mut()
            .expect("input should be a JSON object");
        let vars = input_obj
            .entry("variables")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(vars_obj) = vars.as_object_mut() {
            for (key, value) in default_vars {
                if !key.starts_with('_') {
                    vars_obj.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
    }

    // Strip system variables (prefixed with _)
    if let Some(vars) = input.get_mut("variables").and_then(|v| v.as_object_mut()) {
        vars.retain(|key, _| !key.starts_with('_'));
    }

    input
}

/// Handle start instance request.
#[instrument(skip(state, request), fields(
    tenant_id = %request.tenant_id,
    image_id = %request.image_id,
    instance_id = ?request.instance_id,
))]
pub async fn handle_start_instance(
    state: &EnvironmentHandlerState,
    request: StartInstanceRequest,
) -> Result<StartInstanceResponse> {
    info!(
        image_id = %request.image_id,
        tenant_id = %request.tenant_id,
        "Start instance request received"
    );

    // Validate image_id
    if request.image_id.is_empty() {
        return Ok(StartInstanceResponse::rejected(
            StartRejection::InvalidRequest("image_id is required".to_string()),
        ));
    }

    // Look up image
    let image_registry = ImageRegistry::new(state.pool.clone());
    let image = match image_registry.get(&request.image_id).await {
        Ok(Some(img)) => img,
        Ok(None) => {
            return Ok(StartInstanceResponse::rejected(
                StartRejection::ImageNotFound {
                    image_id: request.image_id.clone(),
                },
            ));
        }
        Err(e) => {
            error!(error = %e, "Failed to look up image");
            // Deliberately NOT an image refusal. The caller repairs a missing
            // image by rebuilding it; doing that because the database blinked
            // would throw away a compilation that was never at fault.
            return Ok(StartInstanceResponse::rejected(StartRejection::Internal(
                format!("failed to look up image: {e}"),
            )));
        }
    };

    // Verify tenant owns this image (multi-tenant isolation)
    if image.tenant_id != request.tenant_id {
        warn!(
            image_id = %request.image_id,
            image_tenant = %image.tenant_id,
            request_tenant = %request.tenant_id,
            "Tenant mismatch: tenant does not own this image"
        );
        return Ok(StartInstanceResponse::rejected(
            StartRejection::ImageNotFound {
                image_id: request.image_id.clone(),
            },
        ));
    }

    let wasm_path = PathBuf::from(&image.binary_path);

    // Generate or use provided instance ID
    let instance_id = request
        .instance_id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // The trigger stream is intentionally at-least-once. A worker can lose its
    // response or fail XACK after Environment has durably reserved the ID. The
    // primary key is therefore an idempotency key, not a permanent start error,
    // so a replay is answered with the original response rather than an error.
    //
    // The claim itself is the INSERT below: it reports whether it won the id,
    // which costs one statement instead of a SELECT that misses on every fresh
    // launch. The only place a replay still has to be recognised *before* then
    // is the artifact check, because an already-accepted start does not depend
    // on the wasm still being on disk.

    // Validate the filesystem half of the image registration before writing
    // any instance state. Otherwise a stale image row creates a failed instance
    // and the trigger retry collides with that row after recompilation.
    if !wasm_path.is_file() {
        // A replay of an already-accepted start is still answered from the
        // existing row: it was accepted when the artifact was present, and
        // losing the file afterwards must not turn a duplicate into an error.
        if let Some(response) =
            existing_start_response(state, &instance_id, &request.tenant_id, &request.image_id)
                .await?
        {
            return Ok(response);
        }
        warn!(
            image_id = %request.image_id,
            binary_path = %wasm_path.display(),
            "Registered image artifact is missing"
        );
        return Ok(StartInstanceResponse::rejected(
            StartRejection::ImageNotRunnable {
                image_id: request.image_id.clone(),
                detail: "registered artifact is missing from disk".to_string(),
            },
        ));
    }

    // A compiled workflow must prove its current lifecycle ABI before we
    // create a pending instance. This keeps a retired `wasi:cli/run` artifact
    // from ever taking a runner permit or consuming admission while it waits.
    if let Err(error) = require_current_workflow_entrypoint(&image).await {
        warn!(
            image_id = %request.image_id,
            error = %error,
            "Refusing workflow image without lifecycle.invoke"
        );
        return Ok(StartInstanceResponse::rejected(
            StartRejection::ImageNotRunnable {
                image_id: request.image_id.clone(),
                detail: error.to_string(),
            },
        ));
    }

    // Prepare the durable input envelope before the atomic initial claim. The
    // dispatcher reads this committed value later; it never retains request
    // memory while it waits for a runner slot.
    let input = request.input.unwrap_or(serde_json::json!({}));
    let input_for_storage = enrich_input_for_storage(input.clone(), &image);
    let input_bytes = serde_json::to_vec(&input_for_storage).ok();

    // Resolve the effective execution timeout once, before claiming an
    // instance. A malformed direct Environment request must not leave a
    // durable pending/queued pair behind.
    let timeout = state
        .execution_timeout_policy
        .resolve_raw(request.timeout_seconds)
        .map_err(|error| {
            crate::error::Error::InvalidRequest(format!("invalid execution timeout: {error}"))
        })?
        .as_duration();

    // The instance, immutable image binding, and first physical launch must be
    // one transaction. A process loss after just the first two writes used to
    // create an unowned `pending` instance that occupied admission forever.
    let repository = LaunchRepository::new(state.pool.clone());
    let request_tenant_id = request.tenant_id.clone();
    let request_image_id = request.image_id.clone();
    // The embedded server supplies workflow identity on the regular guest
    // environment and marks only guarded trigger deliveries with this
    // short-lived reserved key. Consume the marker before persisting guest
    // environment so this internal admission detail never reaches a workflow.
    let mut launch_env = request.env;
    let single_instance = launch_env
        .remove(SINGLE_INSTANCE_LAUNCH_ENV)
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let workflow_id = launch_env
        .get("WORKFLOW_ID")
        .filter(|workflow_id| !workflow_id.is_empty())
        .cloned();
    let launch = EnqueueRequest::immediate(
        uuid::Uuid::new_v4().to_string(),
        instance_id.clone(),
        request_tenant_id.clone(),
        request_image_id.clone(),
        LaunchKind::Start,
        DEFAULT_LAUNCH_QUEUE_TIMEOUT,
    );
    let launch = match workflow_id {
        Some(workflow_id) => launch.with_workflow_scope(workflow_id, single_instance),
        None => launch,
    };
    let initial = InitialLaunchRequest {
        launch,
        input: input_bytes,
        env: Some(launch_env),
        timeout_seconds: Some(
            i64::try_from(timeout.as_secs())
                .expect("bounded execution timeout fits in database integer"),
        ),
    };

    match repository.claim_initial(initial).await {
        Ok(InitialLaunchOutcome::Enqueued(launch)) => {
            state.launch_notifier.notify_one();
            info!(
                instance_id = %instance_id,
                launch_id = %launch.launch_id,
                "Instance start durably queued"
            );
            Ok(StartInstanceResponse::accepted(instance_id, false))
        }
        Ok(InitialLaunchOutcome::SingleInstanceActive) => {
            // This is a deliberate trigger skip, not a failed Environment
            // start. The embedded client maps the stable code back to the
            // trigger worker, which ACKs it without creating an instance.
            Ok(StartInstanceResponse::rejected(
                StartRejection::SingleInstanceActive,
            ))
        }
        Ok(InitialLaunchOutcome::ExistingLaunch(_)) => {
            // The existing active generation is the idempotency winner. Keep
            // the older response contract while never enqueueing a second run.
            if let Some(response) =
                existing_start_response(state, &instance_id, &request_tenant_id, &request_image_id)
                    .await?
            {
                Ok(response)
            } else {
                Ok(StartInstanceResponse::rejected(
                    StartRejection::InstanceAlreadyExists {
                        instance_id: instance_id.clone(),
                    },
                ))
            }
        }
        Ok(InitialLaunchOutcome::ExistingInstance) => {
            // A legacy or malformed row has no active durable owner. Do not
            // call it a successful replay: doing so would hide exactly the
            // stranded state the queue is supposed to surface and recover.
            warn!(instance_id = %instance_id, "Existing instance has no active launch generation");
            Ok(StartInstanceResponse::rejected(StartRejection::Internal(
                format!("instance '{instance_id}' exists without an active launch generation"),
            )))
        }
        Err(error) => {
            error!(error = %error, "Failed to atomically queue instance start");
            Ok(StartInstanceResponse::rejected(StartRejection::Internal(
                error.to_string(),
            )))
        }
    }
}

// ============================================================================
// Stop Instance
// ============================================================================

/// Request to stop an instance.
pub struct StopInstanceRequest {
    /// Instance ID to stop.
    pub instance_id: String,
    /// Reason for stopping.
    pub reason: String,
    /// Grace period before force kill in seconds.
    ///
    /// Accepted for wire compatibility but not currently observed: the stop
    /// path cancels the guest immediately via `Runner::stop`. It previously
    /// only ever populated the cancellation token, which nothing read.
    pub grace_period_seconds: u64,
}

/// Response from stopping an instance.
pub struct StopInstanceResponse {
    /// Whether the stop was initiated.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Handle stop instance request.
#[instrument(skip(state, request), fields(
    instance_id = %request.instance_id,
    reason = %request.reason,
))]
pub async fn handle_stop_instance(
    state: &EnvironmentHandlerState,
    request: StopInstanceRequest,
) -> Result<StopInstanceResponse> {
    info!(
        instance_id = %request.instance_id,
        reason = %request.reason,
        "Stop instance request received"
    );

    // A queued/leased launch has no runner handle yet. Cancel it in the same
    // transaction that terminalizes Core, before falling back to the legacy
    // registry-based running cancellation path.
    let launches = LaunchRepository::new(state.pool.clone());
    match launches.get_active_for_instance(&request.instance_id).await {
        Ok(Some(active)) => match launches.cancel_before_start(&active.launch_id).await {
            Ok(CancelOutcome::Cancelled(cancelled)) => {
                state
                    .lifecycle_observers
                    .notify_released(&cancelled, "cancelled");
                info!(
                    instance_id = %request.instance_id,
                    launch_id = %cancelled.launch_id,
                    "Cancelled instance before runner handoff"
                );
                return Ok(StopInstanceResponse {
                    success: true,
                    error: None,
                });
            }
            Ok(CancelOutcome::TooLate(_)) | Ok(CancelOutcome::NotFound) => {
                // The dispatcher passed the generation's start fence. The
                // registry/runner path below owns cancellation from here.
            }
            Err(error) => {
                return Ok(StopInstanceResponse {
                    success: false,
                    error: Some(format!("Failed to cancel queued launch: {error}")),
                });
            }
        },
        Ok(None) => {}
        Err(error) => {
            return Ok(StopInstanceResponse {
                success: false,
                error: Some(format!("Failed to inspect queued launch: {error}")),
            });
        }
    }

    // Look up container
    let container_registry = ContainerRegistry::new(state.pool.clone());
    let container = match container_registry.get(&request.instance_id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Ok(StopInstanceResponse {
                success: false,
                error: Some(format!(
                    "Instance '{}' not found in container registry",
                    request.instance_id
                )),
            });
        }
        Err(e) => {
            error!(error = %e, "Failed to look up container");
            return Ok(StopInstanceResponse {
                success: false,
                error: Some(format!("Database error: {}", e)),
            });
        }
    };

    // The registry's persisted generation is the cancellation fence. Older
    // rows are backfilled by the migration from their legacy handle id.
    let handle = RunnerHandle {
        launch_id: container.launch_id.clone(),
        handle_id: container.container_id,
        instance_id: request.instance_id.clone(),
        tenant_id: container.tenant_id,
        started_at: container.started_at,
        metrics: None,
    };

    if let Err(e) = state.runner.stop(&handle).await {
        warn!(error = %e, "Runner stop returned error");
    }

    // Update instance status to cancelled via Persistence trait
    let _ = state
        .persistence
        .complete_instance(CompleteInstanceParams::new(
            &request.instance_id,
            CoreInstanceStatus::Cancelled,
        ))
        .await;

    // The runner was already handed this generation, so the queue is released
    // only after the Core cancellation write above has committed. A concurrent
    // monitor can win this transition; in that case it also owns notification.
    match launches
        .mark_terminal(
            &container.launch_id,
            LaunchState::Cancelled,
            Some(&request.reason),
        )
        .await
    {
        Ok(Some(cancelled)) => state
            .lifecycle_observers
            .notify_released(&cancelled, "cancelled"),
        Ok(None) => {}
        Err(error) => warn!(
            instance_id = %request.instance_id,
            launch_id = %container.launch_id,
            error = %error,
            "Failed to terminalize cancelled launch generation"
        ),
    }

    // Clean up container registry
    let _ = container_registry
        .cleanup_handle(
            &request.instance_id,
            &container.launch_id,
            &handle.handle_id,
        )
        .await;

    info!("Instance stopped successfully");

    Ok(StopInstanceResponse {
        success: true,
        error: None,
    })
}

// ============================================================================
// Resume Instance
// ============================================================================

/// Request to resume a suspended instance.
pub struct ResumeInstanceRequest {
    /// Instance ID to resume.
    pub instance_id: String,
}

/// Response from resuming an instance.
pub struct ResumeInstanceResponse {
    /// Whether resume was initiated.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Handle resume instance request.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
pub async fn handle_resume_instance(
    state: &EnvironmentHandlerState,
    request: ResumeInstanceRequest,
) -> Result<ResumeInstanceResponse> {
    info!(instance_id = %request.instance_id, "Resume instance request received");

    // Get instance from DB
    let (status, tenant_id) = match db::instance_identity(&state.pool, &request.instance_id).await?
    {
        Some(identity) => identity,
        None => {
            return Ok(ResumeInstanceResponse {
                success: false,
                error: Some(format!("Instance '{}' not found", request.instance_id)),
            });
        }
    };

    // A physical resume is only meaningful for a parked instance. Failed and
    // cancelled instances are terminal; accepting them here used to bypass
    // their terminal lifecycle state by flipping it to running before a
    // detached runner launch.
    if status != "suspended" {
        return Ok(ResumeInstanceResponse {
            success: false,
            error: Some(format!(
                "Cannot resume instance in '{}' state (must be suspended)",
                status
            )),
        });
    }

    // Read only the durable image binding. Artifact and timeout preflight is
    // owned by the dispatcher, after this request has a recoverable queue row.
    let image_id = match InstanceRepository::new(state.pool.clone())
        .image_binding(&request.instance_id)
        .await?
    {
        Some(binding) => binding.image_id,
        None => {
            return Ok(ResumeInstanceResponse {
                success: false,
                error: Some("Instance has no associated image".to_string()),
            });
        }
    };

    let repository = LaunchRepository::new(state.pool.clone());
    for released in repository
        .reconcile_released_instance(&request.instance_id)
        .await
        .map_err(|error| {
            crate::error::Error::Other(format!("Failed to reconcile parked launch: {error}"))
        })?
    {
        state
            .lifecycle_observers
            .notify_released(&released, "reconciled");
    }
    let enqueue = EnqueueRequest::immediate(
        uuid::Uuid::new_v4().to_string(),
        request.instance_id.clone(),
        tenant_id,
        image_id,
        LaunchKind::Resume,
        DEFAULT_LAUNCH_QUEUE_TIMEOUT,
    );
    match repository.enqueue(enqueue).await {
        Ok(EnqueueOutcome::Enqueued(launch)) | Ok(EnqueueOutcome::Existing(launch)) => {
            // Enqueuing a new resume clears the old timer in its transaction.
            // Clearing here could erase a timer set by a fast resumed guest.
            state.launch_notifier.notify_one();
            info!(
                instance_id = %request.instance_id,
                launch_id = %launch.launch_id,
                "Instance resume durably queued"
            );
            Ok(ResumeInstanceResponse {
                success: true,
                error: None,
            })
        }
        Ok(EnqueueOutcome::SingleInstanceActive) => Ok(ResumeInstanceResponse {
            success: false,
            error: Some(
                "Resume deferred because this single-instance workflow already has active work"
                    .to_string(),
            ),
        }),
        Err(error) => Ok(ResumeInstanceResponse {
            success: false,
            error: Some(format!("Resume could not be queued: {error}")),
        }),
    }
}

// ============================================================================
// Container Monitor
// ============================================================================

/// Spawn a background task that monitors the container and processes output when done.
///
/// This function should be called after launching an instance to monitor its lifecycle
/// and process output when the container finishes. The timeout is enforced here - if the
/// container runs longer than the specified timeout, it will be killed.
///
/// ## Structure
///
/// The body is a `tokio::select!` over two futures:
/// - `runner.wait_for_exit(...)` — runner-specific exit detection (the embedded
///   runner awaits its run task; the default impl polls).
/// - `tokio::time::sleep_until(...)` — the timeout deadline.
///
/// Each runner's `wait_for_exit` impl is cancel-safe, so dropping it when the
/// timeout branch fires is sound.
///
/// ## Exit branch
///
/// When the process exits, we:
/// 1. Collect metrics and stderr (`runner.collect_result`).
/// 2. Persist them best-effort.
/// 3. Claim the registry row this monitor registered. The delete succeeds only
///    while this monitor still owns the instance, so it doubles as the
///    ownership check — a resumed instance gets a new monitor, and the old one
///    must not write crash state for the previous PID.
/// 4. If we're still the owning monitor, mirror Core's view: if the SDK already
///    wrote a terminal status we leave it alone, otherwise we mark the instance
///    failed/crashed (or suspended/shutdown_requested if draining).
/// 5. Clean up the container registry entry.
///
/// ## Timeout branch
///
/// We stop the runner, mark the instance failed with `termination_reason="timeout"`,
/// and clean up the registry. Metrics/stderr are deliberately NOT collected here
/// — the previous implementation did not collect them on timeout either, and
/// doing so now would race with `runner.stop`.
async fn release_launch_after_monitor(
    pool: &PgPool,
    persistence: &dyn Persistence,
    launch_id: &str,
    instance_id: &str,
    lifecycle_observers: &LaunchLifecycleObservers,
) {
    let instance = match persistence.get_instance_meta(instance_id).await {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            warn!(
                launch_id,
                instance_id, "Launched instance disappeared before queue reconciliation"
            );
            return;
        }
        Err(error) => {
            warn!(launch_id, instance_id, error = %error, "Could not read instance after runner exit for queue reconciliation");
            return;
        }
    };
    let repository = LaunchRepository::new(pool.clone());
    let result = match instance.status {
        CoreInstanceStatus::Suspended => repository
            .mark_suspended(launch_id)
            .await
            .map(|launch| launch.map(|launch| (launch, "suspended"))),
        CoreInstanceStatus::Completed => repository
            .mark_terminal(launch_id, LaunchState::Completed, None)
            .await
            .map(|launch| launch.map(|launch| (launch, "completed"))),
        CoreInstanceStatus::Failed => repository
            .mark_terminal(launch_id, LaunchState::Failed, instance.error.as_deref())
            .await
            .map(|launch| launch.map(|launch| (launch, "failed"))),
        CoreInstanceStatus::Cancelled => repository
            .mark_terminal(launch_id, LaunchState::Cancelled, None)
            .await
            .map(|launch| launch.map(|launch| (launch, "cancelled"))),
        status => {
            debug!(
                launch_id,
                instance_id,
                ?status,
                "Launch monitor left nonterminal queue generation active"
            );
            return;
        }
    };

    match result {
        Ok(Some((launch, reason))) => lifecycle_observers.notify_released(&launch, reason),
        Ok(None) => {}
        Err(error) => warn!(
            launch_id,
            instance_id,
            error = %error,
            "Failed to reconcile queue generation after runner exit"
        ),
    }
}

/// Run one bounded cleanup step for a start-gate handoff that never opened.
///
/// Both failure modes are logged and neither aborts the caller: whatever is
/// left behind is reclaimed by durable expiry or restart recovery, which stay
/// authoritative precisely so this path can be best-effort. The two callers
/// carried the same twenty-five line `timeout` / three-way `match` between
/// them, differing only in these two messages.
async fn best_effort_start_gate_cleanup<T, E: std::fmt::Display>(
    instance_id: &str,
    launch_id: &str,
    failed: &'static str,
    timed_out: &'static str,
    step: impl std::future::Future<Output = std::result::Result<T, E>>,
) {
    match tokio::time::timeout(START_GATE_MONITOR_CLEANUP_TIMEOUT, step).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => warn!(
            instance_id = %instance_id,
            launch_id = %launch_id,
            error = %error,
            "{}", failed
        ),
        Err(_) => warn!(
            instance_id = %instance_id,
            launch_id = %launch_id,
            "{}", timed_out
        ),
    }
}

/// Kill and settle a guest that outlived its execution timeout.
///
/// Unlike the exit path there is nothing to collect: the process is still
/// running and is about to be stopped, so this writes the terminal status
/// itself rather than reading one the guest reported.
async fn settle_execution_timeout(
    pool: &PgPool,
    runner: &Arc<dyn Runner>,
    handle: &RunnerHandle,
    persistence: &Arc<dyn Persistence>,
    lifecycle_observers: &LaunchLifecycleObservers,
    instance_id: &str,
    timeout: Duration,
) {
    let container_registry = ContainerRegistry::new(pool.clone());
    warn!(
        instance_id = %instance_id,
        timeout_secs = %timeout.as_secs(),
        "Execution timed out, killing container"
    );
    let _ = runner.stop(handle).await;

    // Update instance status to failed with termination_reason = "timeout"
    if let Err(e) = persistence
        .complete_instance(
            CompleteInstanceParams::new(instance_id, CoreInstanceStatus::Failed)
                .if_running()
                .with_termination("timeout", None)
                .with_error("Execution timed out"),
        )
        .await
    {
        warn!(
            instance_id = %instance_id,
            error = %e,
            "Failed to update instance status after timeout"
        );
    }

    // Clean up container registry, but only the row this monitor
    // registered: a resume may have replaced it with a live run.
    let _ = container_registry
        .cleanup_handle(instance_id, &handle.launch_id, &handle.handle_id)
        .await;

    release_launch_after_monitor(
        pool,
        persistence.as_ref(),
        &handle.launch_id,
        instance_id,
        lifecycle_observers,
    )
    .await;
}

/// Persist what the finished process left behind, and read back the status the
/// guest reported.
///
/// Ordering matters and is the reason this runs before the staleness check: a
/// monitor that has been superseded still holds the only copy of its own run's
/// diagnostics, so it writes them before discovering it is stale.
async fn record_exit_diagnostics(
    pool: &PgPool,
    persistence: &Arc<dyn Persistence>,
    runner: &Arc<dyn Runner>,
    handle: &RunnerHandle,
    instance_id: &str,
) -> (
    std::result::Result<
        Option<(CoreInstanceStatus, Option<String>)>,
        runtara_core::error::CoreError,
    >,
    Option<String>,
) {
    let (_output, stderr, metrics) = runner.collect_result(handle).await;

    let observed_status = match crate::metrics::record_resources_returning_status(
        pool,
        instance_id,
        metrics.memory_peak_bytes,
        metrics.cpu_usage_usec,
    )
    .await
    {
        Ok(observed) => {
            debug!(
                instance_id = %instance_id,
                memory_peak_bytes = ?metrics.memory_peak_bytes,
                cpu_usage_usec = ?metrics.cpu_usage_usec,
                "Stored container metrics"
            );
            Ok(observed)
        }
        Err(e) => {
            warn!(
                instance_id = %instance_id,
                error = %e,
                "Failed to store container metrics"
            );
            // The status this carries decides crash vs normal exit
            // below, so a failed metrics write must not be read as a
            // crash. Fall back to a plain status read, as before.
            persistence
                .get_instance_meta(instance_id)
                .await
                .map(|found| found.map(|i| (i.status, i.termination_reason)))
        }
    };

    // Store stderr via Persistence trait for debugging (even if instance succeeds via Core)
    if let Some(ref stderr_content) = stderr {
        if let Err(e) =
            crate::metrics::record_instance_stderr(pool, instance_id, stderr_content).await
        {
            warn!(
                instance_id = %instance_id,
                error = %e,
                "Failed to store container stderr"
            );
        } else {
            debug!(
                instance_id = %instance_id,
                stderr_len = stderr_content.len(),
                "Stored container stderr"
            );
        }
    }

    (observed_status, stderr)
}

/// Settle a guest that exited without reporting a terminal status.
///
/// Two very different situations reach here and the drain flag is what tells
/// them apart: during a drain this is the expected force-kill, and the instance
/// is parked for the wake scheduler to relaunch; otherwise the process died on
/// its own and the run is a crash.
async fn settle_unreported_exit(
    persistence: &Arc<dyn Persistence>,
    drain: &DrainController,
    instance_id: &str,
    stderr: Option<&str>,
) {
    // Process died without a terminal SDK event. If the environment
    // is draining, this is the expected force-kill path — mark the
    // instance as `suspended + shutdown_requested` so restart-time
    // heartbeat-monitor recovery treats it as a normal suspension
    // rather than a crash.
    let draining = drain.is_draining();
    let (status, termination_reason, default_error) = if draining {
        (
            CoreInstanceStatus::Suspended,
            "shutdown_requested",
            "Process terminated during graceful shutdown",
        )
    } else {
        (
            CoreInstanceStatus::Failed,
            "crashed",
            "Process terminated without SDK event",
        )
    };

    // A crash with no terminal SDK event (e.g. a guest trap such as
    // the per-instance memory limit being exceeded) leaves its only
    // diagnostic in the per-run stderr, where the host writes
    // "workflow failed: <reason>". Fold that reason into the instance
    // `error` so the failure is visible in the API rather than the
    // generic "terminated without SDK event" — otherwise the step that
    // was in flight surfaces as running with a null error.
    let crash_error: String = match stderr.map(str::trim) {
        Some(reason) if !draining && !reason.is_empty() => {
            format!("{default_error}: {}", tail_chars(reason, 2000))
        }
        _ => default_error.to_string(),
    };

    let mut params = CompleteInstanceParams::new(instance_id, status)
        .if_running()
        .with_termination(termination_reason, None)
        .with_error(&crash_error);
    if let Some(s) = stderr {
        params = params.with_stderr(s);
    }
    match persistence.complete_instance(params).await {
        Ok(applied) => {
            if applied {
                if drain.is_draining() {
                    // Schedule an immediate wake so the wake
                    // scheduler relaunches the instance after
                    // restart — without this a force-stopped
                    // instance with no checkpoint stays
                    // suspended forever.
                    if let Err(e) = persistence
                        .schedule_wake(
                            instance_id,
                            chrono::Utc::now(),
                            runtara_core::domain::WakeReason::Recovery,
                        )
                        .await
                    {
                        warn!(
                            instance_id = %instance_id,
                            error = %e,
                            "Failed to schedule post-restart wake"
                        );
                    }
                    info!(
                        instance_id = %instance_id,
                        "Process terminated during drain - suspended for shutdown"
                    );
                } else {
                    warn!(
                        instance_id = %instance_id,
                        "Process terminated without SDK event - marked as crashed"
                    );
                }
            } else {
                info!(
                    instance_id = %instance_id,
                    "Instance SDK event arrived just in time"
                );
            }
        }
        Err(e) => {
            error!(
                instance_id = %instance_id,
                error = %e,
                "Failed to mark instance terminal state"
            );
        }
    }
}

/// What the monitor should do once a start gate has failed to open.
enum AfterFailedGate {
    /// The runner's confirmation won the marker race after all, so there is a
    /// live guest to watch.
    Watch,
    /// The handoff is settled, or has been left to durable recovery. Either
    /// way this monitor has nothing further to do.
    Done,
}

/// Terminalize a handoff whose runner never durably crossed the start gate.
///
/// Every failure here ends in `Done` rather than an error: the durable expiry
/// scan and restart recovery own anything this cannot finish, and a monitor
/// that pressed on regardless would be acting on a generation it no longer
/// owns. The one path back to `Watch` is the race where the confirmation
/// committed just as its response was lost — the marker is already cleared, so
/// there is a running guest and no terminal row to write.
async fn settle_failed_start_gate(
    pool: &PgPool,
    runner: &Arc<dyn Runner>,
    handle: &RunnerHandle,
    lifecycle_observers: &LaunchLifecycleObservers,
    instance_id: &str,
    attempt_count: i32,
) -> AfterFailedGate {
    warn!(
        instance_id = %instance_id,
        launch_id = %handle.launch_id,
        attempt_count,
        "Start gate did not permit guest execution; terminalizing exact handoff"
    );

    let repository = LaunchRepository::new(pool.clone());
    let terminal = match tokio::time::timeout(
        START_GATE_MONITOR_CLEANUP_TIMEOUT,
        repository.fail_unconfirmed_running(
            &handle.launch_id,
            attempt_count,
            "runner did not durably cross start gate",
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            warn!(
                instance_id = %instance_id,
                launch_id = %handle.launch_id,
                attempt_count,
                "Timed out terminalizing failed start-gate handoff; durable expiry will recover it"
            );
            return AfterFailedGate::Done;
        }
    };

    match terminal {
        // This monitor owns the generation: stop the runner and drop the
        // registry row before releasing admission.
        Ok(Some(failed)) => {
            best_effort_start_gate_cleanup(
                instance_id,
                &handle.launch_id,
                "Failed to stop runner after failed start-gate confirmation",
                "Timed out stopping failed start-gate runner; closed gate/lease recovery remains authoritative",
                runner.stop(handle),
            )
            .await;
            best_effort_start_gate_cleanup(
                instance_id,
                &handle.launch_id,
                "Could not remove failed start-gate runner registry row",
                "Timed out removing failed start-gate registry row; restart recovery will reconcile it",
                ContainerRegistry::new(pool.clone()).cleanup_handle(
                    instance_id,
                    &handle.launch_id,
                    &handle.handle_id,
                ),
            )
            .await;
            lifecycle_observers.notify_released(&failed, "start_gate_failed");
            AfterFailedGate::Done
        }
        // Nothing to terminalize. Either the confirmation beat us to the
        // marker — a live guest — or someone else already settled this.
        Ok(None) => match tokio::time::timeout(
            START_GATE_MONITOR_CLEANUP_TIMEOUT,
            repository.is_gate_confirmed(&handle.launch_id, attempt_count),
        )
        .await
        {
            Ok(Ok(true)) => {
                warn!(
                    instance_id = %instance_id,
                    launch_id = %handle.launch_id,
                    attempt_count,
                    "Start gate was confirmed while monitor observed its deadline"
                );
                AfterFailedGate::Watch
            }
            Ok(Ok(false)) => {
                debug!(
                    instance_id = %instance_id,
                    launch_id = %handle.launch_id,
                    attempt_count,
                    "Failed start-gate handoff was already recovered or terminalized"
                );
                AfterFailedGate::Done
            }
            Ok(Err(error)) => {
                warn!(
                    instance_id = %instance_id,
                    launch_id = %handle.launch_id,
                    attempt_count,
                    error = %error,
                    "Could not read failed start-gate handoff; retaining registry for recovery"
                );
                AfterFailedGate::Done
            }
            Err(_) => {
                warn!(
                    instance_id = %instance_id,
                    launch_id = %handle.launch_id,
                    attempt_count,
                    "Timed out reading failed start-gate handoff; retaining registry for durable recovery"
                );
                AfterFailedGate::Done
            }
        },
        Err(error) => {
            warn!(
                instance_id = %instance_id,
                launch_id = %handle.launch_id,
                attempt_count,
                error = %error,
                "Could not terminalize failed start-gate handoff; retaining registry for recovery"
            );
            AfterFailedGate::Done
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Spawn the generation-owned monitor for a runner handoff.
///
/// The monitor collects diagnostics, writes a crash/timeout fallback when the
/// guest did not report one, and reconciles the matching durable launch row
/// only after the Core lifecycle transition has committed.
pub fn spawn_container_monitor(
    pool: PgPool,
    runner: Arc<dyn Runner>,
    handle: RunnerHandle,
    persistence: Arc<dyn Persistence>,
    timeout: Duration,
    drain: DrainController,
    lifecycle_observers: LaunchLifecycleObservers,
    // A durable dispatcher installs the monitor before it opens this gate.
    // The active execution timeout begins only once guest execution is
    // allowed; a closed gate is bounded by its separate handoff lease. The
    // durable attempt fences cleanup when a recovered launch reuses its id.
    start_gate: Option<(StartGate, i32)>,
) {
    let instance_id = handle.instance_id.clone();

    tokio::spawn(async move {
        if let Some((gate, attempt_count)) = start_gate {
            let opened = matches!(
                gate.wait_for_runner_confirmation().await,
                StartGateOutcome::Opened
            );
            if !opened
                && matches!(
                    settle_failed_start_gate(
                        &pool,
                        &runner,
                        &handle,
                        &lifecycle_observers,
                        &instance_id,
                        attempt_count,
                    )
                    .await,
                    AfterFailedGate::Done
                )
            {
                return;
            }
        }

        // Brief initial delay to let the process start before we begin watching it.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let poll_interval = Duration::from_millis(50);
        let container_registry = ContainerRegistry::new(pool.clone());
        let sleep_until = tokio::time::Instant::now() + timeout;

        let wait_fut = runner.wait_for_exit(&handle, poll_interval);
        tokio::pin!(wait_fut);

        tokio::select! {
            _ = &mut wait_fut => {
                info!(
                    instance_id = %instance_id,
                    "Process terminated, checking Core status"
                );

                let (observed_status, stderr) = record_exit_diagnostics(
                    &pool,
                    &persistence,
                    &runner,
                    &handle,
                    &instance_id,
                )
                .await;

                // Guard: check that this monitor is still the active one for this instance.
                // When an instance is resumed, a NEW monitor is spawned for the new process.
                // The OLD monitor (for the previous PID) may still be running and must not
                // interfere with the new execution. The check intentionally happens AFTER
                // metrics/stderr writes so a stale monitor doesn't drop diagnostic data
                // for the previous process.
                // Deleting the row this monitor registered answers both
                // questions in one statement: it succeeds only while this
                // monitor is still the owner, so a `false` IS the stale signal,
                // and the cleanup the tail of this arm used to repeat is
                // already done. That tail deleted by instance alone, which a
                // stale monitor would have used to throw away the row of the
                // run that replaced it.
                let is_stale_monitor = match container_registry
                    .cleanup_handle(&instance_id, &handle.launch_id, &handle.handle_id)
                    .await
                {
                    Ok(owned) => !owned,
                    Err(e) => {
                        // Unknown rather than stale. Being conservative here
                        // would silently drop the crash-detection write on a
                        // transient blip; the row is left to the cleanup
                        // workers instead.
                        warn!(
                            instance_id = %instance_id,
                            error = %e,
                            "Could not claim the container registry row; assuming this monitor still owns it"
                        );
                        false
                    }
                };

                if is_stale_monitor {
                    info!(
                        instance_id = %instance_id,
                        monitor_handle = %handle.handle_id,
                        "Stale monitor detected — instance was resumed with a new process, skipping crash check"
                    );
                } else {
                    // Status came back with the metrics write above, so this
                    // no longer needs a read of its own.
                    match &observed_status {
                        Ok(Some((status, _)))
                            if matches!(status, CoreInstanceStatus::Completed | CoreInstanceStatus::Failed | CoreInstanceStatus::Cancelled | CoreInstanceStatus::Suspended) =>
                        {
                            // SDK already reported terminal status — normal termination
                            info!(
                                instance_id = %instance_id,
                                status = ?status,
                                "Instance completed normally (SDK reported)"
                            );
                        }
                        _ => {
                            settle_unreported_exit(
                                &persistence,
                                &drain,
                                &instance_id,
                                stderr.as_deref(),
                            )
                            .await;
                        }
                    }

                    // The Core transition above (whether guest-reported,
                    // crash-derived, or drain-derived) has committed before
                    // this reconciliation runs. Release admission only after
                    // the matching queue generation is durably terminal or
                    // parked; observer failure is intentionally out-of-band.
                    release_launch_after_monitor(
                        &pool,
                        persistence.as_ref(),
                        &handle.launch_id,
                        &instance_id,
                        &lifecycle_observers,
                    )
                    .await;
                }

            }
            _ = tokio::time::sleep_until(sleep_until) => {
                settle_execution_timeout(
                    &pool,
                    &runner,
                    &handle,
                    &persistence,
                    &lifecycle_observers,
                    &instance_id,
                    timeout,
                )
                .await;
            }
        }
    });
}

// ============================================================================
// Reads and signals
//
// These used to live inside `http_server.rs`, each one implemented directly in
// its axum route. That left the transport owning real behaviour — database
// queries, tenant isolation, the on-signal waker — and made every one of them
// reachable only over a socket. They are plain async functions here for the
// same reason the lifecycle handlers above are: the HTTP layer should decode,
// call, and map, and nothing more.
//
// The response types report what the store holds — instants as instants, bodies
// as the bytes that were written. They used to be wire-shaped (`*_ms`
// timestamps, base64 bodies, `Serialize` with the absent fields skipped)
// because a socket used to carry them. Nothing has serialized them since
// environment became a library the server links, so they are plain structs:
// the one consumer reads their fields and builds its own types.
// ============================================================================

/// What happened to a lifecycle signal.
///
/// The refusals are outcomes rather than errors because each maps to a distinct
/// answer the caller can act on, and only the transport knows how to say so.
#[derive(Debug, PartialEq, Eq)]
pub enum SendSignalOutcome {
    /// The signal was stored for the instance to pick up.
    Delivered,
    /// No such instance.
    InstanceNotFound,
    /// The instance is past the point of accepting signals.
    NotSignalable {
        /// The status that refused it.
        status: String,
    },
    /// The signal type is not one of `cancel`, `pause`, `shutdown`.
    UnknownSignalType {
        /// What the caller asked for.
        signal_type: String,
    },
}

/// Send a lifecycle signal (`cancel`, `pause`, `shutdown`) to an instance.
///
/// `shutdown` is how a draining host asks a guest to checkpoint and exit so it
/// can be resumed after the restart. The drain that runs today,
/// [`crate::runtime::EnvironmentRuntime::drain`], writes that signal straight
/// through `Persistence` and never arrives here — which is how this handler
/// came to disagree with the rest of the crate about what a signal type is.
pub async fn handle_send_signal(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    signal_type: &str,
    payload: Option<&[u8]>,
) -> Result<SendSignalOutcome> {
    let Some(instance) = state.persistence.get_instance_meta(instance_id).await? else {
        return Ok(SendSignalOutcome::InstanceNotFound);
    };

    if !matches!(
        instance.status,
        CoreInstanceStatus::Running | CoreInstanceStatus::Suspended | CoreInstanceStatus::Pending
    ) {
        return Ok(SendSignalOutcome::NotSignalable {
            status: crate::core_types::status_name(instance.status).to_owned(),
        });
    }

    // Decoded with the storage layer's own parser rather than a match here, so
    // the set this accepts is by construction the set the column can hold. The
    // local match had drifted from it: it never admitted `shutdown`, though
    // `signal_type_to_str` writes it, the column holds it and `runtime_host`
    // decodes it. `ShutdownCoordinator::drain_executions` is the caller that
    // would have hit the refusal; its map is unpopulated today, so this was
    // latent rather than live.
    let Ok(signal_type) = runtara_store_postgres::encoding::signal_type_from_str(signal_type)
    else {
        return Ok(SendSignalOutcome::UnknownSignalType {
            signal_type: signal_type.to_string(),
        });
    };

    state
        .persistence
        .insert_signal(instance_id, signal_type, payload.unwrap_or_default())
        .await?;

    if signal_type == runtara_core::domain::SignalType::Cancel {
        for cancelled in state
            .persistence
            .cancel_suspended_instances(Some(instance_id), 1)
            .await?
        {
            state.lifecycle_observers.notify_instance_released(
                cancelled.tenant_id,
                cancelled.instance_id,
                "cancelled",
            );
        }
    }
    Ok(SendSignalOutcome::Delivered)
}

/// What happened to a custom signal.
#[derive(Debug, PartialEq, Eq)]
pub enum SendCustomSignalOutcome {
    /// The signal was stored, and a parked instance woken if one was waiting.
    Delivered {
        /// Identity of the retained value written by this request.
        signal_id: String,
    },
    /// No such instance.
    InstanceNotFound,
}

/// Send a custom (workflow-defined) signal addressed to one checkpoint.
pub async fn handle_send_custom_signal(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    checkpoint_id: &str,
    payload: Option<&[u8]>,
) -> Result<SendCustomSignalOutcome> {
    if state
        .persistence
        .get_instance_meta(instance_id)
        .await?
        .is_none()
    {
        return Ok(SendCustomSignalOutcome::InstanceNotFound);
    }

    if checkpoint_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "checkpoint_id is required".to_string(),
        ));
    }

    let signal_id = state
        .persistence
        .put_custom_signal(instance_id, checkpoint_id, payload.unwrap_or_default())
        .await?;

    wake_suspended_on_signal(state.persistence.as_ref(), instance_id).await;
    Ok(SendCustomSignalOutcome::Delivered { signal_id })
}

/// On-signal waker (store-freeing Wait): a Wait compiled with the store-freeing
/// gate parks as `status='suspended'` with `sleep_until` = its timeout deadline
/// (or NULL when the wait has no timeout). The wake scheduler only relaunches on
/// a due `sleep_until`, so a custom signal for such an instance must stamp
/// `sleep_until=now` to relaunch it BEFORE the timeout (or at all, when there is
/// no timeout). The instance replays, re-polls the now-present signal
/// (non-destructive read), and proceeds.
///
/// No-op unless the instance is currently `suspended` AND was parked by an
/// on-signal wait (`termination_reason = 'waiting_signal'`, stamped by
/// `park_invoke_suspend`). `status='suspended'` alone is NOT sufficient: in
/// the default (blocking) configuration every suspended row is a
/// pause/breakpoint/shutdown ack whose pause signal was already consumed —
/// stamping `sleep_until` on those would relaunch a replay that runs PAST the
/// pause, silently auto-resuming a paused instance on any custom signal.
pub async fn wake_suspended_on_signal(persistence: &dyn Persistence, instance_id: &str) {
    match persistence.get_instance_meta(instance_id).await {
        Ok(Some(inst))
            if inst.status == CoreInstanceStatus::Suspended
                && inst.termination_reason.as_deref()
                    == Some(crate::runner::embedded::WAITING_SIGNAL_TERMINATION) =>
        {
            if let Err(e) = persistence
                .schedule_wake(
                    instance_id,
                    chrono::Utc::now(),
                    runtara_core::domain::WakeReason::CustomSignal,
                )
                .await
            {
                warn!(instance_id, error = %e, "Failed to wake suspended instance after custom signal");
            } else {
                info!(instance_id, "Woke suspended instance for a custom signal");
            }
        }
        Ok(_) => {}
        Err(e) => warn!(instance_id, error = %e, "Waker could not read instance status"),
    }
}

/// Checkpoint summary.
#[derive(Debug)]
pub struct CheckpointSummary {
    /// Checkpoint id.
    pub checkpoint_id: String,
    /// Owning instance.
    pub instance_id: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Size of the stored state.
    pub data_size_bytes: u64,
}

/// Filters for [`handle_list_checkpoints`].
#[derive(Debug, Default)]
pub struct ListCheckpointsParams {
    /// Restrict to one checkpoint id.
    pub checkpoint_id: Option<String>,
    /// Lower bound on creation time.
    pub created_after: Option<chrono::DateTime<chrono::Utc>>,
    /// Upper bound on creation time.
    pub created_before: Option<chrono::DateTime<chrono::Utc>>,
    /// Page size.
    pub limit: i64,
    /// Page offset.
    pub offset: i64,
}

/// A page of checkpoints plus the unpaged total.
#[derive(Debug)]
pub struct ListCheckpointsResult {
    /// The page.
    pub checkpoints: Vec<CheckpointSummary>,
    /// Total matching the filter.
    pub total_count: i64,
}

/// List an instance's checkpoints.
pub async fn handle_list_checkpoints(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    params: &ListCheckpointsParams,
) -> Result<ListCheckpointsResult> {
    let checkpoints = state
        .persistence
        .list_checkpoints(
            instance_id,
            params.checkpoint_id.as_deref(),
            params.limit,
            params.offset,
            params.created_after,
            params.created_before,
        )
        .await?;

    let total_count = state
        .persistence
        .count_checkpoints(
            instance_id,
            params.checkpoint_id.as_deref(),
            params.created_after,
            params.created_before,
        )
        .await
        .unwrap_or(0);

    Ok(ListCheckpointsResult {
        checkpoints: checkpoints
            .into_iter()
            .map(|cp| CheckpointSummary {
                checkpoint_id: cp.checkpoint_id,
                instance_id: cp.instance_id,
                created_at: cp.created_at,
                data_size_bytes: cp.state.len() as u64,
            })
            .collect(),
        total_count,
    })
}

/// Event summary.
#[derive(Debug)]
pub struct EventSummary {
    /// Row id.
    pub id: i64,
    /// Owning instance.
    pub instance_id: String,
    /// Event type.
    pub event_type: String,
    /// Checkpoint the event belongs to, when it has one.
    pub checkpoint_id: Option<String>,
    /// Payload bytes exactly as the emitter wrote them.
    pub payload: Option<Vec<u8>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Event subtype.
    pub subtype: Option<String>,
}

/// A page of events plus the unpaged total.
#[derive(Debug)]
pub struct ListEventsResult {
    /// The page.
    pub events: Vec<EventSummary>,
    /// Total matching the filter.
    pub total_count: i64,
}

/// List an instance's events.
pub async fn handle_list_events(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    filter: &runtara_core::persistence::ListEventsFilter,
    limit: i64,
    offset: i64,
) -> Result<ListEventsResult> {
    let events = state
        .persistence
        .list_events(instance_id, filter, limit, offset)
        .await?;

    let total_count = state
        .persistence
        .count_events(instance_id, filter)
        .await
        .unwrap_or(0);

    Ok(ListEventsResult {
        events: events
            .into_iter()
            .map(|ev| EventSummary {
                id: ev.id.unwrap_or(0),
                instance_id: ev.instance_id,
                event_type: crate::core_types::event_name(ev.event_type).to_owned(),
                checkpoint_id: ev.checkpoint_id,
                payload: ev.payload,
                created_at: ev.created_at,
                subtype: ev.subtype,
            })
            .collect(),
        total_count,
    })
}

/// Step summary.
#[derive(Debug)]
pub struct StepSummary {
    /// Step id.
    pub step_id: String,
    /// Human-readable step name.
    pub step_name: Option<String>,
    /// Step type.
    pub step_type: String,
    /// Whether the step is still open, closed cleanly, or closed failing.
    pub status: PairedRecordStatus,
    /// Start time.
    pub started_at: DateTime<Utc>,
    /// Completion time.
    pub completed_at: Option<DateTime<Utc>>,
    /// Wall-clock duration.
    pub duration_ms: Option<i64>,
    /// Real launch/settle wall-clock (epoch ms) of a parallel branch's async
    /// work — present only for concurrent steps, so the timeline/replay render
    /// the true overlapping interval instead of the sequential assemble cascade.
    pub launched_at_ms: Option<i64>,
    /// See [`Self::launched_at_ms`].
    pub settled_at_ms: Option<i64>,
    /// Resolved step inputs.
    pub inputs: Option<Value>,
    /// Step outputs.
    pub outputs: Option<Value>,
    /// Failure detail.
    pub error: Option<Value>,
    /// Scope this step ran in.
    pub scope_id: Option<String>,
    /// Enclosing scope.
    pub parent_scope_id: Option<String>,
}

/// A page of step summaries plus the unpaged total.
#[derive(Debug)]
pub struct ListStepSummariesResult {
    /// The page.
    pub steps: Vec<StepSummary>,
    /// Total matching the filter.
    pub total_count: i64,
}

/// List an instance's per-step summaries.
pub async fn handle_list_step_summaries(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    filter: &runtara_core::persistence::ListPairedRecordsFilter,
    limit: i64,
    offset: i64,
) -> Result<ListStepSummariesResult> {
    if instance_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "instance_id is required".to_string(),
        ));
    }

    // Steps are this crate's concept, so this is where the kernel is told what
    // the guest's step events are called.
    let vocabulary = crate::step_vocabulary::workflow_steps();

    let steps = state
        .persistence
        .list_paired_records(instance_id, vocabulary, filter, limit, offset)
        .await?;

    let total_count = state
        .persistence
        .count_paired_records(instance_id, vocabulary, filter)
        .await
        .unwrap_or(0);

    Ok(ListStepSummariesResult {
        steps: steps
            .into_iter()
            .map(|step| StepSummary {
                status: step.status,
                step_id: step.correlation_id,
                step_name: step.label,
                step_type: step.kind,
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
        total_count,
    })
}

/// One scope in an ancestry chain.
#[derive(Debug)]
pub struct ScopeInfo {
    /// Scope id.
    pub scope_id: String,
    /// Enclosing scope, absent at the root.
    pub parent_scope_id: Option<String>,
    /// Step that opened the scope.
    pub step_id: String,
    /// Human-readable step name.
    pub step_name: Option<String>,
    /// Step type.
    pub step_type: String,
    /// Iteration index, for scopes opened per-iteration.
    pub index: Option<u32>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// Walk a scope's ancestry, innermost first.
///
/// Reconstructed from `scope_enter` events rather than stored directly. A scope
/// whose enter event is missing ends the walk instead of failing it — a partial
/// chain is more useful than none, and a truncated ancestry is what the caller
/// would have to handle anyway for an instance still running.
pub async fn handle_get_scope_ancestors(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    scope_id: &str,
) -> Result<Vec<ScopeInfo>> {
    use runtara_core::persistence::{EventSortOrder, ListEventsFilter};

    if instance_id.is_empty() || scope_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "instance_id and scope_id are required".to_string(),
        ));
    }

    let filter = ListEventsFilter {
        event_type: Some(runtara_core::domain::EventType::Custom),
        subtype: Some("scope_enter".to_string()),
        created_after: None,
        created_before: None,
        payload_contains: None,
        scope_id: None,
        parent_scope_id: None,
        root_scopes_only: false,
        sort_order: EventSortOrder::Asc,
    };

    let events = state
        .persistence
        .list_events(instance_id, &filter, 10000, 0)
        .await?;

    let mut scope_map: std::collections::HashMap<String, ScopeInfo> =
        std::collections::HashMap::new();

    for event in events {
        let Some(payload) = &event.payload else {
            continue;
        };
        let Ok(payload_json) = serde_json::from_slice::<Value>(payload) else {
            continue;
        };
        let Some(sid) = payload_json.get("scope_id").and_then(|v| v.as_str()) else {
            continue;
        };

        scope_map.insert(
            sid.to_string(),
            ScopeInfo {
                scope_id: sid.to_string(),
                parent_scope_id: payload_json
                    .get("parent_scope_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                step_id: payload_json
                    .get("step_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                step_name: payload_json
                    .get("step_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                step_type: payload_json
                    .get("step_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                index: payload_json
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .map(|i| i as u32),
                created_at: event.created_at,
            },
        );
    }

    let mut ancestors = Vec::new();
    let mut current = Some(scope_id.to_string());

    while let Some(sid) = current {
        // `remove` rather than `get`: a payload that names itself (or a cycle
        // through several scopes) would otherwise loop forever.
        if let Some(info) = scope_map.remove(&sid) {
            current = info.parent_scope_id.clone();
            ancestors.push(info);
        } else {
            break;
        }
    }

    Ok(ancestors)
}

/// Options for tenant metrics aggregation.
#[derive(Debug, Clone)]
pub struct TenantMetricsOptions {
    /// Tenant ID.
    pub tenant_id: String,
    /// Start of time range.
    pub start_time: DateTime<Utc>,
    /// End of time range.
    pub end_time: DateTime<Utc>,
    /// Bucket width in seconds.
    ///
    /// A plain number rather than an enum: the aggregation query only ever needs
    /// the width, and the named granularities callers speak in (`hourly`,
    /// `daily`) belong at the API boundary that has to parse them.
    pub bucket_seconds: u32,
}

/// Widest bucket count the aggregation query may be asked to build.
///
/// Mirrors `runtara_server`'s limit of the same name. Kept as its own constant
/// rather than shared because this crate does not depend on that one, and the
/// bound is a property of the query living here.
pub const MAX_METRIC_BUCKETS: i64 = 1_000;

/// Buckets a width produces over `[start, end]`, matching the query's spine.
///
/// The query floors the first bucket down to a multiple of the width, so a
/// range that starts mid-bucket still yields the partial bucket containing it.
/// Counting any other way would let a request slip past the cap and then build
/// more rows than the cap allows.
fn bucket_count(
    bucket_seconds: u32,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> i64 {
    let width = i64::from(bucket_seconds.max(1));
    let aligned_start = start.timestamp().div_euclid(width) * width;
    ((end.timestamp() - aligned_start).max(0) / width) + 1
}

/// One bucket of tenant execution metrics.
#[derive(Debug)]
pub struct MetricsBucket {
    /// Bucket start.
    pub bucket_time: DateTime<Utc>,
    /// Invocations started in the bucket.
    pub invocation_count: i64,
    /// Invocations that completed successfully.
    pub success_count: i64,
    /// Invocations that failed.
    pub failure_count: i64,
    /// Invocations that were cancelled.
    pub cancelled_count: i64,
    /// Mean duration.
    pub avg_duration_ms: Option<f64>,
    /// Fastest duration.
    pub min_duration_ms: Option<f64>,
    /// Slowest duration.
    pub max_duration_ms: Option<f64>,
    /// Mean peak memory.
    pub avg_memory_bytes: Option<i64>,
    /// Highest peak memory.
    pub max_memory_bytes: Option<i64>,
    /// Successes as a percentage of terminal invocations.
    pub success_rate_percent: Option<f64>,
}

/// Read a tenant's execution metrics, bucketed.
///
/// The success rate is computed over *terminal* invocations only, so runs still
/// in flight neither count against it nor inflate it; a bucket with nothing
/// terminal yet reports `None` rather than 0%.
pub async fn handle_get_tenant_metrics(
    state: &EnvironmentHandlerState,
    options: &TenantMetricsOptions,
) -> Result<Vec<MetricsBucket>> {
    if options.tenant_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "tenant_id is required".to_string(),
        ));
    }

    // Checked here as well as at the HTTP boundary because this crate is a
    // library and that boundary is not its only door. A zero width divides by
    // zero in the query, and an unbounded bucket count turns the empty-bucket
    // spine into the dominant cost of the whole aggregation.
    if options.bucket_seconds == 0 {
        return Err(crate::error::Error::InvalidRequest(
            "bucket_seconds must be at least 1".to_string(),
        ));
    }
    let buckets = bucket_count(options.bucket_seconds, options.start_time, options.end_time);
    if buckets > MAX_METRIC_BUCKETS {
        return Err(crate::error::Error::InvalidRequest(format!(
            "a {}s bucket width over this range would produce {} buckets; the maximum is {}",
            options.bucket_seconds, buckets, MAX_METRIC_BUCKETS
        )));
    }

    let bucket_rows = db::get_tenant_metrics(
        &state.pool,
        &options.tenant_id,
        options.start_time,
        options.end_time,
        options.bucket_seconds,
    )
    .await?;

    Ok(bucket_rows
        .into_iter()
        .map(|row| {
            let terminal_count = row.success_count + row.failure_count + row.cancelled_count;
            MetricsBucket {
                bucket_time: row.bucket_time,
                invocation_count: row.invocation_count,
                success_count: row.success_count,
                failure_count: row.failure_count,
                cancelled_count: row.cancelled_count,
                avg_duration_ms: row.avg_duration_ms,
                min_duration_ms: row.min_duration_ms,
                max_duration_ms: row.max_duration_ms,
                avg_memory_bytes: row.avg_memory_bytes.map(|v| v as i64),
                max_memory_bytes: row.max_memory_bytes,
                success_rate_percent: (terminal_count > 0)
                    .then(|| (row.success_count as f64 / terminal_count as f64) * 100.0),
            }
        })
        .collect())
}

/// Identity and metadata for an uploaded image.
#[derive(Debug)]
pub struct StoreImageParams {
    /// Owning tenant.
    pub tenant_id: String,
    /// Image name; unique per tenant, and re-uploading the same name replaces
    /// the artifact in place rather than creating a second row.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Free-form metadata.
    pub metadata: Option<Value>,
}

/// Why an image upload could not be stored.
///
/// Split by stage rather than collapsed into one error because the caller is
/// told something different by each: a name-claim or register failure is the
/// registry's, an I/O failure is the volume's.
#[derive(Debug)]
pub enum StoreImageError {
    /// Claiming an existing image name failed.
    Lookup(String),
    /// Creating the image directory or writing the artifact failed.
    Io(String),
    /// Writing the image row failed.
    Register(String),
}

impl std::fmt::Display for StoreImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lookup(message) | Self::Io(message) | Self::Register(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for StoreImageError {}

/// Write an uploaded image artifact to disk and register (or replace) its row.
///
/// The database claim comes before filesystem work so all concurrent writers
/// resolve the same image ID.  The binary is uploaded to a private temporary
/// file and renamed only after it is complete, so a launch never observes a
/// partially written artifact.
pub async fn handle_store_image(
    state: &EnvironmentHandlerState,
    params: StoreImageParams,
    binary: &[u8],
) -> std::result::Result<String, StoreImageError> {
    use std::io::Write;

    let image_registry = ImageRegistry::new(state.pool.clone());
    let candidate_image_id = uuid::Uuid::new_v4().to_string();
    let candidate_binary_path = state
        .data_dir
        .join("images")
        .join(&candidate_image_id)
        .join("binary");
    let name_claim = image_registry
        .claim_name(
            &params.tenant_id,
            &params.name,
            &candidate_image_id,
            &candidate_binary_path.to_string_lossy(),
        )
        .await
        .map_err(|e| StoreImageError::Lookup(format!("Failed to claim image name: {e}")))?;
    let image_id = name_claim.image_id;

    let images_dir = state.data_dir.join("images").join(&image_id);
    let binary_path = images_dir.join("binary");

    if let Err(e) = std::fs::create_dir_all(&images_dir) {
        error!(error = %e, "Failed to create image directory");
        return Err(StoreImageError::Io(format!(
            "Failed to create image directory: {}",
            e
        )));
    }

    let temporary_binary_path = images_dir.join(format!(".binary-upload-{}", uuid::Uuid::new_v4()));
    if let Err(e) = std::fs::File::create(&temporary_binary_path).and_then(|mut file| {
        file.write_all(binary)?;
        file.sync_all()
    }) {
        error!(error = %e, "Failed to write binary");
        let _ = std::fs::remove_file(&temporary_binary_path);
        return Err(StoreImageError::Io(format!(
            "Failed to write binary: {}",
            e
        )));
    }
    if let Err(e) = std::fs::rename(&temporary_binary_path, &binary_path) {
        error!(error = %e, "Failed to finalize binary upload");
        let _ = std::fs::remove_file(&temporary_binary_path);
        return Err(StoreImageError::Io(format!(
            "Failed to finalize binary upload: {}",
            e
        )));
    }

    let mut builder = ImageBuilder::new(
        &params.tenant_id,
        &params.name,
        binary_path.to_string_lossy(),
    )
    .image_id(&image_id);
    if let Some(desc) = &params.description {
        builder = builder.description(desc);
    }
    if let Some(meta) = params.metadata {
        builder = builder.metadata(meta);
    }

    let image = builder.build();

    if let Err(e) = image_registry.register(&image).await {
        return Err(StoreImageError::Register(format!(
            "Failed to register image: {}",
            e
        )));
    }

    info!(
        image_id = %image_id,
        claimed_new_name = name_claim.created,
        bytes = binary.len(),
        "Streaming image registration complete"
    );
    Ok(image_id)
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "db-integration-tests")]
    use crate::test_support;

    /// A suspended instance with the given `termination_reason` marker.
    #[cfg(feature = "db-integration-tests")]
    async fn suspended_instance(marker: Option<&str>) -> (Arc<dyn Persistence>, String) {
        let (persistence, instance_id) = test_support::running_instance("waker").await;
        let mut params =
            CompleteInstanceParams::new(&instance_id, CoreInstanceStatus::Suspended).if_running();
        if let Some(marker) = marker {
            params = params.with_termination(marker, None);
        }
        persistence
            .complete_instance(params)
            .await
            .expect("suspend");
        (persistence, instance_id)
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn waker_ignores_a_pause_shaped_suspend() {
        // A pause/breakpoint ack parks `suspended` with NO wake marker and its
        // pause signal already consumed — a custom signal must NOT relaunch it
        // (the replay would run straight past the pause).
        let (persistence, instance_id) = suspended_instance(None).await;
        wake_suspended_on_signal(persistence.as_ref(), &instance_id).await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(inst.status, CoreInstanceStatus::Suspended);
        assert!(
            inst.sleep_until.is_none(),
            "a custom signal must never schedule a wake for a paused instance"
        );
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn waker_stamps_sleep_for_an_on_signal_park() {
        let (persistence, instance_id) =
            suspended_instance(Some(crate::runner::embedded::WAITING_SIGNAL_TERMINATION)).await;
        wake_suspended_on_signal(persistence.as_ref(), &instance_id).await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert_eq!(inst.status, CoreInstanceStatus::Suspended);
        assert!(
            inst.sleep_until.is_some(),
            "an on-signal park must be scheduled for relaunch when its signal arrives"
        );
    }

    #[cfg(feature = "db-integration-tests")]
    #[tokio::test]
    async fn waker_ignores_a_timed_sleep_park() {
        // A store-freeing durable Delay parks with the `sleeping` marker and a
        // deadline; a custom signal must not fast-forward it.
        let (persistence, instance_id) = suspended_instance(Some("sleeping")).await;
        wake_suspended_on_signal(persistence.as_ref(), &instance_id).await;

        let inst = persistence
            .get_instance(&instance_id)
            .await
            .expect("get")
            .expect("instance exists");
        assert!(
            inst.sleep_until.is_none(),
            "a timed sleep is scheduler-woken at its deadline, not signal-woken"
        );
    }

    use super::*;
    use crate::image_registry::Image;
    use chrono::Utc;
    use serde_json::json;

    /// Handler state whose reads and writes all land in memory.
    ///
    /// `handle_send_signal` only ever touches `persistence`, so the pool is
    /// built lazily and never connected — the signal path needs no database.
    fn in_memory_state() -> (
        EnvironmentHandlerState,
        Arc<runtara_core::persistence::memory::InMemoryPersistence>,
    ) {
        let persistence = Arc::new(runtara_core::persistence::memory::InMemoryPersistence::new());
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://localhost:1/unused")
            .expect("a lazy pool never connects");
        let state = EnvironmentHandlerState::new(
            pool,
            persistence.clone(),
            Arc::new(crate::runner::MockRunner::new()),
            std::env::temp_dir(),
        );
        (state, persistence)
    }

    /// Every signal the storage layer can encode must also be one this handler
    /// accepts.
    ///
    /// `shutdown` is the one that regressed: the handler used to match only
    /// `cancel` and `pause`, so the graceful-drain path
    /// (`ShutdownCoordinator::drain_executions` via
    /// `RuntimeClient::signal_shutdown`) had its signal refused as an unknown
    /// type on every call, logged the refusal as a warning, and then waited out
    /// the whole grace period having asked no guest to checkpoint.
    #[tokio::test]
    async fn every_storable_signal_type_is_accepted() {
        use runtara_core::domain::SignalType;

        for signal_type in [SignalType::Cancel, SignalType::Pause, SignalType::Shutdown] {
            let name = runtara_store_postgres::encoding::signal_type_to_str(signal_type);
            let (state, persistence) = in_memory_state();
            let instance_id = format!("signal-{name}");
            persistence
                .register_instance(&instance_id, "tenant-1")
                .await
                .expect("register");
            persistence
                .update_instance_status(&instance_id, CoreInstanceStatus::Running, None)
                .await
                .expect("mark running");

            let outcome = handle_send_signal(&state, &instance_id, name, None)
                .await
                .expect("signal send must not error");

            assert_eq!(
                outcome,
                SendSignalOutcome::Delivered,
                "`{name}` is a signal the column can hold, so the handler must take it"
            );

            let stored = persistence
                .get_pending_signal(&instance_id)
                .await
                .expect("read back the signal")
                .expect("a delivered signal is pending");
            assert_eq!(stored.signal_type, signal_type, "stored as sent: {name}");
        }
    }

    /// A name the column cannot hold is still refused, rather than everything
    /// being waved through now that the parse is shared.
    #[tokio::test]
    async fn an_unstorable_signal_type_is_still_refused() {
        let (state, persistence) = in_memory_state();
        persistence
            .register_instance("signal-bogus", "tenant-1")
            .await
            .expect("register");
        persistence
            .update_instance_status("signal-bogus", CoreInstanceStatus::Running, None)
            .await
            .expect("mark running");

        let outcome = handle_send_signal(&state, "signal-bogus", "detonate", None)
            .await
            .expect("an unknown name is an outcome, not an error");

        assert_eq!(
            outcome,
            SendSignalOutcome::UnknownSignalType {
                signal_type: "detonate".to_string(),
            }
        );
    }

    /// An instant a handler reports must be the instant the store holds,
    /// down to the microsecond Postgres actually keeps.
    ///
    /// These fields used to cross as `created_at_ms: i64` built with
    /// `timestamp_millis()`, so every timestamp the server read had been
    /// silently rounded down to the millisecond. Nothing asserted on it, so
    /// nothing caught it. A time carrying microseconds is the cheapest way to
    /// pin that the truncation is gone and does not come back.
    #[tokio::test]
    async fn instants_survive_the_handler_without_losing_precision() {
        use chrono::TimeZone;
        use runtara_core::domain::EventType;
        use runtara_core::persistence::{EventRecord, ListEventsFilter};

        let (state, persistence) = in_memory_state();
        // 123_456_000 ns = 123.456 ms. Anything below the millisecond is what
        // the old `timestamp_millis()` hop discarded.
        let precise = Utc
            .timestamp_opt(1_700_000_000, 123_456_000)
            .single()
            .expect("a representable instant");
        assert_ne!(
            precise.timestamp_subsec_micros() % 1_000,
            0,
            "the fixture must carry sub-millisecond detail or it proves nothing"
        );

        persistence
            .insert_event(&EventRecord {
                id: None,
                instance_id: "precision-1".to_string(),
                event_type: EventType::Custom,
                checkpoint_id: None,
                payload: None,
                created_at: precise,
                subtype: None,
            })
            .await
            .expect("insert event");

        let page = handle_list_events(&state, "precision-1", &ListEventsFilter::default(), 10, 0)
            .await
            .expect("list events");

        assert_eq!(page.events.len(), 1);
        assert_eq!(
            page.events[0].created_at, precise,
            "the handler must report the stored instant, not a rounded copy"
        );
    }

    /// A stored body crosses as the bytes that were written, whatever they are.
    ///
    /// Bodies used to be base64-encoded here and decoded again by the server
    /// with `serde_json::from_slice(..).ok()`, so anything that was not JSON
    /// arrived as `None` — the same answer as "there was no body". Environment
    /// stores opaque bytes and has no business deciding they are JSON; whether
    /// they parse is the reader's question to ask, and now it can.
    #[tokio::test]
    async fn a_body_that_is_not_json_still_crosses_intact() {
        use runtara_core::domain::EventType;
        use runtara_core::persistence::{EventRecord, ListEventsFilter};

        let (state, persistence) = in_memory_state();
        let raw: Vec<u8> = vec![0x00, 0xff, b'n', b'o', b't', 0x80, b'{'];
        assert!(
            serde_json::from_slice::<serde_json::Value>(&raw).is_err(),
            "the fixture must not be JSON or it proves nothing"
        );

        persistence
            .insert_event(&EventRecord {
                id: None,
                instance_id: "opaque-1".to_string(),
                event_type: EventType::Custom,
                checkpoint_id: None,
                payload: Some(raw.clone()),
                created_at: Utc::now(),
                subtype: None,
            })
            .await
            .expect("insert event");

        let page = handle_list_events(&state, "opaque-1", &ListEventsFilter::default(), 10, 0)
            .await
            .expect("list events");

        assert_eq!(page.events.len(), 1);
        assert_eq!(
            page.events[0].payload.as_deref(),
            Some(raw.as_slice()),
            "the handler must hand back the bytes it was given"
        );
    }

    fn make_image(metadata: Option<serde_json::Value>) -> Image {
        Image {
            image_id: "img-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            name: "test-image".to_string(),
            description: None,
            binary_path: "/tmp/binary".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            metadata,
        }
    }

    #[test]
    fn enrich_input_merges_default_variables() {
        let input = json!({"data": {"key": "value"}});
        let image = make_image(Some(json!({
            "variables": {"color": "red", "size": 42}
        })));

        let result = enrich_input_for_storage(input, &image);

        assert_eq!(result["variables"]["color"], "red");
        assert_eq!(result["variables"]["size"], 42);
        assert_eq!(result["data"]["key"], "value");
    }

    #[test]
    fn enrich_input_does_not_override_explicit_variables() {
        let input = json!({
            "data": {},
            "variables": {"color": "blue"}
        });
        let image = make_image(Some(json!({
            "variables": {"color": "red", "size": 42}
        })));

        let result = enrich_input_for_storage(input, &image);

        assert_eq!(result["variables"]["color"], "blue");
        assert_eq!(result["variables"]["size"], 42);
    }

    #[test]
    fn enrich_input_strips_system_variables() {
        let input = json!({
            "data": {},
            "variables": {
                "user_var": "keep",
                "_workflow_id": "should-be-removed",
                "_scope_id": "should-be-removed",
                "_cache_key_prefix": "should-be-removed",
                "_loop_indices": [0, 1],
                "_parent_workflow_id": "should-be-removed"
            }
        });
        let image = make_image(None);

        let result = enrich_input_for_storage(input, &image);

        let vars = result["variables"].as_object().unwrap();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars["user_var"], "keep");
    }

    #[test]
    fn enrich_input_no_metadata() {
        let input = json!({"data": {"x": 1}});
        let image = make_image(None);

        let result = enrich_input_for_storage(input, &image);

        assert_eq!(result["data"]["x"], 1);
    }

    #[test]
    fn enrich_input_empty_input_with_defaults() {
        let input = json!({});
        let image = make_image(Some(json!({
            "variables": {"name": "default_name", "count": 10}
        })));

        let result = enrich_input_for_storage(input, &image);

        assert_eq!(result["variables"]["name"], "default_name");
        assert_eq!(result["variables"]["count"], 10);
    }

    #[test]
    fn enrich_input_filters_system_vars_from_defaults() {
        let input = json!({});
        let image = make_image(Some(json!({
            "variables": {"user_var": "ok", "_internal": "hidden"}
        })));

        let result = enrich_input_for_storage(input, &image);

        let vars = result["variables"].as_object().unwrap();
        assert_eq!(vars.len(), 1);
        assert_eq!(vars["user_var"], "ok");
    }
}
