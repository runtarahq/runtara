//! S3-Compatible Storage Agent
//!
//! Provides operations for working with S3-compatible object storage:
//! - Bucket management (create, list, delete)
//! - File operations (upload, download, list, delete, copy)
//! - File metadata (head)
//!
//! Operations require an s3_compatible connection. Credentials are injected
//! server-side by the HTTP proxy -- the agent only passes the connection_id.

use crate::connections::RawConnection;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::errors::permanent_error;
use super::s3_client::S3Client;

// ============================================================================
// S3 Client Cache
// ============================================================================

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{OnceLock, RwLock};

/// Cache of S3Client instances keyed by connection_id
static S3_CLIENTS: OnceLock<RwLock<HashMap<String, Arc<S3Client>>>> = OnceLock::new();

fn get_clients() -> &'static RwLock<HashMap<String, Arc<S3Client>>> {
    S3_CLIENTS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn get_or_create_s3_client(connection: &RawConnection) -> Result<Arc<S3Client>, String> {
    let cache_key = connection.connection_id.clone();

    // Check cache
    {
        let clients = get_clients().read().unwrap();
        if let Some(client) = clients.get(&cache_key) {
            return Ok(Arc::clone(client));
        }
    }

    // Determine path_style from connection parameters (default true for S3-compatible stores)
    let path_style = connection
        .parameters
        .get("path_style")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Create new client using proxy pattern (connection_id + relative paths)
    let client = Arc::new(S3Client::new(connection.connection_id.clone(), path_style));

    {
        let mut clients = get_clients().write().unwrap();
        clients.insert(cache_key, Arc::clone(&client));
    }

    Ok(client)
}

fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, String> {
    connection.as_ref().ok_or_else(|| {
        permanent_error(
            "S3_MISSING_CONNECTION",
            "No S3 connection configured. Add an s3_compatible connection to this step.",
            json!({}),
        )
    })
}

// ============================================================================
// Bucket Operations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Bucket Input")]
pub struct CreateBucketInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Bucket Name",
        description = "Name of the bucket to create",
        example = "uploads"
    )]
    pub bucket: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Bucket Output")]
pub struct CreateBucketOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "s3_storage",
    display_name = "Create Bucket",
    description = "Create a new bucket in S3-compatible storage",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible",
    side_effects = true
)]
pub fn storage_create_bucket(input: CreateBucketInput) -> Result<CreateBucketOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    match client.create_bucket(&input.bucket) {
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
#[capability_input(display_name = "List Buckets Input")]
pub struct ListBucketsInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Buckets Output")]
pub struct ListBucketsOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Buckets",
        description = "List of bucket names and creation dates"
    )]
    pub buckets: Vec<Value>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "s3_storage",
    display_name = "List Buckets",
    description = "List all buckets in S3-compatible storage",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible"
)]
pub fn storage_list_buckets(input: ListBucketsInput) -> Result<ListBucketsOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    match client.list_buckets() {
        Ok(buckets) => {
            let bucket_values: Vec<Value> = buckets
                .iter()
                .map(|b| json!({"name": b.name, "creation_date": b.creation_date}))
                .collect();
            Ok(ListBucketsOutput {
                success: true,
                buckets: bucket_values,
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
#[capability_input(display_name = "Delete Bucket Input")]
pub struct DeleteBucketInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Bucket Name",
        description = "Name of the bucket to delete (must be empty)",
        example = "old-exports"
    )]
    pub bucket: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Bucket Output")]
pub struct DeleteBucketOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "s3_storage",
    display_name = "Delete Bucket",
    description = "Delete an empty bucket from S3-compatible storage",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible",
    side_effects = true
)]
pub fn storage_delete_bucket(input: DeleteBucketInput) -> Result<DeleteBucketOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    match client.delete_bucket(&input.bucket) {
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
// File Operations
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload File Input")]
pub struct UploadFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Bucket",
        description = "Bucket to upload to",
        example = "uploads"
    )]
    pub bucket: String,

    #[field(
        display_name = "Key",
        description = "Object key (file path within the bucket)",
        example = "reports/2026-03-22.csv"
    )]
    pub key: String,

    #[field(
        display_name = "Content",
        description = "File content as base64-encoded string or plain text",
        example = "SGVsbG8gV29ybGQ="
    )]
    pub content: String,

    #[field(
        display_name = "Content Type",
        description = "MIME type of the file (e.g., text/csv, image/png)",
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
#[capability_output(display_name = "Upload File Output")]
pub struct UploadFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Key", description = "The key of the uploaded file")]
    pub key: Option<String>,

    #[field(
        display_name = "Size",
        description = "Size in bytes of the uploaded file"
    )]
    pub size: Option<u64>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

/// Size limit for uploads: 50 MB
const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;

