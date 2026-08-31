// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Environment protocol handlers.
//!
//! Handles requests from Management SDK and proxies to Core when needed.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{debug, error, info, instrument, warn};

use runtara_core::persistence::{CompleteInstanceParams, Persistence};

use crate::container_registry::{ContainerInfo, ContainerRegistry};
use crate::db;
use crate::error::Result;
use crate::image_registry::{ImageBuilder, ImageRegistry};
use crate::runner::{LaunchOptions, Runner, RunnerHandle};

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
    /// All instance write operations are delegated to this shared persistence layer.
    pub persistence: Arc<dyn Persistence>,
    /// When the server started (for uptime calculation).
    pub start_time: std::time::Instant,
    /// Server version string.
    pub version: String,
    /// Runner for launching instances.
    pub runner: Arc<dyn Runner>,
    /// Address of runtara-core for instances to connect.
    pub core_addr: String,
    /// Data directory for images and instance I/O.
    pub data_dir: PathBuf,
    /// Request timeout for database operations.
    pub request_timeout: Duration,
    /// Drain signal observed by container monitors and workers.
    pub drain: DrainController,
}

/// Default request timeout for database operations (30 seconds).
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Fallback per-instance execution timeout when no value is persisted and the
/// caller supplies none (1 hour). Generous by design: the timeout is a safety
/// net for stuck guests, not the completion mechanism — workflows that finish
/// report completion immediately via the SDK. Override with
/// `RUNTARA_DEFAULT_INSTANCE_TIMEOUT_SECS`.
const FALLBACK_INSTANCE_TIMEOUT_SECS: u64 = 3600;

