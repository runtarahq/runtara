// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP server for the environment protocol.
//!
//! Provides all environment management operations over HTTP/JSON.
//! Management SDK clients communicate with runtara-environment through this server.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::{
    Router,
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info};

use crate::db;
use crate::handlers::{
    self, EnvironmentHandlerState, GetCapabilityRequest, RegisterImageRequest,
    ResumeInstanceRequest, StartInstanceRequest, StopInstanceRequest,
};

/// Maximum body size for image uploads (64 MB).
const MAX_BODY_SIZE: usize = 64 * 1024 * 1024;

// ============================================================================
// JSON request/response types (mirror the protobuf types)
// ============================================================================

/// Register image request (JSON body).
#[derive(Debug, Deserialize)]
struct RegisterImageJsonRequest {
    tenant_id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// Base64-encoded binary content.
    binary: String,
    #[serde(default)]
    metadata: Option<Value>,
}

/// Register image response.
#[derive(Debug, Serialize)]
struct RegisterImageJsonResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// List images query parameters.
#[derive(Debug, Deserialize)]
struct ListImagesQuery {
    #[serde(default)]
    tenant_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

/// Get/delete image query parameters.
#[derive(Debug, Deserialize)]
struct ImageTenantQuery {
    #[serde(default)]
    tenant_id: Option<String>,
}

/// Start instance request (JSON body).
#[derive(Debug, Deserialize)]
struct StartInstanceJsonRequest {
    image_id: String,
    tenant_id: String,
    #[serde(default)]
    instance_id: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
}

/// Start instance response.
#[derive(Debug, Serialize)]
struct StartInstanceJsonResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deduplicated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Stop instance request (JSON body).
#[derive(Debug, Deserialize)]
struct StopInstanceJsonRequest {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    grace_period_seconds: Option<u64>,
}

/// Resume instance response.
#[derive(Debug, Serialize)]
struct SimpleSuccessResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// List instances query parameters.
#[derive(Debug, Deserialize)]
struct ListInstancesQuery {
    #[serde(default)]
    tenant_id: Option<String>,
    /// Single status filter. Superseded by `statuses`; kept so clients that
    /// predate the multi-status filter keep working.
    #[serde(default)]
    status: Option<String>,
    /// Comma-separated status filter — a row matches if it holds any one of
    /// them. Takes precedence over `status` when both are present.
    #[serde(default)]
    statuses: Option<String>,
    #[serde(default)]
    image_id: Option<String>,
    #[serde(default)]
    image_name_prefix: Option<String>,
    #[serde(default)]
    created_after_ms: Option<i64>,
    #[serde(default)]
    created_before_ms: Option<i64>,
    #[serde(default)]
    finished_after_ms: Option<i64>,
    #[serde(default)]
    finished_before_ms: Option<i64>,
    #[serde(default)]
    order_by: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

/// Resolve the status filter from the two query forms.
///
/// `statuses` (comma-separated) wins when present; `status` is the older
/// single-value form. Blank entries are dropped and repeats collapsed, so a
/// list that normalizes to nothing leaves the status unfiltered rather than
/// matching no rows.
fn resolve_status_filter(statuses: Option<&str>, status: Option<&str>) -> Option<Vec<String>> {
    let raw = statuses.or(status)?;

    let mut resolved: Vec<String> = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !resolved.iter().any(|kept| kept == entry) {
            resolved.push(entry.to_string());
        }
    }

