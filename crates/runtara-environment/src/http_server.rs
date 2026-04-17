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
    response::{IntoResponse, Json},
    routing::{get, post},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info, warn};

use crate::db;
use crate::handlers::{
    self, EnvironmentHandlerState, GetCapabilityRequest, RegisterImageRequest,
    ResumeInstanceRequest, StartInstanceRequest, StopInstanceRequest, TestCapabilityRequest,
};
use crate::image_registry::{ImageRegistry, RunnerType};

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
    runner_type: Option<String>,
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

/// Image summary (used in list/get responses).
#[derive(Debug, Serialize)]
struct ImageSummaryJson {
    image_id: String,
    tenant_id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    runner_type: String,
    created_at_ms: i64,
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

/// Instance status response.
#[derive(Debug, Serialize)]
struct InstanceStatusJsonResponse {
    found: bool,
    instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at_ms: Option<i64>,
    /// Base64-encoded output.
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    /// Base64-encoded input.
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    heartbeat_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_retries: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_peak_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu_usage_usec: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    termination_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
}

/// List instances query parameters.
#[derive(Debug, Deserialize)]
struct ListInstancesQuery {
    #[serde(default)]
    tenant_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
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

/// Instance summary for list responses.
#[derive(Debug, Serialize)]
struct InstanceSummaryJson {
    instance_id: String,
    tenant_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_id: Option<String>,
    status: String,
    created_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at_ms: Option<i64>,
    has_error: bool,
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

/// Checkpoint summary.
#[derive(Debug, Serialize)]
struct CheckpointSummaryJson {
    checkpoint_id: String,
    instance_id: String,
    created_at_ms: i64,
    data_size_bytes: u64,
}

/// Full checkpoint response.
#[derive(Debug, Serialize)]
struct CheckpointDetailJson {
    found: bool,
    checkpoint_id: String,
    instance_id: String,
    created_at_ms: i64,
    /// Base64-encoded checkpoint data.
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
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

/// Event summary.
#[derive(Debug, Serialize)]
struct EventSummaryJson {
    id: i64,
    instance_id: String,
    event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint_id: Option<String>,
    /// Base64-encoded payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<String>,
    created_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtype: Option<String>,
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
    #[serde(default)]
    sort_order: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

/// Step summary.
#[derive(Debug, Serialize)]
struct StepSummaryJson {
    step_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_name: Option<String>,
    step_type: String,
    status: String,
    started_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inputs: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outputs: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_scope_id: Option<String>,
}

/// Scope info for ancestor response.
#[derive(Debug, Serialize)]
struct ScopeInfoJson {
    scope_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_scope_id: Option<String>,
    step_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_name: Option<String>,
    step_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<u32>,
    created_at_ms: i64,
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

/// Metrics bucket.
#[derive(Debug, Serialize)]
struct MetricsBucketJson {
    bucket_time_ms: i64,
    invocation_count: i64,
    success_count: i64,
    failure_count: i64,
    cancelled_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_memory_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_memory_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    success_rate_percent: Option<f64>,
}

/// Test capability request (JSON body).
#[derive(Debug, Deserialize)]
struct TestCapabilityJsonRequest {
    tenant_id: String,
    agent_id: String,
    capability_id: String,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    connection: Option<Value>,
    #[serde(default)]
    timeout_ms: Option<u32>,
}

// ============================================================================
// Helper functions
// ============================================================================

fn runner_type_from_string(s: &str) -> RunnerType {
    match s.to_lowercase().as_str() {
        "native" | "1" => RunnerType::Native,
        "wasm" | "2" => RunnerType::Wasm,
        _ => RunnerType::Oci,
    }
}

fn runner_type_to_string(rt: RunnerType) -> &'static str {
    match rt {
        RunnerType::Oci => "oci",
        RunnerType::Native => "native",
        RunnerType::Wasm => "wasm",
    }
}

fn instance_status_to_string(status: &str) -> &str {
    match status {
        "pending" => "pending",
        "running" => "running",
        "suspended" | "sleeping" => "suspended",
        "completed" => "completed",
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ => "unknown",
    }
}

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

    let runner_type = body
        .runner_type
        .as_deref()
        .map(runner_type_from_string)
        .unwrap_or(RunnerType::Oci);

