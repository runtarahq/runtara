// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Environment protocol handlers.
//!
//! Handles requests from Management SDK and proxies to Core when needed.

use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use runtara_core::persistence::Persistence;

use crate::container_registry::{ContainerInfo, ContainerRegistry};
use crate::db;
use crate::error::Result;
use crate::image_registry::{ImageBuilder, ImageRegistry, RunnerType};
use crate::instance_output::{InstanceOutput, InstanceOutputStatus, output_file_path};
use crate::runner::oci::create_bundle_at_path;
use crate::runner::{LaunchOptions, Runner, RunnerHandle};

/// Convert a path to absolute if it's relative.
///
/// This is critical for paths stored in DB (like bundle_path) - they must be
/// absolute so the OCI runner can find them regardless of the current working
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
}

/// Default request timeout for database operations (30 seconds).
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
        }
    }

    /// Set the request timeout for database operations.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
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
    /// Runner type (OCI, Native, Wasm).
    pub runner_type: RunnerType,
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

    // Generate image ID
    let image_id = uuid::Uuid::new_v4().to_string();

    // Create directories
    let images_dir = state.data_dir.join("images").join(&image_id);
    let binary_path = images_dir.join("binary");
    let bundle_path = images_dir.join("bundle");

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

    // Make binary executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
        {
            warn!(error = %e, "Failed to set binary permissions");
        }
    }

    // Create OCI bundle if runner type is OCI
    let bundle_path_str = if request.runner_type == RunnerType::Oci {
        if let Err(e) = create_bundle_at_path(&bundle_path, &binary_path) {
            error!(error = %e, "Failed to create OCI bundle");
            let _ = std::fs::remove_dir_all(&images_dir);
            return Ok(RegisterImageResponse {
                success: false,
                image_id: String::new(),
                error: Some(format!("Failed to create OCI bundle: {}", e)),
            });
        }
        Some(bundle_path.to_string_lossy().to_string())
    } else {
        None
    };

    // Build image
    let mut builder = ImageBuilder::new(
        &request.tenant_id,
        &request.name,
        binary_path.to_string_lossy(),
    )
    .runner_type(request.runner_type);

    if let Some(desc) = &request.description {
        builder = builder.description(desc);
    }

    if let Some(bp) = &bundle_path_str {
        builder = builder.bundle_path(bp);
    }

    if let Some(meta) = request.metadata {
        builder = builder.metadata(meta);
    }

    let mut image = builder.build();
    image.image_id = image_id.clone();

    // Register in database
    let image_registry = ImageRegistry::new(state.pool.clone());
    if let Err(e) = image_registry.register(&image).await {
        error!(error = %e, "Failed to register image in database");
        let _ = std::fs::remove_dir_all(&images_dir);
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
    /// Error message if failed.
    pub error: Option<String>,
}

