//! S3-compatible storage agent — WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_s3_storage.meta.json` next
//! to the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to resolve the connection,
//! attach AWS SigV4 signing, and forward to the configured S3 endpoint. The
//! component never sees AWS credentials and never signs requests itself.
//!
//! Binary content (upload/download) flows over the wire as base64 inside the
//! JSON capability input/output. The component decodes/encodes base64 itself;
//! the raw bytes only exist as the body of the proxied PUT/GET. Default upload
//! cap is 50 MB (matches the legacy `s3_storage` agent).
#![allow(clippy::result_large_err)]

use base64::Engine as _;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// ============================================================================
// Local AgentError shim
// ============================================================================
//
// The host crate's `runtara_agents::types::AgentError` pulls in `tracing` and
// other host-only baggage. We only need the on-the-wire JSON shape that the
// `#[capability]` macro expects (`Into<String>` returning
// `{"code","message","category","severity",...}`), so we inline a minimal
// version here. Mirrors the shim in `runtara-agent-mailgun` / `-hubspot`.

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, Value>,
}

impl AgentError {
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "permanent",
            severity: "error",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "transient",
            severity: "warning",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }

    #[allow(dead_code)]
    pub fn with_retry_after_ms(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }
}

impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConnection {
    #[serde(default)]
    pub connection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    pub integration_id: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_config: Option<Value>,
}

// ============================================================================
// Shared S3 helpers
// ============================================================================

/// Default request timeout — matches the existing wasm crate.
const S3_TIMEOUT: Duration = Duration::from_secs(60);

/// Size limit for uploads: 50 MB (matches the legacy `s3_storage` constant).
const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;

/// Default lifetime for presigned URLs (1 hour).
const DEFAULT_PRESIGN_EXPIRES_SECONDS: u64 = 3600;

fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "S3_MISSING_CONNECTION",
            "No S3 connection configured. Add an s3_compatible connection to this step.",
        )
        .with_attr("integration", "s3_compatible")
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
) -> Result<runtara_http::HttpResponse, AgentError> {
    let client = runtara_http::HttpClient::with_timeout(S3_TIMEOUT);
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
        AgentError::transient(
            "S3_NETWORK_ERROR",
            format!("S3 request {method} {path} failed: {e}"),
        )
        .with_attr("integration", "s3_compatible")
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3BucketEntry {
    pub name: String,
    pub creation_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3ObjectEntry {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
    pub etag: String,
}

fn parse_list_buckets_xml(xml: &str) -> Vec<S3BucketEntry> {
    let mut buckets = Vec::new();
    for block in xml.split("<Bucket>").skip(1) {
        let name = extract_xml_tag(block, "Name").unwrap_or_default();
        let creation_date = extract_xml_tag(block, "CreationDate").unwrap_or_default();
        if !name.is_empty() {
            buckets.push(S3BucketEntry {
                name,
                creation_date,
            });
        }
    }
    buckets
}

fn parse_list_objects_xml(xml: &str) -> (Vec<S3ObjectEntry>, Option<String>) {
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
            objects.push(S3ObjectEntry {
                key,
                size,
                last_modified,
                etag,
            });
        }
    }
    (objects, next_token)
}

// ============================================================================
// Capability 1: Create Bucket
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Bucket Input")]
pub struct CreateBucketInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Bucket",
        description = "Name of the bucket to create",
        example = "uploads"
    )]
    pub bucket: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Bucket Output")]
pub struct CreateBucketOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-create-bucket",
    module = "s3-storage",
    display_name = "Create Bucket",
    description = "Create a new bucket in S3-compatible storage",
    side_effects = true,
    idempotent = false,
    module_display_name = "S3 Storage",
    module_description = "S3-compatible object storage: bucket and file management, plus presigned URL generation. Credentials are injected server-side by the runtara HTTP proxy.",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "s3_compatible",
    module_secure = true
)]
pub fn storage_create_bucket(input: CreateBucketInput) -> Result<CreateBucketOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let path = bucket_path(&input.bucket);
    let resp = s3_request("PUT", &path, &connection.connection_id, &[], None)?;

    Ok(match resp.status {
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
    })
}

// ============================================================================
// Capability 2: List Buckets
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Buckets Input")]
pub struct ListBucketsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Buckets Output")]
pub struct ListBucketsOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Buckets",
        description = "List of bucket entries with name and creation date"
    )]
    pub buckets: Vec<S3BucketEntry>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-list-buckets",
    module = "s3-storage",
    display_name = "List Buckets",
    description = "List all buckets in S3-compatible storage",
    side_effects = false,
    idempotent = true
)]
pub fn storage_list_buckets(input: ListBucketsInput) -> Result<ListBucketsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let resp = s3_request("GET", "/", &connection.connection_id, &[], None)?;

    Ok(if resp.status == 200 {
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
    })
}