    let req = RegisterImageRequest {
        tenant_id: body.tenant_id,
        name: body.name,
        description: body.description,
        binary,
        runner_type,
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
    use std::io::Write;

    let mut tenant_id = String::new();
    let mut name = String::new();
    let mut description: Option<String> = None;
    let mut runner_type_str: Option<String> = None;
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
            "runner_type" => {
                runner_type_str = Some(field.text().await.unwrap_or_default());
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

    // Now create the image using the same logic as handle_register_image_stream in server.rs
    let image_id = uuid::Uuid::new_v4().to_string();
    let images_dir = state.data_dir.join("images").join(&image_id);
    let binary_path = images_dir.join("binary");
    let bundle_path = images_dir.join("bundle");

    if let Err(e) = std::fs::create_dir_all(&images_dir) {
        error!(error = %e, "Failed to create image directory");
        return error_response(
            "IO_ERROR",
            &format!("Failed to create image directory: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
    }

    if let Err(e) = std::fs::File::create(&binary_path).and_then(|mut f| f.write_all(&binary)) {
        error!(error = %e, "Failed to write binary");
        let _ = std::fs::remove_dir_all(&images_dir);
        return error_response(
            "IO_ERROR",
            &format!("Failed to write binary: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
    }

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755));
    }

    // Create OCI bundle if needed
    let runner_type = runner_type_str
        .as_deref()
        .map(runner_type_from_string)
        .unwrap_or(RunnerType::Oci);

    let bundle_path_str = if runner_type == RunnerType::Oci {
        if let Err(e) = crate::runner::oci::create_bundle_at_path(&bundle_path, &binary_path) {
            let _ = std::fs::remove_dir_all(&images_dir);
            return error_response(
                "BUNDLE_ERROR",
                &format!("Failed to create OCI bundle: {}", e),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
        Some(bundle_path.to_string_lossy().to_string())
    } else {
        None
    };

    // Build image
    let mut builder =
        crate::image_registry::ImageBuilder::new(&tenant_id, &name, binary_path.to_string_lossy())
            .runner_type(runner_type);

    if let Some(desc) = &description {
        builder = builder.description(desc);
    }
    if let Some(bp) = &bundle_path_str {
        builder = builder.bundle_path(bp);
    }
    if let Some(meta) = metadata {
        builder = builder.metadata(meta);
    }

    let mut image = builder.build();
    image.image_id = image_id.clone();

    // Register in database
    let image_registry = ImageRegistry::new(state.pool.clone());
    if let Err(e) = image_registry.register(&image).await {
        let _ = std::fs::remove_dir_all(&images_dir);
        return error_response(
            "REGISTER_IMAGE_ERROR",
            &format!("Failed to register image: {}", e),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response();
    }

    info!(image_id = %image_id, bytes = binary.len(), "Streaming image registration complete (HTTP)");

    (
        StatusCode::CREATED,
        Json(RegisterImageJsonResponse {
            success: true,
            image_id: Some(image_id),
            error: None,
        }),
    )
        .into_response()
}

/// GET /api/v1/images — list images
async fn handle_list_images(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Query(query): Query<ListImagesQuery>,
) -> impl IntoResponse {
    let image_registry = ImageRegistry::new(state.pool.clone());

    let limit = query.limit.unwrap_or(100) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    let images_result = if let Some(ref tenant_id) = query.tenant_id {
        if let Some(ref name) = query.name {
            // Filter by tenant and name
            match image_registry.get_by_name(tenant_id, name).await {
                Ok(Some(img)) => Ok(vec![img]),
                Ok(None) => Ok(vec![]),
                Err(e) => Err(e),
            }
        } else {
            image_registry
                .list_by_tenant(tenant_id, limit, offset)
                .await
        }
    } else {
        image_registry.list_all(limit, offset).await
    };

    match images_result {
        Ok(images) => {
            let summaries: Vec<ImageSummaryJson> = images
                .into_iter()
                .map(|img| ImageSummaryJson {
                    image_id: img.image_id,
                    tenant_id: img.tenant_id,
                    name: img.name,
                    description: img.description,
                    runner_type: runner_type_to_string(img.runner_type).to_string(),
                    created_at_ms: img.created_at.timestamp_millis(),
                })
                .collect();
            Json(json!({
                "images": summaries,
                "total_count": summaries.len(),
            }))
            .into_response()
        }
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
    let image_registry = ImageRegistry::new(state.pool.clone());

    if image_id.is_empty() {
        return error_response(
            "INVALID_REQUEST",
            "image_id is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    match image_registry.get(&image_id).await {
        Ok(Some(img)) => {
            // Tenant isolation
            if let Some(ref tenant_id) = query.tenant_id
                && img.tenant_id != *tenant_id
            {
                return Json(json!({ "found": false })).into_response();
            }

            Json(json!({
                "found": true,
                "image": ImageSummaryJson {
                    image_id: img.image_id,
                    tenant_id: img.tenant_id,
                    name: img.name,
                    description: img.description,
                    runner_type: runner_type_to_string(img.runner_type).to_string(),
                    created_at_ms: img.created_at.timestamp_millis(),
                }
            }))
            .into_response()
        }
        Ok(None) => Json(json!({ "found": false })).into_response(),
        Err(e) => {
            error!("Get image error: {}", e);
            error_response_from("GET_IMAGE_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
    }
}

/// DELETE /api/v1/images/{image_id} — delete image
async fn handle_delete_image(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(image_id): Path<String>,
    Query(query): Query<ImageTenantQuery>,
) -> impl IntoResponse {
    let image_registry = ImageRegistry::new(state.pool.clone());

    if image_id.is_empty() {
        return error_response(
            "INVALID_REQUEST",
            "image_id is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    match image_registry.get(&image_id).await {
        Ok(Some(img)) => {
            // Tenant isolation
            if let Some(ref tenant_id) = query.tenant_id
                && img.tenant_id != *tenant_id
            {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "success": false,
                        "error": format!("Image '{}' not found", image_id)
                    })),
                )
                    .into_response();
            }

            if let Err(e) = image_registry.delete(&image_id).await {
                return error_response_from(
                    "DELETE_IMAGE_ERROR",
                    e,
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
                .into_response();
            }

            // Delete files
            let images_dir = state.data_dir.join("images").join(&image_id);
            let _ = std::fs::remove_dir_all(&images_dir);

            Json(json!({ "success": true })).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "success": false,
                "error": format!("Image '{}' not found", image_id)
            })),
        )
            .into_response(),
        Err(e) => {
            error!("Delete image error: {}", e);
            error_response_from("DELETE_IMAGE_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response()
        }
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
                    StatusCode::CREATED,
                    Json(StartInstanceJsonResponse {
                        success: true,
                        instance_id: Some(resp.instance_id),
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
    match db::get_instance_full(&state.pool, &instance_id).await {
        Ok(Some(inst)) => {
            let status_str = instance_status_to_string(&inst.status);

            Json(InstanceStatusJsonResponse {
                found: true,
                instance_id: inst.instance_id,
                status: Some(status_str.to_string()),
                tenant_id: Some(inst.tenant_id),
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
                heartbeat_at_ms: inst.heartbeat_at.map(|t| t.timestamp_millis()),
                retry_count: Some(inst.attempt as u32),
                max_retries: Some(inst.max_attempts as u32),
                memory_peak_bytes: inst.memory_peak_bytes.map(|v| v as u64),
                cpu_usage_usec: inst.cpu_usage_usec.map(|v| v as u64),
                termination_reason: inst.termination_reason,
                exit_code: inst.exit_code,
            })
            .into_response()
        }
        Ok(None) => Json(InstanceStatusJsonResponse {
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
            heartbeat_at_ms: None,
            retry_count: None,
            max_retries: None,
            memory_peak_bytes: None,
            cpu_usage_usec: None,
            termination_reason: None,
            exit_code: None,
        })
        .into_response(),
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

    let limit = query.limit.unwrap_or(100) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    // Convert status string to match DB format
    let status = query.status;

    // Convert milliseconds to DateTime
    let created_after = query
        .created_after_ms
        .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());
    let created_before = query
        .created_before_ms
        .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());
    let finished_after = query
        .finished_after_ms
        .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());
    let finished_before = query
        .finished_before_ms
        .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single());

    let options = db::ListInstancesOptions {
        tenant_id: query.tenant_id,
        status,
        image_id: query.image_id,
        image_name_prefix: query.image_name_prefix,
        created_after,
        created_before,
        finished_after,
        finished_before,
        order_by: query.order_by,
        limit,
        offset,
    };

    let instances = match db::list_instances(&state.pool, &options).await {
        Ok(v) => v,
        Err(e) => {
            error!("List instances error: {}", e);
            return error_response_from(
                "LIST_INSTANCES_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    };

    let total_count = match db::count_instances(&state.pool, &options).await {
        Ok(c) => c,
        Err(e) => {
            warn!("Count instances error: {}", e);
            0
        }
    };

    let summaries: Vec<InstanceSummaryJson> = instances
        .into_iter()
        .map(|inst| InstanceSummaryJson {
            instance_id: inst.instance_id,
            tenant_id: inst.tenant_id,
            image_id: inst.image_id,
            status: instance_status_to_string(&inst.status).to_string(),
            created_at_ms: inst.created_at.timestamp_millis(),
            started_at_ms: inst.started_at.map(|t| t.timestamp_millis()),
            finished_at_ms: inst.finished_at.map(|t| t.timestamp_millis()),
            has_error: inst.error.is_some(),
        })
        .collect();

    Json(json!({
        "instances": summaries,
        "total_count": total_count,
    }))
    .into_response()
}

/// POST /api/v1/instances/{instance_id}/signals — send signal
async fn handle_send_signal(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Json(body): Json<SendSignalJsonRequest>,
) -> impl IntoResponse {
    // Validate instance exists
    let instance = match state.persistence.get_instance(&instance_id).await {
        Ok(Some(inst)) => inst,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "success": false,
                    "error": format!("Instance '{}' not found", instance_id)
                })),
            )
                .into_response();
        }
        Err(e) => {
            return error_response_from("SEND_SIGNAL_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response();
        }
    };

    // Check terminal state
    if !matches!(
        instance.status.as_str(),
        "running" | "suspended" | "pending"
    ) {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "success": false,
                "error": format!("Cannot send signal to instance in '{}' state", instance.status)
            })),
        )
            .into_response();
    }

    // Map signal type
    let signal_type = match body.signal_type.as_str() {
        "cancel" => "cancel",
        "pause" => "pause",
        "resume" => "resume",
        _ => {
            return error_response(
                "INVALID_SIGNAL_TYPE",
                &format!("Unknown signal type: {}", body.signal_type),
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };

    let payload = body
        .payload
        .as_deref()
        .map(|p| p.as_bytes().to_vec())
        .unwrap_or_default();

    match state
        .persistence
        .insert_signal(&instance_id, signal_type, &payload)
        .await
    {
        Ok(()) => Json(json!({ "success": true })).into_response(),
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
    // Validate instance
    let instance = match state.persistence.get_instance(&instance_id).await {
        Ok(Some(inst)) => inst,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "success": false,
                    "error": format!("Instance '{}' not found", instance_id)
                })),
            )
                .into_response();
        }
        Err(e) => {
            return error_response_from(
                "SEND_CUSTOM_SIGNAL_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    };

    let _ = instance; // Validate existence

    if body.checkpoint_id.is_empty() {
        return error_response(
            "INVALID_REQUEST",
            "checkpoint_id is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    let payload = body
        .payload
        .as_deref()
        .map(|p| p.as_bytes().to_vec())
        .unwrap_or_default();

    match state
        .persistence
        .insert_custom_signal(&instance_id, &body.checkpoint_id, &payload)
        .await
    {
        Ok(()) => Json(json!({ "success": true })).into_response(),
        Err(e) => {
            error!("Send custom signal error: {}", e);
            error_response_from(
                "SEND_CUSTOM_SIGNAL_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response()
        }
    }
}

/// GET /api/v1/instances/{instance_id}/checkpoints — list checkpoints
async fn handle_list_checkpoints(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Query(query): Query<ListCheckpointsQuery>,
) -> impl IntoResponse {
    let created_after = query
        .created_after_ms
        .and_then(chrono::DateTime::from_timestamp_millis);
    let created_before = query
        .created_before_ms
        .and_then(chrono::DateTime::from_timestamp_millis);

    let limit = query.limit.unwrap_or(100) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    let checkpoints = match state
        .persistence
        .list_checkpoints(
            &instance_id,
            query.checkpoint_id.as_deref(),
            limit,
            offset,
            created_after,
            created_before,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            error!("List checkpoints error: {}", e);
            return error_response_from(
                "LIST_CHECKPOINTS_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    };

    let total_count = state
        .persistence
        .count_checkpoints(
            &instance_id,
            query.checkpoint_id.as_deref(),
            created_after,
            created_before,
        )
        .await
        .unwrap_or(0);

    let summaries: Vec<CheckpointSummaryJson> = checkpoints
        .into_iter()
        .map(|cp| CheckpointSummaryJson {
            checkpoint_id: cp.checkpoint_id,
            instance_id: cp.instance_id,
            created_at_ms: cp.created_at.timestamp_millis(),
            data_size_bytes: cp.state.len() as u64,
        })
        .collect();

    Json(json!({
        "checkpoints": summaries,
        "total_count": total_count,
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
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

    match state
        .persistence
        .load_checkpoint(&instance_id, &checkpoint_id)
        .await
    {
        Ok(Some(cp)) => Json(CheckpointDetailJson {
            found: true,
            checkpoint_id: cp.checkpoint_id,
            instance_id: cp.instance_id,
            created_at_ms: cp.created_at.timestamp_millis(),
            data: Some(base64::engine::general_purpose::STANDARD.encode(&cp.state)),
        })
        .into_response(),
        Ok(None) => Json(CheckpointDetailJson {
            found: false,
            checkpoint_id,
            instance_id,
            created_at_ms: 0,
            data: None,
        })
        .into_response(),
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

    let created_after = query
        .created_after_ms
        .and_then(chrono::DateTime::from_timestamp_millis);
    let created_before = query
        .created_before_ms
        .and_then(chrono::DateTime::from_timestamp_millis);

    let limit = query.limit.unwrap_or(100) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    let sort_order = match query.sort_order.as_deref() {
        Some("asc") => EventSortOrder::Asc,
        _ => EventSortOrder::Desc,
    };

    let filter = ListEventsFilter {
        event_type: query.event_type,
        subtype: query.subtype,
        created_after,
        created_before,
        payload_contains: query.payload_contains,
        scope_id: query.scope_id,
        parent_scope_id: query.parent_scope_id,
        root_scopes_only: query.root_scopes_only.unwrap_or(false),
        sort_order,
    };

    let events = match state
        .persistence
        .list_events(&instance_id, &filter, limit, offset)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            error!("List events error: {}", e);
            return error_response_from("LIST_EVENTS_ERROR", e, StatusCode::INTERNAL_SERVER_ERROR)
                .into_response();
        }
    };

    let total_count = state
        .persistence
        .count_events(&instance_id, &filter)
        .await
        .unwrap_or(0);

    let summaries: Vec<EventSummaryJson> = events
        .into_iter()
        .map(|ev| EventSummaryJson {
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
        .collect();

    Json(json!({
        "events": summaries,
        "total_count": total_count,
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
}

/// GET /api/v1/instances/{instance_id}/steps — list step summaries
async fn handle_list_step_summaries(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(instance_id): Path<String>,
    Query(query): Query<ListStepSummariesQuery>,
) -> impl IntoResponse {
    use runtara_core::persistence::{EventSortOrder, ListStepSummariesFilter, StepStatus};

    if instance_id.is_empty() {
        return error_response(
            "INVALID_REQUEST",
            "instance_id is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    let limit = query.limit.unwrap_or(100) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    let sort_order = match query.sort_order.as_deref() {
        Some("asc") => EventSortOrder::Asc,
        _ => EventSortOrder::Desc,
    };

    let status = match query.status.as_deref() {
        Some("running") => Some(StepStatus::Running),
        Some("completed") => Some(StepStatus::Completed),
        Some("failed") => Some(StepStatus::Failed),
        _ => None,
    };

    let filter = ListStepSummariesFilter {
        sort_order,
        status,
        step_type: query.step_type,
        scope_id: query.scope_id,
        parent_scope_id: query.parent_scope_id,
        root_scopes_only: query.root_scopes_only.unwrap_or(false),
    };

    let steps = match state
        .persistence
        .list_step_summaries(&instance_id, &filter, limit, offset)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            error!("List step summaries error: {}", e);
            return error_response_from(
                "LIST_STEP_SUMMARIES_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    };

    let total_count = state
        .persistence
        .count_step_summaries(&instance_id, &filter)
        .await
        .unwrap_or(0);

    let summaries: Vec<StepSummaryJson> = steps
        .into_iter()
        .map(|step| {
            let status_str = match step.status {
                StepStatus::Running => "running",
                StepStatus::Completed => "completed",
                StepStatus::Failed => "failed",
            };

            StepSummaryJson {
                step_id: step.step_id,
                step_name: step.step_name,
                step_type: step.step_type,
                status: status_str.to_string(),
                started_at_ms: step.started_at.timestamp_millis(),
                completed_at_ms: step.completed_at.map(|t| t.timestamp_millis()),
                duration_ms: step.duration_ms,
                inputs: step.inputs,
                outputs: step.outputs,
                error: step.error,
                scope_id: step.scope_id,
                parent_scope_id: step.parent_scope_id,
            }
        })
        .collect();

    Json(json!({
        "steps": summaries,
        "total_count": total_count,
        "limit": limit,
        "offset": offset,
    }))
    .into_response()
}

/// GET /api/v1/instances/{instance_id}/scopes/{scope_id}/ancestors — get scope ancestors
async fn handle_get_scope_ancestors(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path((instance_id, scope_id)): Path<(String, String)>,
) -> impl IntoResponse {
    use runtara_core::persistence::{EventSortOrder, ListEventsFilter};

    if instance_id.is_empty() || scope_id.is_empty() {
        return error_response(
            "INVALID_REQUEST",
            "instance_id and scope_id are required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    // Fetch all scope_enter events
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

    let events = match state
        .persistence
        .list_events(&instance_id, &filter, 10000, 0)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            error!("Get scope ancestors error: {}", e);
            return error_response_from(
                "GET_SCOPE_ANCESTORS_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    };

    // Build scope map
    let mut scope_map: std::collections::HashMap<String, ScopeInfoJson> =
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

        let parent_scope_id = payload_json
            .get("parent_scope_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let step_id = payload_json
            .get("step_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let step_name = payload_json
            .get("step_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let step_type = payload_json
            .get("step_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let index = payload_json
            .get("index")
            .and_then(|v| v.as_u64())
            .map(|i| i as u32);

        scope_map.insert(
            sid.to_string(),
            ScopeInfoJson {
                scope_id: sid.to_string(),
                parent_scope_id,
                step_id,
                step_name,
                step_type,
                index,
                created_at_ms: event.created_at.timestamp_millis(),
            },
        );
    }

    // Walk up the hierarchy
    let mut ancestors = Vec::new();
    let mut current = Some(scope_id);

    while let Some(sid) = current {
        if let Some(info) = scope_map.remove(&sid) {
            current = info.parent_scope_id.clone();
            ancestors.push(info);
        } else {
            break;
        }
    }

    Json(json!({ "ancestors": ancestors })).into_response()
}

/// GET /api/v1/tenants/{tenant_id}/metrics — get tenant metrics
async fn handle_get_tenant_metrics(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Path(tenant_id): Path<String>,
    Query(query): Query<TenantMetricsQuery>,
) -> impl IntoResponse {
    if tenant_id.is_empty() {
        return error_response(
            "INVALID_REQUEST",
            "tenant_id is required",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

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

    let bucket_rows = match db::get_tenant_metrics(&state.pool, &options).await {
        Ok(v) => v,
        Err(e) => {
            error!("Get tenant metrics error: {}", e);
            return error_response_from(
                "GET_TENANT_METRICS_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response();
        }
    };

    let buckets: Vec<MetricsBucketJson> = bucket_rows
        .into_iter()
        .map(|row| {
            let terminal_count = row.success_count + row.failure_count + row.cancelled_count;
            let success_rate = if terminal_count > 0 {
                Some((row.success_count as f64 / terminal_count as f64) * 100.0)
            } else {
                None
            };

            MetricsBucketJson {
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

    let granularity_str = match granularity {
        db::MetricsGranularity::Hourly => "hourly",
        db::MetricsGranularity::Daily => "daily",
    };

    Json(json!({
        "tenant_id": tenant_id,
        "start_time_ms": start_time.timestamp_millis(),
        "end_time_ms": end_time.timestamp_millis(),
        "granularity": granularity_str,
        "buckets": buckets,
    }))
    .into_response()
}

/// POST /api/v1/agents/test — test capability
async fn handle_test_capability(
    State(state): State<Arc<EnvironmentHandlerState>>,
    Json(body): Json<TestCapabilityJsonRequest>,
) -> impl IntoResponse {
    let req = TestCapabilityRequest {
        tenant_id: body.tenant_id,
        agent_id: body.agent_id,
        capability_id: body.capability_id,
        input: body.input,
        connection: body.connection,
        timeout_ms: body.timeout_ms,
    };

    match handlers::handle_test_capability(&state, req).await {
        Ok(resp) => {
            if resp.success {
                Json(json!({
                    "success": true,
                    "output": resp.output,
                    "execution_time_ms": resp.execution_time_ms,
                }))
                .into_response()
            } else {
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": false,
                        "error": resp.error,
                        "execution_time_ms": resp.execution_time_ms,
                    })),
                )
                    .into_response()
            }
        }
        Err(e) => {
            error!("Test capability error: {}", e);
            error_response_from(
                "TEST_CAPABILITY_ERROR",
                e,
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response()
        }
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
        .route("/api/v1/agents/test", post(handle_test_capability))
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