    (!resolved.is_empty()).then_some(resolved)
}

/// Send signal request (JSON body).
#[derive(Debug, Deserialize)]
struct SendSignalJsonRequest {
    signal_type: String,
    #[serde(default)]
    payload: Option<String>,
}

/// Send custom signal request (JSON body).
#[derive(Debug, Deserialize)]
struct SendCustomSignalJsonRequest {
    checkpoint_id: String,
    #[serde(default)]
    payload: Option<String>,
}

/// List checkpoints query parameters.
#[derive(Debug, Deserialize)]
struct ListCheckpointsQuery {
    #[serde(default)]
    checkpoint_id: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
    #[serde(default)]
    created_after_ms: Option<i64>,
    #[serde(default)]
    created_before_ms: Option<i64>,
}

/// List events query parameters.
#[derive(Debug, Deserialize)]
struct ListEventsQuery {
    #[serde(default)]
    event_type: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
    #[serde(default)]
    created_after_ms: Option<i64>,
    #[serde(default)]
    created_before_ms: Option<i64>,
    #[serde(default)]
    payload_contains: Option<String>,
    #[serde(default)]
    scope_id: Option<String>,
    #[serde(default)]
    parent_scope_id: Option<String>,
    #[serde(default)]
    root_scopes_only: Option<bool>,
    #[serde(default)]
    sort_order: Option<String>,
}

/// List step summaries query parameters.
#[derive(Debug, Deserialize)]
struct ListStepSummariesQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    step_type: Option<String>,
    #[serde(default)]
    scope_id: Option<String>,
    #[serde(default)]
    parent_scope_id: Option<String>,
    #[serde(default)]
    root_scopes_only: Option<bool>,
    /// Comma-separated step ids to restrict the result to.
    #[serde(default)]
    step_ids: Option<String>,
    #[serde(default)]
    sort_order: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

/// Tenant metrics query parameters.
#[derive(Debug, Deserialize)]
struct TenantMetricsQuery {
    #[serde(default)]
    start_time_ms: Option<i64>,
    #[serde(default)]
    end_time_ms: Option<i64>,
    #[serde(default)]
    granularity: Option<String>,
}

// ============================================================================
// Helper functions
// ============================================================================

fn error_response(code: &str, message: &str, status: StatusCode) -> (StatusCode, Json<Value>) {
    build_error_response(code, message, status, ErrorDetail::default())
}

/// Emit an error response derived from an error value. Accepts anything
/// that converts into `crate::error::Error` (so callers can pass sqlx,
/// io, or core errors directly). Preserves the legacy `{error, code}`
/// shape and additively attaches structured fields (`category`,
/// `severity`, `retry_hint`, `retry_after_ms`, `attributes`) when the
/// underlying error carries them (e.g. `CoreError` → `StructuredError`).
/// Existing clients that read only `error` / `code` keep working
/// unchanged; new fields are purely additive.
fn error_response_from<E: Into<crate::error::Error>>(
    code: &str,
    err: E,
    status: StatusCode,
) -> (StatusCode, Json<Value>) {
    let err: crate::error::Error = err.into();
    let detail = detail_from_error(&err);
    build_error_response(code, &err.to_string(), status, detail)
}

/// Render an error that may be the caller's fault.
///
/// `Error::InvalidRequest` carries a message written for the caller, so it is
/// rendered verbatim as a 400 — wrapping it in the `Display` prefix would change
/// what clients read. Anything else is ours: log it, then 500. `log` runs only
/// on that second path, matching where these handlers logged before the
/// business logic moved out of them.
fn invalid_request_or(err: &crate::error::Error, code: &str, log: impl FnOnce()) -> Response {
    if let crate::error::Error::InvalidRequest(message) = err {
        return error_response("INVALID_REQUEST", message, StatusCode::BAD_REQUEST).into_response();
    }
    log();
    build_error_response(
        code,
        &err.to_string(),
        StatusCode::INTERNAL_SERVER_ERROR,
        detail_from_error(err),
    )
    .into_response()
}

fn build_error_response(
    code: &str,
    message: &str,
    status: StatusCode,
    detail: ErrorDetail,
) -> (StatusCode, Json<Value>) {
    let mut body = serde_json::Map::new();
    body.insert("error".into(), json!(message));
    body.insert("code".into(), json!(code));
    if let Some(v) = detail.category {
        body.insert("category".into(), json!(v));
    }
    if let Some(v) = detail.severity {
        body.insert("severity".into(), json!(v));
    }
    if let Some(v) = detail.retry_hint {
        body.insert("retry_hint".into(), json!(v));
    }
    if let Some(v) = detail.retry_after_ms {
        body.insert("retry_after_ms".into(), json!(v));
    }
    if let Some(v) = detail.attributes {
        body.insert("attributes".into(), v);
    }
    (status, Json(Value::Object(body)))
}

#[derive(Default)]
struct ErrorDetail {
    category: Option<&'static str>,
    severity: Option<&'static str>,
    retry_hint: Option<&'static str>,
    retry_after_ms: Option<u64>,
    attributes: Option<Value>,
}

fn detail_from_error(err: &crate::error::Error) -> ErrorDetail {
    use runtara_core::error::StructuredError;
    if let crate::error::Error::Core(core) = err {
        let s: StructuredError = core.clone().into();
        ErrorDetail {
            category: Some(s.category.as_str()),
            severity: Some(s.severity.as_str()),
            retry_hint: Some(s.retry_hint.as_str()),
            retry_after_ms: s.retry_hint.retry_after_ms(),
            attributes: if s.attributes.is_empty() {
                None
            } else {
                serde_json::to_value(&s.attributes).ok()
            },
        }
    } else {
        ErrorDetail::default()
    }
}

// ============================================================================
// HTTP handlers
// ============================================================================

/// GET /api/v1/health
async fn handle_health_check(
    State(state): State<Arc<EnvironmentHandlerState>>,
) -> impl IntoResponse {
    match handlers::handle_health_check(&state).await {
        Ok(resp) => Json(json!({
            "healthy": resp.healthy,
            "version": resp.version,
            "uptime_ms": resp.uptime_ms,
        }))
        .into_response(),
        Err(e) => {
            error!("Health check error: {}", e);
            error_response_from("HEALTH_CHECK_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// POST /api/v1/images — register image (JSON with base64 binary)
async fn handle_register_image(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Json(body): Json<RegisterImageJsonRequest>,
) -> impl IntoResponse {
    let binary = match base64::engine::general_purpose::STANDARD.decode(&body.binary) {
        Ok(b) => b,
        Err(e) => {
            return error_response(
                "INVALID_BINARY",
                &format!("Invalid base64 binary: {}", e),
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };

    let req = RegisterImageRequest {
        tenant_id: body.tenant_id,
        name: body.name,
        description: body.description,
        binary,
        metadata: body.metadata,
    };

    match handlers::handle_register_image(&state, req).await {
        Ok(resp) => {
            if resp.success {
                (
                    StatusCode::CREATED,
                    Json(RegisterImageJsonResponse {
                        success: true,
                        image_id: Some(resp.image_id),
                        error: None,
                    }),
                )
                    .into_response()
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(RegisterImageJsonResponse {
                        success: false,
                        image_id: None,
                        error: resp.error,
                    }),
                )
                    .into_response()
            }
        }
        Err(e) => {
            error!("Register image error: {}", e);
            error_response_from("REGISTER_IMAGE_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// POST /api/v1/images/upload — multipart upload for large images
async fn handle_register_image_upload(
    State(state): State<Arc<EnvironmentHandlerState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    use sha2::{Digest, Sha256};

    let mut tenant_id = String::new();
    let mut name = String::new();
    let mut description: Option<String> = None;
    let mut metadata: Option<Value> = None;
    let mut sha256_expected: Option<String> = None;
    let mut binary_data: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "tenant_id" => {
                tenant_id = field.text().await.unwrap_or_default();
            }
            "name" => {
                name = field.text().await.unwrap_or_default();
            }
            "description" => {
                description = Some(field.text().await.unwrap_or_default());
            }
            // `runner_type` selected between backends when more than one
            // existed. Accept and ignore it so older clients don't 400.
            "runner_type" => {
                let _ = field.text().await;
            }
            "metadata" => {
                if let Ok(text) = field.text().await {
                    metadata = serde_json::from_str(&text).ok();
                }
            }
            "sha256" => {
                sha256_expected = Some(field.text().await.unwrap_or_default());
            }
            "binary" => match field.bytes().await {
                Ok(bytes) => binary_data = Some(bytes.to_vec()),
                Err(e) => {
                    return error_response(
                        "UPLOAD_ERROR",
                        &format!("Failed to read binary field: {}", e),
                        StatusCode::BAD_REQUEST,
                    )
                    .into_response();
                }
            },
            _ => {
                // Ignore unknown fields
            }
        }
    }

    let binary = match binary_data {
        Some(b) => b,
        None => {
            return error_response(
                "MISSING_BINARY",
                "binary field is required",
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };

    if tenant_id.is_empty() {
        return error_response(
            "MISSING_TENANT_ID",
            "tenant_id field is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    if name.is_empty() {
        return error_response(
            "MISSING_NAME",
            "name field is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    // Verify SHA-256 if provided
    if let Some(ref expected) = sha256_expected {
        let mut hasher = Sha256::new();
        hasher.update(&binary);
        let actual = format!("{:x}", hasher.finalize());
        if &actual != expected {
            return error_response(
                "CHECKSUM_MISMATCH",
                &format!("Checksum mismatch: expected {}, got {}", expected, actual),
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    }

    let params = handlers::StoreImageParams {
        tenant_id,
        name,
        description,
        metadata,
    };

    match handlers::handle_store_image(&state, params, &binary).await {
        Ok(image_id) => (
            StatusCode::CREATED,
            Json(RegisterImageJsonResponse {
                success: true,
                image_id: Some(image_id),
                error: None,
            }),
        )
            .into_response(),
        Err(handlers::StoreImageError::Io(message)) => {
            error_response("IO_ERROR", &message, StatusCode::INTERNAL_SERVER_ERROR).into_response()
        }
        Err(handlers::StoreImageError::Lookup(message))
        | Err(handlers::StoreImageError::Register(message)) => error_response(
            "REGISTER_IMAGE_ERROR",
            &message,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

/// GET /api/v1/images — list images
async fn handle_list_images(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Query(query): Query<ListImagesQuery>,
) -> impl IntoResponse {
    let params = handlers::ListImagesParams {
        tenant_id: query.tenant_id,
        name: query.name,
        limit: query.limit.unwrap_or(100) as i64,
        offset: query.offset.unwrap_or(0) as i64,
    };

    match handlers::handle_list_images(&state, &params).await {
        Ok(images) => Json(json!({
            "total_count": images.len(),
            "images": images,
        }))
        .into_response(),
        Err(e) => {
            error!("List images error: {}", e);
            error_response_from("LIST_IMAGES_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// GET /api/v1/images/{image_id} — get image
async fn handle_get_image(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(image_id): Path<String>,
    Query(query): Query<ImageTenantQuery>,
) -> impl IntoResponse {
    match handlers::handle_get_image(&state, &image_id, query.tenant_id.as_deref()).await {
        Ok(Some(image)) => Json(json!({ "found": true, "image": image })).into_response(),
        Ok(None) => Json(json!({ "found": false })).into_response(),
        Err(e) => invalid_request_or(&e, "GET_IMAGE_ERROR", || {
            error!("Get image error: {}", e);
        }),
    }
}

/// DELETE /api/v1/images/{image_id} — delete image
async fn handle_delete_image(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(image_id): Path<String>,
    Query(query): Query<ImageTenantQuery>,
) -> impl IntoResponse {
    match handlers::handle_delete_image(&state, &image_id, query.tenant_id.as_deref()).await {
        Ok(true) => Json(json!({ "success": true })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": format!("Image '{}' not found", image_id)
            })),
        )
            .into_response(),
        Err(e) => invalid_request_or(&e, "DELETE_IMAGE_ERROR", || {
            error!("Delete image error: {}", e);
        }),
    }
}

/// POST /api/v1/instances — start instance
async fn handle_start_instance(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Json(body): Json<StartInstanceJsonRequest>,
) -> impl IntoResponse {
    let req = StartInstanceRequest {
        image_id: body.image_id,
        tenant_id: body.tenant_id,
        instance_id: body.instance_id,
        input: body.input,
        timeout_seconds: body.timeout_seconds,
        env: body.env,
    };

    match handlers::handle_start_instance(&state, req).await {
        Ok(resp) => {
            if resp.success {
                (
                    if resp.deduplicated {
                        StatusCode::OK
                    } else {
                        StatusCode::CREATED
                    },
                    Json(StartInstanceJsonResponse {
                        success: true,
                        instance_id: Some(resp.instance_id),
                        deduplicated: resp.deduplicated,
                        error: None,
                    }),
                )
                    .into_response()
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(StartInstanceJsonResponse {
                        success: false,
                        instance_id: if resp.instance_id.is_empty() {
                            None
                        } else {
                            Some(resp.instance_id)
                        },
                        deduplicated: false,
                        error: resp.error,
                    }),
                )
                    .into_response()
            }
        }
        Err(e) => {
            error!("Start instance error: {}", e);
            error_response_from("START_INSTANCE_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/stop — stop instance
async fn handle_stop_instance(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<StopInstanceJsonRequest>,
) -> impl IntoResponse {
    let req = StopInstanceRequest {
        instance_id,
        reason: body.reason.unwrap_or_default(),
        grace_period_seconds: body.grace_period_seconds.unwrap_or(5),
    };

    match handlers::handle_stop_instance(&state, req).await {
        Ok(resp) => Json(SimpleSuccessResponse {
            success: resp.success,
            error: resp.error,
        })
        .into_response(),
        Err(e) => {
            error!("Stop instance error: {}", e);
            error_response_from("STOP_INSTANCE_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/resume — resume instance
async fn handle_resume_instance(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
) -> impl IntoResponse {
    let req = ResumeInstanceRequest { instance_id };

    match handlers::handle_resume_instance(&state, req).await {
        Ok(resp) => Json(SimpleSuccessResponse {
            success: resp.success,
            error: resp.error,
        })
        .into_response(),
        Err(e) => {
            error!("Resume instance error: {}", e);
            error_response_from(
                "RESUME_INSTANCE_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response()
        }
    }
}

/// GET /api/v1/instances/{instance_id} — get instance status
async fn handle_get_instance_status(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
) -> impl IntoResponse {
    match handlers::handle_get_instance_status(&state, &instance_id).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => {
            error!("Get instance status error: {}", e);
            error_response_from(
                "GET_INSTANCE_STATUS_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response()
        }
    }
}

/// GET /api/v1/instances — list instances
async fn handle_list_instances(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Query(query): Query<ListInstancesQuery>,
) -> impl IntoResponse {
    use chrono::TimeZone;

    let to_time = |ms: Option<i64>| ms.and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());

    let options = db::ListInstancesOptions {
        tenant_id: query.tenant_id,
        statuses: resolve_status_filter(query.statuses.as_deref(), query.status.as_deref()),
        image_id: query.image_id,
        image_name_prefix: query.image_name_prefix,
        created_after: to_time(query.created_after_ms),
        created_before: to_time(query.created_before_ms),
        finished_after: to_time(query.finished_after_ms),
        finished_before: to_time(query.finished_before_ms),
        order_by: query.order_by,
        limit: query.limit.unwrap_or(100) as i64,
        offset: query.offset.unwrap_or(0) as i64,
    };

    match handlers::handle_list_instances(&state, &options).await {
        Ok(result) => Json(json!({
            "instances": result.instances,
            "total_count": result.total_count,
        }))
        .into_response(),
        Err(e) => {
            error!("List instances error: {}", e);
            error_response_from("LIST_INSTANCES_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/signals — send signal
async fn handle_send_signal(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<SendSignalJsonRequest>,
) -> impl IntoResponse {
    use handlers::SendSignalOutcome;

    match handlers::handle_send_signal(
        &state,
        &instance_id,
        &body.signal_type,
        body.payload.as_deref(),
    )
    .await
    {
        Ok(SendSignalOutcome::Delivered) => Json(json!({ "success": true })).into_response(),
        Ok(SendSignalOutcome::InstanceNotFound) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": format!("Instance '{}' not found", instance_id)
            })),
        )
            .into_response(),
        Ok(SendSignalOutcome::NotSignalable { status }) => (
            StatusCode::CONFLICT,
            Json(json!({
                "success": false,
                "error": format!("Cannot send signal to instance in '{}' state", status)
            })),
        )
            .into_response(),
        Ok(SendSignalOutcome::UnknownSignalType { signal_type }) => error_response(
            "INVALID_SIGNAL_TYPE",
            &format!("Unknown signal type: {}", signal_type),
            StatusCode::BAD_REQUEST,
        )
        .into_response(),
        Err(e) => {
            error!("Send signal error: {}", e);
            error_response_from("SEND_SIGNAL_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// POST /api/v1/instances/{instance_id}/signals/custom — send custom signal
async fn handle_send_custom_signal(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<SendCustomSignalJsonRequest>,
) -> impl IntoResponse {
    use handlers::SendCustomSignalOutcome;

    match handlers::handle_send_custom_signal(
        &state,
        &instance_id,
        &body.checkpoint_id,
        body.payload.as_deref(),
    )
    .await
    {
        Ok(SendCustomSignalOutcome::Delivered) => Json(json!({ "success": true })).into_response(),
        Ok(SendCustomSignalOutcome::InstanceNotFound) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": format!("Instance '{}' not found", instance_id)
            })),
        )
            .into_response(),
        Err(e) => invalid_request_or(&e, "SEND_CUSTOM_SIGNAL_ERROR", || {
            error!("Send custom signal error: {}", e);
        }),
    }
}

/// GET /api/v1/instances/{instance_id}/checkpoints — list checkpoints
async fn handle_list_checkpoints(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Query(query): Query<ListCheckpointsQuery>,
) -> impl IntoResponse {
    let params = handlers::ListCheckpointsParams {
        checkpoint_id: query.checkpoint_id,
        created_after: query
            .created_after_ms
            .and_then(chrono::DateTime::from_timestamp_millis),
        created_before: query
            .created_before_ms
            .and_then(chrono::DateTime::from_timestamp_millis),
        limit: query.limit.unwrap_or(100) as i64,
        offset: query.offset.unwrap_or(0) as i64,
    };

    match handlers::handle_list_checkpoints(&state, &instance_id, &params).await {
        Ok(result) => Json(json!({
            "checkpoints": result.checkpoints,
            "total_count": result.total_count,
            "limit": params.limit,
            "offset": params.offset,
        }))
        .into_response(),
        Err(e) => {
            error!("List checkpoints error: {}", e);
            error_response_from(
                "LIST_CHECKPOINTS_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response()
        }
    }
}

/// GET /api/v1/instances/{instance_id}/checkpoints/{checkpoint_id} — get checkpoint
async fn handle_get_checkpoint(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path((instance_id, checkpoint_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Percent-decode the checkpoint_id (it may contain special characters)
    let checkpoint_id = percent_encoding::percent_decode_str(&checkpoint_id)
        .decode_utf8_lossy()
        .to_string();

    match handlers::handle_get_checkpoint(&state, &instance_id, &checkpoint_id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(e) => {
            error!("Get checkpoint error: {}", e);
            error_response_from("GET_CHECKPOINT_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// GET /api/v1/instances/{instance_id}/events — list events
async fn handle_list_events(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Query(query): Query<ListEventsQuery>,
) -> impl IntoResponse {
    use runtara_core::persistence::{EventSortOrder, ListEventsFilter};

    let limit = query.limit.unwrap_or(100) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    let filter = ListEventsFilter {
        event_type: query.event_type,
        subtype: query.subtype,
        created_after: query
            .created_after_ms
            .and_then(chrono::DateTime::from_timestamp_millis),
        created_before: query
            .created_before_ms
            .and_then(chrono::DateTime::from_timestamp_millis),
        payload_contains: query.payload_contains,
        scope_id: query.scope_id,
        parent_scope_id: query.parent_scope_id,
        root_scopes_only: query.root_scopes_only.unwrap_or(false),
        sort_order: match query.sort_order.as_deref() {
            Some("asc") => EventSortOrder::Asc,
            _ => EventSortOrder::Desc,
        },
    };

    match handlers::handle_list_events(&state, &instance_id, &filter, limit, offset).await {
        Ok(result) => Json(json!({
            "events": result.events,
            "total_count": result.total_count,
            "limit": limit,
            "offset": offset,
        }))
        .into_response(),
        Err(e) => {
            error!("List events error: {}", e);
            error_response_from("LIST_EVENTS_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// GET /api/v1/instances/{instance_id}/steps — list step summaries
async fn handle_list_step_summaries(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Query(query): Query<ListStepSummariesQuery>,
) -> impl IntoResponse {
    use runtara_core::persistence::{EventSortOrder, ListStepSummariesFilter, StepStatus};

    let limit = query.limit.unwrap_or(100) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    let filter = ListStepSummariesFilter {
        sort_order: match query.sort_order.as_deref() {
            Some("asc") => EventSortOrder::Asc,
            _ => EventSortOrder::Desc,
        },
        status: match query.status.as_deref() {
            Some("running") => Some(StepStatus::Running),
            Some("completed") => Some(StepStatus::Completed),
            Some("failed") => Some(StepStatus::Failed),
            _ => None,
        },
        step_type: query.step_type,
        scope_id: query.scope_id,
        parent_scope_id: query.parent_scope_id,
        root_scopes_only: query.root_scopes_only.unwrap_or(false),
        step_ids: query
            .step_ids
            .map(|ids| {
                ids.split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|ids| !ids.is_empty()),
    };

    match handlers::handle_list_step_summaries(&state, &instance_id, &filter, limit, offset).await {
        Ok(result) => Json(json!({
            "steps": result.steps,
            "total_count": result.total_count,
            "limit": limit,
            "offset": offset,
        }))
        .into_response(),
        Err(e) => invalid_request_or(&e, "LIST_STEP_SUMMARIES_ERROR", || {
            error!("List step summaries error: {}", e);
        }),
    }
}

/// GET /api/v1/instances/{instance_id}/scopes/{scope_id}/ancestors — get scope ancestors
async fn handle_get_scope_ancestors(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path((instance_id, scope_id)): Path<(String, String)>,
) -> impl IntoResponse {
    match handlers::handle_get_scope_ancestors(&state, &instance_id, &scope_id).await {
        Ok(ancestors) => Json(json!({ "ancestors": ancestors })).into_response(),
        Err(e) => invalid_request_or(&e, "GET_SCOPE_ANCESTORS_ERROR", || {
            error!("Get scope ancestors error: {}", e);
        }),
    }
}

/// GET /api/v1/tenants/{tenant_id}/metrics — get tenant metrics
async fn handle_get_tenant_metrics(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(tenant_id): Path<String>,
    Query(query): Query<TenantMetricsQuery>,
) -> impl IntoResponse {
    let now = chrono::Utc::now();
    let end_time = query
        .end_time_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or(now);
    let start_time = query
        .start_time_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or(end_time - chrono::Duration::hours(24));

    let granularity = match query.granularity.as_deref() {
        Some("daily") => db::MetricsGranularity::Daily,
        _ => db::MetricsGranularity::Hourly,
    };

    let options = db::TenantMetricsOptions {
        tenant_id: tenant_id.clone(),
        start_time,
        end_time,
        granularity,
    };

    match handlers::handle_get_tenant_metrics(&state, &options).await {
        Ok(buckets) => Json(json!({
            "tenant_id": tenant_id,
            "start_time_ms": start_time.timestamp_millis(),
            "end_time_ms": end_time.timestamp_millis(),
            "granularity": match granularity {
                db::MetricsGranularity::Hourly => "hourly",
                db::MetricsGranularity::Daily => "daily",
            },
            "buckets": buckets,
        }))
        .into_response(),
        Err(e) => invalid_request_or(&e, "GET_TENANT_METRICS_ERROR", || {
            error!("Get tenant metrics error: {}", e);
        }),
    }
}

/// GET /api/v1/agents — list agents
async fn handle_list_agents(
    State(state): State<Arc<EnvironmentHandlerState>>,
) -> impl IntoResponse {
    match handlers::handle_list_agents(&state).await {
        Ok(resp) => {
            // agents_json is a Vec<u8> containing JSON
            match serde_json::from_slice::<Value>(&resp.agents_json) {
                Ok(agents) => Json(json!({ "agents": agents })).into_response(),
                Err(_) => {
                    // Fall back to base64 if not valid JSON
                    Json(json!({
                        "agents_json": base64::engine::general_purpose::STANDARD.encode(&resp.agents_json)
                    }))
                    .into_response()
                }
            }
        }
        Err(e) => {
            error!("List agents error: {}", e);
            error_response_from("LIST_AGENTS_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// GET /api/v1/agents/{agent_id}/capabilities/{capability_id} — get capability
async fn handle_get_capability(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path((agent_id, capability_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let req = GetCapabilityRequest {
        agent_id,
        capability_id,
    };

    match handlers::handle_get_capability(&state, req).await {
        Ok(resp) => {
            if resp.found {
                let inputs =
                    serde_json::from_slice::<Value>(&resp.inputs_json).unwrap_or(Value::Null);
                let capability =
                    serde_json::from_slice::<Value>(&resp.capability_json).unwrap_or(Value::Null);

                Json(json!({
                    "found": true,
                    "capability": capability,
                    "inputs": inputs,
                }))
                .into_response()
            } else {
                (StatusCode::NOT_FOUND, Json(json!({ "found": false }))).into_response()
            }
        }
        Err(e) => {
            error!("Get capability error: {}", e);
            error_response_from("GET_CAPABILITY_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

// ============================================================================
// Router and server
// ============================================================================

/// Build the environment protocol HTTP router.
///
/// All routes are prefixed with `/api/v1`.
pub fn environment_http_router(state: Arc<EnvironmentHandlerState>) -> Router {
    Router::new()
        // Health check
        .route("/api/v1/health", get(handle_health_check))
        // Image registry
        .route(
            "/api/v1/images",
            post(handle_register_image).get(handle_list_images),
        )
        .route("/api/v1/images/upload", post(handle_register_image_upload))
        .route(
            "/api/v1/images/{image_id}",
            get(handle_get_image).delete(handle_delete_image),
        )
        // Instance lifecycle
        .route(
            "/api/v1/instances",
            post(handle_start_instance).get(handle_list_instances),
        )
        .route(
            "/api/v1/instances/{instance_id}",
            get(handle_get_instance_status),
        )
        .route(
            "/api/v1/instances/{instance_id}/stop",
            post(handle_stop_instance),
        )
        .route(
            "/api/v1/instances/{instance_id}/resume",
            post(handle_resume_instance),
        )
        // Signals
        .route(
            "/api/v1/instances/{instance_id}/signals",
            post(handle_send_signal),
        )
        .route(
            "/api/v1/instances/{instance_id}/signals/custom",
            post(handle_send_custom_signal),
        )
        // Checkpoints
        .route(
            "/api/v1/instances/{instance_id}/checkpoints",
            get(handle_list_checkpoints),
        )
        .route(
            "/api/v1/instances/{instance_id}/checkpoints/{checkpoint_id}",
            get(handle_get_checkpoint),
        )
        // Events
        .route(
            "/api/v1/instances/{instance_id}/events",
            get(handle_list_events),
        )
        // Step summaries
        .route(
            "/api/v1/instances/{instance_id}/steps",
            get(handle_list_step_summaries),
        )
        // Scope ancestors
        .route(
            "/api/v1/instances/{instance_id}/scopes/{scope_id}/ancestors",
            get(handle_get_scope_ancestors),
        )
        // Tenant metrics
        .route(
            "/api/v1/tenants/{tenant_id}/metrics",
            get(handle_get_tenant_metrics),
        )
        // Agent testing
        .route("/api/v1/agents", get(handle_list_agents))
        .route(
            "/api/v1/agents/{agent_id}/capabilities/{capability_id}",
            get(handle_get_capability),
        )
        // Body size limit for uploads
        .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
        .with_state(state)
}

/// Run the environment HTTP server.
///
/// Starts an axum HTTP server on the given address, serving the environment
/// protocol API.
pub async fn run_http_server(
    bind_addr: SocketAddr,
    state: Arc<EnvironmentHandlerState>,
) -> anyhow::Result<()> {
    let app = environment_http_router(state);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;

    info!(addr = %bind_addr, "Environment HTTP server starting");

    axum::serve(listener, app)
        .await
        .map_err(|e| anyhow::anyhow!("HTTP server error: {}", e))?;

    info!("Environment HTTP server stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtara_core::error::CoreError;

    fn owned(values: &[&str]) -> Option<Vec<String>> {
        Some(values.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn resolve_status_filter_splits_the_multi_status_form() {
        assert_eq!(
            resolve_status_filter(Some("failed,cancelled"), None),
            owned(&["failed", "cancelled"])
        );
    }

    #[test]
    fn resolve_status_filter_prefers_statuses_over_status() {
        // A client that sends both means the list; `status` is only there for
        // servers that do not understand `statuses`.
        assert_eq!(
            resolve_status_filter(Some("failed,cancelled"), Some("failed")),
            owned(&["failed", "cancelled"])
        );
    }

    #[test]
    fn resolve_status_filter_falls_back_to_the_single_form() {
        assert_eq!(
            resolve_status_filter(None, Some("running")),
            owned(&["running"])
        );
    }

    #[test]
    fn resolve_status_filter_normalizes_whitespace_and_repeats() {
        assert_eq!(
            resolve_status_filter(Some(" failed , cancelled ,failed"), None),
            owned(&["failed", "cancelled"])
        );
    }

    #[test]
    fn resolve_status_filter_treats_an_empty_list_as_no_filter() {
        assert_eq!(resolve_status_filter(Some(" , "), None), None);
        assert_eq!(resolve_status_filter(Some(""), None), None);
        assert_eq!(resolve_status_filter(None, None), None);
    }

    fn body_of(resp: (StatusCode, Json<Value>)) -> Value {
        resp.1.0
    }

    #[test]
    fn error_response_preserves_legacy_shape() {
        let body = body_of(error_response(
            "HEALTH_CHECK_ERROR",
            "database down",
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
        assert_eq!(body["error"], "database down");
        assert_eq!(body["code"], "HEALTH_CHECK_ERROR");
        assert!(
            body.get("category").is_none(),
            "no structured fields without detail"
        );
        assert!(body.get("severity").is_none());
    }

    #[test]
    fn error_response_from_attaches_structured_fields_for_core_errors() {
        let err = crate::error::Error::from(CoreError::InstanceNotFound {
            instance_id: "inst-42".to_string(),
        });
        let body = body_of(error_response_from(
            "GET_INSTANCE_STATUS_ERROR",
            err,
            StatusCode::NOT_FOUND,
        ));
        // Legacy fields preserved verbatim
        assert_eq!(body["code"], "GET_INSTANCE_STATUS_ERROR");
        assert!(body["error"].as_str().unwrap().contains("inst-42"));
        // New additive fields
        assert_eq!(body["category"], "permanent");
        assert_eq!(body["severity"], "error");
        assert_eq!(body["retry_hint"], "do_not_retry");
    }

    #[test]
    fn error_response_from_transient_db_error_hints_retry() {
        let err = crate::error::Error::from(CoreError::CheckpointSaveFailed {
            instance_id: "inst-1".to_string(),
            reason: "timeout".to_string(),
        });
        let body = body_of(error_response_from(
            "SAVE_CHECKPOINT_ERROR",
            err,
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
        assert_eq!(body["category"], "transient");
        assert_eq!(body["retry_hint"], "retry_with_backoff");
    }

    #[test]
    fn error_response_from_non_core_error_stays_legacy() {
        // sqlx errors wrap into crate::error::Error::Database, not Core —
        // so no structured fields are attached, only legacy error/code.
        let err = crate::error::Error::Other("unexpected state".to_string());
        let body = body_of(error_response_from(
            "OTHER_ERROR",
            err,
            StatusCode::INTERNAL_SERVER_ERROR,
        ));
        assert_eq!(body["code"], "OTHER_ERROR");
        assert_eq!(body["error"], "unexpected state");
        assert!(body.get("category").is_none());
    }
}
