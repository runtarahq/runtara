//! CSV Import/Export HTTP Handlers
//!
//! Thin HTTP layer for CSV import/export operations.
//! Supports both multipart/form-data and JSON with base64-encoded CSV.

use axum::{
    body::Bytes,
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::api::dto::csv_import_export::*;
use crate::api::dto::object_model::ConnectionQueryParams;
use crate::api::handlers::object_model::ObjectModelState;
use crate::api::services::csv_import_export::{self, CsvImportError};
use crate::api::services::object_model::ServiceError;

// ============================================================================
// Export CSV
// ============================================================================

/// Export instances as CSV
///
/// Exports filtered and sorted instances from a schema as a CSV file.
/// Supports column selection and all existing filter/sort capabilities.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/instances/schema/{name}/export-csv",
    params(
        ("name" = String, Path, description = "Schema name"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID"),
    ),
    request_body(content = CsvExportRequest, description = "Export configuration"),
    responses(
        (status = 200, description = "CSV file", content_type = "text/csv"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Schema not found"),
    ),
)]
pub async fn export_csv(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_name): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    Json(request): Json<CsvExportRequest>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let csv_bytes = csv_import_export::export_csv(
        &state.manager,
        &state.connections,
        &tenant_id,
        &schema_name,
        request,
        params.connection_id.as_deref(),
    )
    .await
    .map_err(service_error_to_response)?;

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response_headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}.csv\"", schema_name))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment; filename=\"export.csv\"")),
    );

    Ok((StatusCode::OK, response_headers, Bytes::from(csv_bytes)).into_response())
}

// ============================================================================
// Import CSV Preview
// ============================================================================

/// Preview CSV import
///
/// Parses CSV headers and sample rows, returns schema columns and
/// auto-suggested column mappings. Accepts multipart/form-data or JSON with base64.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/instances/schema/{name}/import-csv/preview",
    params(
        ("name" = String, Path, description = "Schema name"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID"),
    ),
    responses(
        (status = 200, description = "Preview result", body = ImportPreviewResponse),
        (status = 400, description = "Invalid CSV"),
        (status = 404, description = "Schema not found"),
    ),
)]
pub async fn import_csv_preview(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_name): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    request: Request,
) -> Result<(StatusCode, Json<ImportPreviewResponse>), (StatusCode, Json<Value>)> {
    let csv_data = extract_csv_data(request).await?;

    let preview = csv_import_export::preview_import(
        &state.manager,
        &state.connections,
        &tenant_id,
        &schema_name,
        &csv_data,
        params.connection_id.as_deref(),
    )
    .await
    .map_err(service_error_to_response)?;

    Ok((StatusCode::OK, Json(preview)))
}

// ============================================================================
// Import CSV
// ============================================================================

/// Import CSV data
///
/// Imports CSV data into a schema with column mapping.
/// Supports create (insert) and upsert modes.
/// Accepts multipart/form-data or JSON with base64.
/// Atomic: all rows validated first, none imported if any fail.
#[utoipa::path(
    post,
    path = "/api/runtime/object-model/instances/schema/{name}/import-csv",
    params(
        ("name" = String, Path, description = "Schema name"),
        ("connectionId" = Option<String>, Query, description = "Optional connection ID"),
    ),
    responses(
        (status = 200, description = "Import result", body = CsvImportResponse),
        (status = 400, description = "Validation error or bad CSV"),
        (status = 404, description = "Schema not found"),
    ),
)]
pub async fn import_csv(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(state): State<Arc<ObjectModelState>>,
    Path(schema_name): Path<String>,
    Query(params): Query<ConnectionQueryParams>,
    request: Request,
) -> Result<(StatusCode, Json<CsvImportResponse>), (StatusCode, Json<Value>)> {
    let (csv_data, config) = extract_csv_and_config(request).await?;

    let result = csv_import_export::import_csv(
        &state.manager,
        &state.connections,
        &tenant_id,
        &schema_name,
        &csv_data,
        config,
        params.connection_id.as_deref(),
    )
    .await
    .map_err(import_error_to_response)?;

    Ok((StatusCode::OK, Json(result)))
}

// ============================================================================
// Helpers: Content-type detection & extraction
// ============================================================================

/// Extract CSV data from a request that can be either multipart or JSON.
async fn extract_csv_data(request: Request) -> Result<Vec<u8>, (StatusCode, Json<Value>)> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.contains("multipart/form-data") {
        extract_csv_from_multipart(request).await
    } else {
        extract_csv_from_json_preview(request).await
    }
}