/// Resolve the default per-instance execution timeout, honoring
/// `RUNTARA_DEFAULT_INSTANCE_TIMEOUT_SECS` and falling back to
/// [`FALLBACK_INSTANCE_TIMEOUT_SECS`]. Used for first launch when the request
/// omits a timeout, and on wake/resume when no per-instance value was persisted.
pub fn default_instance_timeout() -> Duration {
    let secs = std::env::var("RUNTARA_DEFAULT_INSTANCE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(FALLBACK_INSTANCE_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

impl EnvironmentHandlerState {
    /// Create a new environment handler state.
    ///
    /// # Arguments
    ///
    /// * `pool` - PostgreSQL pool for Environment-specific queries (reads with JOINs)
    /// * `persistence` - Core persistence layer for all instance write operations
    /// * `runner` - Container runner for launching instances
    /// * `core_addr` - Address of runtara-core for instances to connect
    /// * `data_dir` - Data directory for images and instance I/O
    pub fn new(
        pool: PgPool,
        persistence: Arc<dyn Persistence>,
        runner: Arc<dyn Runner>,
        core_addr: String,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            pool,
            persistence,
            start_time: std::time::Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            runner,
            core_addr,
            data_dir: ensure_absolute_path(data_dir),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            drain: DrainController::new(),
        }
    }

    /// Set the request timeout for database operations.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Attach an externally-managed drain controller.
    pub fn with_drain(mut self, drain: DrainController) -> Self {
        self.drain = drain;
        self
    }

    /// Get the server uptime in milliseconds.
    pub fn uptime_ms(&self) -> i64 {
        self.start_time.elapsed().as_millis() as i64
    }
}

// ============================================================================
// Health Check
// ============================================================================

/// Handle health check request.
pub async fn handle_health_check(state: &EnvironmentHandlerState) -> Result<HealthCheckResponse> {
    let db_healthy = db::health_check(&state.pool).await.unwrap_or(false);

    Ok(HealthCheckResponse {
        healthy: db_healthy,
        version: state.version.clone(),
        uptime_ms: state.uptime_ms(),
    })
}

/// Health check response.
#[derive(Debug)]
pub struct HealthCheckResponse {
    /// Whether the server is healthy (database connected).
    pub healthy: bool,
    /// Server version.
    pub version: String,
    /// Server uptime in milliseconds.
    pub uptime_ms: i64,
}

// ============================================================================
// Image Registration
// ============================================================================

/// Request to register a new image.
pub struct RegisterImageRequest {
    /// Tenant ID for multi-tenancy isolation.
    pub tenant_id: String,
    /// Image name.
    pub name: String,
    /// Optional image description.
    pub description: Option<String>,
    /// Binary content of the image.
    pub binary: Vec<u8>,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// Response from image registration.
pub struct RegisterImageResponse {
    /// Whether registration succeeded.
    pub success: bool,
    /// Assigned image ID.
    pub image_id: String,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Handle image registration request.
#[instrument(skip(state, request), fields(
    tenant_id = %request.tenant_id,
    name = %request.name,
))]
pub async fn handle_register_image(
    state: &EnvironmentHandlerState,
    request: RegisterImageRequest,
) -> Result<RegisterImageResponse> {
    info!(
        tenant_id = %request.tenant_id,
        name = %request.name,
        binary_size = request.binary.len(),
        "Register image request received"
    );

    // Validate request
    if request.tenant_id.is_empty() {
        return Ok(RegisterImageResponse {
            success: false,
            image_id: String::new(),
            error: Some("tenant_id is required".to_string()),
        });
    }

    if request.name.is_empty() {
        return Ok(RegisterImageResponse {
            success: false,
            image_id: String::new(),
            error: Some("name is required".to_string()),
        });
    }

    if request.binary.is_empty() {
        return Ok(RegisterImageResponse {
            success: false,
            image_id: String::new(),
            error: Some("binary is required".to_string()),
        });
    }

    let image_registry = ImageRegistry::new(state.pool.clone());
    let existing_image = match image_registry
        .get_by_name(&request.tenant_id, &request.name)
        .await
    {
        Ok(image) => image,
        Err(e) => {
            error!(error = %e, "Failed to look up existing image");
            return Ok(RegisterImageResponse {
                success: false,
                image_id: String::new(),
                error: Some(format!("Failed to look up existing image: {}", e)),
            });
        }
    };
    let replacing_existing = existing_image.is_some();
    let image_id = existing_image
        .map(|image| image.image_id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Create directories
    let images_dir = state.data_dir.join("images").join(&image_id);
    let binary_path = images_dir.join("binary");

    if let Err(e) = std::fs::create_dir_all(&images_dir) {
        error!(error = %e, "Failed to create image directory");
        return Ok(RegisterImageResponse {
            success: false,
            image_id: String::new(),
            error: Some(format!("Failed to create image directory: {}", e)),
        });
    }

    // Write binary
    if let Err(e) = std::fs::write(&binary_path, &request.binary) {
        error!(error = %e, "Failed to write binary");
        return Ok(RegisterImageResponse {
            success: false,
            image_id: String::new(),
            error: Some(format!("Failed to write binary: {}", e)),
        });
    }

    // Build image
    let mut builder = ImageBuilder::new(
        &request.tenant_id,
        &request.name,
        binary_path.to_string_lossy(),
    );

    if let Some(desc) = &request.description {
        builder = builder.description(desc);
    }

    if let Some(meta) = request.metadata {
        builder = builder.metadata(meta);
    }

    let mut image = builder.build();
    image.image_id = image_id.clone();

    // Register in database
    if let Err(e) = image_registry.register(&image).await {
        error!(error = %e, "Failed to register image in database");
        if !replacing_existing {
            let _ = std::fs::remove_dir_all(&images_dir);
        }
        return Ok(RegisterImageResponse {
            success: false,
            image_id: String::new(),
            error: Some(format!("Failed to register image: {}", e)),
        });
    }

    info!(image_id = %image_id, "Image registered successfully");

    Ok(RegisterImageResponse {
        success: true,
        image_id,
        error: None,
    })
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
    /// Whether the instance was started.
    pub success: bool,
    /// Instance ID (assigned or generated).
    pub instance_id: String,
    /// Whether an earlier request had already reserved this exact instance.
    /// A deduplicated response never launches another process.
    pub deduplicated: bool,
    /// Error message if failed.
    pub error: Option<String>,
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
        return Ok(Some(StartInstanceResponse {
            success: false,
            instance_id: String::new(),
            deduplicated: false,
            error: Some(format!("Instance '{}' already exists", instance_id)),
        }));
    }

    match db::get_instance_image_id(&state.pool, instance_id).await? {
        Some(existing_image_id) if existing_image_id == image_id => {}
        Some(existing_image_id) => {
            warn!(
                instance_id,
                requested_image_id = image_id,
                existing_image_id,
                "Rejecting reuse of an instance ID for a different image"
            );
            return Ok(Some(StartInstanceResponse {
                success: false,
                instance_id: String::new(),
                deduplicated: false,
                error: Some(format!("Instance '{}' already exists", instance_id)),
            }));
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
            return Ok(Some(StartInstanceResponse {
                success: false,
                instance_id: String::new(),
                deduplicated: false,
                error: Some(format!("Instance '{}' already exists", instance_id)),
            }));
        }
    }

    info!(
        instance_id,
        tenant_id,
        image_id,
        status = %existing.status,
        "Instance start already accepted; returning deduplicated response"
    );
    Ok(Some(StartInstanceResponse {
        success: true,
        instance_id: instance_id.to_string(),
        deduplicated: true,
        error: None,
    }))
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
        return Ok(StartInstanceResponse {
            success: false,
            instance_id: String::new(),
            deduplicated: false,
            error: Some("image_id is required".to_string()),
        });
    }

    // Look up image
    let image_registry = ImageRegistry::new(state.pool.clone());
    let image = match image_registry.get(&request.image_id).await {
        Ok(Some(img)) => img,
        Ok(None) => {
            return Ok(StartInstanceResponse {
                success: false,
                instance_id: String::new(),
                deduplicated: false,
                error: Some(format!("Image '{}' not found", request.image_id)),
            });
        }
        Err(e) => {
            error!(error = %e, "Failed to look up image");
            return Ok(StartInstanceResponse {
                success: false,
                instance_id: String::new(),
                deduplicated: false,
                error: Some(format!("Database error: {}", e)),
            });
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
        return Ok(StartInstanceResponse {
            success: false,
            instance_id: String::new(),
            deduplicated: false,
            error: Some(format!("Image '{}' not found", request.image_id)),
        });
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
        return Ok(StartInstanceResponse {
            success: false,
            instance_id: String::new(),
            deduplicated: false,
            error: Some(format!("Image '{}' artifact not found", request.image_id)),
        });
    }

    // Parse input for runner
    let input = request.input.unwrap_or(serde_json::json!({}));

    // Enrich input for DB storage: merge variable defaults, strip system variables
    let input_for_storage = enrich_input_for_storage(input.clone(), &image);
    let input_bytes = serde_json::to_vec(&input_for_storage).ok();

    // Create instance record (with input and env for persistence across resume/wake)
    let env_for_db = if request.env.is_empty() {
        None
    } else {
        Some(&request.env)
    };

    // Create instance in Core's table via Persistence trait. This doubles as the
    // idempotency claim: ON CONFLICT DO NOTHING means a replay, or a concurrent
    // retry that got there first, comes back as `false` rather than an error.
    // The input rides along on that same statement rather than a follow-up
    // UPDATE, so a launch writes the instance row exactly once.
    let claimed = state
        .persistence
        .try_register_instance(&instance_id, &request.tenant_id, input_bytes.as_deref())
        .await;
    if !matches!(claimed, Ok(true)) {
        // Either the id was already taken or the insert failed outright. If
        // another request reserved the same compatible ID, it owns the launch
        // and this request is an idempotent replay.
        if let Some(response) =
            existing_start_response(state, &instance_id, &request.tenant_id, &request.image_id)
                .await?
        {
            return Ok(response);
        }
        let e = match claimed {
            Err(e) => e.to_string(),
            // Claim reported "already exists", but no usable instance backs it
            // up - `existing_start_response` already logged why it was rejected.
            Ok(_) => format!("Instance '{}' already exists", instance_id),
        };
        error!(error = %e, "Failed to register instance via Persistence");
        return Ok(StartInstanceResponse {
            success: false,
            instance_id: String::new(),
            deduplicated: false,
            error: Some(format!("Failed to create instance: {}", e)),
        });
    }

    // The claim above persisted the input, so hand those same bytes to the
    // runner rather than making the launch read back what it just wrote.
    let prepersisted_input = input_bytes.clone();

    // Resolve the effective execution timeout once, so the value persisted for
    // wake/resume matches the one the monitor enforces on this first run.
    let timeout = Duration::from_secs(
        request
            .timeout_seconds
            .unwrap_or(default_instance_timeout().as_secs()),
    );

    // Associate instance with image in Environment's table (Environment-specific data).
    // The timeout is persisted here so wake/resume can honor the same budget.
    if let Err(e) = db::associate_instance_image(
        &state.pool,
        &instance_id,
        &request.image_id,
        &request.tenant_id,
        env_for_db,
        Some(timeout.as_secs() as i64),
    )
    .await
    {
        error!(error = %e, "Failed to associate instance with image");
        return Ok(StartInstanceResponse {
            success: false,
            instance_id: String::new(),
            deduplicated: false,
            error: Some(format!("Failed to create instance: {}", e)),
        });
    }

    // Build launch options (using the shared image artifact)
    let options = LaunchOptions {
        instance_id: instance_id.clone(),
        tenant_id: request.tenant_id.clone(),
        wasm_path,
        input,
        timeout,
        checkpoint_id: None,
        env: request.env,
        prepersisted_input,
    };

    // Launch via runner (detached)
    match state.runner.launch_detached(&options).await {
        Ok(handle) => {
            info!(
                instance_id = %instance_id,
                handle_id = %handle.handle_id,
                "Instance launched successfully"
            );

            // Clone values for the registry before moving them
            let handle_id_for_registry = handle.handle_id.clone();

            // Register in container registry
            let container_registry = ContainerRegistry::new(state.pool.clone());
            let container_info = ContainerInfo {
                container_id: handle_id_for_registry,
                instance_id: instance_id.clone(),
                tenant_id: request.tenant_id,
                binary_path: image.binary_path,
                started_at: handle.started_at,
                timeout_seconds: Some(timeout.as_secs() as i64),
            };
            if let Err(e) = container_registry.register(&container_info).await {
                warn!(error = %e, "Failed to register container (instance still running)");
            }

            // The runner promotes the instance to `running` from inside the run
            // task, before the guest starts, on both the invoke and the legacy
            // branch. Stamping it again here would be a second write of the
            // same fact, and a racy one: `launch_detached` returns as soon as
            // the run is spawned, so a workflow that parks immediately can
            // already be `suspended` by the time this line is reached.

            // Spawn background task to monitor container and process output when done
            spawn_container_monitor(
                state.pool.clone(),
                state.runner.clone(),
                handle,
                state.persistence.clone(),
                timeout,
                state.drain.clone(),
            );

            Ok(StartInstanceResponse {
                success: true,
                instance_id,
                deduplicated: false,
                error: None,
            })
        }
        Err(e) => {
            error!(error = %e, "Failed to launch instance");
            let launch_error = format!("Launch failed: {}", e);
            let _ = state
                .persistence
                .complete_instance(
                    CompleteInstanceParams::new(&instance_id, "failed").with_error(&launch_error),
                )
                .await;

            Ok(StartInstanceResponse {
                success: false,
                instance_id,
                deduplicated: false,
                error: Some(launch_error),
            })
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

    // Build runner handle and stop
    let handle = RunnerHandle {
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
            "cancelled",
        ))
        .await;

    // Clean up container registry
    let _ = container_registry.cleanup(&request.instance_id).await;

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
    let instance = match db::get_instance(&state.pool, &request.instance_id).await? {
        Some(inst) => inst,
        None => {
            return Ok(ResumeInstanceResponse {
                success: false,
                error: Some(format!("Instance '{}' not found", request.instance_id)),
            });
        }
    };

    // Check status — allow resume from suspended, failed, or cancelled
    if !matches!(
        instance.status.as_str(),
        "suspended" | "failed" | "cancelled"
    ) {
        return Ok(ResumeInstanceResponse {
            success: false,
            error: Some(format!(
                "Cannot resume instance in '{}' state (must be suspended, failed, or cancelled)",
                instance.status
            )),
        });
    }

    // Get checkpoint ID from instance record, or look up the latest checkpoint
    let checkpoint_id = match instance.checkpoint_id {
        Some(id) => Some(id),
        None => {
            // Failed instances may not have checkpoint_id on the record if the crash
            // happened before the SDK could update it. Fall back to the latest
            // checkpoint stored in the checkpoints table.
            match runtara_core::persistence::postgres::load_latest_checkpoint(
                &state.pool,
                &request.instance_id,
            )
            .await
            {
                Ok(Some(record)) => {
                    info!(
                        instance_id = %request.instance_id,
                        checkpoint_id = %record.checkpoint_id,
                        "Found latest checkpoint for failed instance"
                    );
                    Some(record.checkpoint_id)
                }
                _ => {
                    // No checkpoint anywhere (e.g. suspended for shutdown while
                    // blocked in a non-durable step before the first checkpoint).
                    // The workflow model is replay-from-start with checkpoints as
                    // a result cache, so relaunching without one is valid.
                    info!(
                        instance_id = %request.instance_id,
                        "No checkpoint recorded; relaunching from the start"
                    );
                    None
                }
            }
        }
    };

    // Get image ID and stored env from instance_images table
    let (image_id, stored_env) =
        match db::get_instance_image_with_env(&state.pool, &request.instance_id).await? {
            Some(result) => result,
            None => {
                return Ok(ResumeInstanceResponse {
                    success: false,
                    error: Some("Instance has no associated image".to_string()),
                });
            }
        };

    let image_registry = ImageRegistry::new(state.pool.clone());
    let image = match image_registry.get(&image_id).await? {
        Some(img) => img,
        None => {
            return Ok(ResumeInstanceResponse {
                success: false,
                error: Some(format!("Image '{}' not found", image_id)),
            });
        }
    };

    if image.tenant_id != instance.tenant_id {
        warn!(
            image_id = %image_id,
            image_tenant = %image.tenant_id,
            instance_tenant = %instance.tenant_id,
            "Tenant mismatch when resuming instance"
        );
        // Return "not found" to avoid leaking existence
        return Ok(ResumeInstanceResponse {
            success: false,
            error: Some(format!("Image '{}' not found", image_id)),
        });
    }

    // Every image is wasm now, so always read binary directly.
    let wasm_path = PathBuf::from(&image.binary_path);

    // Honor the per-instance timeout persisted at first launch so a long replay
    // isn't force-killed by a hardcoded default; fall back to the configured
    // default for instances predating the persisted value.
    let timeout = db::get_instance_timeout_seconds(&state.pool, &request.instance_id)
        .await
        .ok()
        .flatten()
        .map(|s| Duration::from_secs(s as u64))
        .unwrap_or_else(default_instance_timeout);

    // Build launch options with checkpoint and restored env
    let options = LaunchOptions {
        instance_id: request.instance_id.clone(),
        tenant_id: instance.tenant_id.clone(),
        wasm_path,
        input: serde_json::json!({}), // Input was consumed on first run
        timeout,
        checkpoint_id: checkpoint_id.clone(),
        // A resume must re-read the stored envelope: the input on this
        // request is a relaunch placeholder, not the instance's real input.
        prepersisted_input: None,
        env: stored_env, // Restore env from initial launch
    };

    // Remove the old container registry entry BEFORE launching the new process.
    // This ensures any still-running old monitor will see its handle_id is gone
    // and skip crash detection, preventing a race where the old monitor marks the
    // instance as "failed" between launch and new container registration.
    {
        let container_registry = ContainerRegistry::new(state.pool.clone());
        let _ = container_registry.cleanup(&request.instance_id).await;
    }

    // Update status to "running" BEFORE launch so the WASM process can
    // immediately perform checkpoint lookups (the Core checkpoint handler
    // rejects requests from non-running instances).
    if let Err(e) = state
        .persistence
        .update_instance_status(&request.instance_id, "running", Some(chrono::Utc::now()))
        .await
    {
        warn!(error = %e, "Failed to update instance status to running before launch");
    }
    // Also update checkpoint_id on the instance record
    if let Some(cp_id) = checkpoint_id.as_deref()
        && let Err(e) = state
            .persistence
            .update_instance_checkpoint(&request.instance_id, cp_id)
            .await
    {
        warn!(error = %e, "Failed to update instance checkpoint before launch");
    }
    // Clear any pending wake so the wake scheduler doesn't relaunch an
    // instance we're resuming manually (shutdown-suspended instances carry
    // sleep_until = now for post-restart recovery).
    if let Err(e) = state
        .persistence
        .clear_instance_sleep(&request.instance_id)
        .await
    {
        warn!(error = %e, "Failed to clear sleep_until before resume");
    }

    // Launch
    match state.runner.launch_detached(&options).await {
        Ok(handle) => {
            info!(
                instance_id = %request.instance_id,
                handle_id = %handle.handle_id,
                checkpoint_id = ?checkpoint_id,
                "Instance resumed successfully"
            );

            // Clone values for the registry before moving them
            let handle_id_for_registry = handle.handle_id.clone();

            // Register in container registry
            let container_registry = ContainerRegistry::new(state.pool.clone());
            let container_info = ContainerInfo {
                container_id: handle_id_for_registry,
                instance_id: request.instance_id.clone(),
                tenant_id: instance.tenant_id,
                binary_path: image.binary_path,
                started_at: handle.started_at,
                timeout_seconds: Some(timeout.as_secs() as i64),
            };
            if let Err(e) = container_registry.register(&container_info).await {
                warn!(error = %e, "Failed to register container");
            }

            // Spawn background task to monitor container and process output when done
            spawn_container_monitor(
                state.pool.clone(),
                state.runner.clone(),
                handle,
                state.persistence.clone(),
                options.timeout,
                state.drain.clone(),
            );

            Ok(ResumeInstanceResponse {
                success: true,
                error: None,
            })
        }
        Err(e) => {
            error!(error = %e, "Failed to resume instance");
            Ok(ResumeInstanceResponse {
                success: false,
                error: Some(format!("Resume failed: {}", e)),
            })
        }
    }
}

// ============================================================================
// Container Monitor
// ============================================================================

/// True if this monitor's handle no longer owns the container registry
/// entry for the instance.
///
/// When an instance is resumed, a NEW monitor is spawned for the new process
/// and the registry is rewritten with that monitor's `handle_id`. The OLD
/// monitor (still polling the previous PID) must NOT touch instance state
/// when it observes its own process exit, otherwise it would clobber the
/// fresh execution.
///
/// Semantics (preserved from the inline check that previously lived in
/// `spawn_container_monitor`):
/// - registry has a different `container_id` than this monitor's handle → stale
/// - registry has no entry (e.g. cleared by resume before relaunch) → stale
/// - registry lookup errors → assume fresh, since being conservative here
///   would cause us to silently drop the crash-detection write on a transient
///   DB blip
pub async fn detect_stale_monitor(
    registry: &ContainerRegistry,
    instance_id: &str,
    monitor_handle_id: &str,
) -> bool {
    match registry.get(instance_id).await {
        Ok(Some(current)) => current.container_id != monitor_handle_id,
        Ok(None) => true,
        Err(_) => false,
    }
}

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
/// 3. Check whether this monitor still owns the instance (`detect_stale_monitor`)
///    — a resumed instance gets a new monitor, and the old one must not write
///    crash state for the previous PID.
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
#[allow(clippy::too_many_arguments)]
pub fn spawn_container_monitor(
    pool: PgPool,
    runner: Arc<dyn Runner>,
    handle: RunnerHandle,
    persistence: Arc<dyn Persistence>,
    timeout: Duration,
    drain: DrainController,
) {
    let instance_id = handle.instance_id.clone();

    tokio::spawn(async move {
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

                // Collect metrics and stderr from cgroup before container cleanup
                let (_output, stderr, metrics) = runner.collect_result(&handle).await;

                // Store metrics and pick up the status the SDK reported in the
                // same statement: this monitor needs both, and they are the
                // same row. Kept even when there are no metrics to write, so
                // the crash check below always has a status to look at.
                let observed_status = match persistence
                    .update_metrics_returning_status(
                        &instance_id,
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
                            .get_instance_meta(&instance_id)
                            .await
                            .map(|found| found.map(|i| (i.status, i.termination_reason)))
                    }
                };

                // Store stderr via Persistence trait for debugging (even if instance succeeds via Core)
                if let Some(ref stderr_content) = stderr {
                    if let Err(e) = persistence
                        .update_instance_stderr(&instance_id, stderr_content)
                        .await
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

                // Guard: check that this monitor is still the active one for this instance.
                // When an instance is resumed, a NEW monitor is spawned for the new process.
                // The OLD monitor (for the previous PID) may still be running and must not
                // interfere with the new execution. The check intentionally happens AFTER
                // metrics/stderr writes so a stale monitor doesn't drop diagnostic data
                // for the previous process.
                let is_stale_monitor =
                    detect_stale_monitor(&container_registry, &instance_id, &handle.handle_id)
                        .await;

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
                            if matches!(
                                status.as_str(),
                                "completed" | "failed" | "cancelled" | "suspended"
                            ) =>
                        {
                            // SDK already reported terminal status — normal termination
                            info!(
                                instance_id = %instance_id,
                                status = %status,
                                "Instance completed normally (SDK reported)"
                            );
                        }
                        _ => {
                            // Process died without a terminal SDK event. If the environment
                            // is draining, this is the expected force-kill path — mark the
                            // instance as `suspended + shutdown_requested` so restart-time
                            // heartbeat-monitor recovery treats it as a normal suspension
                            // rather than a crash.
                            let draining = drain.is_draining();
                            let (status, termination_reason, default_error) = if draining {
                                (
                                    "suspended",
                                    "shutdown_requested",
                                    "Process terminated during graceful shutdown",
                                )
                            } else {
                                ("failed", "crashed", "Process terminated without SDK event")
                            };

                            // A crash with no terminal SDK event (e.g. a guest trap such as
                            // the per-instance memory limit being exceeded) leaves its only
                            // diagnostic in the per-run stderr, where the host writes
                            // "workflow failed: <reason>". Fold that reason into the instance
                            // `error` so the failure is visible in the API rather than the
                            // generic "terminated without SDK event" — otherwise the step that
                            // was in flight surfaces as running with a null error.
                            let crash_error: String = match stderr.as_deref().map(str::trim) {
                                Some(reason) if !draining && !reason.is_empty() => {
                                    format!("{default_error}: {}", tail_chars(reason, 2000))
                                }
                                _ => default_error.to_string(),
                            };

                            let mut params = CompleteInstanceParams::new(&instance_id, status)
                                .if_running()
                                .with_termination(termination_reason, None)
                                .with_error(&crash_error);
                            if let Some(s) = stderr.as_deref() {
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
                                                .set_instance_sleep(
                                                    &instance_id,
                                                    chrono::Utc::now(),
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
                    }
                }

                // Clean up container registry
                let _ = container_registry.cleanup(&instance_id).await;
            }
            _ = tokio::time::sleep_until(sleep_until) => {
                warn!(
                    instance_id = %instance_id,
                    timeout_secs = %timeout.as_secs(),
                    "Execution timed out, killing container"
                );
                let _ = runner.stop(&handle).await;

                // Update instance status to failed with termination_reason = "timeout"
                if let Err(e) = persistence
                    .complete_instance(
                        CompleteInstanceParams::new(&instance_id, "failed")
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

                // Clean up container registry
                let _ = container_registry.cleanup(&instance_id).await;
            }
        }
    });
}

/// Response for listing agents.
pub struct ListAgentsResponse {
    /// JSON-encoded list of agents.
    pub agents_json: Vec<u8>,
}

/// Handle list agents request.
///
/// This returns metadata about all available agents and their capabilities.
pub async fn handle_list_agents(_state: &EnvironmentHandlerState) -> Result<ListAgentsResponse> {
    // The environment is no longer the agent-metadata authority. The agent
    // catalog now lives on runtara-server, sourced from the in-process
    // component dispatcher (component `meta.json`). This legacy endpoint is
    // deprecated.
    Err(crate::error::Error::Other(
        "Environment-side /api/v1/agents was removed. Use runtara-server's \
         GET /api/runtime/agents instead."
            .to_string(),
    ))
}

/// Request to get capability details.
pub struct GetCapabilityRequest {
    /// Agent module name.
    pub agent_id: String,
    /// Capability ID.
    pub capability_id: String,
}

/// Response for getting capability details.
pub struct GetCapabilityResponse {
    /// Whether the capability was found.
    pub found: bool,
    /// JSON-encoded capability info.
    pub capability_json: Vec<u8>,
    /// JSON-encoded input fields.
    pub inputs_json: Vec<u8>,
}

/// Handle get capability request.
///
/// This returns detailed information about a specific capability including its input schema.
#[instrument(skip(_state, request), fields(
    agent_id = %request.agent_id,
    capability_id = %request.capability_id,
))]
pub async fn handle_get_capability(
    _state: &EnvironmentHandlerState,
    request: GetCapabilityRequest,
) -> Result<GetCapabilityResponse> {
    // See `handle_list_agents`: agent + capability metadata now lives on
    // runtara-server (component dispatcher), not the environment's
    // statically-linked registry.
    Err(crate::error::Error::Other(
        "Environment-side /api/v1/agents/{agent}/capabilities/{capability} was \
         removed. Use runtara-server's \
         GET /api/runtime/agents/{name}/capabilities/{capability} instead."
            .to_string(),
    ))
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
// The response types are wire-shaped on purpose (`*_ms` timestamps, base64
// bodies): they are what the management protocol already promises, and keeping
// them identical is what makes this a move rather than a rewrite.
// ============================================================================

/// Normalize a raw instance status for the wire.
///
/// Unknown values pass through untouched rather than being coerced, so a status
/// added to the database but not yet to this list is visible instead of silently
/// becoming something else.
pub fn instance_status_to_string(status: &str) -> &str {
    status
}

/// Image summary as the management protocol reports it.
#[derive(Debug, Serialize)]
pub struct ImageSummary {
    /// Image id.
    pub image_id: String,
    /// Owning tenant.
    pub tenant_id: String,
    /// Image name.
    pub name: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: i64,
    /// Free-form metadata recorded at registration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Filters for [`handle_list_images`].
#[derive(Debug, Default)]
pub struct ListImagesParams {
    /// Restrict to one tenant.
    pub tenant_id: Option<String>,
    /// Exact name match; only meaningful together with `tenant_id`.
    pub name: Option<String>,
    /// Page size.
    pub limit: i64,
    /// Page offset.
    pub offset: i64,
}

/// List images, optionally scoped to a tenant and an exact name.
///
/// A `name` without a `tenant_id` is ignored rather than applied globally —
/// image names are unique per tenant, so a cross-tenant name lookup has no
/// well-defined answer.
pub async fn handle_list_images(
    state: &EnvironmentHandlerState,
    params: &ListImagesParams,
) -> Result<Vec<ImageSummary>> {
    let image_registry = ImageRegistry::new(state.pool.clone());

    let images = match (&params.tenant_id, &params.name) {
        (Some(tenant_id), Some(name)) => match image_registry.get_by_name(tenant_id, name).await? {
            Some(img) => vec![img],
            None => vec![],
        },
        (Some(tenant_id), None) => {
            image_registry
                .list_by_tenant(tenant_id, params.limit, params.offset)
                .await?
        }
        (None, _) => image_registry.list_all(params.limit, params.offset).await?,
    };

    Ok(images.into_iter().map(image_summary).collect())
}

/// Look up one image, enforcing tenant isolation when a tenant is supplied.
///
/// A hit that belongs to another tenant reads as `None`, not as a rejection:
/// telling a caller "this exists but is not yours" would leak the existence of
/// another tenant's image.
pub async fn handle_get_image(
    state: &EnvironmentHandlerState,
    image_id: &str,
    tenant_id: Option<&str>,
) -> Result<Option<ImageSummary>> {
    if image_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "image_id is required".to_string(),
        ));
    }

    let image_registry = ImageRegistry::new(state.pool.clone());
    let Some(img) = image_registry.get(image_id).await? else {
        return Ok(None);
    };

    if let Some(tenant_id) = tenant_id
        && img.tenant_id != tenant_id
    {
        return Ok(None);
    }

    Ok(Some(image_summary(img)))
}