#[capability(
    module = "s3_storage",
    display_name = "Upload File",
    description = "Upload a file to S3-compatible storage. Content can be base64-encoded binary or plain text.",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible",
    side_effects = true
)]
pub fn storage_upload_file(input: UploadFileInput) -> Result<UploadFileOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    let is_base64 = input.is_base64.unwrap_or(true);
    let data = if is_base64 {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(&input.content)
            .map_err(|e| {
                permanent_error(
                    "S3_INVALID_CONTENT",
                    &format!("Invalid base64: {}", e),
                    json!({}),
                )
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
                "File exceeds maximum size of {} MB",
                MAX_UPLOAD_SIZE / 1024 / 1024
            )),
        });
    }

    let size = data.len() as u64;
    let ct = input.content_type.as_deref();

    match client.put_object(&input.bucket, &input.key, data, ct) {
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
#[capability_input(display_name = "Download File Input")]
pub struct DownloadFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Bucket",
        description = "Bucket to download from",
        example = "uploads"
    )]
    pub bucket: String,

    #[field(
        display_name = "Key",
        description = "Object key (file path within the bucket)",
        example = "reports/2026-03-22.csv"
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
#[capability_output(display_name = "Download File Output")]
pub struct DownloadFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Content",
        description = "File content (base64 or text)"
    )]
    pub content: Option<String>,

    #[field(display_name = "Content Type", description = "MIME type of the file")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "Size in bytes")]
    pub size: Option<u64>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "s3_storage",
    display_name = "Download File",
    description = "Download a file from S3-compatible storage. Returns base64-encoded content by default, or UTF-8 text if as_text is true.",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible"
)]
pub fn storage_download_file(input: DownloadFileInput) -> Result<DownloadFileOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    // Get metadata first for content_type
    let metadata = client.head_object(&input.bucket, &input.key).ok();

    match client.get_object(&input.bucket, &input.key) {
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
#[capability_input(display_name = "List Files Input")]
pub struct ListFilesInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Bucket",
        description = "Bucket to list files from",
        example = "uploads"
    )]
    pub bucket: String,

    #[field(
        display_name = "Prefix",
        description = "Filter files by key prefix (like a folder path)",
        example = "reports/"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    #[field(
        display_name = "Max Keys",
        description = "Maximum number of files to return (default: 1000)",
        example = "100"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_keys: Option<u32>,

    #[field(
        display_name = "Continuation Token",
        description = "Token for paginating through results"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Files Output")]
pub struct ListFilesOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Files",
        description = "List of file objects with key, size, last_modified, etag"
    )]
    pub files: Vec<Value>,

    #[field(display_name = "Count", description = "Number of files returned")]
    pub count: u32,

    #[field(
        display_name = "Next Token",
        description = "Token for fetching the next page"
    )]
    pub next_continuation_token: Option<String>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "s3_storage",
    display_name = "List Files",
    description = "List files in a bucket with optional prefix filter and pagination",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible"
)]
pub fn storage_list_files(input: ListFilesInput) -> Result<ListFilesOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    match client.list_objects(
        &input.bucket,
        input.prefix.as_deref(),
        input.max_keys,
        input.continuation_token.as_deref(),
    ) {
        Ok((objects, next_token)) => {
            let count = objects.len() as u32;
            let files: Vec<Value> = objects
                .iter()
                .map(|o| {
                    json!({
                        "key": o.key,
                        "size": o.size,
                        "last_modified": o.last_modified,
                        "etag": o.etag,
                    })
                })
                .collect();
            Ok(ListFilesOutput {
                success: true,
                files,
                count,
                next_continuation_token: next_token,
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
#[capability_input(display_name = "Get File Info Input")]
pub struct GetFileInfoInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Bucket", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/2026-03-22.csv")]
    pub key: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get File Info Output")]
pub struct GetFileInfoOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Content Type")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "File size in bytes")]
    pub size: Option<u64>,

    #[field(display_name = "ETag")]
    pub etag: Option<String>,

    #[field(display_name = "Last Modified")]
    pub last_modified: Option<String>,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "s3_storage",
    display_name = "Get File Info",
    description = "Get metadata about a file without downloading it (content type, size, last modified)",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible"
)]
pub fn storage_get_file_info(input: GetFileInfoInput) -> Result<GetFileInfoOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    match client.head_object(&input.bucket, &input.key) {
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
#[capability_input(display_name = "Delete File Input")]
pub struct DeleteFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Bucket", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/old-report.csv")]
    pub key: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete File Output")]
pub struct DeleteFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "s3_storage",
    display_name = "Delete File",
    description = "Delete a file from S3-compatible storage",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible",
    side_effects = true
)]
pub fn storage_delete_file(input: DeleteFileInput) -> Result<DeleteFileOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    match client.delete_object(&input.bucket, &input.key) {
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
#[capability_input(display_name = "Copy File Input")]
pub struct CopyFileInput {
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Source Bucket", example = "uploads")]
    pub source_bucket: String,

    #[field(display_name = "Source Key", example = "temp/file.csv")]
    pub source_key: String,

    #[field(display_name = "Destination Bucket", example = "archive")]
    pub destination_bucket: String,

    #[field(display_name = "Destination Key", example = "2026/03/file.csv")]
    pub destination_key: String,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Copy File Output")]
pub struct CopyFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    pub error: Option<String>,
}

#[capability(
    module = "s3_storage",
    display_name = "Copy File",
    description = "Copy a file within or across buckets in S3-compatible storage",
    module_display_name = "S3 Storage",
    module_supports_connections = true,
    module_integration_ids = "s3_compatible",
    side_effects = true
)]
pub fn storage_copy_file(input: CopyFileInput) -> Result<CopyFileOutput, String> {
    let conn = require_connection(&input._connection)?;
    let client = get_or_create_s3_client(conn)?;

    match client.copy_object(
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
