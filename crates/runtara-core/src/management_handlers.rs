// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Management protocol handlers for runtara-core.
//!
//! These handlers process internal requests from Environment for:
//! - Health check
//! - Send signals to instances (Environment proxies these)
//! - Query instance status
//! - List instances
//!
//! Note: Image registration and instance start/stop are now handled by Environment.

use anyhow::Result;
use sqlx::PgPool;
use tracing::{debug, info, instrument};

use runtara_protocol::management_proto::{
    CheckpointSummary, GetCheckpointRequest, GetCheckpointResponse, GetInstanceStatusRequest,
    GetInstanceStatusResponse, HealthCheckRequest, HealthCheckResponse, InstanceStatus,
    InstanceSummary, ListCheckpointsRequest, ListCheckpointsResponse, ListInstancesRequest,
    ListInstancesResponse, SendCustomSignalRequest, SendCustomSignalResponse, SendSignalRequest,
    SendSignalResponse, SignalType,
};

use crate::persistence;

/// Shared state for management handlers.
///
/// Contains database connection and server metadata for health checks.
pub struct ManagementHandlerState {
    /// PostgreSQL connection pool.
    pub pool: PgPool,
    /// When the server started (for uptime calculation).
    pub start_time: std::time::Instant,
    /// Server version string.
    pub version: String,
}

impl ManagementHandlerState {
    /// Create a new management handler state with the given database pool.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            start_time: std::time::Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
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
///
/// Returns server health status including:
/// - Database connectivity
/// - Server version
/// - Uptime in milliseconds
/// - Count of active (running) instances
#[instrument(skip(state, _request))]
pub async fn handle_health_check(
    state: &ManagementHandlerState,
    _request: HealthCheckRequest,
) -> Result<HealthCheckResponse> {
    debug!("Health check requested");

    // 1. Check database connectivity
    let db_healthy = persistence::health_check_db(&state.pool)
        .await
        .unwrap_or(false);

    // 2. Count active instances
    let active_instances = if db_healthy {
        persistence::count_active_instances(&state.pool)
            .await
            .unwrap_or(0)
    } else {
        0
    };

    Ok(HealthCheckResponse {
        healthy: db_healthy,
        version: state.version.clone(),
        uptime_ms: state.uptime_ms(),
        active_instances,
    })
}

// ============================================================================
// Send Signal (Environment → Core → instance)
// ============================================================================

/// Handle signal delivery request from Environment.
///
/// Stores a pending signal for the instance. The instance will receive
/// the signal on its next checkpoint or poll_signals call.
///
/// Signals can only be sent to instances in running, suspended, or pending state.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
pub async fn handle_send_signal(
    state: &ManagementHandlerState,
    request: SendSignalRequest,
) -> Result<SendSignalResponse> {
    info!(
        signal_type = ?request.signal_type,
        "Received signal request"
    );

    // 1. Validate instance exists
    let instance = persistence::get_instance(&state.pool, &request.instance_id).await?;
    let instance = match instance {
        Some(inst) => inst,
        None => {
            return Ok(SendSignalResponse {
                success: false,
                error: format!("Instance '{}' not found", request.instance_id),
            });
        }
    };

    // 2. Check if instance is in a state that can receive signals
    if !matches!(
        instance.status.as_str(),
        "running" | "suspended" | "pending"
    ) {
        return Ok(SendSignalResponse {
            success: false,
            error: format!(
                "Cannot send signal to instance in '{}' state (terminal state)",
                instance.status
            ),
        });
    }

    // 3. Map proto signal type to DB enum
    let signal_type = map_signal_type(request.signal_type());

    // 4. Insert pending signal (upsert - one signal per instance)
    persistence::insert_signal(
        &state.pool,
        &request.instance_id,
        signal_type,
        &request.payload,
    )
    .await?;

    info!("Signal stored successfully");

    Ok(SendSignalResponse {
        success: true,
        error: String::new(),
    })
}

/// Handle custom checkpoint-scoped signal delivery from Environment.
///
/// Stores a pending custom signal for the instance/checkpoint. The waiting
/// checkpoint will receive it on next checkpoint/poll call.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_send_custom_signal(
    state: &ManagementHandlerState,
    request: SendCustomSignalRequest,
) -> Result<SendCustomSignalResponse> {
    info!(
        checkpoint_id = %request.checkpoint_id,
        "Received custom signal request"
    );

    // Validate instance exists
    let instance = persistence::get_instance(&state.pool, &request.instance_id).await?;
    let Some(inst) = instance else {
        return Ok(SendCustomSignalResponse {
            success: false,
            error: format!("Instance '{}' not found", request.instance_id),
        });
    };

    // Validate checkpoint_id
    if request.checkpoint_id.is_empty() {
        return Ok(SendCustomSignalResponse {
            success: false,
            error: "checkpoint_id is required".to_string(),
        });
    }

    // Store pending custom signal (upsert)
    persistence::insert_custom_signal(
        &state.pool,
        &request.instance_id,
        &request.checkpoint_id,
        &request.payload,
    )
    .await?;

    info!(
        instance_status = %inst.status,
        "Custom signal stored successfully"
    );

    Ok(SendCustomSignalResponse {
        success: true,
        error: String::new(),
    })
}