/// Delete an image and its on-disk artifacts.
///
/// Returns `false` when the image does not exist, or exists under a different
/// tenant — the same conflation as [`handle_get_image`], for the same reason.
pub async fn handle_delete_image(
    state: &EnvironmentHandlerState,
    image_id: &str,
    tenant_id: Option<&str>,
) -> Result<bool> {
    if image_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "image_id is required".to_string(),
        ));
    }

    let image_registry = ImageRegistry::new(state.pool.clone());
    let Some(img) = image_registry.get(image_id).await? else {
        return Ok(false);
    };

    if let Some(tenant_id) = tenant_id
        && img.tenant_id != tenant_id
    {
        return Ok(false);
    }

    image_registry.delete(image_id).await?;

    // Best-effort: the row is already gone, and leaving a directory behind is
    // recoverable where failing the call after the delete committed is not.
    let images_dir = state.data_dir.join("images").join(image_id);
    let _ = std::fs::remove_dir_all(&images_dir);

    Ok(true)
}

fn image_summary(img: crate::image_registry::Image) -> ImageSummary {
    ImageSummary {
        image_id: img.image_id,
        tenant_id: img.tenant_id,
        name: img.name,
        description: img.description,
        created_at_ms: img.created_at.timestamp_millis(),
        metadata: img.metadata,
    }
}