/// Enrich instance input for storage (display/audit purposes):
/// 1. Merge default variable values from image metadata (fill missing only)
/// 2. Strip system variables (prefixed with `_`)
///
/// This ensures the stored input reflects what the scenario actually receives,
/// while hiding internal runtime variables from API users.
pub fn enrich_input_for_storage(
    mut input: serde_json::Value,
    image: &crate::image_registry::Image,
) -> serde_json::Value {
    // Merge defaults from image metadata (if available)
    if let Some(ref metadata) = image.metadata {
        if let Some(default_vars) = metadata.get("variables").and_then(|v| v.as_object()) {
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
    }

    // Strip system variables (prefixed with _)
    if let Some(vars) = input.get_mut("variables").and_then(|v| v.as_object_mut()) {
        vars.retain(|key, _| !key.starts_with('_'));
    }

    input
}

/// Handle start instance request.
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
                error: Some(format!("Image '{}' not found", request.image_id)),
            });
        }
        Err(e) => {
            error!(error = %e, "Failed to look up image");
            return Ok(StartInstanceResponse {
                success: false,
                instance_id: String::new(),
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
            error: Some(format!("Image '{}' not found", request.image_id)),
        });
    }

    // Ensure bundle exists
    let bundle_path = match &image.bundle_path {
        Some(path) => PathBuf::from(path),
        None => {
            error!(image_id = %request.image_id, "Image has no bundle path");
            return Ok(StartInstanceResponse {
                success: false,
                instance_id: String::new(),
                error: Some(format!("Image '{}' has no bundle", request.image_id)),
            });
        }
    };

    // Generate or use provided instance ID
    let instance_id = request
        .instance_id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

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

    // Create instance in Core's table via Persistence trait
    if let Err(e) = state
        .persistence
        .register_instance(&instance_id, &request.tenant_id)
        .await
    {
        error!(error = %e, "Failed to register instance via Persistence");
        return Ok(StartInstanceResponse {
            success: false,
            instance_id: String::new(),
            error: Some(format!("Failed to create instance: {}", e)),
        });
    }

    // Store input data via Persistence trait
    if let Some(ref input_data) = input_bytes
        && let Err(e) = state
            .persistence
            .store_instance_input(&instance_id, input_data)
            .await
    {
        warn!(error = %e, "Failed to store instance input (non-fatal)");
    }

    // Associate instance with image in Environment's table (Environment-specific data)
    if let Err(e) = db::associate_instance_image(
        &state.pool,
        &instance_id,
        &request.image_id,
        &request.tenant_id,
        env_for_db,
    )
    .await
    {
        error!(error = %e, "Failed to associate instance with image");
        return Ok(StartInstanceResponse {
            success: false,
            instance_id: String::new(),
            error: Some(format!("Failed to create instance: {}", e)),
        });
    }

    // Build launch options (using the shared image bundle)
    let timeout = Duration::from_secs(request.timeout_seconds.unwrap_or(300));
    let options = LaunchOptions {
        instance_id: instance_id.clone(),
        tenant_id: request.tenant_id.clone(),
        bundle_path,
        input,
        timeout,
        runtara_core_addr: state.core_addr.clone(),
        checkpoint_id: None,
        env: request.env,
    };

    // Launch via runner (detached)
    match state.runner.launch_detached(&options).await {
        Ok(handle) => {
            info!(
                instance_id = %instance_id,
                handle_id = %handle.handle_id,
                "Instance launched successfully"
            );

            // Clone values for monitor before moving them
            let tenant_id_for_monitor = request.tenant_id.clone();
            let handle_id_for_registry = handle.handle_id.clone();

            // Use the PID captured at spawn time (more reliable than querying crun state)
            let pid = handle.spawned_pid.map(|p| p as i32);
            if pid.is_some() {
                debug!(
                    instance_id = %instance_id,
                    pid = ?pid,
                    "Using spawned process PID for monitoring"
                );
            }

            // Register in container registry (with PID if available)
            let container_registry = ContainerRegistry::new(state.pool.clone());
            let container_info = ContainerInfo {
                container_id: handle_id_for_registry,
                instance_id: instance_id.clone(),
                tenant_id: request.tenant_id,
                binary_path: image.binary_path,
                bundle_path: image.bundle_path,
                started_at: handle.started_at,
                pid,
                timeout_seconds: Some(timeout.as_secs() as i64),
                process_killed: false,
            };
            if let Err(e) = container_registry.register(&container_info).await {
                warn!(error = %e, "Failed to register container (instance still running)");
            }

            // Update instance status to running via Persistence trait
            if let Err(e) = state
                .persistence
                .update_instance_status(&instance_id, "running", Some(chrono::Utc::now()))
                .await
            {
                error!(
                    error = %e,
                    instance_id = %instance_id,
                    "Failed to update instance status to running (instance launched but status may be incorrect)"
                );
            }

            // Spawn background task to monitor container and process output when done
            spawn_container_monitor(
                state.pool.clone(),
                state.runner.clone(),
                handle,
                tenant_id_for_monitor,
                state.data_dir.clone(),
                state.persistence.clone(),
                timeout,
                pid,
            );

            Ok(StartInstanceResponse {
                success: true,
                instance_id,
                error: None,
            })
        }
        Err(e) => {
            error!(error = %e, "Failed to launch instance");
            let _ = state
                .persistence
                .complete_instance(&instance_id, None, Some(&format!("Launch failed: {}", e)))
                .await;

            Ok(StartInstanceResponse {
                success: false,
                instance_id,
                error: Some(format!("Launch failed: {}", e)),
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

    // Request cancellation
    let grace_period = Duration::from_secs(request.grace_period_seconds.max(1));
    if let Err(e) = container_registry
        .request_cancellation(&request.instance_id, grace_period, &request.reason)
        .await
    {
        warn!(error = %e, "Failed to write cancellation token");
    }

    // Build runner handle and stop
    let handle = RunnerHandle {
        handle_id: container.container_id,
        instance_id: request.instance_id.clone(),
        tenant_id: container.tenant_id,
        started_at: container.started_at,
        spawned_pid: container.pid.map(|p| p as u32),
    };

    if let Err(e) = state.runner.stop(&handle).await {
        warn!(error = %e, "Runner stop returned error");
    }

    // Update instance status to cancelled via Persistence trait
    let _ = state
        .persistence
        .complete_instance_extended(&request.instance_id, "cancelled", None, None, None, None)
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

    // Check status
    if instance.status != "suspended" {
        return Ok(ResumeInstanceResponse {
            success: false,
            error: Some(format!(
                "Cannot resume instance in '{}' state (must be suspended)",
                instance.status
            )),
        });
    }

    // Get checkpoint ID
    let checkpoint_id = match instance.checkpoint_id {
        Some(id) => id,
        None => {
            return Ok(ResumeInstanceResponse {
                success: false,
                error: Some("Instance has no checkpoint to resume from".to_string()),
            });
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

    // Ensure bundle exists
    let bundle_path = match &image.bundle_path {
        Some(path) => PathBuf::from(path),
        None => {
            error!(image_id = %image_id, "Image has no bundle path");
            return Ok(ResumeInstanceResponse {
                success: false,
                error: Some(format!("Image '{}' has no bundle", image_id)),
            });
        }
    };

    // Build launch options with checkpoint and restored env
    let options = LaunchOptions {
        instance_id: request.instance_id.clone(),
        tenant_id: instance.tenant_id.clone(),
        bundle_path,
        input: serde_json::json!({}), // Input was consumed on first run
        timeout: Duration::from_secs(300),
        runtara_core_addr: state.core_addr.clone(),
        checkpoint_id: Some(checkpoint_id.clone()),
        env: stored_env, // Restore env from initial launch
    };

    // Launch
    match state.runner.launch_detached(&options).await {
        Ok(handle) => {
            info!(
                instance_id = %request.instance_id,
                handle_id = %handle.handle_id,
                checkpoint_id = %checkpoint_id,
                "Instance resumed successfully"
            );

            // Clone values for monitor before moving them
            let tenant_id_for_monitor = instance.tenant_id.clone();
            let handle_id_for_registry = handle.handle_id.clone();

            // Get PID from runner (for PID-based termination detection)
            let pid = state.runner.get_pid(&handle).await.map(|p| p as i32);
            if pid.is_some() {
                debug!(
                    instance_id = %request.instance_id,
                    pid = ?pid,
                    "Captured container PID for monitoring (resume)"
                );
            }

            // Register in container registry (with PID if available)
            let container_registry = ContainerRegistry::new(state.pool.clone());
            let container_info = ContainerInfo {
                container_id: handle_id_for_registry,
                instance_id: request.instance_id.clone(),
                tenant_id: instance.tenant_id,
                binary_path: image.binary_path,
                bundle_path: image.bundle_path,
                started_at: handle.started_at,
                pid,
                timeout_seconds: Some(300),
                process_killed: false,
            };
            if let Err(e) = container_registry.register(&container_info).await {
                warn!(error = %e, "Failed to register container");
            }

            // Update status via Persistence trait
            if let Err(e) = state
                .persistence
                .update_instance_status(&request.instance_id, "running", Some(chrono::Utc::now()))
                .await
            {
                warn!(error = %e, "Failed to update instance status to running");
            }
            // Also update checkpoint_id
            if let Err(e) = state
                .persistence
                .update_instance_checkpoint(&request.instance_id, &checkpoint_id)
                .await
            {
                warn!(error = %e, "Failed to update instance checkpoint");
            }

            // Spawn background task to monitor container and process output when done
            spawn_container_monitor(
                state.pool.clone(),
                state.runner.clone(),
                handle,
                tenant_id_for_monitor,
                state.data_dir.clone(),
                state.persistence.clone(),
                options.timeout,
                pid,
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

/// Check if a process is alive by checking /proc/<pid> existence.
fn is_process_alive(pid: i32) -> bool {
    std::path::Path::new(&format!("/proc/{}", pid)).exists()
}

/// Spawn a background task that monitors the container and processes output when done.
///
/// This function should be called after launching an instance to monitor its lifecycle
/// and process output when the container finishes. The timeout is enforced here - if the
/// container runs longer than the specified timeout, it will be killed.
///
/// ## PID-based Monitoring
///
/// When a PID is provided, we use `/proc/<pid>` to detect process termination.
/// This is more reliable than querying crun state. When the process terminates:
///
/// - If Core already has a terminal status (completed/failed/cancelled/suspended) → normal exit
/// - If Core still shows "running" → process crashed without sending SDK event
///
/// Falls back to `runner.is_running()` if no PID is available.
#[allow(clippy::too_many_arguments)]
pub fn spawn_container_monitor(
    pool: PgPool,
    runner: Arc<dyn Runner>,
    handle: RunnerHandle,
    _tenant_id: String,
    _data_dir: PathBuf,
    persistence: Arc<dyn Persistence>,
    timeout: Duration,
    pid: Option<i32>,
) {
    let instance_id = handle.instance_id.clone();

    tokio::spawn(async move {
        // Brief initial delay to let the process start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Poll to check if container is still running
        let poll_interval = Duration::from_millis(50);
        let start = std::time::Instant::now();

        // Note: pid comes from child.id() at spawn time, so it's reliable.
        // If pid is None (shouldn't happen normally), fall back to runner.is_running().

        loop {
            // Check timeout first
            if start.elapsed() > timeout {
                warn!(
                    instance_id = %instance_id,
                    timeout_secs = %timeout.as_secs(),
                    "Execution timed out, killing container"
                );
                let _ = runner.stop(&handle).await;

                // Update instance status to failed with termination_reason = "timeout"
                if let Err(e) = persistence
                    .complete_instance_with_termination_if_running(
                        &instance_id,
                        "failed",
                        Some("timeout"),             // termination_reason
                        None,                        // exit_code
                        None,                        // output
                        Some("Execution timed out"), // error
                        None,                        // stderr
                        None,                        // checkpoint_id
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
                let container_registry = ContainerRegistry::new(pool.clone());
                let _ = container_registry.cleanup(&instance_id).await;

                return;
            }

            // Check if process is still running
            // Use PID-based checking if available (faster and more reliable)
            let is_alive = if let Some(p) = pid {
                is_process_alive(p)
            } else {
                runner.is_running(&handle).await
            };

            if !is_alive {
                info!(
                    instance_id = %instance_id,
                    pid = ?pid,
                    "Process terminated, checking Core status"
                );

                // Collect metrics and stderr from cgroup before container cleanup
                let (_output, stderr, metrics) = runner.collect_result(&handle).await;

                // Store metrics via Persistence trait
                if metrics.memory_peak_bytes.is_some() || metrics.cpu_usage_usec.is_some() {
                    if let Err(e) = persistence
                        .update_instance_metrics(
                            &instance_id,
                            metrics.memory_peak_bytes,
                            metrics.cpu_usage_usec,
                        )
                        .await
                    {
                        warn!(
                            instance_id = %instance_id,
                            error = %e,
                            "Failed to store container metrics"
                        );
                    } else {
                        debug!(
                            instance_id = %instance_id,
                            memory_peak_bytes = ?metrics.memory_peak_bytes,
                            cpu_usage_usec = ?metrics.cpu_usage_usec,
                            "Stored container metrics"
                        );
                    }
                }

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

                // Check Core status to determine if this is a crash or normal termination
                // This replaces the output.json processing - SDK events are the single source of truth
                match db::get_instance(&pool, &instance_id).await {
                    Ok(Some(inst)) => {
                        let status = inst.status.as_str();
                        if matches!(status, "completed" | "failed" | "cancelled" | "suspended") {
                            // Core already has terminal status - normal termination via SDK
                            info!(
                                instance_id = %instance_id,
                                status = %status,
                                "Instance completed normally (SDK reported)"
                            );
                        } else {
                            // Core still shows "running" but PID is gone - process crashed
                            // without sending SDK completion event
                            warn!(
                                instance_id = %instance_id,
                                status = %status,
                                pid = ?pid,
                                "Process terminated without SDK event - marking as crashed"
                            );
                            if let Err(e) = persistence
                                .complete_instance_with_termination_if_running(
                                    &instance_id,
                                    "failed",
                                    Some("crashed"), // termination_reason
                                    None,            // exit_code
                                    None,            // output
                                    Some("Process terminated without SDK event"), // error
                                    stderr.as_deref(), // stderr
                                    None,            // checkpoint_id
                                )
                                .await
                            {
                                error!(
                                    instance_id = %instance_id,
                                    error = %e,
                                    "Failed to mark instance as crashed"
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        warn!(
                            instance_id = %instance_id,
                            "Instance not found in database after termination"
                        );
                    }
                    Err(e) => {
                        error!(
                            instance_id = %instance_id,
                            error = %e,
                            "Failed to check instance status after termination"
                        );
                    }
                }

                // Clean up container registry
                let container_registry = ContainerRegistry::new(pool.clone());
                let _ = container_registry.cleanup(&instance_id).await;

                break;
            }

            // Sleep before next check
            tokio::time::sleep(poll_interval).await;
        }
    });
}

// ============================================================================
// Agent Testing
// ============================================================================

/// Request to test a capability.
pub struct TestCapabilityRequest {
    /// Tenant ID for isolation.
    pub tenant_id: String,
    /// Agent module name (e.g., "http", "utils", "transform").
    pub agent_id: String,
    /// Capability ID (e.g., "http-request", "random-double").
    pub capability_id: String,
    /// Capability input as JSON.
    pub input: serde_json::Value,
    /// Optional connection credentials.
    pub connection: Option<serde_json::Value>,
    /// Execution timeout in milliseconds (default: 30000).
    pub timeout_ms: Option<u32>,
}

/// Response from testing a capability.
pub struct TestCapabilityResponse {
    /// Whether the test succeeded.
    pub success: bool,
    /// Output value on success (JSON).
    pub output: Option<serde_json::Value>,
    /// Error message on failure.
    pub error: Option<String>,
    /// Execution time in milliseconds.
    pub execution_time_ms: u64,
}

/// Handle test capability request.
///
/// This runs the test harness binary in an OCI container, passing the test request
/// via /data/input.json and reading the result from /data/output.json.
pub async fn handle_test_capability(
    state: &EnvironmentHandlerState,
    request: TestCapabilityRequest,
) -> Result<TestCapabilityResponse> {
    let start = std::time::Instant::now();

    info!(
        tenant_id = %request.tenant_id,
        agent_id = %request.agent_id,
        capability_id = %request.capability_id,
        "Test capability request received"
    );

    // Get or register the test harness image
    let image_registry = ImageRegistry::new(state.pool.clone());
    let test_harness_image =
        match get_or_register_test_harness(&image_registry, &state.data_dir).await {
            Ok(image) => image,
            Err(e) => {
                error!(error = %e, "Failed to get test harness image");
                return Ok(TestCapabilityResponse {
                    success: false,
                    output: None,
                    error: Some(format!("Test harness not available: {}", e)),
                    execution_time_ms: start.elapsed().as_millis() as u64,
                });
            }
        };

    // Build test request JSON for input.json
    let test_input = serde_json::json!({
        "agent_id": request.agent_id,
        "capability_id": request.capability_id,
        "input": request.input,
        "connection": request.connection,
    });

    // Ensure bundle exists
    let bundle_path = match &test_harness_image.bundle_path {
        Some(path) => PathBuf::from(path),
        None => {
            error!("Test harness image has no bundle path");
            return Ok(TestCapabilityResponse {
                success: false,
                output: None,
                error: Some("Test harness image has no bundle".to_string()),
                execution_time_ms: start.elapsed().as_millis() as u64,
            });
        }
    };

    // Generate a unique instance ID for this test
    let instance_id = format!("test-{}", uuid::Uuid::new_v4());
    let timeout = Duration::from_millis(request.timeout_ms.unwrap_or(30000) as u64);

    // Build launch options
    let options = LaunchOptions {
        instance_id: instance_id.clone(),
        tenant_id: request.tenant_id.clone(),
        bundle_path,
        input: test_input,
        timeout,
        runtara_core_addr: state.core_addr.clone(),
        checkpoint_id: None,
        env: std::collections::HashMap::new(), // Test harness doesn't need custom env
    };

    // Run synchronously (wait for completion)
    let launch_result = state.runner.run(&options, None).await;

    let execution_time_ms = start.elapsed().as_millis() as u64;

    match launch_result {
        Ok(_) => {
            // Read output from output.json
            let output_path = output_file_path(&state.data_dir, &request.tenant_id, &instance_id);
            match InstanceOutput::read_from_file(&output_path).await {
                Ok(output) => match output.status {
                    InstanceOutputStatus::Completed => Ok(TestCapabilityResponse {
                        success: true,
                        output: output.result,
                        error: None,
                        execution_time_ms,
                    }),
                    InstanceOutputStatus::Failed => Ok(TestCapabilityResponse {
                        success: false,
                        output: None,
                        error: output
                            .error
                            .or(Some("Capability execution failed".to_string())),
                        execution_time_ms,
                    }),
                    _ => Ok(TestCapabilityResponse {
                        success: false,
                        output: None,
                        error: Some(format!("Unexpected status: {:?}", output.status)),
                        execution_time_ms,
                    }),
                },
                Err(e) => {
                    warn!(error = %e, "Failed to read test output");
                    Ok(TestCapabilityResponse {
                        success: false,
                        output: None,
                        error: Some(format!("Failed to read test output: {}", e)),
                        execution_time_ms,
                    })
                }
            }
        }
        Err(e) => {
            error!(error = %e, "Test harness execution failed");
            Ok(TestCapabilityResponse {
                success: false,
                output: None,
                error: Some(format!("Execution failed: {}", e)),
                execution_time_ms,
            })
        }
    }
}

/// Get the test harness image, or register it if not present.
async fn get_or_register_test_harness(
    image_registry: &ImageRegistry,
    data_dir: &std::path::Path,
) -> std::result::Result<crate::image_registry::Image, String> {
    // System tenant for test harness
    const SYSTEM_TENANT: &str = "__system__";
    const TEST_HARNESS_NAME: &str = "test-harness";

    // Check if already registered
    if let Ok(Some(image)) = image_registry
        .get_by_name(SYSTEM_TENANT, TEST_HARNESS_NAME)
        .await
    {
        return Ok(image);
    }

    // Look for pre-compiled test harness binary
    // Expected locations (in order of preference):
    // 1. $DATA_DIR/test-harness/binary
    // 2. /usr/share/runtara/test-harness
    let possible_paths = [
        data_dir.join("test-harness").join("binary"),
        PathBuf::from("/usr/share/runtara/test-harness"),
    ];

    let binary_path = possible_paths.iter().find(|p| p.exists()).ok_or_else(|| {
        "Test harness binary not found. Please compile and install runtara-test-harness."
            .to_string()
    })?;

    // Read binary
    let binary = std::fs::read(binary_path)
        .map_err(|e| format!("Failed to read test harness binary: {}", e))?;

    // Create image directory
    let image_id = uuid::Uuid::new_v4().to_string();
    let images_dir = data_dir.join("images").join(&image_id);
    let image_binary_path = images_dir.join("binary");
    let bundle_path = images_dir.join("bundle");

    std::fs::create_dir_all(&images_dir)
        .map_err(|e| format!("Failed to create image directory: {}", e))?;

    std::fs::write(&image_binary_path, &binary)
        .map_err(|e| format!("Failed to write binary: {}", e))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            std::fs::set_permissions(&image_binary_path, std::fs::Permissions::from_mode(0o755));
    }

    // Create OCI bundle
    create_bundle_at_path(&bundle_path, &image_binary_path)
        .map_err(|e| format!("Failed to create OCI bundle: {}", e))?;

    // Build and register image
    let mut image = ImageBuilder::new(
        SYSTEM_TENANT,
        TEST_HARNESS_NAME,
        image_binary_path.to_string_lossy(),
    )
    .runner_type(RunnerType::Oci)
    .description("Agent test harness binary")
    .bundle_path(bundle_path.to_string_lossy())
    .build();

    image.image_id = image_id.clone();

    image_registry
        .register(&image)
        .await
        .map_err(|e| format!("Failed to register test harness image: {}", e))?;

    info!(image_id = %image_id, "Test harness image registered");

    Ok(image)
}

/// Response for listing agents.
pub struct ListAgentsResponse {
    /// JSON-encoded list of agents.
    pub agents_json: Vec<u8>,
}

/// Handle list agents request.
///
/// This returns metadata about all available agents and their capabilities.
/// It runs in-process (no OCI container needed) since it only returns metadata.
pub async fn handle_list_agents(_state: &EnvironmentHandlerState) -> Result<ListAgentsResponse> {
    use runtara_dsl::agent_meta::get_agents;

    let agents = get_agents();
    let agents_json = serde_json::to_vec(&agents)
        .map_err(|e| crate::error::Error::Other(format!("Failed to serialize agents: {}", e)))?;

    Ok(ListAgentsResponse { agents_json })
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
/// It runs in-process (no OCI container needed) since it only returns metadata.
pub async fn handle_get_capability(
    _state: &EnvironmentHandlerState,
    request: GetCapabilityRequest,
) -> Result<GetCapabilityResponse> {
    use runtara_dsl::agent_meta::get_capability_inputs;

    match get_capability_inputs(&request.agent_id, &request.capability_id) {
        Some(inputs) => {
            let inputs_json = serde_json::to_vec(&inputs).map_err(|e| {
                crate::error::Error::Other(format!("Failed to serialize inputs: {}", e))
            })?;

            Ok(GetCapabilityResponse {
                found: true,
                capability_json: vec![], // Could be expanded to include more info
                inputs_json,
            })
        }
        None => Ok(GetCapabilityResponse {
            found: false,
            capability_json: vec![],
            inputs_json: vec![],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_registry::{Image, RunnerType};
    use chrono::Utc;
    use serde_json::json;

    fn make_image(metadata: Option<serde_json::Value>) -> Image {
        Image {
            image_id: "img-1".to_string(),
            tenant_id: "tenant-1".to_string(),
            name: "test-image".to_string(),
            description: None,
            binary_path: "/tmp/binary".to_string(),
            bundle_path: None,
            runner_type: RunnerType::Oci,
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
                "_scenario_id": "should-be-removed",
                "_scope_id": "should-be-removed",
                "_cache_key_prefix": "should-be-removed",
                "_loop_indices": [0, 1],
                "_parent_scenario_id": "should-be-removed"
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