// ============================================================================
// Get Instance Status
// ============================================================================

/// Handle instance status query from Environment.
///
/// Returns the current status of an instance including timestamps,
/// output data (if completed), and error message (if failed).
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
pub async fn handle_get_instance_status(
    state: &ManagementHandlerState,
    request: GetInstanceStatusRequest,
) -> Result<GetInstanceStatusResponse> {
    debug!("Getting instance status via management API");

    let instance = persistence::get_instance(&state.pool, &request.instance_id).await?;

    match instance {
        Some(inst) => {
            let status = map_status(&inst.status);

            Ok(GetInstanceStatusResponse {
                instance_id: request.instance_id,
                status: status.into(),
                checkpoint_id: inst.checkpoint_id,
                started_at_ms: inst.started_at.map(|t| t.timestamp_millis()).unwrap_or(0),
                finished_at_ms: inst.finished_at.map(|t| t.timestamp_millis()),
                output: inst.output,
                error: inst.error,
            })
        }
        None => Ok(GetInstanceStatusResponse {
            instance_id: request.instance_id,
            status: InstanceStatus::StatusUnknown.into(),
            checkpoint_id: None,
            started_at_ms: 0,
            finished_at_ms: None,
            output: None,
            error: Some("Instance not found".to_string()),
        }),
    }
}

// ============================================================================
// List Instances
// ============================================================================

/// Handle list instances request.
///
/// Returns a paginated list of instances, optionally filtered by tenant and status.
#[instrument(skip(state, request))]
pub async fn handle_list_instances(
    state: &ManagementHandlerState,
    request: ListInstancesRequest,
) -> Result<ListInstancesResponse> {
    // Convert status enum to string filter if provided
    let status_filter = request.status.and_then(|s| {
        if s == 0 {
            None // STATUS_UNKNOWN means no filter
        } else {
            Some(map_status_enum_to_db(s))
        }
    });

    debug!(
        tenant_id = ?request.tenant_id,
        status_filter = ?status_filter,
        limit = request.limit,
        offset = request.offset,
        "Listing instances"
    );

    let instances = persistence::list_instances(
        &state.pool,
        request.tenant_id.as_deref(),
        status_filter.as_deref(),
        request.limit as i64,
        request.offset as i64,
    )
    .await?;

    let instance_summaries: Vec<InstanceSummary> = instances
        .into_iter()
        .map(|inst| InstanceSummary {
            instance_id: inst.instance_id,
            tenant_id: inst.tenant_id,
            status: map_status(&inst.status).into(),
            created_at_ms: inst.created_at.timestamp_millis(),
        })
        .collect();

    let total_count = instance_summaries.len() as u32;

    Ok(ListInstancesResponse {
        instances: instance_summaries,
        total_count,
    })
}

// ============================================================================
// Checkpoints
// ============================================================================

