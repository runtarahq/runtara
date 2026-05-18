//! S3-Compatible Storage integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/s3_storage.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope, injecting `X-Runtara-Connection-Id` so the proxy can resolve
//! the connection, compute SigV4, and forward to S3. The component never sees
//! AWS credentials or performs any signing.
//!
//! No OnceLock, no client cache, no Arc<S3Client>. Each invoke() call is a
//! fresh wasmtime::Store; caching across calls is useless overhead here.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use std::time::Duration;

use base64::Engine as _;
use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "s3_storage".into(),
            display_name: "S3 Storage".into(),
            description: "S3-compatible object storage: bucket and file management, \
                          plus presigned URL generation. Credentials are injected \
                          server-side by the runtara HTTP proxy."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["s3_compatible".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            // Bucket operations
            cap(
                "storage-create-bucket",
                "storage_create_bucket",
                "Create Bucket",
                "Create a new bucket in S3-compatible storage",
                false,
                true,
                STORAGE_CREATE_BUCKET_INPUT_SCHEMA,
                STORAGE_CREATE_BUCKET_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-list-buckets",
                "storage_list_buckets",
                "List Buckets",
                "List all buckets in S3-compatible storage",
                false,
                false,
                STORAGE_LIST_BUCKETS_INPUT_SCHEMA,
                STORAGE_LIST_BUCKETS_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-delete-bucket",
                "storage_delete_bucket",
                "Delete Bucket",
                "Delete an empty bucket from S3-compatible storage",
                false,
                true,
                STORAGE_DELETE_BUCKET_INPUT_SCHEMA,
                STORAGE_DELETE_BUCKET_OUTPUT_SCHEMA,
            ),
            // File operations
            cap(
                "storage-upload-file",
                "storage_upload_file",
                "Upload File",
                "Upload a file to S3-compatible storage. Content can be base64-encoded binary or plain text.",
                false,
                true,
                STORAGE_UPLOAD_FILE_INPUT_SCHEMA,
                STORAGE_UPLOAD_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-download-file",
                "storage_download_file",
                "Download File",
                "Download a file from S3-compatible storage. Returns base64-encoded content by default, or UTF-8 text if as_text is true.",
                false,
                false,
                STORAGE_DOWNLOAD_FILE_INPUT_SCHEMA,
                STORAGE_DOWNLOAD_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-list-files",
                "storage_list_files",
                "List Files",
                "List files in a bucket with optional prefix filter and pagination",
                false,
                false,
                STORAGE_LIST_FILES_INPUT_SCHEMA,
                STORAGE_LIST_FILES_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-get-file-info",
                "storage_get_file_info",
                "Get File Info",
                "Get metadata about a file without downloading it (content type, size, last modified)",
                false,
                false,
                STORAGE_GET_FILE_INFO_INPUT_SCHEMA,
                STORAGE_GET_FILE_INFO_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-delete-file",
                "storage_delete_file",
                "Delete File",
                "Delete a file from S3-compatible storage",
                false,
                true,
                STORAGE_DELETE_FILE_INPUT_SCHEMA,
                STORAGE_DELETE_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-copy-file",
                "storage_copy_file",
                "Copy File",
                "Copy a file within or across buckets in S3-compatible storage",
                false,
                true,
                STORAGE_COPY_FILE_INPUT_SCHEMA,
                STORAGE_COPY_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-generate-presigned-url",
                "storage_generate_presigned_url",
                "Generate Presigned URL",
                "Generate a time-limited presigned URL for downloading, uploading, or deleting an object. \
                 Callers consume the URL directly without going through runtara.",
                false,
                false,
                STORAGE_GENERATE_PRESIGNED_URL_INPUT_SCHEMA,
                STORAGE_GENERATE_PRESIGNED_URL_OUTPUT_SCHEMA,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "storage-create-bucket" => storage_create_bucket(&input, connection.as_ref()),
            "storage-list-buckets" => storage_list_buckets(&input, connection.as_ref()),
            "storage-delete-bucket" => storage_delete_bucket(&input, connection.as_ref()),
            "storage-upload-file" => storage_upload_file(&input, connection.as_ref()),
            "storage-download-file" => storage_download_file(&input, connection.as_ref()),
            "storage-list-files" => storage_list_files(&input, connection.as_ref()),
            "storage-get-file-info" => storage_get_file_info(&input, connection.as_ref()),
            "storage-delete-file" => storage_delete_file(&input, connection.as_ref()),
            "storage-copy-file" => storage_copy_file(&input, connection.as_ref()),
            "storage-generate-presigned-url" => {
                storage_generate_presigned_url(&input, connection.as_ref())
            }
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("s3_storage agent has no capability `{other}`"),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Helper: build a CapabilityInfo
// -----------------------------------------------------------------------------

fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    is_idempotent: bool,
    has_side_effects: bool,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects,
        is_idempotent,
        rate_limited: false,
        tags: vec!["s3".into(), "storage".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// -----------------------------------------------------------------------------
// Shared helpers
// -----------------------------------------------------------------------------

/// Require a connection or return `S3_MISSING_CONNECTION`.
fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection.ok_or_else(|| {
        permanent_err(
            "S3_MISSING_CONNECTION",
            "No S3 connection configured. Add an s3_compatible connection to this step.",
        )
    })
}

/// Build the relative path for a bucket: `/{bucket}`.
fn bucket_path(bucket: &str) -> String {
    format!("/{}", bucket)
}

/// Build the relative path for an object: `/{bucket}/{encoded_key}`.
fn object_path(bucket: &str, key: &str) -> String {
    format!("/{}/{}", bucket, url_encode_s3_key(key))
}

/// S3-safe URL encoding: keeps `/` unencoded (keys may be hierarchical).
fn url_encode_s3_key(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                String::from(b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// Fire an HTTP request via the runtara proxy (SigV4 is done server-side).
fn s3_request(
    method: &str,
    path: &str,
    connection_id: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<runtara_http::HttpResponse, ErrorInfo> {
    let client = runtara_http::HttpClient::with_timeout(Duration::from_secs(60));
    let mut req = client
        .request(method, path)
        .header("X-Runtara-Connection-Id", connection_id);

    for (k, v) in headers {
        req = req.header(k, v);
    }

    if let Some(data) = body {
        req = req.body_bytes(data);
    }

    req.call_agent().map_err(|e| {
        transient_err(
            "NETWORK_ERROR",
            format!("S3 request {method} {path} failed: {e}"),
        )
    })
}

/// Extract a human-readable message from an S3 XML error body.
fn parse_s3_error(body: &str) -> String {
    let code = extract_xml_tag(body, "Code");
    let message = extract_xml_tag(body, "Message");
    match (code, message) {
        (Some(c), Some(m)) => format!("{}: {}", c, m),
        (Some(c), None) => c,
        (None, Some(m)) => m,
        (None, None) => {
            let trimmed = body.trim();
            if trimmed.len() > 200 {
                format!("{}...", &trimmed[..200])
            } else {
                trimmed.to_string()
            }
        }
    }
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

fn parse_list_buckets_xml(xml: &str) -> Vec<Value> {
    let mut buckets = Vec::new();
    for block in xml.split("<Bucket>").skip(1) {
        let name = extract_xml_tag(block, "Name").unwrap_or_default();
        let creation_date = extract_xml_tag(block, "CreationDate").unwrap_or_default();
        if !name.is_empty() {
            buckets.push(json!({"name": name, "creation_date": creation_date}));
        }
    }
    buckets
}

fn parse_list_objects_xml(xml: &str) -> (Vec<Value>, Option<String>) {
    let next_token = extract_xml_tag(xml, "NextContinuationToken");
    let mut objects = Vec::new();
    for block in xml.split("<Contents>").skip(1) {
        let key = extract_xml_tag(block, "Key").unwrap_or_default();
        let size: u64 = extract_xml_tag(block, "Size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let last_modified = extract_xml_tag(block, "LastModified").unwrap_or_default();
        let etag = extract_xml_tag(block, "ETag").unwrap_or_default();
        if !key.is_empty() {
            objects.push(json!({
                "key": key,
                "size": size,
                "last_modified": last_modified,
                "etag": etag,
            }));
        }
    }
    (objects, next_token)
}

// -----------------------------------------------------------------------------
// Capability 1: Create Bucket
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateBucketInput {
    bucket: String,
}

#[derive(Debug, Serialize)]
struct CreateBucketOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_create_bucket(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: CreateBucketInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let path = bucket_path(&input.bucket);
    let resp = s3_request("PUT", &path, &conn.connection_id, &[], None)?;

    let out = match resp.status {
        200 | 201 | 409 => CreateBucketOutput {
            success: true,
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            CreateBucketOutput {
                success: false,
                error: Some(format!("CreateBucket failed: {}", parse_s3_error(&body))),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 2: List Buckets
// -----------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ListBucketsOutput {
    success: bool,
    buckets: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_list_buckets(
    _input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let conn = require_connection(connection)?;

    let resp = s3_request("GET", "/", &conn.connection_id, &[], None)?;

    let out = if resp.status == 200 {
        let xml = String::from_utf8_lossy(&resp.body).to_string();
        ListBucketsOutput {
            success: true,
            buckets: parse_list_buckets_xml(&xml),
            error: None,
        }
    } else {
        let body = String::from_utf8_lossy(&resp.body).to_string();
        ListBucketsOutput {
            success: false,
            buckets: vec![],
            error: Some(format!("ListBuckets failed: {}", parse_s3_error(&body))),
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 3: Delete Bucket
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DeleteBucketInput {
    bucket: String,
}

#[derive(Debug, Serialize)]
struct DeleteBucketOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_delete_bucket(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: DeleteBucketInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let path = bucket_path(&input.bucket);
    let resp = s3_request("DELETE", &path, &conn.connection_id, &[], None)?;

    let out = match resp.status {
        200 | 204 | 404 => DeleteBucketOutput {
            success: true,
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            DeleteBucketOutput {
                success: false,
                error: Some(format!("DeleteBucket failed: {}", parse_s3_error(&body))),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 4: Upload File
// -----------------------------------------------------------------------------

/// Size limit for uploads: 50 MB (matches legacy constant).
const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct UploadFileInput {
    bucket: String,
    key: String,
    content: String,
    #[serde(default)]
    content_type: Option<String>,
    /// Whether content is base64-encoded (default: true).
    #[serde(default = "default_true_opt")]
    is_base64: Option<bool>,
}

fn default_true_opt() -> Option<bool> {
    Some(true)
}

#[derive(Debug, Serialize)]
struct UploadFileOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_upload_file(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: UploadFileInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    // Decode content
    let is_base64 = input.is_base64.unwrap_or(true);
    let data = if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(&input.content)
            .map_err(|e| permanent_err("S3_INVALID_CONTENT", format!("Invalid base64: {}", e)))?
    } else {
        input.content.into_bytes()
    };

    if data.len() > MAX_UPLOAD_SIZE {
        let out = UploadFileOutput {
            success: false,
            key: None,
            size: None,
            error: Some(format!(
                "File exceeds maximum size of {} MB",
                MAX_UPLOAD_SIZE / 1024 / 1024
            )),
        };
        return serde_json::to_string(&out)
            .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()));
    }

    let size = data.len() as u64;
    let ct = input
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    let path = object_path(&input.bucket, &input.key);

    let resp = s3_request(
        "PUT",
        &path,
        &conn.connection_id,
        &[("Content-Type", ct)],
        Some(&data),
    )?;

    let out = match resp.status {
        200 | 201 => UploadFileOutput {
            success: true,
            key: Some(input.key),
            size: Some(size),
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            UploadFileOutput {
                success: false,
                key: None,
                size: None,
                error: Some(format!("PutObject failed: {}", parse_s3_error(&body))),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 5: Download File
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DownloadFileInput {
    bucket: String,
    key: String,
    #[serde(default)]
    as_text: Option<bool>,
}

#[derive(Debug, Serialize)]
struct DownloadFileOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_download_file(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: DownloadFileInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    // HEAD first to get content_type (mirrors legacy behaviour).
    let head_path = object_path(&input.bucket, &input.key);
    let content_type = s3_request("HEAD", &head_path, &conn.connection_id, &[], None)
        .ok()
        .and_then(|r| {
            if r.status == 200 {
                r.headers
                    .get("content-type")
                    .cloned()
                    .or_else(|| Some("application/octet-stream".to_string()))
            } else {
                None
            }
        });

    let get_path = object_path(&input.bucket, &input.key);
    let resp = s3_request("GET", &get_path, &conn.connection_id, &[], None)?;

    let out = if resp.status == 200 {
        let size = resp.body.len() as u64;
        let content = if input.as_text.unwrap_or(false) {
            String::from_utf8(resp.body).unwrap_or_else(|e| {
                base64::engine::general_purpose::STANDARD.encode(e.into_bytes())
            })
        } else {
            base64::engine::general_purpose::STANDARD.encode(&resp.body)
        };
        DownloadFileOutput {
            success: true,
            content: Some(content),
            content_type,
            size: Some(size),
            error: None,
        }
    } else {
        let body = String::from_utf8_lossy(&resp.body).to_string();
        DownloadFileOutput {
            success: false,
            content: None,
            content_type: None,
            size: None,
            error: Some(format!("GetObject failed: {}", parse_s3_error(&body))),
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 6: List Files
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListFilesInput {
    bucket: String,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    max_keys: Option<u32>,
    #[serde(default)]
    continuation_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct ListFilesOutput {
    success: bool,
    files: Vec<Value>,
    count: u32,
    next_continuation_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_list_files(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: ListFilesInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let mut query_parts = vec!["list-type=2".to_string()];
    if let Some(p) = &input.prefix {
        query_parts.push(format!("prefix={}", url_encode_s3_key(p)));
    }
    if let Some(m) = input.max_keys {
        query_parts.push(format!("max-keys={}", m));
    }
    if let Some(t) = &input.continuation_token {
        query_parts.push(format!("continuation-token={}", url_encode_s3_key(t)));
    }

    let path = format!("/{}?{}", input.bucket, query_parts.join("&"));
    let resp = s3_request("GET", &path, &conn.connection_id, &[], None)?;

    let out = if resp.status == 200 {
        let xml = String::from_utf8_lossy(&resp.body).to_string();
        let (files, next_token) = parse_list_objects_xml(&xml);
        let count = files.len() as u32;
        ListFilesOutput {
            success: true,
            files,
            count,
            next_continuation_token: next_token,
            error: None,
        }
    } else {
        let body = String::from_utf8_lossy(&resp.body).to_string();
        ListFilesOutput {
            success: false,
            files: vec![],
            count: 0,
            next_continuation_token: None,
            error: Some(format!("ListObjects failed: {}", parse_s3_error(&body))),
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 7: Get File Info
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GetFileInfoInput {
    bucket: String,
    key: String,
}

#[derive(Debug, Serialize)]
struct GetFileInfoOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_get_file_info(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: GetFileInfoInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let path = object_path(&input.bucket, &input.key);
    let resp = s3_request("HEAD", &path, &conn.connection_id, &[], None)?;

    let out = if resp.status == 200 {
        let content_type = resp
            .headers
            .get("content-type")
            .cloned()
            .or_else(|| Some("application/octet-stream".to_string()));
        let size: Option<u64> = resp
            .headers
            .get("content-length")
            .and_then(|v| v.parse().ok());
        let etag = resp.headers.get("etag").cloned();
        let last_modified = resp.headers.get("last-modified").cloned();
        GetFileInfoOutput {
            success: true,
            content_type,
            size,
            etag,
            last_modified,
            error: None,
        }
    } else {
        GetFileInfoOutput {
            success: false,
            content_type: None,
            size: None,
            etag: None,
            last_modified: None,
            error: Some(format!("HeadObject failed (status {})", resp.status)),
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 8: Delete File
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DeleteFileInput {
    bucket: String,
    key: String,
}

#[derive(Debug, Serialize)]
struct DeleteFileOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_delete_file(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: DeleteFileInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let path = object_path(&input.bucket, &input.key);
    let resp = s3_request("DELETE", &path, &conn.connection_id, &[], None)?;

    let out = match resp.status {
        200 | 204 | 404 => DeleteFileOutput {
            success: true,
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            DeleteFileOutput {
                success: false,
                error: Some(format!("DeleteObject failed: {}", parse_s3_error(&body))),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 9: Copy File
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CopyFileInput {
    source_bucket: String,
    source_key: String,
    destination_bucket: String,
    destination_key: String,
}

#[derive(Debug, Serialize)]
struct CopyFileOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_copy_file(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: CopyFileInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let dst_path = object_path(&input.destination_bucket, &input.destination_key);
    let copy_source = format!("/{}/{}", input.source_bucket, input.source_key);
    let resp = s3_request(
        "PUT",
        &dst_path,
        &conn.connection_id,
        &[("x-amz-copy-source", &copy_source)],
        None,
    )?;

    let out = match resp.status {
        200 | 201 => CopyFileOutput {
            success: true,
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            CopyFileOutput {
                success: false,
                error: Some(format!("CopyObject failed: {}", parse_s3_error(&body))),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 10: Generate Presigned URL
// -----------------------------------------------------------------------------

const DEFAULT_PRESIGN_EXPIRES_SECONDS: u64 = 3600;

#[derive(Debug, Deserialize)]
struct GeneratePresignedUrlInput {
    bucket: String,
    key: String,
    operation: String,
    #[serde(default)]
    expires_in_seconds: Option<u64>,
    #[serde(default)]
    content_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct GeneratePresignedUrlOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn storage_generate_presigned_url(
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let input: GeneratePresignedUrlInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let conn = require_connection(connection)?;

    let method = match input.operation.to_lowercase().as_str() {
        "download" | "get" | "read" => "GET",
        "upload" | "put" | "write" | "create" => "PUT",
        "delete" => "DELETE",
        other => {
            let out = GeneratePresignedUrlOutput {
                success: false,
                url: None,
                expires_in_seconds: None,
                error: Some(format!(
                    "Unsupported operation `{}` (expected download, upload, or delete)",
                    other
                )),
            };
            return serde_json::to_string(&out)
                .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()));
        }
    };

    let path = format!("/{}/{}", input.bucket, input.key);
    let expires = input
        .expires_in_seconds
        .unwrap_or(DEFAULT_PRESIGN_EXPIRES_SECONDS);

    let out = match runtara_http::presign(
        &conn.connection_id,
        method,
        &path,
        expires,
        input.content_type.as_deref(),
    ) {
        Ok(result) => GeneratePresignedUrlOutput {
            success: true,
            url: Some(result.url),
            expires_in_seconds: Some(result.expires_in_seconds),
            error: None,
        },
        Err(e) => GeneratePresignedUrlOutput {
            success: false,
            url: None,
            expires_in_seconds: None,
            error: Some(e.to_string()),
        },
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Error helpers
// -----------------------------------------------------------------------------

fn permanent_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn transient_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "transient".into(),
        severity: "warning".into(),
        retryable: true,
        retry_after_ms: None,
        attributes: None,
    }
}

// -----------------------------------------------------------------------------
// JSON Schemas — mirror legacy field names and defaults exactly
// -----------------------------------------------------------------------------

const STORAGE_CREATE_BUCKET_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket"],
    "properties": {
        "bucket": { "type": "string", "description": "Name of the bucket to create", "example": "uploads" }
    }
}"#;

const STORAGE_CREATE_BUCKET_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean" },
        "error":   { "type": "string" }
    }
}"#;

const STORAGE_LIST_BUCKETS_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {}
}"#;

const STORAGE_LIST_BUCKETS_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean" },
        "buckets": { "type": "array", "items": {}, "description": "List of bucket names and creation dates" },
        "error":   { "type": "string" }
    }
}"#;

const STORAGE_DELETE_BUCKET_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket"],
    "properties": {
        "bucket": { "type": "string", "description": "Name of the bucket to delete (must be empty)", "example": "old-exports" }
    }
}"#;

const STORAGE_DELETE_BUCKET_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean" },
        "error":   { "type": "string" }
    }
}"#;

const STORAGE_UPLOAD_FILE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket", "key", "content"],
    "properties": {
        "bucket":       { "type": "string", "description": "Bucket to upload to", "example": "uploads" },
        "key":          { "type": "string", "description": "Object key (file path within the bucket)", "example": "reports/2026-03-22.csv" },
        "content":      { "type": "string", "description": "File content as base64-encoded string or plain text", "example": "SGVsbG8gV29ybGQ=" },
        "content_type": { "type": "string", "description": "MIME type of the file (e.g., text/csv, image/png)", "example": "text/csv" },
        "is_base64":    { "type": "boolean", "description": "Whether content is base64-encoded (default: true)", "default": true }
    }
}"#;

const STORAGE_UPLOAD_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean" },
        "key":     { "type": "string", "description": "The key of the uploaded file" },
        "size":    { "type": "integer", "description": "Size in bytes of the uploaded file" },
        "error":   { "type": "string" }
    }
}"#;

const STORAGE_DOWNLOAD_FILE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket", "key"],
    "properties": {
        "bucket":  { "type": "string", "description": "Bucket to download from", "example": "uploads" },
        "key":     { "type": "string", "description": "Object key (file path within the bucket)", "example": "reports/2026-03-22.csv" },
        "as_text": { "type": "boolean", "description": "Return content as UTF-8 text instead of base64 (default: false)", "default": false }
    }
}"#;

const STORAGE_DOWNLOAD_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":      { "type": "boolean" },
        "content":      { "type": "string", "description": "File content (base64 or text)" },
        "content_type": { "type": "string", "description": "MIME type of the file" },
        "size":         { "type": "integer", "description": "Size in bytes" },
        "error":        { "type": "string" }
    }
}"#;

const STORAGE_LIST_FILES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket"],
    "properties": {
        "bucket":             { "type": "string", "description": "Bucket to list files from", "example": "uploads" },
        "prefix":             { "type": "string", "description": "Filter files by key prefix (like a folder path)", "example": "reports/" },
        "max_keys":           { "type": "integer", "description": "Maximum number of files to return (default: 1000)", "example": 100 },
        "continuation_token": { "type": "string", "description": "Token for paginating through results" }
    }
}"#;

const STORAGE_LIST_FILES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":                  { "type": "boolean" },
        "files":                    { "type": "array", "items": {}, "description": "List of file objects with key, size, last_modified, etag" },
        "count":                    { "type": "integer", "description": "Number of files returned" },
        "next_continuation_token":  { "type": "string", "description": "Token for fetching the next page" },
        "error":                    { "type": "string" }
    }
}"#;

const STORAGE_GET_FILE_INFO_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket", "key"],
    "properties": {
        "bucket": { "type": "string", "example": "uploads" },
        "key":    { "type": "string", "example": "reports/2026-03-22.csv" }
    }
}"#;

const STORAGE_GET_FILE_INFO_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":       { "type": "boolean" },
        "content_type":  { "type": "string" },
        "size":          { "type": "integer", "description": "File size in bytes" },
        "etag":          { "type": "string" },
        "last_modified": { "type": "string" },
        "error":         { "type": "string" }
    }
}"#;

const STORAGE_DELETE_FILE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket", "key"],
    "properties": {
        "bucket": { "type": "string", "example": "uploads" },
        "key":    { "type": "string", "example": "reports/old-report.csv" }
    }
}"#;

const STORAGE_DELETE_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean" },
        "error":   { "type": "string" }
    }
}"#;

const STORAGE_COPY_FILE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["source_bucket", "source_key", "destination_bucket", "destination_key"],
    "properties": {
        "source_bucket":      { "type": "string", "example": "uploads" },
        "source_key":         { "type": "string", "example": "temp/file.csv" },
        "destination_bucket": { "type": "string", "example": "archive" },
        "destination_key":    { "type": "string", "example": "2026/03/file.csv" }
    }
}"#;

const STORAGE_COPY_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean" },
        "error":   { "type": "string" }
    }
}"#;

const STORAGE_GENERATE_PRESIGNED_URL_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket", "key", "operation"],
    "properties": {
        "bucket":             { "type": "string", "example": "uploads" },
        "key":                { "type": "string", "example": "reports/2026-05-16.csv" },
        "operation":          { "type": "string", "description": "What the URL will be used for: download, upload, or delete", "enum": ["download", "get", "read", "upload", "put", "write", "create", "delete"], "example": "download" },
        "expires_in_seconds": { "type": "integer", "description": "Lifetime of the signed URL in seconds (max 604800 = 7 days, default 3600)", "default": 3600, "example": 3600 },
        "content_type":       { "type": "string", "description": "For upload URLs, the MIME type the caller will use (e.g., text/csv)", "example": "text/csv" }
    }
}"#;

const STORAGE_GENERATE_PRESIGNED_URL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":            { "type": "boolean" },
        "url":                { "type": "string", "description": "Time-limited presigned URL" },
        "expires_in_seconds": { "type": "integer", "description": "Actual lifetime of the URL after server-side clamping" },
        "error":              { "type": "string" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
