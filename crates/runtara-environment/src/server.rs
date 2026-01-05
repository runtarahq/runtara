// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Environment QUIC server.
//!
//! Handles requests from Management SDK for image and instance management.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, error, info, warn};

use runtara_protocol::environment_proto::{
    self, RpcError, RpcRequest, RpcResponse, rpc_request::Request, rpc_response::Response,
};
use runtara_protocol::frame::Frame;
use runtara_protocol::server::{ConnectionHandler, RuntaraServer, StreamHandler};

use crate::db;
use crate::handlers::{
    EnvironmentHandlerState, GetCapabilityRequest, RegisterImageRequest, ResumeInstanceRequest,
    StartInstanceRequest, StopInstanceRequest, TestCapabilityRequest, handle_get_capability,
    handle_health_check, handle_list_agents, handle_register_image, handle_resume_instance,
    handle_start_instance, handle_stop_instance, handle_test_capability,
};
use crate::image_registry::{ImageRegistry, RunnerType};

/// Run the Environment QUIC server.
pub async fn run_environment_server(
    bind_addr: SocketAddr,
    state: Arc<EnvironmentHandlerState>,
) -> Result<()> {
    let server = RuntaraServer::localhost(bind_addr)?;

    info!(addr = %bind_addr, "Environment QUIC server starting");

    server
        .run(move |conn: ConnectionHandler| {
            let state = state.clone();
            async move {
                handle_connection(conn, state).await;
            }
        })
        .await?;

    Ok(())
}

/// Handle a single connection.
pub async fn handle_connection(conn: ConnectionHandler, state: Arc<EnvironmentHandlerState>) {
    info!(remote = %conn.remote_address(), "New environment connection accepted");

    conn.run(move |stream: StreamHandler| {
        let state = state.clone();
        async move {
            if let Err(e) = handle_stream(stream, state).await {
                error!("Stream error: {}", e);
            }
        }
    })
    .await;

    debug!("Environment connection closed");
}

