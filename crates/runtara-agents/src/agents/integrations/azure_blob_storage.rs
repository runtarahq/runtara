//! Azure Blob Storage Agent
//!
//! Provides operations for working with Azure Blob Storage:
//! - Container management (create, list, delete)
//! - Blob operations (upload, download, list, delete, copy)
//! - Blob metadata (head)
//!
//! The capability surface mirrors the s3_storage agent so workflows can be
//! ported between providers with minimal rewiring. Internally "bucket" maps
//! to a container and "key" maps to a blob name.
//!
//! Operations require an `azure_blob_storage` connection. Credentials are
//! injected server-side by the HTTP proxy — the agent only passes the
//! connection_id.

use crate::connections::RawConnection;
use crate::types::AgentError;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::azure_blob_client::AzureBlobClient;

// ============================================================================
// Client cache
// ============================================================================

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{OnceLock, RwLock};

static AZURE_CLIENTS: OnceLock<RwLock<HashMap<String, Arc<AzureBlobClient>>>> = OnceLock::new();

fn clients() -> &'static RwLock<HashMap<String, Arc<AzureBlobClient>>> {
    AZURE_CLIENTS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn get_or_create_client(connection: &RawConnection) -> Result<Arc<AzureBlobClient>, AgentError> {
    let key = connection.connection_id.clone();
    {
        let cache = clients().read().unwrap();
        if let Some(client) = cache.get(&key) {
            return Ok(Arc::clone(client));
        }
    }
    let client = Arc::new(AzureBlobClient::new(connection.connection_id.clone()));
    clients().write().unwrap().insert(key, Arc::clone(&client));
    Ok(client)
}

fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "AZURE_BLOB_MISSING_CONNECTION",
            "No Azure Blob Storage connection configured. Add an azure_blob_storage connection to this step.",
        )
        .with_attrs(json!({}))
    })
}

// ============================================================================
// Container (a.k.a. bucket) operations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Container Input")]
pub struct CreateBucketInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Container Name",
        description = "Name of the container to create",
        example = "uploads"
    )]
    pub bucket: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Container Output")]
pub struct CreateBucketOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "Create Container",
    description = "Create a new container in Azure Blob Storage",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage",
    side_effects = true
)]
pub fn storage_create_bucket(input: CreateBucketInput) -> Result<CreateBucketOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;
    match client.create_container(&input.bucket) {
        Ok(()) => Ok(CreateBucketOutput {
            success: true,
            error: None,
        }),
        Err(e) => Ok(CreateBucketOutput {
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Containers Input")]
pub struct ListBucketsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Containers Output")]
pub struct ListBucketsOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Containers",
        description = "List of container names and last-modified timestamps"
    )]
    pub buckets: Vec<Value>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "List Containers",
    description = "List all containers in the storage account",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage"
)]
pub fn storage_list_buckets(input: ListBucketsInput) -> Result<ListBucketsOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;
    match client.list_containers() {
        Ok(containers) => {
            let values: Vec<Value> = containers
                .iter()
                .map(|c| json!({"name": c.name, "last_modified": c.last_modified}))
                .collect();
            Ok(ListBucketsOutput {
                success: true,
                buckets: values,
                error: None,
            })
        }
        Err(e) => Ok(ListBucketsOutput {
            success: false,
            buckets: vec![],
            error: Some(e.to_string()),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Container Input")]
pub struct DeleteBucketInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Container Name",
        description = "Name of the container to delete (must be empty)",
        example = "old-exports"
    )]
    pub bucket: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Container Output")]
pub struct DeleteBucketOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "Delete Container",
    description = "Delete an empty container from Azure Blob Storage",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage",
    side_effects = true
)]
pub fn storage_delete_bucket(input: DeleteBucketInput) -> Result<DeleteBucketOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;
    match client.delete_container(&input.bucket) {
        Ok(()) => Ok(DeleteBucketOutput {
            success: true,
            error: None,
        }),
        Err(e) => Ok(DeleteBucketOutput {
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

// ============================================================================
// Blob (a.k.a. file) operations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload Blob Input")]
pub struct UploadFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Container",
        description = "Container to upload to",
        example = "uploads"
    )]
    pub bucket: String,

    #[field(
        display_name = "Key",
        description = "Blob name (path within the container)",
        example = "reports/2026-05-16.csv"
    )]
    pub key: String,

    #[field(
        display_name = "Content",
        description = "Blob content as base64-encoded string or plain text",
        example = "SGVsbG8gV29ybGQ="
    )]
    pub content: String,

    #[field(
        display_name = "Content Type",
        description = "MIME type of the blob (e.g., text/csv, image/png)",
        example = "text/csv"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(
        display_name = "Is Base64",
        description = "Whether content is base64-encoded (default: true)",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub is_base64: Option<bool>,
}

