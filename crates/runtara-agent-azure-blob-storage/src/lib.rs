//! Azure Blob Storage integration agent — WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/integrations/azure_blob_storage.rs`.
//!
//! Routing model: the underlying `runtara-http` client reads
//! `RUNTARA_HTTP_PROXY_URL` and forwards every request through the proxy as a
//! JSON envelope, injecting `X-Runtara-Connection-Id` so the proxy can resolve
//! the connection, compute Azure Shared Key HMAC, and forward to
//! `https://{account}.blob.core.windows.net`. The component never sees storage
//! account keys and performs no signing.
//!
//! Presigned SAS URLs are obtained via `runtara_http::presign(...)` — same
//! mechanism as the s3_storage agent, fully server-side.
//!
//! No OnceLock, no client cache, no Arc<AzureBlobClient>. Each invoke() call
//! is a fresh wasmtime::Store; per-call caching is useless overhead here.

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
            id: "azure_blob_storage".into(),
            display_name: "Azure Blob Storage".into(),
            description: "Azure Blob Storage: container and blob management, \
                          plus SAS URL generation. Credentials are injected \
                          server-side by the runtara HTTP proxy — the component \
                          never handles Shared Key material."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["azure_blob_storage".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            // Container operations
            cap(
                "storage-create-bucket",
                "storage_create_bucket",
                "Create Container",
                "Create a new container in Azure Blob Storage",
                false,
                true,
                STORAGE_CREATE_BUCKET_INPUT_SCHEMA,
                STORAGE_CREATE_BUCKET_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-list-buckets",
                "storage_list_buckets",
                "List Containers",
                "List all containers in the storage account",
                false,
                false,
                STORAGE_LIST_BUCKETS_INPUT_SCHEMA,
                STORAGE_LIST_BUCKETS_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-delete-bucket",
                "storage_delete_bucket",
                "Delete Container",
                "Delete an empty container from Azure Blob Storage",
                false,
                true,
                STORAGE_DELETE_BUCKET_INPUT_SCHEMA,
                STORAGE_DELETE_BUCKET_OUTPUT_SCHEMA,
            ),
            // Blob operations
            cap(
                "storage-upload-file",
                "storage_upload_file",
                "Upload Blob",
                "Upload a blob to Azure Blob Storage. Content can be base64-encoded binary or plain text.",
                false,
                true,
                STORAGE_UPLOAD_FILE_INPUT_SCHEMA,
                STORAGE_UPLOAD_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-download-file",
                "storage_download_file",
                "Download Blob",
                "Download a blob from Azure Blob Storage. Returns base64-encoded content by default, or UTF-8 text if as_text is true.",
                false,
                false,
                STORAGE_DOWNLOAD_FILE_INPUT_SCHEMA,
                STORAGE_DOWNLOAD_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-list-files",
                "storage_list_files",
                "List Blobs",
                "List blobs in a container with optional prefix filter and pagination",
                false,
                false,
                STORAGE_LIST_FILES_INPUT_SCHEMA,
                STORAGE_LIST_FILES_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-get-file-info",
                "storage_get_file_info",
                "Get Blob Info",
                "Get metadata about a blob without downloading it (content type, size, last modified)",
                false,
                false,
                STORAGE_GET_FILE_INFO_INPUT_SCHEMA,
                STORAGE_GET_FILE_INFO_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-delete-file",
                "storage_delete_file",
                "Delete Blob",
                "Delete a blob from Azure Blob Storage",
                false,
                true,
                STORAGE_DELETE_FILE_INPUT_SCHEMA,
                STORAGE_DELETE_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-copy-file",
                "storage_copy_file",
                "Copy Blob",
                "Copy a blob within or across containers in the same Azure Blob Storage account",
                false,
                true,
                STORAGE_COPY_FILE_INPUT_SCHEMA,
                STORAGE_COPY_FILE_OUTPUT_SCHEMA,
            ),
            cap(
                "storage-generate-presigned-url",
                "storage_generate_presigned_url",
                "Generate Presigned URL",
                "Generate a time-limited Shared Access Signature (SAS) URL for downloading, uploading, or deleting a blob. \
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
                format!("azure_blob_storage agent has no capability `{other}`"),
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
        tags: vec!["azure".into(), "storage".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

// -----------------------------------------------------------------------------
// Shared helpers
// -----------------------------------------------------------------------------

/// Require a connection or return `AZURE_BLOB_MISSING_CONNECTION`.
fn require_connection(connection: Option<&ConnectionInfo>) -> Result<&ConnectionInfo, ErrorInfo> {
    connection.ok_or_else(|| {
        permanent_err(
            "AZURE_BLOB_MISSING_CONNECTION",
            "No Azure Blob Storage connection configured. Add an azure_blob_storage connection to this step.",
        )
    })
}

/// Build the relative path for a container: `/{container}?restype=container`.
fn container_path(container: &str) -> String {
    format!("/{}?restype=container", container)
}

/// Build the relative path for a blob: `/{container}/{encoded_key}`.
fn blob_path(container: &str, key: &str) -> String {
    format!("/{}/{}", container, url_encode_blob_key(key))
}

/// Azure-safe URL encoding: keeps `/` unencoded (keys may be hierarchical).
fn url_encode_blob_key(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                String::from(b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// Fire an HTTP request via the runtara proxy (Azure Shared Key signing is done server-side).
fn azure_request(
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
            format!("Azure Blob request {method} {path} failed: {e}"),
        )
    })
}

/// Extract a human-readable message from an Azure Blob Storage XML error body.
fn parse_azure_error(body: &str) -> String {
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

/// Parse the Azure `?comp=list` XML response for listing containers.
/// Returns vec of `{"name": "...", "last_modified": "..."}` objects.
fn parse_list_containers_xml(xml: &str) -> Vec<Value> {
    let mut containers = Vec::new();
    for block in xml.split("<Container>").skip(1) {
        let name = extract_xml_tag(block, "Name").unwrap_or_default();
        let last_modified = extract_xml_tag(block, "Last-Modified").unwrap_or_default();
        if !name.is_empty() {
            containers.push(json!({"name": name, "last_modified": last_modified}));
        }
    }
    containers
}

/// Parse the Azure `?restype=container&comp=list` XML response for listing blobs.
/// Returns `(files, next_marker)` where files have `key`, `size`, `last_modified`, `etag`.
fn parse_list_blobs_xml(xml: &str) -> (Vec<Value>, Option<String>) {
    // Azure uses <NextMarker> (empty tag = no more pages)
    let next_marker = extract_xml_tag(xml, "NextMarker").filter(|s| !s.is_empty());
    let mut objects = Vec::new();
    for block in xml.split("<Blob>").skip(1) {
        let key = extract_xml_tag(block, "Name").unwrap_or_default();
        let size: u64 = extract_xml_tag(block, "Content-Length")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let last_modified = extract_xml_tag(block, "Last-Modified").unwrap_or_default();
        let etag = extract_xml_tag(block, "Etag").unwrap_or_default();
        if !key.is_empty() {
            objects.push(json!({
                "key": key,
                "size": size,
                "last_modified": last_modified,
                "etag": etag,
            }));
        }
    }
    (objects, next_marker)
}

// -----------------------------------------------------------------------------
// Capability 1: Create Container
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

    // PUT /{container}?restype=container
    let path = container_path(&input.bucket);
    let resp = azure_request("PUT", &path, &conn.connection_id, &[], None)?;

    let out = match resp.status {
        201 | 409 => CreateBucketOutput {
            success: true,
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            CreateBucketOutput {
                success: false,
                error: Some(format!(
                    "CreateContainer failed: {}",
                    parse_azure_error(&body)
                )),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 2: List Containers
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

    // GET /?comp=list  — lists all containers in the storage account
    let resp = azure_request("GET", "/?comp=list", &conn.connection_id, &[], None)?;

    let out = if resp.status == 200 {
        let xml = String::from_utf8_lossy(&resp.body).to_string();
        ListBucketsOutput {
            success: true,
            buckets: parse_list_containers_xml(&xml),
            error: None,
        }
    } else {
        let body = String::from_utf8_lossy(&resp.body).to_string();
        ListBucketsOutput {
            success: false,
            buckets: vec![],
            error: Some(format!(
                "ListContainers failed: {}",
                parse_azure_error(&body)
            )),
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 3: Delete Container
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

    // DELETE /{container}?restype=container
    let path = container_path(&input.bucket);
    let resp = azure_request("DELETE", &path, &conn.connection_id, &[], None)?;

    let out = match resp.status {
        202 | 404 => DeleteBucketOutput {
            success: true,
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            DeleteBucketOutput {
                success: false,
                error: Some(format!(
                    "DeleteContainer failed: {}",
                    parse_azure_error(&body)
                )),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 4: Upload Blob
// -----------------------------------------------------------------------------

/// Size limit for single-shot uploads: 50 MB (matches legacy constant).
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
            .map_err(|e| {
                permanent_err(
                    "AZURE_BLOB_INVALID_CONTENT",
                    format!("Invalid base64: {}", e),
                )
            })?
    } else {
        input.content.into_bytes()
    };

    if data.len() > MAX_UPLOAD_SIZE {
        let out = UploadFileOutput {
            success: false,
            key: None,
            size: None,
            error: Some(format!(
                "Blob exceeds maximum size of {} MB",
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
    let path = blob_path(&input.bucket, &input.key);

    // Azure requires x-ms-blob-type: BlockBlob for PutBlob
    let resp = azure_request(
        "PUT",
        &path,
        &conn.connection_id,
        &[("Content-Type", ct), ("x-ms-blob-type", "BlockBlob")],
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
                error: Some(format!("PutBlob failed: {}", parse_azure_error(&body))),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 5: Download Blob
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
    let head_path = blob_path(&input.bucket, &input.key);
    let content_type = azure_request("HEAD", &head_path, &conn.connection_id, &[], None)
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

    let get_path = blob_path(&input.bucket, &input.key);
    let resp = azure_request("GET", &get_path, &conn.connection_id, &[], None)?;

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
            error: Some(format!("GetBlob failed: {}", parse_azure_error(&body))),
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 6: List Blobs
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

    // GET /{container}?restype=container&comp=list[&prefix=...][&maxresults=...][&marker=...]
    let mut query_parts = vec!["restype=container".to_string(), "comp=list".to_string()];
    if let Some(p) = &input.prefix {
        query_parts.push(format!("prefix={}", url_encode_blob_key(p)));
    }
    if let Some(m) = input.max_keys {
        query_parts.push(format!("maxresults={}", m));
    }
    if let Some(t) = &input.continuation_token {
        query_parts.push(format!("marker={}", url_encode_blob_key(t)));
    }

    let path = format!("/{}?{}", input.bucket, query_parts.join("&"));
    let resp = azure_request("GET", &path, &conn.connection_id, &[], None)?;

    let out = if resp.status == 200 {
        let xml = String::from_utf8_lossy(&resp.body).to_string();
        let (files, next_marker) = parse_list_blobs_xml(&xml);
        let count = files.len() as u32;
        ListFilesOutput {
            success: true,
            files,
            count,
            next_continuation_token: next_marker,
            error: None,
        }
    } else {
        let body = String::from_utf8_lossy(&resp.body).to_string();
        ListFilesOutput {
            success: false,
            files: vec![],
            count: 0,
            next_continuation_token: None,
            error: Some(format!("ListBlobs failed: {}", parse_azure_error(&body))),
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 7: Get Blob Info
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

    let path = blob_path(&input.bucket, &input.key);
    let resp = azure_request("HEAD", &path, &conn.connection_id, &[], None)?;

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
            error: Some(format!("HeadBlob failed (status {})", resp.status)),
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 8: Delete Blob
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

    let path = blob_path(&input.bucket, &input.key);
    let resp = azure_request("DELETE", &path, &conn.connection_id, &[], None)?;

    let out = match resp.status {
        202 | 404 => DeleteFileOutput {
            success: true,
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            DeleteFileOutput {
                success: false,
                error: Some(format!("DeleteBlob failed: {}", parse_azure_error(&body))),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 9: Copy Blob
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

    // Azure Copy Blob: PUT on the destination with x-ms-copy-source pointing at the source.
    // The proxy rewrites the absolute URL from the relative source path.
    let dst_path = blob_path(&input.destination_bucket, &input.destination_key);
    let copy_source = format!("/{}/{}", input.source_bucket, input.source_key);

    let resp = azure_request(
        "PUT",
        &dst_path,
        &conn.connection_id,
        &[("x-ms-copy-source", &copy_source)],
        None,
    )?;

    let out = match resp.status {
        202 | 200 | 201 => CopyFileOutput {
            success: true,
            error: None,
        },
        _ => {
            let body = String::from_utf8_lossy(&resp.body).to_string();
            CopyFileOutput {
                success: false,
                error: Some(format!("CopyBlob failed: {}", parse_azure_error(&body))),
            }
        }
    };
    serde_json::to_string(&out)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

// -----------------------------------------------------------------------------
// Capability 10: Generate Presigned (SAS) URL
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
        "bucket": { "type": "string", "description": "Name of the container to create", "example": "uploads" }
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
        "buckets": { "type": "array", "items": {}, "description": "List of container names and last-modified timestamps" },
        "error":   { "type": "string" }
    }
}"#;

const STORAGE_DELETE_BUCKET_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket"],
    "properties": {
        "bucket": { "type": "string", "description": "Name of the container to delete (must be empty)", "example": "old-exports" }
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
        "bucket":       { "type": "string", "description": "Container to upload to", "example": "uploads" },
        "key":          { "type": "string", "description": "Blob name (path within the container)", "example": "reports/2026-05-16.csv" },
        "content":      { "type": "string", "description": "Blob content as base64-encoded string or plain text", "example": "SGVsbG8gV29ybGQ=" },
        "content_type": { "type": "string", "description": "MIME type of the blob (e.g., text/csv, image/png)", "example": "text/csv" },
        "is_base64":    { "type": "boolean", "description": "Whether content is base64-encoded (default: true)", "default": true }
    }
}"#;

const STORAGE_UPLOAD_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success": { "type": "boolean" },
        "key":     { "type": "string", "description": "Blob name of the uploaded blob" },
        "size":    { "type": "integer", "description": "Size in bytes of the uploaded blob" },
        "error":   { "type": "string" }
    }
}"#;

const STORAGE_DOWNLOAD_FILE_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket", "key"],
    "properties": {
        "bucket":  { "type": "string", "description": "Container to download from", "example": "uploads" },
        "key":     { "type": "string", "description": "Blob name (path within the container)", "example": "reports/2026-05-16.csv" },
        "as_text": { "type": "boolean", "description": "Return content as UTF-8 text instead of base64 (default: false)", "default": false }
    }
}"#;

const STORAGE_DOWNLOAD_FILE_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":      { "type": "boolean" },
        "content":      { "type": "string", "description": "Blob content (base64 or text)" },
        "content_type": { "type": "string", "description": "MIME type of the blob" },
        "size":         { "type": "integer", "description": "Size in bytes" },
        "error":        { "type": "string" }
    }
}"#;

const STORAGE_LIST_FILES_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket"],
    "properties": {
        "bucket":             { "type": "string", "description": "Container to list blobs from", "example": "uploads" },
        "prefix":             { "type": "string", "description": "Filter blobs by name prefix (like a folder path)", "example": "reports/" },
        "max_keys":           { "type": "integer", "description": "Maximum number of blobs to return (default: 5000)", "example": 100 },
        "continuation_token": { "type": "string", "description": "Marker for paginating through results" }
    }
}"#;

const STORAGE_LIST_FILES_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":                  { "type": "boolean" },
        "files":                    { "type": "array", "items": {}, "description": "List of blob objects with key, size, last_modified, etag" },
        "count":                    { "type": "integer", "description": "Number of blobs returned" },
        "next_continuation_token":  { "type": "string", "description": "Marker for fetching the next page" },
        "error":                    { "type": "string" }
    }
}"#;

const STORAGE_GET_FILE_INFO_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["bucket", "key"],
    "properties": {
        "bucket": { "type": "string", "example": "uploads" },
        "key":    { "type": "string", "example": "reports/2026-05-16.csv" }
    }
}"#;

const STORAGE_GET_FILE_INFO_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":       { "type": "boolean" },
        "content_type":  { "type": "string" },
        "size":          { "type": "integer", "description": "Blob size in bytes" },
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
        "destination_key":    { "type": "string", "example": "2026/05/file.csv" }
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
        "expires_in_seconds": { "type": "integer", "description": "Lifetime of the SAS URL in seconds (max 604800 = 7 days, default 3600)", "default": 3600, "example": 3600 },
        "content_type":       { "type": "string", "description": "For upload URLs, the MIME type the caller will use (e.g., text/csv)", "example": "text/csv" }
    }
}"#;

const STORAGE_GENERATE_PRESIGNED_URL_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "success":            { "type": "boolean" },
        "url":                { "type": "string", "description": "Time-limited SAS URL" },
        "expires_in_seconds": { "type": "integer", "description": "Actual lifetime of the URL after server-side clamping" },
        "error":              { "type": "string" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