/// Handle a single stream (request/response).
async fn handle_stream(
    mut stream: StreamHandler,
    state: Arc<EnvironmentHandlerState>,
) -> Result<()> {
    // Read request frame
    let request_frame = stream.read_frame().await?;

    // Decode as RpcRequest wrapper
    let rpc_request: RpcRequest = request_frame.decode()?;

    let request = match rpc_request.request {
        Some(req) => req,
        None => {
            warn!("Received empty RpcRequest");
            let response = RpcResponse {
                response: Some(Response::Error(RpcError {
                    code: "EMPTY_REQUEST".to_string(),
                    message: "RpcRequest contained no request".to_string(),
                })),
            };
            stream.write_frame(&Frame::response(&response)?).await?;
            stream.finish()?;
            return Ok(());
        }
    };

    debug!(
        "Received environment request: {:?}",
        std::mem::discriminant(&request)
    );

    // Route to appropriate handler
    let response = match request {
        Request::HealthCheck(_) => {
            match handle_health_check(&state).await {
                Ok(resp) => Response::HealthCheck(environment_proto::HealthCheckResponse {
                    healthy: resp.healthy,
                    version: resp.version,
                    uptime_ms: resp.uptime_ms,
                    active_instances: 0, // TODO: count from container registry
                }),
                Err(e) => Response::Error(RpcError {
                    code: "HEALTH_CHECK_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::RegisterImage(req) => {
            let runner_type = convert_runner_type(req.runner_type());
            let handler_req = RegisterImageRequest {
                tenant_id: req.tenant_id,
                name: req.name,
                description: req.description,
                binary: req.binary,
                runner_type,
                metadata: req.metadata.and_then(|b| serde_json::from_slice(&b).ok()),
            };
            match handle_register_image(&state, handler_req).await {
                Ok(resp) => Response::RegisterImage(environment_proto::RegisterImageResponse {
                    success: resp.success,
                    image_id: resp.image_id,
                    error: resp.error.unwrap_or_default(),
                }),
                Err(e) => Response::Error(RpcError {
                    code: "REGISTER_IMAGE_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::RegisterImageStream(req) => {
            // Handle streaming image registration
            match handle_register_image_stream(&mut stream, &state, req).await {
                Ok(resp) => Response::RegisterImage(resp),
                Err(e) => Response::Error(RpcError {
                    code: "REGISTER_IMAGE_STREAM_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::ListImages(req) => match handle_list_images(&state, req).await {
            Ok(resp) => Response::ListImages(resp),
            Err(e) => Response::Error(RpcError {
                code: "LIST_IMAGES_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::GetImage(req) => match handle_get_image(&state, req).await {
            Ok(resp) => Response::GetImage(resp),
            Err(e) => Response::Error(RpcError {
                code: "GET_IMAGE_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::DeleteImage(req) => match handle_delete_image(&state, req).await {
            Ok(resp) => Response::DeleteImage(resp),
            Err(e) => Response::Error(RpcError {
                code: "DELETE_IMAGE_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::StartInstance(req) => {
            let handler_req = StartInstanceRequest {
                image_id: req.image_id,
                tenant_id: req.tenant_id,
                instance_id: req.instance_id,
                input: serde_json::from_slice(&req.input).ok(),
                timeout_seconds: req.timeout_seconds.map(|t| t as u64),
                env: req.env,
            };
            match handle_start_instance(&state, handler_req).await {
                Ok(resp) => Response::StartInstance(environment_proto::StartInstanceResponse {
                    success: resp.success,
                    instance_id: resp.instance_id,
                    error: resp.error.unwrap_or_default(),
                }),
                Err(e) => Response::Error(RpcError {
                    code: "START_INSTANCE_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::StopInstance(req) => {
            let handler_req = StopInstanceRequest {
                instance_id: req.instance_id,
                reason: req.reason,
                grace_period_seconds: req.grace_period_seconds as u64,
            };
            match handle_stop_instance(&state, handler_req).await {
                Ok(resp) => Response::StopInstance(environment_proto::StopInstanceResponse {
                    success: resp.success,
                    error: resp.error.unwrap_or_default(),
                }),
                Err(e) => Response::Error(RpcError {
                    code: "STOP_INSTANCE_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::ResumeInstance(req) => {
            let handler_req = ResumeInstanceRequest {
                instance_id: req.instance_id,
            };
            match handle_resume_instance(&state, handler_req).await {
                Ok(resp) => Response::ResumeInstance(environment_proto::ResumeInstanceResponse {
                    success: resp.success,
                    error: resp.error.unwrap_or_default(),
                }),
                Err(e) => Response::Error(RpcError {
                    code: "RESUME_INSTANCE_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::GetInstanceStatus(req) => match handle_get_instance_status(&state, req).await {
            Ok(resp) => Response::GetInstanceStatus(resp),
            Err(e) => Response::Error(RpcError {
                code: "GET_INSTANCE_STATUS_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::ListInstances(req) => match handle_list_instances(&state, req).await {
            Ok(resp) => Response::ListInstances(resp),
            Err(e) => Response::Error(RpcError {
                code: "LIST_INSTANCES_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::SendSignal(req) => match handle_send_signal(&state, req).await {
            Ok(resp) => Response::SendSignal(resp),
            Err(e) => Response::Error(RpcError {
                code: "SEND_SIGNAL_ERROR".to_string(),
                message: e.to_string(),
            }),
        },
        Request::SendCustomSignal(req) => match handle_send_custom_signal(&state, req).await {
            Ok(resp) => Response::SendCustomSignal(resp),
            Err(e) => Response::Error(RpcError {
                code: "SEND_CUSTOM_SIGNAL_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::TestCapability(req) => {
            let handler_req = TestCapabilityRequest {
                tenant_id: req.tenant_id,
                agent_id: req.agent_id,
                capability_id: req.capability_id,
                input: serde_json::from_slice(&req.input).unwrap_or_default(),
                connection: req.connection.and_then(|b| serde_json::from_slice(&b).ok()),
                timeout_ms: req.timeout_ms,
            };
            match handle_test_capability(&state, handler_req).await {
                Ok(resp) => Response::TestCapability(environment_proto::TestCapabilityResponse {
                    success: resp.success,
                    output: resp
                        .output
                        .map(|v| serde_json::to_vec(&v).unwrap_or_default())
                        .unwrap_or_default(),
                    error: resp.error,
                    execution_time_ms: resp.execution_time_ms,
                }),
                Err(e) => Response::Error(RpcError {
                    code: "TEST_CAPABILITY_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::ListAgents(_req) => match handle_list_agents(&state).await {
            Ok(resp) => Response::ListAgents(environment_proto::ListAgentsResponse {
                agents_json: resp.agents_json,
            }),
            Err(e) => Response::Error(RpcError {
                code: "LIST_AGENTS_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::GetCapability(req) => {
            let handler_req = GetCapabilityRequest {
                agent_id: req.agent_id,
                capability_id: req.capability_id,
            };
            match handle_get_capability(&state, handler_req).await {
                Ok(resp) => Response::GetCapability(environment_proto::GetCapabilityResponse {
                    found: resp.found,
                    capability_json: resp.capability_json,
                    inputs_json: resp.inputs_json,
                }),
                Err(e) => Response::Error(RpcError {
                    code: "GET_CAPABILITY_ERROR".to_string(),
                    message: e.to_string(),
                }),
            }
        }

        Request::ListCheckpoints(req) => match handle_list_checkpoints(&state, req).await {
            Ok(resp) => Response::ListCheckpoints(resp),
            Err(e) => Response::Error(RpcError {
                code: "LIST_CHECKPOINTS_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::GetCheckpoint(req) => match handle_get_checkpoint(&state, req).await {
            Ok(resp) => Response::GetCheckpoint(resp),
            Err(e) => Response::Error(RpcError {
                code: "GET_CHECKPOINT_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::ListEvents(req) => match handle_list_events(&state, req).await {
            Ok(resp) => Response::ListEvents(resp),
            Err(e) => Response::Error(RpcError {
                code: "LIST_EVENTS_ERROR".to_string(),
                message: e.to_string(),
            }),
        },

        Request::GetTenantMetrics(req) => match handle_get_tenant_metrics(&state, req).await {
            Ok(resp) => Response::GetTenantMetrics(resp),
            Err(e) => Response::Error(RpcError {
                code: "GET_TENANT_METRICS_ERROR".to_string(),
                message: e.to_string(),
            }),
        },
    };

    // Send response
    let rpc_response = RpcResponse {
        response: Some(response),
    };
    stream.write_frame(&Frame::response(&rpc_response)?).await?;
    stream.finish()?;

    Ok(())
}

// ============================================================================
// Helper Handlers (these convert proto types to/from internal types)
// ============================================================================

fn convert_runner_type(proto_type: environment_proto::RunnerType) -> RunnerType {
    match proto_type {
        environment_proto::RunnerType::RunnerOci => RunnerType::Oci,
        environment_proto::RunnerType::RunnerNative => RunnerType::Native,
        environment_proto::RunnerType::RunnerWasm => RunnerType::Wasm,
    }
}

fn convert_runner_type_to_proto(runner_type: RunnerType) -> i32 {
    match runner_type {
        RunnerType::Oci => environment_proto::RunnerType::RunnerOci as i32,
        RunnerType::Native => environment_proto::RunnerType::RunnerNative as i32,
        RunnerType::Wasm => environment_proto::RunnerType::RunnerWasm as i32,
    }
}

fn convert_instance_status(status: &str) -> i32 {
    match status {
        "pending" => environment_proto::InstanceStatus::StatusPending as i32,
        "running" => environment_proto::InstanceStatus::StatusRunning as i32,
        "suspended" | "sleeping" => environment_proto::InstanceStatus::StatusSuspended as i32,
        "completed" => environment_proto::InstanceStatus::StatusCompleted as i32,
        "failed" => environment_proto::InstanceStatus::StatusFailed as i32,
        "cancelled" => environment_proto::InstanceStatus::StatusCancelled as i32,
        _ => environment_proto::InstanceStatus::StatusUnknown as i32,
    }
}

async fn handle_register_image_stream(
    stream: &mut StreamHandler,
    state: &EnvironmentHandlerState,
    req: environment_proto::RegisterImageStreamStart,
) -> Result<environment_proto::RegisterImageResponse, crate::error::Error> {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    eprintln!("DEBUG SERVER: data_dir = {:?}", state.data_dir);
    info!(
        tenant_id = %req.tenant_id,
        name = %req.name,
        binary_size = req.binary_size,
        data_dir = ?state.data_dir,
        "Streaming image registration started"
    );

    // Create temp file for binary
    let image_id = uuid::Uuid::new_v4().to_string();
    let images_dir = state.data_dir.join("images").join(&image_id);
    eprintln!("DEBUG SERVER: images_dir = {:?}", images_dir);
    info!(images_dir = ?images_dir, "Creating image directory");
    let binary_path = images_dir.join("binary");
    let bundle_path = images_dir.join("bundle");

    std::fs::create_dir_all(&images_dir)?;

    // Stream binary data to file
    let mut file = std::fs::File::create(&binary_path)?;
    let mut hasher = Sha256::new();
    let mut total_bytes = 0u64;
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = stream
            .read_bytes(&mut buf)
            .await
            .map_err(|e| crate::error::Error::Other(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        total_bytes += n as u64;
    }

    drop(file);

    // Verify checksum if provided
    if let Some(expected_sha256) = &req.sha256 {
        let actual_sha256 = format!("{:x}", hasher.finalize());
        if &actual_sha256 != expected_sha256 {
            let _ = std::fs::remove_dir_all(&images_dir);
            return Ok(environment_proto::RegisterImageResponse {
                success: false,
                image_id: String::new(),
                error: format!(
                    "Checksum mismatch: expected {}, got {}",
                    expected_sha256, actual_sha256
                ),
            });
        }
    }

    // Verify size
    if req.binary_size > 0 && total_bytes != req.binary_size {
        let _ = std::fs::remove_dir_all(&images_dir);
        return Ok(environment_proto::RegisterImageResponse {
            success: false,
            image_id: String::new(),
            error: format!(
                "Size mismatch: expected {}, got {}",
                req.binary_size, total_bytes
            ),
        });
    }

    // Make binary executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))?;
    }

    // Create OCI bundle if needed
    let runner_type = convert_runner_type(req.runner_type());
    let bundle_path_str = if runner_type == RunnerType::Oci {
        if let Err(e) = crate::runner::oci::create_bundle_at_path(&bundle_path, &binary_path) {
            let _ = std::fs::remove_dir_all(&images_dir);
            return Ok(environment_proto::RegisterImageResponse {
                success: false,
                image_id: String::new(),
                error: format!("Failed to create OCI bundle: {}", e),
            });
        }
        Some(bundle_path.to_string_lossy().to_string())
    } else {
        None
    };

    // Build image
    let mut builder = crate::image_registry::ImageBuilder::new(
        &req.tenant_id,
        &req.name,
        binary_path.to_string_lossy(),
    )
    .runner_type(runner_type);

    if let Some(desc) = &req.description {
        builder = builder.description(desc);
    }

    if let Some(bp) = &bundle_path_str {
        builder = builder.bundle_path(bp);
    }

    if let Some(meta) = &req.metadata {
        if let Ok(json) = serde_json::from_slice(meta) {
            builder = builder.metadata(json);
        }
    }

    let mut image = builder.build();
    image.image_id = image_id.clone();

    // Register in database
    let image_registry = ImageRegistry::new(state.pool.clone());
    if let Err(e) = image_registry.register(&image).await {
        let _ = std::fs::remove_dir_all(&images_dir);
        return Ok(environment_proto::RegisterImageResponse {
            success: false,
            image_id: String::new(),
            error: format!("Failed to register image: {}", e),
        });
    }

    info!(image_id = %image_id, bytes = total_bytes, "Streaming image registration complete");

    Ok(environment_proto::RegisterImageResponse {
        success: true,
        image_id,
        error: String::new(),
    })
}

async fn handle_list_images(
    state: &EnvironmentHandlerState,
    req: environment_proto::ListImagesRequest,
) -> Result<environment_proto::ListImagesResponse, crate::error::Error> {
    let image_registry = ImageRegistry::new(state.pool.clone());

    let limit = if req.limit == 0 {
        100
    } else {
        req.limit as i64
    };
    let offset = req.offset as i64;

    let images = if let Some(tenant_id) = &req.tenant_id {
        image_registry
            .list_by_tenant(tenant_id, limit, offset)
            .await?
    } else {
        image_registry.list_all(limit, offset).await?
    };

    let summaries = images
        .into_iter()
        .map(|img| environment_proto::ImageSummary {
            image_id: img.image_id.to_string(),
            tenant_id: img.tenant_id,
            name: img.name,
            description: img.description,
            runner_type: convert_runner_type_to_proto(img.runner_type).into(),
            created_at_ms: img.created_at.timestamp_millis(),
        })
        .collect();

    Ok(environment_proto::ListImagesResponse {
        images: summaries,
        total_count: 0, // TODO: count query
    })
}

async fn handle_get_image(
    state: &EnvironmentHandlerState,
    req: environment_proto::GetImageRequest,
) -> Result<environment_proto::GetImageResponse, crate::error::Error> {
    let image_registry = ImageRegistry::new(state.pool.clone());

    if req.image_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "image_id is required".to_string(),
        ));
    }

    match image_registry.get(&req.image_id).await? {
        Some(img) => {
            // Verify tenant owns this image (multi-tenant isolation)
            // Return "not found" for tenant mismatch to avoid leaking existence
            if img.tenant_id != req.tenant_id {
                debug!(
                    image_id = %req.image_id,
                    image_tenant = %img.tenant_id,
                    request_tenant = %req.tenant_id,
                    "GetImage: tenant mismatch, returning not found"
                );
                return Ok(environment_proto::GetImageResponse {
                    found: false,
                    image: None,
                });
            }

            Ok(environment_proto::GetImageResponse {
                found: true,
                image: Some(environment_proto::ImageSummary {
                    image_id: img.image_id.to_string(),
                    tenant_id: img.tenant_id,
                    name: img.name,
                    description: img.description,
                    runner_type: convert_runner_type_to_proto(img.runner_type).into(),
                    created_at_ms: img.created_at.timestamp_millis(),
                }),
            })
        }
        None => Ok(environment_proto::GetImageResponse {
            found: false,
            image: None,
        }),
    }
}

async fn handle_delete_image(
    state: &EnvironmentHandlerState,
    req: environment_proto::DeleteImageRequest,
) -> Result<environment_proto::DeleteImageResponse, crate::error::Error> {
    let image_registry = ImageRegistry::new(state.pool.clone());

    if req.image_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "image_id is required".to_string(),
        ));
    }

    // Get image to find file paths
    if let Some(img) = image_registry.get(&req.image_id).await? {
        // Verify tenant owns this image (multi-tenant isolation)
        // Return "not found" for tenant mismatch to avoid leaking existence
        if img.tenant_id != req.tenant_id {
            debug!(
                image_id = %req.image_id,
                image_tenant = %img.tenant_id,
                request_tenant = %req.tenant_id,
                "DeleteImage: tenant mismatch, returning not found"
            );
            return Ok(environment_proto::DeleteImageResponse {
                success: false,
                error: format!("Image '{}' not found", req.image_id),
            });
        }

        // Delete from database
        image_registry.delete(&req.image_id).await?;

        // Delete files
        let images_dir = state.data_dir.join("images").join(&req.image_id);
        let _ = std::fs::remove_dir_all(&images_dir);

        Ok(environment_proto::DeleteImageResponse {
            success: true,
            error: String::new(),
        })
    } else {
        Ok(environment_proto::DeleteImageResponse {
            success: false,
            error: format!("Image '{}' not found", req.image_id),
        })
    }
}

async fn handle_get_instance_status(
    state: &EnvironmentHandlerState,
    req: environment_proto::GetInstanceStatusRequest,
) -> Result<environment_proto::GetInstanceStatusResponse, crate::error::Error> {
    match db::get_instance_full(&state.pool, &req.instance_id).await? {
        Some(inst) => Ok(environment_proto::GetInstanceStatusResponse {
            found: true,
            instance_id: inst.instance_id,
            status: convert_instance_status(&inst.status),
            checkpoint_id: inst.checkpoint_id,
            created_at_ms: inst.created_at.timestamp_millis(),
            started_at_ms: inst.started_at.map(|t| t.timestamp_millis()),
            finished_at_ms: inst.finished_at.map(|t| t.timestamp_millis()),
            output: inst.output,
            error: inst.error,
            stderr: inst.stderr,
            // Extended fields
            image_id: inst.image_id.unwrap_or_default(),
            image_name: inst.image_name.unwrap_or_default(),
            tenant_id: inst.tenant_id,
            input: inst.input,
            heartbeat_at_ms: inst.heartbeat_at.map(|t| t.timestamp_millis()),
            retry_count: inst.attempt as u32,
            max_retries: inst.max_attempts as u32,
            // Execution metrics (available for terminal states)
            memory_peak_bytes: inst.memory_peak_bytes.map(|v| v as u64),
            cpu_usage_usec: inst.cpu_usage_usec.map(|v| v as u64),
        }),
        None => Ok(environment_proto::GetInstanceStatusResponse {
            found: false,
            instance_id: req.instance_id,
            status: environment_proto::InstanceStatus::StatusUnknown as i32,
            checkpoint_id: None,
            created_at_ms: 0,
            started_at_ms: None,
            finished_at_ms: None,
            output: None,
            error: None,
            stderr: None,
            // Extended fields - defaults for not found
            image_id: String::new(),
            image_name: String::new(),
            tenant_id: String::new(),
            input: None,
            heartbeat_at_ms: None,
            retry_count: 0,
            max_retries: 0,
            // Execution metrics - not available for not found
            memory_peak_bytes: None,
            cpu_usage_usec: None,
        }),
    }
}

async fn handle_list_instances(
    state: &EnvironmentHandlerState,
    req: environment_proto::ListInstancesRequest,
) -> Result<environment_proto::ListInstancesResponse, crate::error::Error> {
    use chrono::TimeZone;

    let limit = if req.limit == 0 {
        100
    } else {
        req.limit as i64
    };
    let offset = req.offset as i64;

    // Convert status enum to string
    let status = req
        .status
        .and_then(|s| match environment_proto::InstanceStatus::try_from(s) {
            Ok(environment_proto::InstanceStatus::StatusPending) => Some("pending".to_string()),
            Ok(environment_proto::InstanceStatus::StatusRunning) => Some("running".to_string()),
            Ok(environment_proto::InstanceStatus::StatusSuspended) => Some("suspended".to_string()),
            Ok(environment_proto::InstanceStatus::StatusCompleted) => Some("completed".to_string()),
            Ok(environment_proto::InstanceStatus::StatusFailed) => Some("failed".to_string()),
            Ok(environment_proto::InstanceStatus::StatusCancelled) => Some("cancelled".to_string()),
            _ => None,
        });

    // Use image_id directly as string
    let image_id = req.image_id.filter(|id| !id.is_empty());

    // Convert milliseconds to DateTime
    let created_after = req
        .created_after_ms
        .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());
    let created_before = req
        .created_before_ms
        .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());
    let finished_after = req
        .finished_after_ms
        .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());
    let finished_before = req
        .finished_before_ms
        .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());

    let options = db::ListInstancesOptions {
        tenant_id: req.tenant_id,
        status,
        image_id,
        image_name_prefix: req.image_name_prefix,
        created_after,
        created_before,
        finished_after,
        finished_before,
        order_by: req.order_by,
        limit,
        offset,
    };

    let instances = db::list_instances(&state.pool, &options).await?;
    let total_count = db::count_instances(&state.pool, &options).await? as u32;

    let summaries = instances
        .into_iter()
        .map(|inst| environment_proto::InstanceSummary {
            instance_id: inst.instance_id,
            tenant_id: inst.tenant_id,
            image_id: inst.image_id.unwrap_or_default(),
            status: convert_instance_status(&inst.status),
            created_at_ms: inst.created_at.timestamp_millis(),
            started_at_ms: inst.started_at.map(|t| t.timestamp_millis()),
            finished_at_ms: inst.finished_at.map(|t| t.timestamp_millis()),
            has_error: inst.error.is_some(),
        })
        .collect();

    Ok(environment_proto::ListInstancesResponse {
        instances: summaries,
        total_count,
    })
}

/// Map environment proto signal type to database signal type string.
fn map_signal_type(signal_type: environment_proto::SignalType) -> &'static str {
    match signal_type {
        environment_proto::SignalType::SignalCancel => "cancel",
        environment_proto::SignalType::SignalPause => "pause",
        environment_proto::SignalType::SignalResume => "resume",
    }
}

async fn handle_send_signal(
    state: &EnvironmentHandlerState,
    req: environment_proto::SendSignalRequest,
) -> Result<environment_proto::SendSignalResponse, crate::error::Error> {
    info!(
        instance_id = %req.instance_id,
        signal_type = ?req.signal_type,
        "Sending signal via shared persistence"
    );

    // Check if we have Core persistence available
    let persistence = match &state.core_persistence {
        Some(p) => p,
        None => {
            return Ok(environment_proto::SendSignalResponse {
                success: false,
                error: "Core persistence not configured".to_string(),
            });
        }
    };

    // Validate instance exists
    let instance = persistence
        .get_instance(&req.instance_id)
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to get instance: {}", e)))?;

    let instance = match instance {
        Some(inst) => inst,
        None => {
            return Ok(environment_proto::SendSignalResponse {
                success: false,
                error: format!("Instance '{}' not found", req.instance_id),
            });
        }
    };

    // Check if instance is in a state that can receive signals
    if !matches!(
        instance.status.as_str(),
        "running" | "suspended" | "pending"
    ) {
        return Ok(environment_proto::SendSignalResponse {
            success: false,
            error: format!(
                "Cannot send signal to instance in '{}' state (terminal state)",
                instance.status
            ),
        });
    }

    // Map proto signal type to DB enum
    let signal_type = match environment_proto::SignalType::try_from(req.signal_type) {
        Ok(st) => map_signal_type(st),
        Err(_) => {
            return Ok(environment_proto::SendSignalResponse {
                success: false,
                error: format!("Unknown signal type: {}", req.signal_type),
            });
        }
    };

    // Insert pending signal via persistence
    persistence
        .insert_signal(&req.instance_id, signal_type, &req.payload)
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to insert signal: {}", e)))?;

    info!("Signal stored successfully via persistence");

    Ok(environment_proto::SendSignalResponse {
        success: true,
        error: String::new(),
    })
}

async fn handle_send_custom_signal(
    state: &EnvironmentHandlerState,
    req: environment_proto::SendCustomSignalRequest,
) -> Result<environment_proto::SendCustomSignalResponse, crate::error::Error> {
    info!(
        instance_id = %req.instance_id,
        checkpoint_id = %req.checkpoint_id,
        "Sending custom signal via shared persistence"
    );

    // Check if we have Core persistence available
    let persistence = match &state.core_persistence {
        Some(p) => p,
        None => {
            return Ok(environment_proto::SendCustomSignalResponse {
                success: false,
                error: "Core persistence not configured".to_string(),
            });
        }
    };

    // Validate instance exists
    let instance = persistence
        .get_instance(&req.instance_id)
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to get instance: {}", e)))?;

    if instance.is_none() {
        return Ok(environment_proto::SendCustomSignalResponse {
            success: false,
            error: format!("Instance '{}' not found", req.instance_id),
        });
    }

    // Validate checkpoint_id
    if req.checkpoint_id.is_empty() {
        return Ok(environment_proto::SendCustomSignalResponse {
            success: false,
            error: "checkpoint_id is required".to_string(),
        });
    }

    // Store pending custom signal via persistence
    persistence
        .insert_custom_signal(&req.instance_id, &req.checkpoint_id, &req.payload)
        .await
        .map_err(|e| {
            crate::error::Error::Other(format!("Failed to insert custom signal: {}", e))
        })?;

    info!("Custom signal stored successfully via persistence");

    Ok(environment_proto::SendCustomSignalResponse {
        success: true,
        error: String::new(),
    })
}

async fn handle_list_checkpoints(
    state: &EnvironmentHandlerState,
    req: environment_proto::ListCheckpointsRequest,
) -> Result<environment_proto::ListCheckpointsResponse, crate::error::Error> {
    debug!(
        instance_id = %req.instance_id,
        checkpoint_id_filter = ?req.checkpoint_id,
        limit = ?req.limit,
        offset = ?req.offset,
        "Listing checkpoints via shared persistence"
    );

    // Check if we have Core persistence available
    let persistence = match &state.core_persistence {
        Some(p) => p,
        None => {
            return Err(crate::error::Error::Other(
                "Core persistence not configured".to_string(),
            ));
        }
    };

    // Parse timestamps from milliseconds
    let created_after = req
        .created_after_ms
        .and_then(chrono::DateTime::from_timestamp_millis);
    let created_before = req
        .created_before_ms
        .and_then(chrono::DateTime::from_timestamp_millis);

    let limit = req.limit.unwrap_or(100) as i64;
    let offset = req.offset.unwrap_or(0) as i64;

    // Get checkpoints from persistence
    let checkpoints = persistence
        .list_checkpoints(
            &req.instance_id,
            req.checkpoint_id.as_deref(),
            limit,
            offset,
            created_after,
            created_before,
        )
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to list checkpoints: {}", e)))?;

    // Get total count for pagination
    let total_count = persistence
        .count_checkpoints(
            &req.instance_id,
            req.checkpoint_id.as_deref(),
            created_after,
            created_before,
        )
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to count checkpoints: {}", e)))?;

    // Convert to proto summaries
    let summaries: Vec<environment_proto::CheckpointSummary> = checkpoints
        .into_iter()
        .map(|cp| environment_proto::CheckpointSummary {
            checkpoint_id: cp.checkpoint_id,
            instance_id: cp.instance_id,
            created_at_ms: cp.created_at.timestamp_millis(),
            data_size_bytes: cp.state.len() as u64,
        })
        .collect();

    Ok(environment_proto::ListCheckpointsResponse {
        checkpoints: summaries,
        total_count: total_count as u32,
        limit: limit as u32,
        offset: offset as u32,
    })
}

async fn handle_get_checkpoint(
    state: &EnvironmentHandlerState,
    req: environment_proto::GetCheckpointRequest,
) -> Result<environment_proto::GetCheckpointResponse, crate::error::Error> {
    debug!(
        instance_id = %req.instance_id,
        checkpoint_id = %req.checkpoint_id,
        "Getting checkpoint via shared persistence"
    );

    // Check if we have Core persistence available
    let persistence = match &state.core_persistence {
        Some(p) => p,
        None => {
            return Err(crate::error::Error::Other(
                "Core persistence not configured".to_string(),
            ));
        }
    };

    let checkpoint = persistence
        .load_checkpoint(&req.instance_id, &req.checkpoint_id)
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to load checkpoint: {}", e)))?;

    match checkpoint {
        Some(cp) => Ok(environment_proto::GetCheckpointResponse {
            found: true,
            checkpoint_id: cp.checkpoint_id,
            instance_id: cp.instance_id,
            created_at_ms: cp.created_at.timestamp_millis(),
            data: cp.state,
        }),
        None => Ok(environment_proto::GetCheckpointResponse {
            found: false,
            checkpoint_id: req.checkpoint_id,
            instance_id: req.instance_id,
            created_at_ms: 0,
            data: Vec::new(),
        }),
    }
}

async fn handle_list_events(
    state: &EnvironmentHandlerState,
    req: environment_proto::ListEventsRequest,
) -> Result<environment_proto::ListEventsResponse, crate::error::Error> {
    use runtara_core::persistence::ListEventsFilter;

    debug!(
        instance_id = %req.instance_id,
        event_type = ?req.event_type,
        subtype = ?req.subtype,
        limit = ?req.limit,
        offset = ?req.offset,
        payload_contains = ?req.payload_contains,
        "Listing events via shared persistence"
    );

    // Check if we have Core persistence available
    let persistence = match &state.core_persistence {
        Some(p) => p,
        None => {
            return Err(crate::error::Error::Other(
                "Core persistence not configured".to_string(),
            ));
        }
    };

    // Parse timestamps from milliseconds
    let created_after = req
        .created_after_ms
        .and_then(chrono::DateTime::from_timestamp_millis);
    let created_before = req
        .created_before_ms
        .and_then(chrono::DateTime::from_timestamp_millis);

    let limit = req.limit.unwrap_or(100) as i64;
    let offset = req.offset.unwrap_or(0) as i64;

    // Build filter
    let filter = ListEventsFilter {
        event_type: req.event_type,
        subtype: req.subtype,
        created_after,
        created_before,
        payload_contains: req.payload_contains,
    };

    // Get events from persistence
    let events = persistence
        .list_events(&req.instance_id, &filter, limit, offset)
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to list events: {}", e)))?;

    // Get total count for pagination
    let total_count = persistence
        .count_events(&req.instance_id, &filter)
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to count events: {}", e)))?;

    // Convert to proto summaries
    let summaries: Vec<environment_proto::EventSummary> = events
        .into_iter()
        .map(|ev| environment_proto::EventSummary {
            id: ev.id.unwrap_or(0),
            instance_id: ev.instance_id,
            event_type: ev.event_type,
            checkpoint_id: ev.checkpoint_id,
            payload: ev.payload,
            created_at_ms: ev.created_at.timestamp_millis(),
            subtype: ev.subtype,
        })
        .collect();

    Ok(environment_proto::ListEventsResponse {
        events: summaries,
        total_count: total_count as u32,
        limit: limit as u32,
        offset: offset as u32,
    })
}

/// Handle get tenant metrics request.
async fn handle_get_tenant_metrics(
    state: &EnvironmentHandlerState,
    req: environment_proto::GetTenantMetricsRequest,
) -> Result<environment_proto::GetTenantMetricsResponse, crate::error::Error> {
    debug!(
        tenant_id = %req.tenant_id,
        "Getting tenant metrics"
    );

    // Validate tenant_id
    if req.tenant_id.is_empty() {
        return Err(crate::error::Error::InvalidRequest(
            "tenant_id is required".to_string(),
        ));
    }

    // Parse timestamps with defaults
    let now = chrono::Utc::now();
    let end_time = req
        .end_time_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or(now);
    let start_time = req
        .start_time_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or(end_time - chrono::Duration::hours(24));

    // Parse granularity
    let granularity = match req.granularity {
        Some(g) => match environment_proto::MetricsGranularity::try_from(g) {
            Ok(environment_proto::MetricsGranularity::GranularityDaily) => {
                db::MetricsGranularity::Daily
            }
            _ => db::MetricsGranularity::Hourly,
        },
        None => db::MetricsGranularity::Hourly,
    };

    let options = db::TenantMetricsOptions {
        tenant_id: req.tenant_id.clone(),
        start_time,
        end_time,
        granularity,
    };

    let bucket_rows = db::get_tenant_metrics(&state.pool, &options)
        .await
        .map_err(|e| crate::error::Error::Other(format!("Failed to get tenant metrics: {}", e)))?;

    // Convert to proto
    let buckets: Vec<environment_proto::MetricsBucket> = bucket_rows
        .into_iter()
        .map(|row| {
            let terminal_count = row.success_count + row.failure_count + row.cancelled_count;
            let success_rate = if terminal_count > 0 {
                Some((row.success_count as f64 / terminal_count as f64) * 100.0)
            } else {
                None
            };

            environment_proto::MetricsBucket {
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
                success_rate_percent: success_rate,
            }
        })
        .collect();

    let proto_granularity = match granularity {
        db::MetricsGranularity::Hourly => {
            environment_proto::MetricsGranularity::GranularityHourly as i32
        }
        db::MetricsGranularity::Daily => {
            environment_proto::MetricsGranularity::GranularityDaily as i32
        }
    };

    Ok(environment_proto::GetTenantMetricsResponse {
        tenant_id: req.tenant_id,
        start_time_ms: start_time.timestamp_millis(),
        end_time_ms: end_time.timestamp_millis(),
        granularity: proto_granularity,
        buckets,
    })
}