fn default_true() -> Option<bool> {
    Some(true)
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Upload Blob Output")]
pub struct UploadFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Key", description = "Blob name of the uploaded blob")]
    pub key: Option<String>,

    #[field(
        display_name = "Size",
        description = "Size in bytes of the uploaded blob"
    )]
    pub size: Option<u64>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

/// Size limit for single-shot uploads. Azure supports much larger blobs via the
/// Put Block + Put Block List APIs; this cap matches the s3_storage agent.
const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;

#[capability(
    module = "azure_blob_storage",
    display_name = "Upload Blob",
    description = "Upload a blob to Azure Blob Storage. Content can be base64-encoded binary or plain text.",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage",
    side_effects = true
)]
pub fn storage_upload_file(input: UploadFileInput) -> Result<UploadFileOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;

    let is_base64 = input.is_base64.unwrap_or(true);
    let data = if is_base64 {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(&input.content)
            .map_err(|e| {
                AgentError::permanent(
                    "AZURE_BLOB_INVALID_CONTENT",
                    format!("Invalid base64: {}", e),
                )
                .with_attrs(json!({}))
            })?
    } else {
        input.content.into_bytes()
    };

    if data.len() > MAX_UPLOAD_SIZE {
        return Ok(UploadFileOutput {
            success: false,
            key: None,
            size: None,
            error: Some(format!(
                "Blob exceeds maximum size of {} MB",
                MAX_UPLOAD_SIZE / 1024 / 1024
            )),
        });
    }

    let size = data.len() as u64;
    let ct = input.content_type.as_deref();

    match client.put_blob(&input.bucket, &input.key, data, ct) {
        Ok(()) => Ok(UploadFileOutput {
            success: true,
            key: Some(input.key),
            size: Some(size),
            error: None,
        }),
        Err(e) => Ok(UploadFileOutput {
            success: false,
            key: None,
            size: None,
            error: Some(e.to_string()),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Download Blob Input")]
pub struct DownloadFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Container",
        description = "Container to download from",
        example = "uploads"
    )]
    pub bucket: String,

    #[field(
        display_name = "Key",
        description = "Blob name (path within the container)",
        example = "reports/2026-05-16.csv"
    )]
    pub key: String,

    #[field(
        display_name = "As Text",
        description = "Return content as UTF-8 text instead of base64 (default: false)",
        default = "false"
    )]
    #[serde(default)]
    pub as_text: Option<bool>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Download Blob Output")]