/// Full instance state as the management protocol reports it.
#[derive(Debug, Serialize)]
pub struct InstanceStatusResponse {
    /// Whether the instance exists.
    pub found: bool,
    /// Instance id (echoed even when not found).
    pub instance_id: String,
    /// Lifecycle status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Owning tenant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    /// Image the instance was launched from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    /// Image name, resolved at read time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_name: Option<String>,
    /// Most recent checkpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Creation time, epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<i64>,
    /// First-run start time, epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    /// Terminal time, epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<i64>,
    /// Base64-encoded output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Base64-encoded input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Failure message, when the instance failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Captured guest stderr.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// Attempts used so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    /// Attempt ceiling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Peak guest linear memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_peak_bytes: Option<u64>,
    /// CPU time consumed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    /// Why the instance stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination_reason: Option<String>,
    /// Guest exit code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl InstanceStatusResponse {
    /// The "no such instance" answer, which is a 200 with `found: false` rather
    /// than an error: absence is a normal reply to a status poll.
    fn not_found(instance_id: String) -> Self {
        Self {
            found: false,
            instance_id,
            status: None,
            tenant_id: None,
            image_id: None,
            image_name: None,
            checkpoint_id: None,
            created_at_ms: None,
            started_at_ms: None,
            finished_at_ms: None,
            output: None,
            input: None,
            error: None,
            stderr: None,
            retry_count: None,
            max_retries: None,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
            termination_reason: None,
            exit_code: None,
        }
    }
}