// ============================================================================
// Capability 3: Delete Bucket
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Bucket Input")]
pub struct DeleteBucketInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Bucket",
        description = "Name of the bucket to delete (must be empty)",
        example = "old-exports"
    )]
    pub bucket: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Bucket Output")]
pub struct DeleteBucketOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-delete-bucket",
    module = "s3-storage",
    display_name = "Delete Bucket",
    description = "Delete an empty bucket from S3-compatible storage",
    side_effects = true,
    idempotent = true
)]
pub fn storage_delete_bucket(input: DeleteBucketInput) -> Result<DeleteBucketOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let path = bucket_path(&input.bucket);
    let resp = s3_request("DELETE", &path, &connection.connection_id, &[], None)?;

    Ok(match resp.status {
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
    })
}

// ============================================================================
// Capability 4: Upload File
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload File Input")]
pub struct UploadFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
        description = "File content as base64-encoded string (default) or plain text. Max 50 MB after decode.",
        example = "SGVsbG8gV29ybGQ="
    )]
    pub content: String,

    #[field(
        display_name = "Content Type",
        description = "MIME type of the file (e.g., text/csv, image/png)",
        example = "text/csv"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(
        display_name = "Is Base64",
        description = "Whether content is base64-encoded (default: true)",
        default = "true"
    )]
    #[serde(default = "default_true_opt")]
    pub is_base64: Option<bool>,
}

fn default_true_opt() -> Option<bool> {
    Some(true)
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Upload File Output")]
pub struct UploadFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Key", description = "The key of the uploaded file")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    #[field(
        display_name = "Size",
        description = "Size in bytes of the uploaded file"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-upload-file",
    module = "s3-storage",
    display_name = "Upload File",
    description = "Upload a file to S3-compatible storage. Content can be base64-encoded binary or plain text.",
    side_effects = true,
    idempotent = false
)]
pub fn storage_upload_file(input: UploadFileInput) -> Result<UploadFileOutput, AgentError> {
    let connection = require_connection(&input._connection)?;

    let is_base64 = input.is_base64.unwrap_or(true);
    let data = if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(&input.content)
            .map_err(|e| {
                AgentError::permanent("S3_INVALID_CONTENT", format!("Invalid base64: {}", e))
                    .with_attr("integration", "s3_compatible")
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
    let ct = input
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    let path = object_path(&input.bucket, &input.key);

    let resp = s3_request(
        "PUT",
        &path,
        &connection.connection_id,
        &[("Content-Type", ct)],
        Some(&data),
    )?;

    Ok(match resp.status {
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
    })
}

// ============================================================================
// Capability 5: Download File
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Download File Input")]
pub struct DownloadFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Download File Output")]
pub struct DownloadFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Content",
        description = "File content (base64 by default, or UTF-8 text when as_text is true)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[field(display_name = "Content Type", description = "MIME type of the file")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "Size in bytes")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-download-file",
    module = "s3-storage",
    display_name = "Download File",
    description = "Download a file from S3-compatible storage. Returns base64-encoded content by default, or UTF-8 text if as_text is true.",
    side_effects = false,
    idempotent = true
)]
pub fn storage_download_file(input: DownloadFileInput) -> Result<DownloadFileOutput, AgentError> {
    let connection = require_connection(&input._connection)?;

    // HEAD first to pull content_type without re-streaming the body. Mirrors
    // the legacy behaviour; failure of HEAD doesn't abort the GET.
    let head_path = object_path(&input.bucket, &input.key);
    let content_type = s3_request("HEAD", &head_path, &connection.connection_id, &[], None)
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
    let resp = s3_request("GET", &get_path, &connection.connection_id, &[], None)?;

    Ok(if resp.status == 200 {
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
    })
}

// ============================================================================
// Capability 6: List Files
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Files Input")]
pub struct ListFilesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    #[field(
        display_name = "Max Keys",
        description = "Maximum number of files to return (default: 1000)",
        example = "100"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_keys: Option<u32>,

    #[field(
        display_name = "Continuation Token",
        description = "Token for paginating through results"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Files Output")]
pub struct ListFilesOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Files",
        description = "List of file objects with key, size, last_modified, etag"
    )]
    pub files: Vec<S3ObjectEntry>,

    #[field(display_name = "Count", description = "Number of files returned")]
    pub count: u32,

    #[field(
        display_name = "Next Continuation Token",
        description = "Token for fetching the next page"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_continuation_token: Option<String>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-list-files",
    module = "s3-storage",
    display_name = "List Files",
    description = "List files in a bucket with optional prefix filter and pagination",
    side_effects = false,
    idempotent = true
)]
pub fn storage_list_files(input: ListFilesInput) -> Result<ListFilesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;

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
    let resp = s3_request("GET", &path, &connection.connection_id, &[], None)?;

    Ok(if resp.status == 200 {
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
    })
}

