// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP server for the instance protocol.
//!
//! Provides instance protocol operations over HTTP/JSON.
//! This enables scenarios (native or WASM) using the HTTP SDK backend
//! to communicate with runtara-core.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info, warn};

use crate::instance_handlers::{
    self, CheckpointRequest as HandlerCheckpointRequest,
    GetInstanceStatusRequest as HandlerGetStatusRequest, InstanceEvent as HandlerInstanceEvent,
    InstanceEventType as HandlerEventType, InstanceHandlerState, InstanceStatus,
    PollSignalsRequest as HandlerPollSignalsRequest,
    RegisterInstanceRequest as HandlerRegisterRequest,
    RetryAttemptEvent as HandlerRetryAttemptEvent, SignalAck as HandlerSignalAck, SignalType,
    SleepRequest as HandlerSleepRequest,
};

// ============================================================================
// JSON request/response types (mirror the protobuf types)
// ============================================================================

/// Register instance request
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    /// Tenant ID
    pub tenant_id: String,
    /// Optional checkpoint ID to resume from
    #[serde(default)]
    pub checkpoint_id: Option<String>,
}

/// Register instance response
#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Checkpoint request
#[derive(Debug, Deserialize)]
pub struct CheckpointRequest {
    /// Checkpoint identifier (unique per durable function call)
    pub checkpoint_id: String,
    /// Serialized workflow state (base64-encoded)
    pub state: String,
}

/// Checkpoint response
#[derive(Debug, Serialize)]
pub struct CheckpointResponse {
    /// True if a checkpoint with this ID already existed (resume case)
    pub found: bool,
    /// Existing checkpoint state (base64-encoded, present when found=true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Pending instance-wide signal (cancel/pause)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<SignalInfo>,
    /// Pending custom signal (WaitForSignal)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_signal: Option<CustomSignalInfo>,
    /// Last error from a previous checkpoint attempt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<ErrorInfo>,
}

/// Signal information
#[derive(Debug, Serialize)]
pub struct SignalInfo {
    pub signal_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
}

/// Custom signal information
#[derive(Debug, Serialize)]
pub struct CustomSignalInfo {
    pub checkpoint_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
}

/// Error information
#[derive(Debug, Serialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
}

/// Poll signals response
#[derive(Debug, Serialize)]
pub struct PollSignalsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<SignalInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_signal: Option<CustomSignalInfo>,
}

/// Instance event request
#[derive(Debug, Deserialize)]
pub struct InstanceEventRequest {
    /// Event type: "completed", "failed", "suspended", "custom"
    pub event_type: String,
    #[serde(default)]
    pub checkpoint_id: Option<String>,
    /// Payload (base64-encoded)
    #[serde(default)]
    pub payload: Option<String>,
    #[serde(default)]
    pub subtype: Option<String>,
}

/// Sleep request
#[derive(Debug, Deserialize)]
pub struct SleepRequest {
    pub duration_ms: u64,
    pub checkpoint_id: String,
    /// Serialized state (base64-encoded)
    pub state: String,
}

/// Signal acknowledgement request
#[derive(Debug, Deserialize)]
pub struct SignalAckRequest {
    pub signal_type: String,
}

/// Retry attempt event
#[derive(Debug, Deserialize)]
pub struct RetryAttemptRequest {
    pub checkpoint_id: String,
    pub attempt: u32,
    #[serde(default)]
    pub error_message: Option<String>,
}

/// Generic success response
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
}

// ============================================================================
// Helper: convert proto signal types
// ============================================================================

fn signal_type_to_string(st: i32) -> String {
    match st {
        0 => "cancel".to_string(),   // SignalCancel
        1 => "pause".to_string(),    // SignalPause
        2 => "resume".to_string(),   // SignalResume
        3 => "shutdown".to_string(), // SignalShutdown
        _ => format!("unknown({})", st),
    }
}