/// Read one instance's full state.
pub async fn handle_get_instance_status(
    state: &EnvironmentHandlerState,
    instance_id: &str,
) -> Result<InstanceStatusResponse> {
    use base64::Engine;

    let Some(inst) = db::get_instance_full(&state.pool, instance_id).await? else {
        return Ok(InstanceStatusResponse::not_found(instance_id.to_string()));
    };

    Ok(InstanceStatusResponse {
        found: true,
        status: Some(instance_status_to_string(&inst.status).to_string()),
        tenant_id: Some(inst.tenant_id),
        instance_id: inst.instance_id,
        image_id: inst.image_id,
        image_name: inst.image_name,
        checkpoint_id: inst.checkpoint_id,
        created_at_ms: Some(inst.created_at.timestamp_millis()),
        started_at_ms: inst.started_at.map(|t| t.timestamp_millis()),
        finished_at_ms: inst.finished_at.map(|t| t.timestamp_millis()),
        output: inst
            .output
            .map(|o| base64::engine::general_purpose::STANDARD.encode(&o)),
        input: inst
            .input
            .map(|i| base64::engine::general_purpose::STANDARD.encode(&i)),
        error: inst.error,
        stderr: inst.stderr,
        retry_count: Some(inst.attempt as u32),
        max_retries: Some(inst.max_attempts as u32),
        memory_peak_bytes: inst.memory_peak_bytes.map(|v| v as u64),
        cpu_usage_usec: inst.cpu_usage_usec.map(|v| v as u64),
        termination_reason: inst.termination_reason,
        exit_code: inst.exit_code,
    })
}