// ============================================================================
// Capability 7: Get File Info
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get File Info Input")]
pub struct GetFileInfoInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Bucket", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/2026-03-22.csv")]
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get File Info Output")]
pub struct GetFileInfoOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Content Type")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "File size in bytes")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,

    #[field(display_name = "ETag")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,

    #[field(display_name = "Last Modified")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-get-file-info",
    module = "s3-storage",
    display_name = "Get File Info",
    description = "Get metadata about a file without downloading it (content type, size, last modified)",
    side_effects = false,
    idempotent = true
)]
pub fn storage_get_file_info(input: GetFileInfoInput) -> Result<GetFileInfoOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let path = object_path(&input.bucket, &input.key);
    let resp = s3_request("HEAD", &path, &connection.connection_id, &[], None)?;

    Ok(if resp.status == 200 {
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
    })
}

// ============================================================================
// Capability 8: Delete File
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete File Input")]
pub struct DeleteFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Bucket", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/old-report.csv")]
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete File Output")]
pub struct DeleteFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-delete-file",
    module = "s3-storage",
    display_name = "Delete File",
    description = "Delete a file from S3-compatible storage",
    side_effects = true,
    idempotent = true
)]
pub fn storage_delete_file(input: DeleteFileInput) -> Result<DeleteFileOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let path = object_path(&input.bucket, &input.key);
    let resp = s3_request("DELETE", &path, &connection.connection_id, &[], None)?;

    Ok(match resp.status {
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
    })
}

// ============================================================================
// Capability 9: Copy File
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Copy File Input")]
pub struct CopyFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Copy File Output")]
pub struct CopyFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-copy-file",
    module = "s3-storage",
    display_name = "Copy File",
    description = "Copy a file within or across buckets in S3-compatible storage",
    side_effects = true,
    idempotent = true
)]
pub fn storage_copy_file(input: CopyFileInput) -> Result<CopyFileOutput, AgentError> {
    let connection = require_connection(&input._connection)?;

    let dst_path = object_path(&input.destination_bucket, &input.destination_key);
    let copy_source = format!("/{}/{}", input.source_bucket, input.source_key);
    let resp = s3_request(
        "PUT",
        &dst_path,
        &connection.connection_id,
        &[("x-amz-copy-source", &copy_source)],
        None,
    )?;

    Ok(match resp.status {
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
    })
}

// ============================================================================
// Capability 10: Generate Presigned URL
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Generate Presigned URL Input")]
pub struct GeneratePresignedUrlInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Bucket", example = "uploads")]
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
        display_name = "Expires In Seconds",
        description = "Lifetime of the signed URL in seconds (max 604800 = 7 days, default 3600)",
        default = "3600",
        example = "3600"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,

    #[field(
        display_name = "Content Type",
        description = "For upload URLs, the MIME type the caller will use (e.g., text/csv)",
        example = "text/csv"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Generate Presigned URL Output")]
