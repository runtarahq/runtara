// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP server for the instance protocol.
//!
//! Provides runtara-core's instance protocol operations over HTTP/JSON. This
//! enables workflows (native or WASM) using the SDK's HTTP backend to reach
//! core over a socket; workflows composed against the runtime as a host import
//! reach the same handlers in-process instead, through
//! `runtara_environment::runtime_host`.
//!
//! Every handler here is wire plumbing — decode, delegate to
//! [`runtara_core::instance_handlers`], map the error to a status. Core owns
//! the semantics and knows nothing about HTTP.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info, warn};

use runtara_core::error::{CoreError, CoreErrorClass};
use runtara_core::instance_handlers::{
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
// Helper: map handler failures onto the wire
// ============================================================================

/// The HTTP status that describes a [`CoreError`].
///
/// 4xx says "stop, the request is wrong"; 5xx says "retry, I am unwell".
/// Collapsing both onto 500 makes an ordinary drain race — a checkpoint landing
/// on an instance that already finished — indistinguishable from the database
/// being down.
///
/// The classification comes from core, which matches its own variants
/// exhaustively in [`CoreError::classify`] — so a new variant fails to compile
/// until someone decides what it means, rather than silently defaulting back to
/// 500 here. This function only translates that verdict into HTTP.
fn status_for_core(err: &CoreError) -> StatusCode {
    match err.classify() {
        CoreErrorClass::Missing => StatusCode::NOT_FOUND,
        CoreErrorClass::Conflict => StatusCode::CONFLICT,
        CoreErrorClass::Invalid => StatusCode::BAD_REQUEST,
        CoreErrorClass::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// The status for a handler failure.
///
/// Handlers return `anyhow::Error`, so the classification is only available when
/// a [`CoreError`] is underneath. When there is none, nothing has told us the
/// failure is the caller's fault, and 500 is the honest answer.
fn status_for(err: &anyhow::Error) -> StatusCode {
    err.downcast_ref::<CoreError>()
        .map(status_for_core)
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// How long a client should wait before retrying, in seconds. Shared with the
/// drain and concurrency-cap refusals so every "come back later" this server
/// sends asks for the same delay.
pub(crate) const RETRY_AFTER_SECONDS: &str = "30";

/// Render a handler failure: status from the error's own classification, body in
/// the established `{error, code}` shape carrying the caller's route code.
///
/// Server-side failures log at `error`, caller mistakes at `warn` — a checkpoint
/// racing a drain is routine and should stop reading like an outage. A 503 also
/// carries `Retry-After`, since telling a client to retry without saying when
/// invites it to hammer a server that is already struggling.
fn core_error_response(route_code: &str, context: &str, err: impl Into<anyhow::Error>) -> Response {
    let err: anyhow::Error = err.into();
    let status = status_for(&err);
    if status.is_server_error() {
        error!(
            code = route_code,
            status = status.as_u16(),
            "{}: {}",
            context,
            err
        );
    } else {
        warn!(
            code = route_code,
            status = status.as_u16(),
            "{}: {}",
            context,
            err
        );
    }

    let body = Json(json!({
        "error": err.to_string(),
        "code": route_code,
    }));

    if status == StatusCode::SERVICE_UNAVAILABLE {
        (status, [("Retry-After", RETRY_AFTER_SECONDS)], body).into_response()
    } else {
        (status, body).into_response()
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
                    (status, [("Retry-After", RETRY_AFTER_SECONDS)], body).into_response()
                } else {
                    (status, body).into_response()
                }
            }
        }
        Err(e) => core_error_response("REGISTER_ERROR", "Register handler error", e),
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

            Json(CheckpointResponse {
                found: resp.found,
                state: if resp.state.is_empty() {
                    None
                } else {
                    Some(base64::engine::general_purpose::STANDARD.encode(&resp.state))
                },
                signal,
                custom_signal,
            })
            .into_response()
        }
        Err(e) => core_error_response("CHECKPOINT_ERROR", "Checkpoint handler error", e),
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
        Err(e) => core_error_response("POLL_SIGNALS_ERROR", "Poll signals error", e),
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
        Err(e) => core_error_response("POLL_CUSTOM_SIGNAL_ERROR", "Poll custom signal error", e),
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
                // No `CoreError` underneath to classify — this arm carries only a
                // string, so 500 is the honest answer rather than a guess.
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
        Err(e) => core_error_response("EVENT_ERROR", "Instance event error", e),
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
        Err(e) => core_error_response("COMPLETED_ERROR", "Completed handler error", e),
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
        Err(e) => core_error_response("FAILED_ERROR", "Failed handler error", e),
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
        Err(e) => core_error_response("SUSPENDED_ERROR", "Suspended handler error", e),
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
        Err(e) => core_error_response("SLEEP_ERROR", "Sleep handler error", e),
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
        Err(e) => core_error_response("SIGNAL_ACK_ERROR", "Signal ack error", e),
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
        Err(e) => core_error_response("RETRY_ERROR", "Retry attempt error", e),
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
        Err(e) => core_error_response("STATUS_ERROR", "Status handler error", e),
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
        Err(e) => core_error_response("INPUT_ERROR", "Input handler error", e),
    }
}