/// Instance summary for list responses.
#[derive(Debug, Serialize)]
pub struct InstanceSummary {
    /// Instance id.
    pub instance_id: String,
    /// Owning tenant.
    pub tenant_id: String,
    /// Image the instance was launched from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    /// Lifecycle status.
    pub status: String,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: i64,
    /// First-run start time, epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    /// Terminal time, epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<i64>,
    /// Whether a failure message is recorded.
    pub has_error: bool,
}

/// A page of instances plus the unpaged total.
#[derive(Debug)]
pub struct ListInstancesResult {
    /// The page.
    pub instances: Vec<InstanceSummary>,
    /// Total matching the filter, ignoring limit/offset.
    pub total_count: i64,
}

/// List instances matching `options`.
///
/// A failing count degrades to `0` rather than failing the call: the page is
/// the answer the caller asked for, and losing it because a second query
/// stumbled would be the worse outcome.
pub async fn handle_list_instances(
    state: &EnvironmentHandlerState,
    options: &db::ListInstancesOptions,
) -> Result<ListInstancesResult> {
    let instances = db::list_instances(&state.pool, options).await?;

    let total_count = match db::count_instances(&state.pool, options).await {
        Ok(c) => c,
        Err(e) => {
            warn!("Count instances error: {}", e);
            0
        }
    };

    Ok(ListInstancesResult {
        instances: instances
            .into_iter()
            .map(|inst| InstanceSummary {
                instance_id: inst.instance_id,
                tenant_id: inst.tenant_id,
                image_id: inst.image_id,
                status: instance_status_to_string(&inst.status).to_string(),
                created_at_ms: inst.created_at.timestamp_millis(),
                started_at_ms: inst.started_at.map(|t| t.timestamp_millis()),
                finished_at_ms: inst.finished_at.map(|t| t.timestamp_millis()),
                has_error: inst.error.is_some(),
            })
            .collect(),
        total_count,
    })
}

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
    /// The signal type is not one of `cancel`, `pause`, `resume`.
    UnknownSignalType {
        /// What the caller asked for.
        signal_type: String,
    },
}