/// Handle list checkpoints request.
///
/// Returns a paginated list of checkpoints for an instance, optionally filtered.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id))]
pub async fn handle_list_checkpoints(
    state: &ManagementHandlerState,
    request: ListCheckpointsRequest,
) -> Result<ListCheckpointsResponse> {
    debug!(
        checkpoint_id_filter = ?request.checkpoint_id,
        limit = ?request.limit,
        offset = ?request.offset,
        "Listing checkpoints"
    );

    // Parse timestamps from milliseconds
    let created_after = request
        .created_after_ms
        .and_then(chrono::DateTime::from_timestamp_millis);
    let created_before = request
        .created_before_ms
        .and_then(chrono::DateTime::from_timestamp_millis);

    let limit = request.limit.unwrap_or(100) as i64;
    let offset = request.offset.unwrap_or(0) as i64;

    // Get checkpoints from database
    let checkpoints = persistence::list_checkpoints(
        &state.pool,
        &request.instance_id,
        request.checkpoint_id.as_deref(),
        limit,
        offset,
        created_after,
        created_before,
    )
    .await?;

    // Get total count for pagination
    let total_count = persistence::count_checkpoints(
        &state.pool,
        &request.instance_id,
        request.checkpoint_id.as_deref(),
        created_after,
        created_before,
    )
    .await?;

    // Convert to proto summaries
    let summaries: Vec<CheckpointSummary> = checkpoints
        .into_iter()
        .map(|cp| CheckpointSummary {
            checkpoint_id: cp.checkpoint_id,
            instance_id: cp.instance_id,
            created_at_ms: cp.created_at.timestamp_millis(),
            data_size_bytes: cp.state.len() as u64,
        })
        .collect();

    Ok(ListCheckpointsResponse {
        checkpoints: summaries,
        total_count: total_count as u32,
        limit: limit as u32,
        offset: offset as u32,
    })
}

/// Handle get checkpoint request.
///
/// Returns the full checkpoint data for a specific checkpoint.
#[instrument(skip(state, request), fields(instance_id = %request.instance_id, checkpoint_id = %request.checkpoint_id))]
pub async fn handle_get_checkpoint(
    state: &ManagementHandlerState,
    request: GetCheckpointRequest,
) -> Result<GetCheckpointResponse> {
    debug!("Getting checkpoint");

    let checkpoint =
        persistence::load_checkpoint(&state.pool, &request.instance_id, &request.checkpoint_id)
            .await?;

    match checkpoint {
        Some(cp) => Ok(GetCheckpointResponse {
            found: true,
            checkpoint_id: cp.checkpoint_id,
            instance_id: cp.instance_id,
            created_at_ms: cp.created_at.timestamp_millis(),
            data: cp.state,
        }),
        None => Ok(GetCheckpointResponse {
            found: false,
            checkpoint_id: request.checkpoint_id,
            instance_id: request.instance_id,
            created_at_ms: 0,
            data: Vec::new(),
        }),
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Map proto signal type to database enum string.
pub fn map_signal_type(signal_type: SignalType) -> &'static str {
    match signal_type {
        SignalType::SignalCancel => "cancel",
        SignalType::SignalPause => "pause",
        SignalType::SignalResume => "resume",
    }
}

/// Map database status string to proto enum.
pub fn map_status(status: &str) -> InstanceStatus {
    match status {
        "pending" => InstanceStatus::StatusPending,
        "running" => InstanceStatus::StatusRunning,
        "suspended" => InstanceStatus::StatusSuspended,
        "completed" => InstanceStatus::StatusCompleted,
        "failed" => InstanceStatus::StatusFailed,
        "cancelled" => InstanceStatus::StatusCancelled,
        _ => InstanceStatus::StatusUnknown,
    }
}

/// Map proto status enum value to database status string.
fn map_status_enum_to_db(status: i32) -> String {
    match status {
        1 => "pending".to_string(),
        2 => "running".to_string(),
        3 => "suspended".to_string(),
        4 => "completed".to_string(),
        5 => "failed".to_string(),
        6 => "cancelled".to_string(),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_type_mapping() {
        assert_eq!(map_signal_type(SignalType::SignalCancel), "cancel");
        assert_eq!(map_signal_type(SignalType::SignalPause), "pause");
        assert_eq!(map_signal_type(SignalType::SignalResume), "resume");
    }

    #[test]
    fn test_status_mapping_all_variants() {
        assert_eq!(map_status("pending"), InstanceStatus::StatusPending);
        assert_eq!(map_status("running"), InstanceStatus::StatusRunning);
        assert_eq!(map_status("suspended"), InstanceStatus::StatusSuspended);
        assert_eq!(map_status("completed"), InstanceStatus::StatusCompleted);
        assert_eq!(map_status("failed"), InstanceStatus::StatusFailed);
        assert_eq!(map_status("cancelled"), InstanceStatus::StatusCancelled);
        assert_eq!(map_status("invalid"), InstanceStatus::StatusUnknown);
        assert_eq!(map_status(""), InstanceStatus::StatusUnknown);
    }
}
