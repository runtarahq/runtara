//! File Storage HTTP Handlers
//!
//! REST API for S3-compatible file storage operations.
//! Supports bucket management and file upload/download/list/delete.
//! Uses the tenant's s3_compatible connection (internal by default).

use axum::{
    body::Bytes,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use runtara_connections::ConnectionsFacade;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::api::dto::file_storage::*;
use crate::api::services::file_storage::{FileStorageError, FileStorageService};

fn require_connection_id(
    params: &FileStorageQueryParams,
) -> Result<&str, (StatusCode, Json<Value>)> {
    params.connection_id.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "connectionId query parameter is required"
            })),
        )
    })
}

fn error_response(e: FileStorageError) -> (StatusCode, Json<Value>) {
    let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        Json(json!({"success": false, "error": e.message()})),
    )
}

// ============================================================================
// Bucket Handlers
// ============================================================================

/// List all buckets
#[utoipa::path(
    get,
    path = "/api/runtime/files/buckets",
    params(
        ("connectionId" = Option<String>, Query, description = "Optional s3_compatible connection ID"),
    ),
    responses(
        (status = 200, description = "List of buckets", body = ListBucketsResponse),
    ),
    tag = "file-storage",
    security(("tenant_auth" = []))
)]
pub async fn list_buckets(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(connections): State<Arc<ConnectionsFacade>>,
    Query(params): Query<FileStorageQueryParams>,
) -> Result<Json<ListBucketsResponse>, (StatusCode, Json<Value>)> {
    let connection_id = require_connection_id(&params)?;
    let buckets = FileStorageService::list_buckets(&connections, connection_id, &tenant_id)
        .await
        .map_err(error_response)?;

    Ok(Json(ListBucketsResponse {
        buckets: buckets
            .into_iter()
            .map(|b| BucketDto {
                name: b.name,
                creation_date: b.creation_date,
            })
            .collect(),
    }))
}

/// Create a bucket
#[utoipa::path(
    post,
    path = "/api/runtime/files/buckets",
    params(
        ("connectionId" = Option<String>, Query, description = "Optional s3_compatible connection ID"),
    ),
    request_body(content = CreateBucketRequest),
    responses(
        (status = 201, description = "Bucket created", body = CreateBucketResponse),
    ),
    tag = "file-storage",
    security(("tenant_auth" = []))
)]
pub async fn create_bucket(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(connections): State<Arc<ConnectionsFacade>>,
    Query(params): Query<FileStorageQueryParams>,
    Json(request): Json<CreateBucketRequest>,
) -> Result<(StatusCode, Json<CreateBucketResponse>), (StatusCode, Json<Value>)> {
    let connection_id = require_connection_id(&params)?;
    FileStorageService::create_bucket(&connections, connection_id, &tenant_id, &request.name)
        .await
        .map_err(error_response)?;

    Ok((
        StatusCode::CREATED,
        Json(CreateBucketResponse { success: true }),
    ))
}

/// Delete a bucket (must be empty)
#[utoipa::path(
    delete,
    path = "/api/runtime/files/buckets/{bucket}",
    params(
        ("bucket" = String, Path, description = "Bucket name"),
        ("connectionId" = Option<String>, Query, description = "Optional s3_compatible connection ID"),
    ),
    responses(
        (status = 200, description = "Bucket deleted", body = DeleteResponse),
    ),
    tag = "file-storage",
    security(("tenant_auth" = []))
)]
pub async fn delete_bucket(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(connections): State<Arc<ConnectionsFacade>>,
    Path(bucket): Path<String>,
    Query(params): Query<FileStorageQueryParams>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<Value>)> {
    let connection_id = require_connection_id(&params)?;
    FileStorageService::delete_bucket(&connections, connection_id, &tenant_id, &bucket)
        .await
        .map_err(error_response)?;

    Ok(Json(DeleteResponse { success: true }))
}

// ============================================================================
// Object / File Handlers
// ============================================================================

/// List files in a bucket
#[utoipa::path(
    get,
    path = "/api/runtime/files/{bucket}",
    params(
        ("bucket" = String, Path, description = "Bucket name"),
        ("connectionId" = Option<String>, Query, description = "Optional s3_compatible connection ID"),
        ("prefix" = Option<String>, Query, description = "Filter by key prefix"),
        ("maxKeys" = Option<u32>, Query, description = "Max results (default: 1000)"),
        ("continuationToken" = Option<String>, Query, description = "Pagination token"),
    ),
    responses(
        (status = 200, description = "List of files", body = ListObjectsResponse),
    ),
    tag = "file-storage",
    security(("tenant_auth" = []))
)]
pub async fn list_objects(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(connections): State<Arc<ConnectionsFacade>>,
    Path(bucket): Path<String>,
    Query(params): Query<ListObjectsQueryParams>,
) -> Result<Json<ListObjectsResponse>, (StatusCode, Json<Value>)> {
    let connection_id = params.connection_id.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "connectionId query parameter is required"})),
        )
    })?;
    let (objects, next_token) = FileStorageService::list_objects(
        &connections,
        connection_id,
        &tenant_id,
        &bucket,
        params.prefix.as_deref(),
        params.max_keys,
        params.continuation_token.as_deref(),
    )
    .await
    .map_err(error_response)?;

    let count = objects.len() as u32;
    Ok(Json(ListObjectsResponse {
        files: objects
            .into_iter()
            .map(|o| FileObjectDto {
                key: o.key,
                size: o.size,
                last_modified: o.last_modified,
                etag: o.etag,
            })
            .collect(),
        count,
        next_continuation_token: next_token,
    }))
}