/// Send a lifecycle signal (`cancel`, `pause`, `resume`) to an instance.
pub async fn handle_send_signal(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    signal_type: &str,
    payload: Option<&str>,
) -> Result<SendSignalOutcome> {
    let Some(instance) = state.persistence.get_instance_meta(instance_id).await? else {
        return Ok(SendSignalOutcome::InstanceNotFound);
    };

    if !matches!(
        instance.status.as_str(),
        "running" | "suspended" | "pending"
    ) {
        return Ok(SendSignalOutcome::NotSignalable {
            status: instance.status,
        });
    }

    if !matches!(signal_type, "cancel" | "pause" | "resume") {
        return Ok(SendSignalOutcome::UnknownSignalType {
            signal_type: signal_type.to_string(),
        });
    }

    let payload = payload.map(|p| p.as_bytes().to_vec()).unwrap_or_default();
    state
        .persistence
        .insert_signal(instance_id, signal_type, &payload)
        .await?;

    Ok(SendSignalOutcome::Delivered)
}

/// What happened to a custom signal.
#[derive(Debug, PartialEq, Eq)]
pub enum SendCustomSignalOutcome {
    /// The signal was stored, and a parked instance woken if one was waiting.
    Delivered,
    /// No such instance.
    InstanceNotFound,
}

/// Send a custom (workflow-defined) signal addressed to one checkpoint.
pub async fn handle_send_custom_signal(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    checkpoint_id: &str,
    payload: Option<&str>,
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

    let payload = payload.map(|p| p.as_bytes().to_vec()).unwrap_or_default();
    state
        .persistence
        .insert_custom_signal(instance_id, checkpoint_id, &payload)
        .await?;

    wake_suspended_on_signal(state.persistence.as_ref(), instance_id).await;
    Ok(SendCustomSignalOutcome::Delivered)
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
            if inst.status == "suspended"
                && inst.termination_reason.as_deref()
                    == Some(crate::runner::embedded::WAITING_SIGNAL_TERMINATION) =>
        {
            if let Err(e) = persistence
                .set_instance_sleep(instance_id, chrono::Utc::now())
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
#[derive(Debug, Serialize)]
pub struct CheckpointSummary {
    /// Checkpoint id.
    pub checkpoint_id: String,
    /// Owning instance.
    pub instance_id: String,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: i64,
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
                created_at_ms: cp.created_at.timestamp_millis(),
                data_size_bytes: cp.state.len() as u64,
            })
            .collect(),
        total_count,
    })
}

/// Full checkpoint, state included.
#[derive(Debug, Serialize)]
pub struct CheckpointDetail {
    /// Whether the checkpoint exists.
    pub found: bool,
    /// Checkpoint id.
    pub checkpoint_id: String,
    /// Owning instance.
    pub instance_id: String,
    /// Creation time, epoch milliseconds; `0` when not found.
    pub created_at_ms: i64,
    /// Base64-encoded checkpoint state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// Load one checkpoint's stored state.
pub async fn handle_get_checkpoint(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    checkpoint_id: &str,
) -> Result<CheckpointDetail> {
    use base64::Engine;

    let Some(cp) = state
        .persistence
        .load_checkpoint(instance_id, checkpoint_id)
        .await?
    else {
        return Ok(CheckpointDetail {
            found: false,
            checkpoint_id: checkpoint_id.to_string(),
            instance_id: instance_id.to_string(),
            created_at_ms: 0,
            data: None,
        });
    };

    Ok(CheckpointDetail {
        found: true,
        created_at_ms: cp.created_at.timestamp_millis(),
        data: Some(base64::engine::general_purpose::STANDARD.encode(&cp.state)),
        checkpoint_id: cp.checkpoint_id,
        instance_id: cp.instance_id,
    })
}

/// Event summary.
#[derive(Debug, Serialize)]
pub struct EventSummary {
    /// Row id.
    pub id: i64,
    /// Owning instance.
    pub instance_id: String,
    /// Event type.
    pub event_type: String,
    /// Checkpoint the event belongs to, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Base64-encoded payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: i64,
    /// Event subtype.
    #[serde(skip_serializing_if = "Option::is_none")]
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
    use base64::Engine;

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
                event_type: ev.event_type,
                checkpoint_id: ev.checkpoint_id,
                payload: ev
                    .payload
                    .map(|p| base64::engine::general_purpose::STANDARD.encode(&p)),
                created_at_ms: ev.created_at.timestamp_millis(),
                subtype: ev.subtype,
            })
            .collect(),
        total_count,
    })
}