/// Extract CSV data and import config from a request (multipart or JSON).
async fn extract_csv_and_config(
    request: Request,
) -> Result<(Vec<u8>, CsvImportConfig), (StatusCode, Json<Value>)> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.contains("multipart/form-data") {
        extract_csv_and_config_from_multipart(request).await
    } else {
        extract_csv_and_config_from_json(request).await
    }
}

/// Extract CSV bytes from a multipart form upload (for preview).
async fn extract_csv_from_multipart(
    request: Request,
) -> Result<Vec<u8>, (StatusCode, Json<Value>)> {
    let mut multipart = Multipart::from_request(request, &()).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Invalid multipart request: {}", e)})),
        )
    })?;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Failed to read multipart field: {}", e)})),
        )
    })? {
        if field.name() == Some("file") {
            let data = field.bytes().await.map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"success": false, "error": format!("Failed to read file: {}", e)})),
                )
            })?;
            return Ok(data.to_vec());
        }
    }

    Err((
        StatusCode::BAD_REQUEST,
        Json(json!({"success": false, "error": "Missing 'file' field in multipart request"})),
    ))
}

/// Extract CSV bytes from a JSON body with base64 data (for preview).
async fn extract_csv_from_json_preview(
    request: Request,
) -> Result<Vec<u8>, (StatusCode, Json<Value>)> {
    let body = axum::body::to_bytes(request.into_body(), 50 * 1024 * 1024)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("Failed to read body: {}", e)})),
            )
        })?;

    let json_req: CsvPreviewJsonRequest = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Invalid JSON: {}", e)})),
        )
    })?;

    BASE64.decode(&json_req.data).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Invalid base64: {}", e)})),
        )
    })
}

/// Extract CSV bytes and config from multipart (for import).
async fn extract_csv_and_config_from_multipart(
    request: Request,
) -> Result<(Vec<u8>, CsvImportConfig), (StatusCode, Json<Value>)> {
    let mut multipart = Multipart::from_request(request, &()).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Invalid multipart request: {}", e)})),
        )
    })?;

    let mut csv_data: Option<Vec<u8>> = None;
    let mut config: Option<CsvImportConfig> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Failed to read multipart field: {}", e)})),
        )
    })? {
        match field.name() {
            Some("file") => {
                let data = field.bytes().await.map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"success": false, "error": format!("Failed to read file: {}", e)})),
                    )
                })?;
                csv_data = Some(data.to_vec());
            }
            Some("config") => {
                let text = field.text().await.map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"success": false, "error": format!("Failed to read config: {}", e)})),
                    )
                })?;
                config = Some(serde_json::from_str(&text).map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"success": false, "error": format!("Invalid config JSON: {}", e)})),
                    )
                })?);
            }
            _ => {}
        }
    }

    let csv_data = csv_data.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "Missing 'file' field in multipart request"})),
        )
    })?;

    let config = config.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "Missing 'config' field in multipart request"})),
        )
    })?;

    Ok((csv_data, config))
}

/// Extract CSV bytes and config from JSON body with base64 (for import).
async fn extract_csv_and_config_from_json(
    request: Request,
) -> Result<(Vec<u8>, CsvImportConfig), (StatusCode, Json<Value>)> {
    let body = axum::body::to_bytes(request.into_body(), 50 * 1024 * 1024)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"success": false, "error": format!("Failed to read body: {}", e)})),
            )
        })?;

    let json_req: CsvImportJsonRequest = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Invalid JSON: {}", e)})),
        )
    })?;

    let csv_data = BASE64.decode(&json_req.data).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Invalid base64: {}", e)})),
        )
    })?;

    let config = CsvImportConfig {
        column_mapping: json_req.column_mapping,
        mode: json_req.mode,
        conflict_columns: json_req.conflict_columns,
        on_error: json_req.on_error,
    };

    Ok((csv_data, config))
}

// ============================================================================
// Error mapping
// ============================================================================

fn service_error_to_response(err: ServiceError) -> (StatusCode, Json<Value>) {
    match err {
        ServiceError::ValidationError(msg) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": msg})),
        ),
        ServiceError::NotFound(msg) => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": msg})),
        ),
        ServiceError::Conflict(msg) => (
            StatusCode::CONFLICT,
            Json(json!({"success": false, "error": msg})),
        ),
        ServiceError::DatabaseError(msg) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": msg})),
        ),
    }
}

fn import_error_to_response(err: CsvImportError) -> (StatusCode, Json<Value>) {
    match err {
        CsvImportError::Service(service_err) => service_error_to_response(service_err),
        CsvImportError::Validation(validation_err) => (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::to_value(validation_err)
                    .unwrap_or_else(|_| json!({"success": false, "error": "Validation failed"})),
            ),
        ),
    }
}
