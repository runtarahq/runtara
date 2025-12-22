// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Environment protocol handlers.
//!
//! Handles requests from Management SDK and proxies to Core when needed.

use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

use runtara_core::persistence::Persistence;

use crate::container_registry::{ContainerInfo, ContainerRegistry};
use crate::db;
use crate::error::Result;
use crate::image_registry::{ImageBuilder, ImageRegistry, RunnerType};
use crate::instance_output::{InstanceOutput, InstanceOutputStatus, output_file_path};
use crate::runner::oci::create_bundle_at_path;
use crate::runner::{LaunchOptions, Runner, RunnerHandle};

/// Shared state for environment handlers.
///
/// Contains database connection, runner, and configuration shared across all handlers.
pub struct EnvironmentHandlerState {
    /// PostgreSQL connection pool (for Environment-specific tables: images, containers, etc.).
    pub pool: PgPool,
    /// Core persistence layer (for instance lifecycle, checkpoints, signals).
    /// When set, instance operations are delegated to Core's shared persistence.
    pub core_persistence: Option<Arc<dyn Persistence>>,
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
}

impl EnvironmentHandlerState {
    /// Create a new environment handler state.
    pub fn new(
        pool: PgPool,
        runner: Arc<dyn Runner>,
        core_addr: String,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            pool,
            core_persistence: None,
            start_time: std::time::Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            runner,
            core_addr,
            data_dir,
        }
    }

    /// Create a new environment handler state with Core persistence.
    ///
    /// When Core persistence is provided, instance lifecycle operations
    /// (create, update status, etc.) are delegated to Core's shared persistence layer.
    /// This enables both Environment and Core to share the same instance data.
    pub fn with_core_persistence(
        pool: PgPool,
        core_persistence: Arc<dyn Persistence>,
        runner: Arc<dyn Runner>,
        core_addr: String,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            pool,
            core_persistence: Some(core_persistence),
            start_time: std::time::Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            runner,
            core_addr,
            data_dir,
        }
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

    // Parse input for runner (not stored in DB, Core doesn't track it)
    let input = request.input.unwrap_or(serde_json::json!({}));

    // Create instance record
    if let Err(e) = db::create_instance(
        &state.pool,
        &instance_id,
        &request.tenant_id,
        &request.image_id,
    )
    .await
    {
        error!(error = %e, "Failed to create instance in DB");
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

            // Register in container registry
            let container_registry = ContainerRegistry::new(state.pool.clone());
            let container_info = ContainerInfo {
                container_id: handle_id_for_registry,
                instance_id: instance_id.clone(),
                tenant_id: request.tenant_id,
                binary_path: image.binary_path,
                bundle_path: image.bundle_path,
                started_at: handle.started_at,
                pid: None,
                timeout_seconds: Some(timeout.as_secs() as i64),
            };
            if let Err(e) = container_registry.register(&container_info).await {
                warn!(error = %e, "Failed to register container (instance still running)");
            }

            // Update instance status to running
            let _ = db::update_instance_status(&state.pool, &instance_id, "running", None).await;

            // Spawn background task to monitor container and process output when done
            spawn_container_monitor(
                state.pool.clone(),
                state.runner.clone(),
                handle,
                tenant_id_for_monitor,
                state.data_dir.clone(),
            );

            Ok(StartInstanceResponse {
                success: true,
                instance_id,
                error: None,
            })
        }
        Err(e) => {
            error!(error = %e, "Failed to launch instance");
            let _ = db::update_instance_status(&state.pool, &instance_id, "failed", None).await;

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
    };

    if let Err(e) = state.runner.stop(&handle).await {
        warn!(error = %e, "Runner stop returned error");
    }

    // Update instance status
    let _ = db::update_instance_status(&state.pool, &request.instance_id, "cancelled", None).await;

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

    // Get image ID from instance_images table
    let image_id = match db::get_instance_image_id(&state.pool, &request.instance_id).await? {
        Some(id) => id,
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

    // Build launch options with checkpoint
    let options = LaunchOptions {
        instance_id: request.instance_id.clone(),
        tenant_id: instance.tenant_id.clone(),
        bundle_path,
        input: serde_json::json!({}), // Input was consumed on first run
        timeout: Duration::from_secs(300),
        runtara_core_addr: state.core_addr.clone(),
        checkpoint_id: Some(checkpoint_id.clone()),
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

            // Register in container registry
            let container_registry = ContainerRegistry::new(state.pool.clone());
            let container_info = ContainerInfo {
                container_id: handle_id_for_registry,
                instance_id: request.instance_id.clone(),
                tenant_id: instance.tenant_id,
                binary_path: image.binary_path,
                bundle_path: image.bundle_path,
                started_at: handle.started_at,
                pid: None,
                timeout_seconds: Some(300),
            };
            if let Err(e) = container_registry.register(&container_info).await {
                warn!(error = %e, "Failed to register container");
            }

            // Update status
            let _ = db::update_instance_status(
                &state.pool,
                &request.instance_id,
                "running",
                Some(&checkpoint_id),
            )
            .await;

            // Spawn background task to monitor container and process output when done
            spawn_container_monitor(
                state.pool.clone(),
                state.runner.clone(),
                handle,
                tenant_id_for_monitor,
                state.data_dir.clone(),
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
// Instance Output Processing
// ============================================================================

/// Process instance output after container exits.
/// Called by the container watcher when a container finishes.
pub async fn process_instance_output(
    state: &EnvironmentHandlerState,
    instance_id: &str,
    tenant_id: &str,
) -> Result<()> {
    let output_path = output_file_path(&state.data_dir, tenant_id, instance_id);

    let output = match InstanceOutput::read_from_file(&output_path).await {
        Ok(o) => o,
        Err(e) => {
            warn!(
                instance_id = %instance_id,
                error = %e,
                "Failed to read instance output, marking as failed"
            );
            db::update_instance_result(
                &state.pool,
                instance_id,
                "failed",
                None,
                Some("No output.json found"),
                None,
            )
            .await?;
            return Ok(());
        }
    };

    info!(
        instance_id = %instance_id,
        status = ?output.status,
        "Processing instance output"
    );

    match output.status {
        InstanceOutputStatus::Completed => {
            let result_bytes = output
                .result
                .as_ref()
                .and_then(|v| serde_json::to_vec(v).ok());
            db::update_instance_result(
                &state.pool,
                instance_id,
                "completed",
                result_bytes.as_deref(),
                None,
                None,
            )
            .await?;
        }
        InstanceOutputStatus::Failed => {
            db::update_instance_result(
                &state.pool,
                instance_id,
                "failed",
                None,
                output.error.as_deref(),
                None,
            )
            .await?;
        }
        InstanceOutputStatus::Suspended => {
            db::update_instance_result(
                &state.pool,
                instance_id,
                "suspended",
                None,
                None,
                output.checkpoint_id.as_deref(),
            )
            .await?;
        }
        InstanceOutputStatus::Sleeping => {
            // Schedule wake
            if let (Some(wake_after_ms), Some(checkpoint_id)) =
                (output.wake_after_ms, &output.checkpoint_id)
            {
                let wake_at =
                    chrono::Utc::now() + chrono::Duration::milliseconds(wake_after_ms as i64);
                db::schedule_wake(&state.pool, instance_id, checkpoint_id, wake_at).await?;
                db::update_instance_result(
                    &state.pool,
                    instance_id,
                    "sleeping",
                    None,
                    None,
                    Some(checkpoint_id),
                )
                .await?;

                info!(
                    instance_id = %instance_id,
                    wake_at = %wake_at,
                    checkpoint_id = %checkpoint_id,
                    "Scheduled wake for sleeping instance"
                );
            } else {
                warn!(
                    instance_id = %instance_id,
                    "Sleeping output missing wake_after_ms or checkpoint_id"
                );
                db::update_instance_result(
                    &state.pool,
                    instance_id,
                    "failed",
                    None,
                    Some("Invalid sleep output"),
                    None,
                )
                .await?;
            }
        }
        InstanceOutputStatus::Cancelled => {
            db::update_instance_result(&state.pool, instance_id, "cancelled", None, None, None)
                .await?;
        }
    }

    // Clean up container registry
    let container_registry = ContainerRegistry::new(state.pool.clone());
    let _ = container_registry.cleanup(instance_id).await;

    Ok(())
}

// ============================================================================
// Container Monitor
// ============================================================================

/// State needed to process instance output (subset of EnvironmentHandlerState)
struct OutputProcessorState {
    pool: PgPool,
    data_dir: PathBuf,
}

/// Spawn a background task that monitors the container and processes output when done.
///
/// This function should be called after launching an instance to monitor its lifecycle
/// and process output when the container finishes.
pub fn spawn_container_monitor(
    pool: PgPool,
    runner: Arc<dyn Runner>,
    handle: RunnerHandle,
    tenant_id: String,
    data_dir: PathBuf,
) {
    let instance_id = handle.instance_id.clone();

    tokio::spawn(async move {
        // Poll every 500ms to check if container is still running
        let poll_interval = Duration::from_millis(500);

        loop {
            tokio::time::sleep(poll_interval).await;

            if !runner.is_running(&handle).await {
                info!(
                    instance_id = %instance_id,
                    "Container finished, processing output"
                );

                // Create minimal state for processing output
                let output_state = OutputProcessorState {
                    pool: pool.clone(),
                    data_dir: data_dir.clone(),
                };

                if let Err(e) = process_output(&output_state, &instance_id, &tenant_id).await {
                    error!(
                        instance_id = %instance_id,
                        error = %e,
                        "Failed to process instance output"
                    );
                    // Mark as failed if we couldn't process output
                    let _ = db::update_instance_result(
                        &pool,
                        &instance_id,
                        "failed",
                        None,
                        Some(&format!("Failed to process output: {}", e)),
                        None,
                    )
                    .await;
                }

                // Clean up container registry
                let container_registry = ContainerRegistry::new(pool.clone());
                let _ = container_registry.cleanup(&instance_id).await;

                break;
            }
        }
    });
}

/// Process instance output (simpler version that doesn't need full state).
async fn process_output(
    state: &OutputProcessorState,
    instance_id: &str,
    tenant_id: &str,
) -> Result<()> {
    let output_path = output_file_path(&state.data_dir, tenant_id, instance_id);

    let output = match InstanceOutput::read_from_file(&output_path).await {
        Ok(o) => o,
        Err(e) => {
            warn!(
                instance_id = %instance_id,
                error = %e,
                "Failed to read instance output, marking as failed"
            );
            db::update_instance_result(
                &state.pool,
                instance_id,
                "failed",
                None,
                Some("No output.json found"),
                None,
            )
            .await?;
            return Ok(());
        }
    };

    info!(
        instance_id = %instance_id,
        status = ?output.status,
        "Processing instance output"
    );

    match output.status {
        InstanceOutputStatus::Completed => {
            let result_bytes = output
                .result
                .as_ref()
                .and_then(|v| serde_json::to_vec(v).ok());
            db::update_instance_result(
                &state.pool,
                instance_id,
                "completed",
                result_bytes.as_deref(),
                None,
                None,
            )
            .await?;
        }
        InstanceOutputStatus::Failed => {
            db::update_instance_result(
                &state.pool,
                instance_id,
                "failed",
                None,
                output.error.as_deref(),
                None,
            )
            .await?;
        }
        InstanceOutputStatus::Suspended => {
            db::update_instance_result(
                &state.pool,
                instance_id,
                "suspended",
                None,
                None,
                output.checkpoint_id.as_deref(),
            )
            .await?;
        }
        InstanceOutputStatus::Sleeping => {
            // For sleeping, we need to schedule a wake
            if let (Some(wake_after_ms), Some(checkpoint_id)) =
                (output.wake_after_ms, output.checkpoint_id)
            {
                let wake_at =
                    chrono::Utc::now() + chrono::Duration::milliseconds(wake_after_ms as i64);

                db::update_instance_result(
                    &state.pool,
                    instance_id,
                    "suspended",
                    None,
                    None,
                    Some(&checkpoint_id),
                )
                .await?;

                db::schedule_wake(&state.pool, instance_id, &checkpoint_id, wake_at).await?;

                info!(
                    instance_id = %instance_id,
                    wake_at = %wake_at,
                    checkpoint_id = %checkpoint_id,
                    "Scheduled wake for sleeping instance"
                );
            } else {
                warn!(
                    instance_id = %instance_id,
                    "Sleeping output missing wake_after_ms or checkpoint_id"
                );
                db::update_instance_result(
                    &state.pool,
                    instance_id,
                    "failed",
                    None,
                    Some("Invalid sleep output"),
                    None,
                )
                .await?;
            }
        }
        InstanceOutputStatus::Cancelled => {
            db::update_instance_result(&state.pool, instance_id, "cancelled", None, None, None)
                .await?;
        }
    }

    Ok(())
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
/// as INPUT_JSON and reading the result from output.json.
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

    // Build test request JSON for INPUT_JSON
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
    data_dir: &PathBuf,
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