/// Upload a file (multipart/form-data)
///
/// Upload a file to the specified bucket. Send as multipart/form-data with a `file` field.
/// Optionally include a `key` field to specify the object key; defaults to the filename.
#[utoipa::path(
    post,
    path = "/api/runtime/files/{bucket}",
    params(
        ("bucket" = String, Path, description = "Bucket name"),
        ("connectionId" = Option<String>, Query, description = "Optional s3_compatible connection ID"),
    ),
    request_body(content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "File uploaded", body = UploadResponse),
        (status = 400, description = "Invalid request or file too large"),
    ),
    tag = "file-storage",
    security(("tenant_auth" = []))
)]
pub async fn upload_object(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(connections): State<Arc<ConnectionsFacade>>,
    Path(bucket): Path<String>,
    Query(params): Query<FileStorageQueryParams>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<UploadResponse>), (StatusCode, Json<Value>)> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut key_override: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": format!("Multipart error: {}", e)})),
        )
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                content_type = field.content_type().map(|s| s.to_string());
                file_name = field.file_name().map(|s| s.to_string());
                file_data = Some(field.bytes().await.map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"success": false, "error": format!("Failed to read file: {}", e)})),
                    )
                })?.to_vec());
            }
            "key" => {
                let text = field.text().await.unwrap_or_default();
                if !text.is_empty() {
                    key_override = Some(text);
                }
            }
            _ => {}
        }
    }

    let data = file_data.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "No 'file' field in multipart upload"})),
        )
    })?;

    let key = key_override
        .or(file_name)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let connection_id = require_connection_id(&params)?;
    let size = FileStorageService::upload_object(
        &connections,
        connection_id,
        &tenant_id,
        &bucket,
        &key,
        data,
        content_type.as_deref(),
    )
    .await
    .map_err(error_response)?;

    Ok((
        StatusCode::CREATED,
        Json(UploadResponse {
            success: true,
            key,
            size,
        }),
    ))
}

/// Download a file
#[utoipa::path(
    get,
    path = "/api/runtime/files/{bucket}/{key}",
    params(
        ("bucket" = String, Path, description = "Bucket name"),
        ("key" = String, Path, description = "Object key"),
        ("connectionId" = Option<String>, Query, description = "Optional s3_compatible connection ID"),
    ),
    responses(
        (status = 200, description = "File content"),
        (status = 404, description = "File not found"),
    ),
    tag = "file-storage",
    security(("tenant_auth" = []))
)]
pub async fn download_object(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(connections): State<Arc<ConnectionsFacade>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(params): Query<FileStorageQueryParams>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let connection_id = require_connection_id(&params)?;
    let (data, metadata) =
        FileStorageService::download_object(&connections, connection_id, &tenant_id, &bucket, &key)
            .await
            .map_err(error_response)?;

    let mut headers = HeaderMap::new();
    if let Ok(ct) = HeaderValue::from_str(&metadata.content_type) {
        headers.insert(header::CONTENT_TYPE, ct);
    }

    // Extract filename from key (last segment)
    let filename = key.rsplit('/').next().unwrap_or(&key);
    if let Ok(cd) = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename)) {
        headers.insert(header::CONTENT_DISPOSITION, cd);
    }

    Ok((StatusCode::OK, headers, Bytes::from(data)).into_response())
}

/// Get file metadata (HEAD)
#[utoipa::path(
    get,
    path = "/api/runtime/files/{bucket}/{key}/info",
    params(
        ("bucket" = String, Path, description = "Bucket name"),
        ("key" = String, Path, description = "Object key"),
        ("connectionId" = Option<String>, Query, description = "Optional s3_compatible connection ID"),
    ),
    responses(
        (status = 200, description = "File metadata", body = FileMetadataResponse),
        (status = 404, description = "File not found"),
    ),
    tag = "file-storage",
    security(("tenant_auth" = []))
)]
pub async fn get_object_info(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(connections): State<Arc<ConnectionsFacade>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(params): Query<FileStorageQueryParams>,
) -> Result<Json<FileMetadataResponse>, (StatusCode, Json<Value>)> {
    let connection_id = require_connection_id(&params)?;
    let metadata =
        FileStorageService::head_object(&connections, connection_id, &tenant_id, &bucket, &key)
            .await
            .map_err(error_response)?;

    Ok(Json(FileMetadataResponse {
        content_type: metadata.content_type,
        content_length: metadata.content_length,
        etag: metadata.etag,
        last_modified: metadata.last_modified,
    }))
}

/// Delete a file
#[utoipa::path(
    delete,
    path = "/api/runtime/files/{bucket}/{key}",
    params(
        ("bucket" = String, Path, description = "Bucket name"),
        ("key" = String, Path, description = "Object key"),
        ("connectionId" = Option<String>, Query, description = "Optional s3_compatible connection ID"),
    ),
    responses(
        (status = 200, description = "File deleted", body = DeleteResponse),
    ),
    tag = "file-storage",
    security(("tenant_auth" = []))
)]
pub async fn delete_object(
    crate::middleware::tenant_auth::OrgId(tenant_id): crate::middleware::tenant_auth::OrgId,
    State(connections): State<Arc<ConnectionsFacade>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(params): Query<FileStorageQueryParams>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<Value>)> {
    let connection_id = require_connection_id(&params)?;
    FileStorageService::delete_object(&connections, connection_id, &tenant_id, &bucket, &key)
        .await
        .map_err(error_response)?;

    Ok(Json(DeleteResponse { success: true }))
}