/// Step summary.
#[derive(Debug, Serialize)]
pub struct StepSummary {
    /// Step id.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
    /// Step type.
    pub step_type: String,
    /// `running`, `completed` or `failed`.
    pub status: String,
    /// Start time, epoch milliseconds.
    pub started_at_ms: i64,
    /// Completion time, epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<i64>,
    /// Wall-clock duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Real launch/settle wall-clock (epoch ms) of a parallel branch's async
    /// work — present only for concurrent steps, so the timeline/replay render
    /// the true overlapping interval instead of the sequential assemble cascade.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launched_at_ms: Option<i64>,
    /// See [`Self::launched_at_ms`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at_ms: Option<i64>,
    /// Resolved step inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Value>,
    /// Step outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Value>,
    /// Failure detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    /// Scope this step ran in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    /// Enclosing scope.
    #[serde(skip_serializing_if = "Option::is_none")]
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
    filter: &runtara_core::persistence::ListStepSummariesFilter,
    limit: i64,
    offset: i64,
) -> Result<ListStepSummariesResult> {
    use runtara_core::persistence::StepStatus;

    if instance_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "instance_id is required".to_string(),
        ));
    }

    let steps = state
        .persistence
        .list_step_summaries(instance_id, filter, limit, offset)
        .await?;

    let total_count = state
        .persistence
        .count_step_summaries(instance_id, filter)
        .await
        .unwrap_or(0);

    Ok(ListStepSummariesResult {
        steps: steps
            .into_iter()
            .map(|step| StepSummary {
                status: match step.status {
                    StepStatus::Running => "running",
                    StepStatus::Completed => "completed",
                    StepStatus::Failed => "failed",
                }
                .to_string(),
                step_id: step.step_id,
                step_name: step.step_name,
                step_type: step.step_type,
                started_at_ms: step.started_at.timestamp_millis(),
                completed_at_ms: step.completed_at.map(|t| t.timestamp_millis()),
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
#[derive(Debug, Serialize)]
pub struct ScopeInfo {
    /// Scope id.
    pub scope_id: String,
    /// Enclosing scope, absent at the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_scope_id: Option<String>,
    /// Step that opened the scope.
    pub step_id: String,
    /// Human-readable step name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
    /// Step type.
    pub step_type: String,
    /// Iteration index, for scopes opened per-iteration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// Creation time, epoch milliseconds.
    pub created_at_ms: i64,
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
        event_type: Some("scope_enter".to_string()),
        subtype: None,
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
                created_at_ms: event.created_at.timestamp_millis(),
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

/// One bucket of tenant execution metrics.
#[derive(Debug, Serialize)]
pub struct MetricsBucket {
    /// Bucket start, epoch milliseconds.
    pub bucket_time_ms: i64,
    /// Invocations started in the bucket.
    pub invocation_count: i64,
    /// Invocations that completed successfully.
    pub success_count: i64,
    /// Invocations that failed.
    pub failure_count: i64,
    /// Invocations that were cancelled.
    pub cancelled_count: i64,
    /// Mean duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_duration_ms: Option<f64>,
    /// Fastest duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_duration_ms: Option<f64>,
    /// Slowest duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<f64>,
    /// Mean peak memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_memory_bytes: Option<i64>,
    /// Highest peak memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_memory_bytes: Option<i64>,
    /// Successes as a percentage of terminal invocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_rate_percent: Option<f64>,
}

/// Read a tenant's execution metrics, bucketed.
///
/// The success rate is computed over *terminal* invocations only, so runs still
/// in flight neither count against it nor inflate it; a bucket with nothing
/// terminal yet reports `None` rather than 0%.
pub async fn handle_get_tenant_metrics(
    state: &EnvironmentHandlerState,
    options: &db::TenantMetricsOptions,
) -> Result<Vec<MetricsBucket>> {
    if options.tenant_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "tenant_id is required".to_string(),
        ));
    }

    let bucket_rows = db::get_tenant_metrics(&state.pool, options).await?;

    Ok(bucket_rows
        .into_iter()
        .map(|row| {
            let terminal_count = row.success_count + row.failure_count + row.cancelled_count;
            MetricsBucket {
                bucket_time_ms: row.bucket_time.timestamp_millis(),
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
/// told something different by each: a lookup or register failure is the
/// registry's, an I/O failure is the volume's.
#[derive(Debug)]
pub enum StoreImageError {
    /// Looking up an existing image with the same name failed.
    Lookup(String),
    /// Creating the image directory or writing the artifact failed.
    Io(String),
    /// Writing the image row failed.
    Register(String),
}

/// Write an uploaded image artifact to disk and register (or replace) its row.
///
/// On any failure after the directory is created, a *new* image's directory is
/// removed again — but a replacement's is left alone, because deleting it would
/// destroy the artifact the existing row still points at.
pub async fn handle_store_image(
    state: &EnvironmentHandlerState,
    params: StoreImageParams,
    binary: &[u8],
) -> std::result::Result<String, StoreImageError> {
    use std::io::Write;

    let image_registry = ImageRegistry::new(state.pool.clone());
    let existing_image = image_registry
        .get_by_name(&params.tenant_id, &params.name)
        .await
        .map_err(|e| StoreImageError::Lookup(format!("Failed to look up existing image: {}", e)))?;

    let replacing_existing = existing_image.is_some();
    let image_id = existing_image
        .map(|image| image.image_id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let images_dir = state.data_dir.join("images").join(&image_id);
    let binary_path = images_dir.join("binary");

    if let Err(e) = std::fs::create_dir_all(&images_dir) {
        error!(error = %e, "Failed to create image directory");
        return Err(StoreImageError::Io(format!(
            "Failed to create image directory: {}",
            e
        )));
    }

    let cleanup = || {
        if !replacing_existing {
            let _ = std::fs::remove_dir_all(&images_dir);
        }
    };

    if let Err(e) = std::fs::File::create(&binary_path).and_then(|mut f| f.write_all(binary)) {
        error!(error = %e, "Failed to write binary");
        cleanup();
        return Err(StoreImageError::Io(format!(
            "Failed to write binary: {}",
            e
        )));
    }

    let mut builder = ImageBuilder::new(
        &params.tenant_id,
        &params.name,
        binary_path.to_string_lossy(),
    );
    if let Some(desc) = &params.description {
        builder = builder.description(desc);
    }
    if let Some(meta) = params.metadata {
        builder = builder.metadata(meta);
    }

    let mut image = builder.build();
    image.image_id = image_id.clone();

    if let Err(e) = image_registry.register(&image).await {
        cleanup();
        return Err(StoreImageError::Register(format!(
            "Failed to register image: {}",
            e
        )));
    }

    info!(image_id = %image_id, bytes = binary.len(), "Streaming image registration complete");
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
        let mut params = CompleteInstanceParams::new(&instance_id, "suspended").if_running();
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
        assert_eq!(inst.status, "suspended");
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
        assert_eq!(inst.status, "suspended");
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