fn event_type_from_string(s: &str) -> i32 {
    match s {
        "heartbeat" => HandlerEventType::EventHeartbeat as i32,
        "completed" => HandlerEventType::EventCompleted as i32,
        "failed" => HandlerEventType::EventFailed as i32,
        "suspended" => HandlerEventType::EventSuspended as i32,
        "custom" => HandlerEventType::EventCustom as i32,
        _ => HandlerEventType::EventCustom as i32,
    }
}

// ============================================================================
// HTTP handlers
// ============================================================================

/// POST /api/v1/instances/{instance_id}/register
async fn register_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<RegisterRequest>,
) -> impl IntoResponse {
    let request = HandlerRegisterRequest {
        instance_id,
        tenant_id: body.tenant_id,
        checkpoint_id: body.checkpoint_id,
    };

    match instance_handlers::handle_register_instance(&state, request).await {
        Ok(resp) => {
            if resp.success {
                Json(RegisterResponse {
                    success: true,
                    error: None,
                })
                .into_response()
            } else {
                let status = match resp.error.as_str() {
                    instance_handlers::ERROR_SERVER_DRAINING => StatusCode::SERVICE_UNAVAILABLE,
                    instance_handlers::ERROR_MAX_CONCURRENT_INSTANCES => {
                        StatusCode::TOO_MANY_REQUESTS
                    }
                    _ => StatusCode::BAD_REQUEST,
                };
                let body = Json(RegisterResponse {
                    success: false,
                    error: Some(resp.error),
                });
                // Surface Retry-After for the rate-limited/draining cases so SDK
                // clients can back off sensibly.
                if status == StatusCode::SERVICE_UNAVAILABLE
                    || status == StatusCode::TOO_MANY_REQUESTS
                {
                    (status, [("Retry-After", "30")], body).into_response()
                } else {
                    (status, body).into_response()
                }
            }
        }
        Err(e) => {
            error!("Register handler error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "REGISTER_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/checkpoint
async fn checkpoint_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<CheckpointRequest>,
) -> impl IntoResponse {
    use base64::Engine;

    let state_bytes = match base64::engine::general_purpose::STANDARD.decode(&body.state) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid base64 state: {}", e),
                    "code": "INVALID_STATE"
                })),
            )
                .into_response();
        }
    };

    let request = HandlerCheckpointRequest {
        instance_id,
        checkpoint_id: body.checkpoint_id,
        state: state_bytes,
    };

    match instance_handlers::handle_checkpoint(&state, request).await {
        Ok(resp) => {
            let signal = resp.pending_signal.map(|s| SignalInfo {
                signal_type: signal_type_to_string(s.signal_type),
                payload: if s.payload.is_empty() {
                    None
                } else {
                    Some(base64::engine::general_purpose::STANDARD.encode(&s.payload))
                },
            });

            let custom_signal = resp.custom_signal.map(|cs| CustomSignalInfo {
                checkpoint_id: cs.checkpoint_id,
                payload: if cs.payload.is_empty() {
                    None
                } else {
                    Some(base64::engine::general_purpose::STANDARD.encode(&cs.payload))
                },
            });

            let last_error = resp.last_error.map(|e| ErrorInfo {
                code: e.code,
                message: e.message,
            });

            Json(CheckpointResponse {
                found: resp.found,
                state: if resp.state.is_empty() {
                    None
                } else {
                    Some(base64::engine::general_purpose::STANDARD.encode(&resp.state))
                },
                signal,
                custom_signal,
                last_error,
            })
            .into_response()
        }
        Err(e) => {
            error!("Checkpoint handler error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "CHECKPOINT_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/instances/{instance_id}/signals
async fn poll_signals_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
) -> impl IntoResponse {
    let request = HandlerPollSignalsRequest {
        instance_id,
        checkpoint_id: None,
    };

    match instance_handlers::handle_poll_signals(&state, request).await {
        Ok(resp) => {
            let signal = resp.signal.map(|s| SignalInfo {
                signal_type: signal_type_to_string(s.signal_type),
                payload: if s.payload.is_empty() {
                    None
                } else {
                    Some(base64::engine::general_purpose::STANDARD.encode(&s.payload))
                },
            });

            let custom_signal = resp.custom_signal.map(|cs| CustomSignalInfo {
                checkpoint_id: cs.checkpoint_id,
                payload: if cs.payload.is_empty() {
                    None
                } else {
                    Some(base64::engine::general_purpose::STANDARD.encode(&cs.payload))
                },
            });

            Json(PollSignalsResponse {
                signal,
                custom_signal,
            })
            .into_response()
        }
        Err(e) => {
            error!("Poll signals error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "POLL_SIGNALS_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/instances/{instance_id}/signals/{signal_id}
async fn poll_custom_signal_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path((instance_id, signal_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let request = HandlerPollSignalsRequest {
        instance_id,
        checkpoint_id: Some(signal_id),
    };

    match instance_handlers::handle_poll_signals(&state, request).await {
        Ok(resp) => {
            let custom_signal = resp.custom_signal.map(|cs| CustomSignalInfo {
                checkpoint_id: cs.checkpoint_id,
                payload: if cs.payload.is_empty() {
                    None
                } else {
                    Some(base64::engine::general_purpose::STANDARD.encode(&cs.payload))
                },
            });

            Json(PollSignalsResponse {
                signal: None,
                custom_signal,
            })
            .into_response()
        }
        Err(e) => {
            error!("Poll custom signal error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "POLL_CUSTOM_SIGNAL_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/events
async fn instance_event_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<InstanceEventRequest>,
) -> impl IntoResponse {
    let payload = body
        .payload
        .as_deref()
        .map(|p| base64::engine::general_purpose::STANDARD.decode(p))
        .transpose();

    let payload = match payload {
        Ok(p) => p.unwrap_or_default(),
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid base64 payload: {}", e),
                    "code": "INVALID_PAYLOAD"
                })),
            )
                .into_response();
        }
    };

    let event = HandlerInstanceEvent {
        instance_id,
        event_type: event_type_from_string(&body.event_type),
        checkpoint_id: body.checkpoint_id,
        payload,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: body.subtype,
    };

    match instance_handlers::handle_instance_event(&state, event).await {
        Ok(resp) => {
            if resp.success {
                Json(SuccessResponse { success: true }).into_response()
            } else {
                let error = resp.error.unwrap_or_else(|| "Unknown error".to_string());
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "success": false,
                        "error": error,
                    })),
                )
                    .into_response()
            }
        }
        Err(e) => {
            error!("Instance event error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "EVENT_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/completed
async fn completed_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let payload = body
        .get("output")
        .and_then(|v| v.as_str())
        .map(|s| base64::engine::general_purpose::STANDARD.decode(s))
        .transpose();

    let payload = match payload {
        Ok(p) => p.unwrap_or_default(),
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid base64 output: {}", e),
                    "code": "INVALID_OUTPUT"
                })),
            )
                .into_response();
        }
    };

    let event = HandlerInstanceEvent {
        instance_id,
        event_type: HandlerEventType::EventCompleted as i32,
        checkpoint_id: None,
        payload,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: None,
    };

    match instance_handlers::handle_instance_event(&state, event).await {
        Ok(_) => Json(SuccessResponse { success: true }).into_response(),
        Err(e) => {
            error!("Completed handler error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "COMPLETED_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/failed
async fn failed_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let error_msg = body
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown error");

    let event = HandlerInstanceEvent {
        instance_id,
        event_type: HandlerEventType::EventFailed as i32,
        checkpoint_id: None,
        payload: error_msg.as_bytes().to_vec(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: None,
    };

    match instance_handlers::handle_instance_event(&state, event).await {
        Ok(_) => Json(SuccessResponse { success: true }).into_response(),
        Err(e) => {
            error!("Failed handler error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "FAILED_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/suspended
async fn suspended_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
) -> impl IntoResponse {
    let event = HandlerInstanceEvent {
        instance_id,
        event_type: HandlerEventType::EventSuspended as i32,
        checkpoint_id: None,
        payload: Vec::new(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        subtype: None,
    };

    match instance_handlers::handle_instance_event(&state, event).await {
        Ok(_) => Json(SuccessResponse { success: true }).into_response(),
        Err(e) => {
            error!("Suspended handler error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "SUSPENDED_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/sleep
async fn sleep_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<SleepRequest>,
) -> impl IntoResponse {
    let state_bytes = match base64::engine::general_purpose::STANDARD.decode(&body.state) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Invalid base64 state: {}", e),
                    "code": "INVALID_STATE"
                })),
            )
                .into_response();
        }
    };

    let request = HandlerSleepRequest {
        instance_id,
        duration_ms: body.duration_ms,
        checkpoint_id: body.checkpoint_id,
        state: state_bytes,
    };

    match instance_handlers::handle_sleep(&state, request).await {
        Ok(_) => Json(SuccessResponse { success: true }).into_response(),
        Err(e) => {
            error!("Sleep handler error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "SLEEP_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/signals/ack
async fn signal_ack_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<SignalAckRequest>,
) -> impl IntoResponse {
    let signal_type = match body.signal_type.as_str() {
        "cancel" => SignalType::SignalCancel as i32,
        "pause" => SignalType::SignalPause as i32,
        "resume" => SignalType::SignalResume as i32,
        "shutdown" => SignalType::SignalShutdown as i32,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Unknown signal type: {}", body.signal_type),
                    "code": "INVALID_SIGNAL_TYPE"
                })),
            )
                .into_response();
        }
    };

    let ack = HandlerSignalAck {
        instance_id,
        signal_type,
        acknowledged: true,
    };

    match instance_handlers::handle_signal_ack(&state, ack).await {
        Ok(()) => Json(SuccessResponse { success: true }).into_response(),
        Err(e) => {
            warn!("Signal ack error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "SIGNAL_ACK_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/retry
async fn retry_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<RetryAttemptRequest>,
) -> impl IntoResponse {
    let event = HandlerRetryAttemptEvent {
        instance_id,
        checkpoint_id: body.checkpoint_id,
        attempt_number: body.attempt,
        error_message: body.error_message,
        error_metadata: None,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    };

    match instance_handlers::handle_retry_attempt(&state, event).await {
        Ok(()) => Json(SuccessResponse { success: true }).into_response(),
        Err(e) => {
            warn!("Retry attempt error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "RETRY_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// Instance status response
#[derive(Debug, Serialize)]
pub struct InstanceStatusResponse {
    pub found: bool,
    pub instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>, // base64
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// GET /api/v1/instances/{instance_id}/status
async fn status_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
) -> impl IntoResponse {
    let request = HandlerGetStatusRequest {
        instance_id: instance_id.clone(),
    };

    match instance_handlers::handle_get_instance_status(&state, request).await {
        Ok(resp) => {
            let status_str = match InstanceStatus::try_from_i32(resp.status) {
                Some(InstanceStatus::StatusPending) => "pending",
                Some(InstanceStatus::StatusRunning) => "running",
                Some(InstanceStatus::StatusSuspended) => "suspended",
                Some(InstanceStatus::StatusCompleted) => "completed",
                Some(InstanceStatus::StatusFailed) => "failed",
                Some(InstanceStatus::StatusCancelled) => "cancelled",
                _ => "unknown",
            };

            let output = resp
                .output
                .as_ref()
                .map(|o| base64::engine::general_purpose::STANDARD.encode(o));

            let found =
                status_str != "unknown" || resp.error.as_deref() != Some("Instance not found");

            Json(InstanceStatusResponse {
                found,
                instance_id: resp.instance_id,
                status: Some(status_str.to_string()),
                checkpoint_id: resp.checkpoint_id,
                output,
                error: resp.error,
            })
            .into_response()
        }
        Err(e) => {
            error!("Status handler error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "STATUS_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/instances/{instance_id}/input
async fn input_handler(
    State(state): State<Arc<InstanceHandlerState>>,
    Path(instance_id): Path<String>,
) -> impl IntoResponse {
    match state.persistence.get_instance(&instance_id).await {
        Ok(Some(inst)) => {
            if let Some(input_bytes) = inst.input {
                let encoded = base64::engine::general_purpose::STANDARD.encode(&input_bytes);
                Json(json!({
                    "found": true,
                    "instance_id": instance_id,
                    "input": encoded,
                }))
                .into_response()
            } else {
                Json(json!({
                    "found": true,
                    "instance_id": instance_id,
                    "input": null,
                }))
                .into_response()
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "found": false,
                "instance_id": instance_id,
                "error": "Instance not found",
            })),
        )
            .into_response(),
        Err(e) => {
            error!("Input handler error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": e.to_string(),
                    "code": "INPUT_ERROR"
                })),
            )
                .into_response()
        }
    }
}

/// GET /health
async fn health_handler(State(state): State<Arc<InstanceHandlerState>>) -> impl IntoResponse {
    let db_ok = state.persistence.health_check_db().await.unwrap_or(false);
    if db_ok {
        Json(json!({"status": "healthy"})).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "unhealthy",
                "error": "database check failed"
            })),
        )
            .into_response()
    }
}

// ============================================================================
// Router and server
// ============================================================================

/// Build the instance protocol HTTP router.
///
/// All routes are prefixed with `/api/v1`.
pub fn instance_http_router(state: Arc<InstanceHandlerState>) -> Router {
    Router::new()
        // Instance lifecycle
        .route(
            "/api/v1/instances/{instance_id}/register",
            post(register_handler),
        )
        // Checkpointing
        .route(
            "/api/v1/instances/{instance_id}/checkpoint",
            post(checkpoint_handler),
        )
        // Signal polling
        .route(
            "/api/v1/instances/{instance_id}/signals",
            get(poll_signals_handler),
        )
        .route(
            "/api/v1/instances/{instance_id}/signals/{signal_id}",
            get(poll_custom_signal_handler),
        )
        .route(
            "/api/v1/instances/{instance_id}/signals/ack",
            post(signal_ack_handler),
        )
        // Instance events (completion, failure, suspension)
        .route(
            "/api/v1/instances/{instance_id}/completed",
            post(completed_handler),
        )
        .route(
            "/api/v1/instances/{instance_id}/failed",
            post(failed_handler),
        )
        .route(
            "/api/v1/instances/{instance_id}/suspended",
            post(suspended_handler),
        )
        .route(
            "/api/v1/instances/{instance_id}/events",
            post(instance_event_handler),
        )
        // Sleep/wake
        .route("/api/v1/instances/{instance_id}/sleep", post(sleep_handler))
        // Retry tracking
        .route("/api/v1/instances/{instance_id}/retry", post(retry_handler))
        // Instance status
        .route(
            "/api/v1/instances/{instance_id}/status",
            get(status_handler),
        )
        // Instance input
        .route("/api/v1/instances/{instance_id}/input", get(input_handler))
        // Health check
        .route("/health", get(health_handler))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}

/// Run the instance HTTP server.
///
/// Starts an axum HTTP server on the given address, serving the instance
/// protocol API for all clients (native scenarios, WASM scenarios, debugging, etc.).
pub async fn run_http_server(
    bind_addr: SocketAddr,
    state: Arc<InstanceHandlerState>,
) -> anyhow::Result<()> {
    let app = instance_http_router(state);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    info!(addr = %bind_addr, "Instance HTTP server starting");

    axum::serve(listener, app)
        .await
        .map_err(|e| anyhow::anyhow!("HTTP server error: {}", e))?;

    info!("Instance HTTP server stopped");
    Ok(())
}