pub struct GeneratePresignedUrlOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "URL", description = "Time-limited presigned URL")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    #[field(
        display_name = "Expires In Seconds",
        description = "Actual lifetime of the URL after server-side clamping"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "storage-generate-presigned-url",
    module = "s3-storage",
    display_name = "Generate Presigned URL",
    description = "Generate a time-limited presigned URL for downloading, uploading, or deleting an object. Callers consume the URL directly without going through runtara.",
    side_effects = false,
    idempotent = true
)]
pub fn storage_generate_presigned_url(
    input: GeneratePresignedUrlInput,
) -> Result<GeneratePresignedUrlOutput, AgentError> {
    let connection = require_connection(&input._connection)?;

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

    Ok(
        match runtara_http::presign(
            &connection.connection_id,
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
        },
    )
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_STORAGE_CREATE_BUCKET,
        &__CAPABILITY_META_STORAGE_LIST_BUCKETS,
        &__CAPABILITY_META_STORAGE_DELETE_BUCKET,
        &__CAPABILITY_META_STORAGE_UPLOAD_FILE,
        &__CAPABILITY_META_STORAGE_DOWNLOAD_FILE,
        &__CAPABILITY_META_STORAGE_LIST_FILES,
        &__CAPABILITY_META_STORAGE_GET_FILE_INFO,
        &__CAPABILITY_META_STORAGE_DELETE_FILE,
        &__CAPABILITY_META_STORAGE_COPY_FILE,
        &__CAPABILITY_META_STORAGE_GENERATE_PRESIGNED_URL,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "CreateBucketInput",
            &__INPUT_META_CreateBucketInput as &InputTypeMeta,
        ),
        (
            "ListBucketsInput",
            &__INPUT_META_ListBucketsInput as &InputTypeMeta,
        ),
        (
            "DeleteBucketInput",
            &__INPUT_META_DeleteBucketInput as &InputTypeMeta,
        ),
        (
            "UploadFileInput",
            &__INPUT_META_UploadFileInput as &InputTypeMeta,
        ),
        (
            "DownloadFileInput",
            &__INPUT_META_DownloadFileInput as &InputTypeMeta,
        ),
        (
            "ListFilesInput",
            &__INPUT_META_ListFilesInput as &InputTypeMeta,
        ),
        (
            "GetFileInfoInput",
            &__INPUT_META_GetFileInfoInput as &InputTypeMeta,
        ),
        (
            "DeleteFileInput",
            &__INPUT_META_DeleteFileInput as &InputTypeMeta,
        ),
        (
            "CopyFileInput",
            &__INPUT_META_CopyFileInput as &InputTypeMeta,
        ),
        (
            "GeneratePresignedUrlInput",
            &__INPUT_META_GeneratePresignedUrlInput as &InputTypeMeta,
        ),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "CreateBucketOutput",
            &__OUTPUT_META_CreateBucketOutput as &OutputTypeMeta,
        ),
        (
            "ListBucketsOutput",
            &__OUTPUT_META_ListBucketsOutput as &OutputTypeMeta,
        ),
        (
            "DeleteBucketOutput",
            &__OUTPUT_META_DeleteBucketOutput as &OutputTypeMeta,
        ),
        (
            "UploadFileOutput",
            &__OUTPUT_META_UploadFileOutput as &OutputTypeMeta,
        ),
        (
            "DownloadFileOutput",
            &__OUTPUT_META_DownloadFileOutput as &OutputTypeMeta,
        ),
        (
            "ListFilesOutput",
            &__OUTPUT_META_ListFilesOutput as &OutputTypeMeta,
        ),
        (
            "GetFileInfoOutput",
            &__OUTPUT_META_GetFileInfoOutput as &OutputTypeMeta,
        ),
        (
            "DeleteFileOutput",
            &__OUTPUT_META_DeleteFileOutput as &OutputTypeMeta,
        ),
        (
            "CopyFileOutput",
            &__OUTPUT_META_CopyFileOutput as &OutputTypeMeta,
        ),
        (
            "GeneratePresignedUrlOutput",
            &__OUTPUT_META_GeneratePresignedUrlOutput as &OutputTypeMeta,
        ),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api_with_types(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
                &output_types,
            )
        })
        .collect();

    AgentInfo {
        id: "s3-storage".into(),
        name: "S3 Storage".into(),
        description: "S3-compatible object storage: bucket and file management, plus presigned URL generation. Credentials are injected server-side by the runtara HTTP proxy.".into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["s3_compatible".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_s3_storage::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "storage-create-bucket" => __executor_storage_create_bucket(value),
            "storage-list-buckets" => __executor_storage_list_buckets(value),
            "storage-delete-bucket" => __executor_storage_delete_bucket(value),
            "storage-upload-file" => __executor_storage_upload_file(value),
            "storage-download-file" => __executor_storage_download_file(value),
            "storage-list-files" => __executor_storage_list_files(value),
            "storage-get-file-info" => __executor_storage_get_file_info(value),
            "storage-delete-file" => __executor_storage_delete_file(value),
            "storage-copy-file" => __executor_storage_copy_file(value),
            "storage-generate-presigned-url" => __executor_storage_generate_presigned_url(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("s3-storage agent has no capability `{other}`"),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                });
            }
        };
        executor_result
            .map_err(error_string_to_error_info)
            .and_then(|out_value| serde_json::to_vec(&out_value).map_err(bad_json))
    }
}

#[cfg(target_arch = "wasm32")]
fn bad_json(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

/// The `#[capability]` macro packages each error as a JSON-string with
/// `{ code, message, category, severity, ... }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        let category = value
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("permanent")
            .to_string();
        let retryable = value
            .get("retryable")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| category == "transient");
        ErrorInfo {
            code: value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAPABILITY_ERROR")
                .into(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&s)
                .into(),
            category,
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable,
            retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
            attributes: value.get("attributes").map(|v| v.to_string()),
        }
    } else {
        ErrorInfo {
            code: "CAPABILITY_ERROR".into(),
            message: s,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);