pub struct DownloadFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Content",
        description = "Blob content (base64 or text)"
    )]
    pub content: Option<String>,

    #[field(display_name = "Content Type", description = "MIME type of the blob")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "Size in bytes")]
    pub size: Option<u64>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "Download Blob",
    description = "Download a blob from Azure Blob Storage. Returns base64-encoded content by default, or UTF-8 text if as_text is true.",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage"
)]
pub fn storage_download_file(input: DownloadFileInput) -> Result<DownloadFileOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;

    let metadata = client.head_blob(&input.bucket, &input.key).ok();

    match client.get_blob(&input.bucket, &input.key) {
        Ok(data) => {
            let size = data.len() as u64;
            let content = if input.as_text.unwrap_or(false) {
                String::from_utf8(data).unwrap_or_else(|e| {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.encode(e.into_bytes())
                })
            } else {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&data)
            };

            Ok(DownloadFileOutput {
                success: true,
                content: Some(content),
                content_type: metadata.map(|m| m.content_type),
                size: Some(size),
                error: None,
            })
        }
        Err(e) => Ok(DownloadFileOutput {
            success: false,
            content: None,
            content_type: None,
            size: None,
            error: Some(e.to_string()),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Blobs Input")]
pub struct ListFilesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Container",
        description = "Container to list blobs from",
        example = "uploads"
    )]
    pub bucket: String,

    #[field(
        display_name = "Prefix",
        description = "Filter blobs by name prefix (like a folder path)",
        example = "reports/"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    #[field(
        display_name = "Max Keys",
        description = "Maximum number of blobs to return (default: 5000)",
        example = "100"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_keys: Option<u32>,

    #[field(
        display_name = "Continuation Token",
        description = "Marker for paginating through results"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Blobs Output")]
pub struct ListFilesOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Files",
        description = "List of blob objects with key, size, last_modified, etag"
    )]
    pub files: Vec<Value>,

    #[field(display_name = "Count", description = "Number of blobs returned")]
    pub count: u32,

    #[field(
        display_name = "Next Token",
        description = "Marker for fetching the next page"
    )]
    pub next_continuation_token: Option<String>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "List Blobs",
    description = "List blobs in a container with optional prefix filter and pagination",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage"
)]
pub fn storage_list_files(input: ListFilesInput) -> Result<ListFilesOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;

    match client.list_blobs(
        &input.bucket,
        input.prefix.as_deref(),
        input.max_keys,
        input.continuation_token.as_deref(),
    ) {
        Ok((blobs, next_marker)) => {
            let count = blobs.len() as u32;
            let files: Vec<Value> = blobs
                .iter()
                .map(|b| {
                    json!({
                        "key": b.name,
                        "size": b.size,
                        "last_modified": b.last_modified,
                        "etag": b.etag,
                    })
                })
                .collect();
            Ok(ListFilesOutput {
                success: true,
                files,
                count,
                next_continuation_token: next_marker,
                error: None,
            })
        }
        Err(e) => Ok(ListFilesOutput {
            success: false,
            files: vec![],
            count: 0,
            next_continuation_token: None,
            error: Some(e.to_string()),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Blob Info Input")]
pub struct GetFileInfoInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Container", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/2026-05-16.csv")]
    pub key: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Blob Info Output")]
pub struct GetFileInfoOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Content Type")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "Blob size in bytes")]
    pub size: Option<u64>,

    #[field(display_name = "ETag")]
    pub etag: Option<String>,

    #[field(display_name = "Last Modified")]
    pub last_modified: Option<String>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "Get Blob Info",
    description = "Get metadata about a blob without downloading it (content type, size, last modified)",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage"
)]
pub fn storage_get_file_info(input: GetFileInfoInput) -> Result<GetFileInfoOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;
    match client.head_blob(&input.bucket, &input.key) {
        Ok(meta) => Ok(GetFileInfoOutput {
            success: true,
            content_type: Some(meta.content_type),
            size: Some(meta.content_length),
            etag: Some(meta.etag),
            last_modified: Some(meta.last_modified),
            error: None,
        }),
        Err(e) => Ok(GetFileInfoOutput {
            success: false,
            content_type: None,
            size: None,
            etag: None,
            last_modified: None,
            error: Some(e.to_string()),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Blob Input")]
pub struct DeleteFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Container", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/old-report.csv")]
    pub key: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Blob Output")]
pub struct DeleteFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "Delete Blob",
    description = "Delete a blob from Azure Blob Storage",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage",
    side_effects = true
)]
pub fn storage_delete_file(input: DeleteFileInput) -> Result<DeleteFileOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;
    match client.delete_blob(&input.bucket, &input.key) {
        Ok(()) => Ok(DeleteFileOutput {
            success: true,
            error: None,
        }),
        Err(e) => Ok(DeleteFileOutput {
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Copy Blob Input")]
pub struct CopyFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Source Container", example = "uploads")]
    pub source_bucket: String,

    #[field(display_name = "Source Key", example = "temp/file.csv")]
    pub source_key: String,

    #[field(display_name = "Destination Container", example = "archive")]
    pub destination_bucket: String,

    #[field(display_name = "Destination Key", example = "2026/05/file.csv")]
    pub destination_key: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Copy Blob Output")]
pub struct CopyFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "Copy Blob",
    description = "Copy a blob within or across containers in the same Azure Blob Storage account",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage",
    side_effects = true
)]
pub fn storage_copy_file(input: CopyFileInput) -> Result<CopyFileOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_client(conn)?;
    match client.copy_blob(
        &input.source_bucket,
        &input.source_key,
        &input.destination_bucket,
        &input.destination_key,
    ) {
        Ok(()) => Ok(CopyFileOutput {
            success: true,
            error: None,
        }),
        Err(e) => Ok(CopyFileOutput {
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Generate SAS URL Input")]
pub struct GeneratePresignedUrlInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Container", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/2026-05-16.csv")]
    pub key: String,

    #[field(
        display_name = "Operation",
        description = "What the URL will be used for: download, upload, or delete",
        example = "download"
    )]
    pub operation: String,

    #[field(
        display_name = "Expires In (seconds)",
        description = "Lifetime of the SAS URL in seconds (max 604800 = 7 days, default 3600)",
        example = "3600"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,

    #[field(
        display_name = "Content Type",
        description = "For upload URLs, the MIME type the caller will use (e.g., text/csv)",
        example = "text/csv"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Generate SAS URL Output")]