/// GET /health
async fn health_handler(State(state): State<Arc<InstanceHandlerState>>) -> impl IntoResponse {
    let db_ok = state.persistence.health_check().await.unwrap_or(false);
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

/// Run the instance HTTP server until the process ends.
///
/// Starts an axum HTTP server on the given address, serving the instance
/// protocol API for all clients (native workflows, WASM workflows, debugging, etc.).
///
/// This never returns on its own. To be able to stop the server without
/// severing in-flight requests, use [`run_http_server_with_shutdown`].
pub async fn run_http_server(
    bind_addr: SocketAddr,
    state: Arc<InstanceHandlerState>,
) -> anyhow::Result<()> {
    run_http_server_with_shutdown(bind_addr, state, std::future::pending()).await
}

/// Run the instance HTTP server until `shutdown` resolves.
///
/// Once `shutdown` completes the listener stops accepting, and the server waits
/// for the requests already being served to finish before returning. Instances
/// mid-checkpoint therefore get to finish writing rather than being cut off —
/// which is the whole point of the drain sequence in
/// [`CoreRuntime`](super::CoreRuntime).
///
/// Nothing here bounds that wait: a caller that needs shutdown to be bounded
/// applies its own deadline, as `CoreRuntime::shutdown` does.
pub async fn run_http_server_with_shutdown(
    bind_addr: SocketAddr,
    state: Arc<InstanceHandlerState>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let app = instance_http_router(state);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    info!(addr = %bind_addr, "Instance HTTP server starting");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| anyhow::anyhow!("HTTP server error: {}", e))?;

    info!("Instance HTTP server stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use runtara_core::instance_handlers::mock_persistence::{MockPersistence, make_instance};
    use tower::ServiceExt;

    fn router_over(persistence: MockPersistence) -> Router {
        instance_http_router(Arc::new(InstanceHandlerState::new(Arc::new(persistence))))
    }

    /// Drive a request through the real router and return `(status, body)`.
    ///
    /// Going through `oneshot` rather than calling the handler directly is the
    /// point: it exercises the routing and extraction the handlers are wired
    /// into, so a mapping that is correct in isolation but mis-wired still fails.
    async fn send(router: Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = router.oneshot(request).await.expect("router call failed");
        let (status, body, _) = read(response).await;
        (status, body)
    }

    /// Unpack a response into `(status, body, retry_after)`.
    async fn read(response: Response) -> (StatusCode, Value, Option<String>) {
        let status = response.status();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .map(|v| v.to_str().expect("Retry-After was not text").to_string());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read body");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("body was not JSON")
        };
        (status, body, retry_after)
    }

    fn post(path: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get(path: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    fn checkpoint_body() -> Value {
        json!({ "checkpoint_id": "cp-1", "state": "aGk=" })
    }

    #[test]
    fn status_for_core_classifies_every_core_error() {
        // Exhaustive by construction: `CoreError` has eight variants and all
        // eight are listed here, so a new one cannot slip through untested.
        let cases: Vec<(CoreError, StatusCode)> = vec![
            (
                CoreError::InstanceNotFound {
                    instance_id: "i".into(),
                },
                StatusCode::NOT_FOUND,
            ),
            (
                CoreError::CheckpointNotFound {
                    instance_id: "i".into(),
                    checkpoint_id: Some("cp".into()),
                },
                StatusCode::NOT_FOUND,
            ),
            (
                CoreError::InvalidInstanceState {
                    instance_id: "i".into(),
                    expected: "running".into(),
                    actual: "completed".into(),
                },
                StatusCode::CONFLICT,
            ),
            (
                CoreError::InstanceAlreadyExists {
                    instance_id: "i".into(),
                },
                StatusCode::CONFLICT,
            ),
            (
                CoreError::ValidationError {
                    field: "instance_id".into(),
                    message: "required".into(),
                },
                StatusCode::BAD_REQUEST,
            ),
            (
                CoreError::PersistenceError {
                    operation: "insert".into(),
                    details: "connection refused".into(),
                },
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                CoreError::CheckpointSaveFailed {
                    instance_id: "i".into(),
                    reason: "disk full".into(),
                },
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                CoreError::SignalDeliveryFailed {
                    instance_id: "i".into(),
                    signal_type: "cancel".into(),
                    reason: "timeout".into(),
                },
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(
                status_for_core(&error),
                expected,
                "{:?} should map to {}",
                error,
                expected
            );
        }
    }

    #[test]
    fn an_error_with_no_core_error_underneath_stays_internal() {
        // Handlers return `anyhow::Error`, so a failure can arrive with nothing
        // that classifies it. That case must keep answering 500 rather than
        // being guessed into a 4xx the caller would wrongly treat as final.
        let opaque = anyhow::anyhow!("something went wrong");
        assert_eq!(status_for(&opaque), StatusCode::INTERNAL_SERVER_ERROR);

        // ...and a `CoreError` still classifies once wrapped by anyhow, which is
        // how every handler error actually reaches the HTTP layer.
        let wrapped = anyhow::Error::from(CoreError::InstanceNotFound {
            instance_id: "i".into(),
        });
        assert_eq!(status_for(&wrapped), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn core_error_response_keeps_the_route_code_in_the_body() {
        // The status is what this ticket changes; the body shape is a contract
        // existing clients already read, so it must not move with it.
        let (status, body, retry_after) = read(core_error_response(
            "CHECKPOINT_ERROR",
            "Checkpoint handler error",
            CoreError::InstanceNotFound {
                instance_id: "inst-1".into(),
            },
        ))
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "CHECKPOINT_ERROR");
        assert!(body["error"].as_str().unwrap().contains("inst-1"));
        // Retry-After belongs to "come back later", not to "you were wrong".
        assert_eq!(retry_after, None);
    }

    #[tokio::test]
    async fn a_503_says_when_to_come_back() {
        // The drain and cap refusals already send this. A 503 without it invites
        // a client to hammer a server that is already struggling.
        let (status, _, retry_after) = read(core_error_response(
            "REGISTER_ERROR",
            "Register handler error",
            CoreError::PersistenceError {
                operation: "register_instance".into(),
                details: "connection refused".into(),
            },
        ))
        .await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry_after.as_deref(), Some(RETRY_AFTER_SECONDS));
    }

    #[tokio::test]
    async fn checkpoint_on_an_unknown_instance_is_not_found() {
        let (status, body) = send(
            router_over(MockPersistence::new()),
            post("/api/v1/instances/nope/checkpoint", checkpoint_body()),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "CHECKPOINT_ERROR");
    }

    #[tokio::test]
    async fn checkpoint_on_a_terminal_instance_is_a_conflict() {
        // The drain race this ticket exists for: the instance finished between
        // the client's last step and this checkpoint. The client is wrong, not
        // the server, and retrying will never help.
        let persistence = MockPersistence::new().with_instance(make_instance(
            "done",
            "t1",
            runtara_core::domain::InstanceStatus::Completed,
        ));

        let (status, body) = send(
            router_over(persistence),
            post("/api/v1/instances/done/checkpoint", checkpoint_body()),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "CHECKPOINT_ERROR");
    }

    #[tokio::test]
    async fn a_database_failure_is_service_unavailable() {
        // The other half of the distinction: same route, but the server really
        // is unwell, so the client should back off rather than give up.
        let persistence = MockPersistence::new();
        persistence.set_fail_register();

        let response = router_over(persistence)
            .oneshot(post(
                "/api/v1/instances/inst-1/register",
                json!({ "tenant_id": "t1" }),
            ))
            .await
            .expect("router call failed");
        let (status, body, retry_after) = read(response).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "REGISTER_ERROR");
        assert_eq!(retry_after.as_deref(), Some(RETRY_AFTER_SECONDS));
    }

    #[tokio::test]
    async fn resuming_from_a_missing_checkpoint_is_not_found() {
        // The same fact as a missing checkpoint on the checkpoint route, so it
        // has to get the same status — registration used to answer 400 here.
        let persistence = MockPersistence::new().with_instance(make_instance(
            "inst-1",
            "t1",
            runtara_core::domain::InstanceStatus::Pending,
        ));

        let (status, body) = send(
            router_over(persistence),
            post(
                "/api/v1/instances/inst-1/register",
                json!({ "tenant_id": "t1", "checkpoint_id": "nope" }),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "REGISTER_ERROR");
    }

    #[tokio::test]
    async fn input_of_an_unknown_instance_stays_not_found() {
        // Already 404 before this change — pinned so the rewrite cannot regress it.
        let (status, body) = send(
            router_over(MockPersistence::new()),
            get("/api/v1/instances/nope/input"),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["found"], false);
    }

    #[tokio::test]
    async fn a_checkpoint_that_succeeds_is_untouched() {
        // The rewrite touched every error arm; this proves it left the happy
        // path — and its response body — alone.
        let persistence = MockPersistence::new().with_instance(make_instance(
            "live",
            "t1",
            runtara_core::domain::InstanceStatus::Running,
        ));

        let (status, body) = send(
            router_over(persistence),
            post("/api/v1/instances/live/checkpoint", checkpoint_body()),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["found"], false);
    }
}