pub struct GeneratePresignedUrlOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "URL", description = "Time-limited SAS URL")]
    pub url: Option<String>,

    #[field(
        display_name = "Expires In (seconds)",
        description = "Actual lifetime of the URL after server-side clamping"
    )]
    pub expires_in_seconds: Option<u64>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

const DEFAULT_PRESIGN_EXPIRES_SECONDS: u64 = 3600;

#[capability(
    module = "azure_blob_storage",
    display_name = "Generate Presigned URL",
    description = "Generate a time-limited Shared Access Signature URL for downloading, uploading, or deleting a blob. Callers consume the URL directly without going through runtara.",
    module_display_name = "Azure Blob Storage",
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage"
)]
pub fn storage_generate_presigned_url(
    input: GeneratePresignedUrlInput,
) -> Result<GeneratePresignedUrlOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let method = match input.operation.to_lowercase().as_str() {
        "download" | "get" | "read" => "GET",
        "upload" | "put" | "write" | "create" => "PUT",
        "delete" => "DELETE",
        other => {
            return Ok(GeneratePresignedUrlOutput {
                success: false,
                url: None,
                expires_in_seconds: None,
                error: Some(format!(
                    "Unsupported operation `{}` (expected download, upload, or delete)",
                    other
                )),
            });
        }
    };

    let path = format!("/{}/{}", input.bucket, input.key);
    let expires = input
        .expires_in_seconds
        .unwrap_or(DEFAULT_PRESIGN_EXPIRES_SECONDS);

    match runtara_http::presign(
        &conn.connection_id,
        method,
        &path,
        expires,
        input.content_type.as_deref(),
    ) {
        Ok(result) => Ok(GeneratePresignedUrlOutput {
            success: true,
            url: Some(result.url),
            expires_in_seconds: Some(result.expires_in_seconds),
            error: None,
        }),
        Err(e) => Ok(GeneratePresignedUrlOutput {
            success: false,
            url: None,
            expires_in_seconds: None,
            error: Some(e.to_string()),
        }),
    }
}
