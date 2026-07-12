//! Azure Blob Storage integration agent — WebAssembly Component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_azure_blob_storage.meta.json`
//! next to the `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to resolve the storage
//! account, compute Azure Shared Key HMAC signatures, and forward to
//! `https://{account}.blob.core.windows.net`. The component never sees storage
//! account keys and performs no signing.
//!
//! Presigned SAS URLs are obtained via `runtara_http::presign(...)` — same
//! mechanism as the s3_storage agent, fully server-side.
//!
//! The capability surface mirrors the s3_storage agent so workflows can be
//! ported between providers with minimal rewiring. "bucket" maps to a container
//! and "key" maps to a blob name.
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
// version here. Mirrors the shim in `runtara-agent-mailgun`.

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

    pub fn with_retry_after_ms(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }
}

/// Serialize into the canonical JSON envelope so the `#[capability]` macro
/// executor passes us straight through to `error_string_to_error_info` on the
/// wasm side (which parses the JSON back into a typed `ErrorInfo`).
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================
//
// The host crate's `RawConnection` lives in `runtara-agents` and isn't a
// wasm-compatible dependency. We mirror just the struct so the macro-derived
// executor can deserialize what the wasm Guest::invoke wrapper injects into
// the input JSON under the `_connection` key.

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
// Shared helpers
// ============================================================================

/// Size limit for single-shot uploads: 50 MB (matches legacy constant).
const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;

/// Default SAS lifetime for the presigned URL capability.
const DEFAULT_PRESIGN_EXPIRES_SECONDS: u64 = 3600;

fn require_connection(conn: &Option<RawConnection>) -> Result<&RawConnection, AgentError> {
    conn.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "AZURE_BLOB_MISSING_CONNECTION",
            "No Azure Blob Storage connection configured. Add an azure_blob_storage connection to this step.",
        )
        .with_attr("integration", "azure_blob_storage")
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
) -> Result<runtara_http::HttpResponse, AgentError> {
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
        AgentError::transient(
            "AZURE_BLOB_NETWORK_ERROR",
            format!("Azure Blob request {method} {path} failed: {e}"),
        )
        .with_attr("integration", "azure_blob_storage")
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

/// One Azure container in the `storage_list_buckets` response.
///
/// Field names mirror the legacy `runtara-agents::agents::integrations::
/// azure_blob_client::ContainerInfo` and the sibling
/// `runtara-agent-s3-storage::S3BucketEntry` (typed capability outputs,
/// not opaque `Value`) for consistency across the storage agent family.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub name: String,
    pub last_modified: String,
}

/// One Azure blob in the `storage_list_files` response.
///
/// `key` (not `name`) mirrors the s3 capability surface so workflows
/// can swap between s3 and azure-blob without remapping field names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobInfo {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
    pub etag: String,
}

/// Parse the Azure `?comp=list` XML response for listing containers.
fn parse_list_containers_xml(xml: &str) -> Vec<ContainerInfo> {
    let mut containers = Vec::new();
    for block in xml.split("<Container>").skip(1) {
        let name = extract_xml_tag(block, "Name").unwrap_or_default();
        let last_modified = extract_xml_tag(block, "Last-Modified").unwrap_or_default();
        if !name.is_empty() {
            containers.push(ContainerInfo {
                name,
                last_modified,
            });
        }
    }
    containers
}

/// Parse the Azure `?restype=container&comp=list` XML response for listing blobs.
/// Returns `(blobs, next_marker)`.
fn parse_list_blobs_xml(xml: &str) -> (Vec<BlobInfo>, Option<String>) {
    // Azure uses <NextMarker> (empty tag = no more pages).
    let next_marker = extract_xml_tag(xml, "NextMarker").filter(|s| !s.is_empty());
    let mut objects = Vec::new();
    for block in xml.split("<Blob>").skip(1) {
        let key = extract_xml_tag(block, "Name").unwrap_or_default();
        let size: u64 = extract_xml_tag(block, "Content-Length")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let last_modified = extract_xml_tag(block, "Last-Modified").unwrap_or_default();
        // Azure's REST docs use `<Etag>` but historical responses sometimes
        // surface `<ETag>` (capitalized "T"). Try both so the parser is
        // robust to either form — same fallback the legacy crate had.
        let etag = extract_xml_tag(block, "Etag")
            .or_else(|| extract_xml_tag(block, "ETag"))
            .unwrap_or_default();
        if !key.is_empty() {
            objects.push(BlobInfo {
                key,
                size,
                last_modified,
                etag,
            });
        }
    }
    (objects, next_marker)
}

fn default_true() -> Option<bool> {
    Some(true)
}

// ============================================================================
// Create Container (storage_create_bucket)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Container Input")]
pub struct CreateBucketInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (the UI/runtime fills it from the
    /// configured connection, not from user input).
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Container Name",
        description = "Name of the container to create",
        example = "uploads"
    )]
    pub bucket: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Create Container Output")]
pub struct CreateBucketOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    module = "azure_blob_storage",
    display_name = "Create Container",
    description = "Create a new container in Azure Blob Storage",
    module_display_name = "Azure Blob Storage",
    module_description = "Azure Blob Storage: container and blob management, plus SAS URL generation. Credentials are injected server-side by the runtara HTTP proxy — the component never handles Shared Key material.",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "azure_blob_storage",
    module_secure = true,
    side_effects = true
)]
pub fn storage_create_bucket(input: CreateBucketInput) -> Result<CreateBucketOutput, AgentError> {
    let conn = require_connection(&input._connection)?;
    let path = container_path(&input.bucket);
    let resp = azure_request("PUT", &path, &conn.connection_id, &[], None)?;

    Ok(match resp.status {
        // 409 (container already exists) is treated as success — matches the
        // legacy host agent's behaviour for idempotent retries.
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
    })
}

// ============================================================================
// List Containers (storage_list_buckets)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Containers Input")]
pub struct ListBucketsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Containers Output")]
pub struct ListBucketsOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Containers",
        description = "List of container names and last-modified timestamps"
    )]
    pub buckets: Vec<ContainerInfo>,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    // GET /?comp=list — lists all containers in the storage account.
    let resp = azure_request("GET", "/?comp=list", &conn.connection_id, &[], None)?;

    Ok(if resp.status == 200 {
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
    })
}

// ============================================================================
// Delete Container (storage_delete_bucket)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Container Input")]
pub struct DeleteBucketInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Container Name",
        description = "Name of the container to delete (must be empty)",
        example = "old-exports"
    )]
    pub bucket: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Container Output")]
pub struct DeleteBucketOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    let path = container_path(&input.bucket);
    let resp = azure_request("DELETE", &path, &conn.connection_id, &[], None)?;

    Ok(match resp.status {
        // 404 (already gone) is treated as success — matches legacy idempotent semantics.
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
    })
}

// ============================================================================
// Upload Blob (storage_upload_file)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Upload Blob Input")]
pub struct UploadFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(
        display_name = "Is Base64",
        description = "Whether content is base64-encoded (default: true)",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub is_base64: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Upload Blob Output")]
pub struct UploadFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Key", description = "Blob name of the uploaded blob")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    #[field(
        display_name = "Size",
        description = "Size in bytes of the uploaded blob"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

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

    let is_base64 = input.is_base64.unwrap_or(true);
    let data = if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(&input.content)
            .map_err(|e| {
                AgentError::permanent(
                    "AZURE_BLOB_INVALID_CONTENT",
                    format!("Invalid base64: {}", e),
                )
                .with_attr("integration", "azure_blob_storage")
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
    let ct = input
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    let path = blob_path(&input.bucket, &input.key);

    // Azure requires x-ms-blob-type: BlockBlob for single-shot PutBlob.
    let resp = azure_request(
        "PUT",
        &path,
        &conn.connection_id,
        &[("Content-Type", ct), ("x-ms-blob-type", "BlockBlob")],
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
                error: Some(format!("PutBlob failed: {}", parse_azure_error(&body))),
            }
        }
    })
}

// ============================================================================
// Download Blob (storage_download_file)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Download Blob Input")]
pub struct DownloadFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_text: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Download Blob Output")]
pub struct DownloadFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Content",
        description = "Blob content (base64 or text)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[field(display_name = "Content Type", description = "MIME type of the blob")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "Size in bytes")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
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

    // HEAD first to grab content-type without paying the body cost. Mirrors
    // legacy behaviour; a HEAD failure shouldn't block the GET.
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
            error: Some(format!("GetBlob failed: {}", parse_azure_error(&body))),
        }
    })
}

// ============================================================================
// List Blobs (storage_list_files)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Blobs Input")]
pub struct ListFilesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    #[field(
        display_name = "Max Keys",
        description = "Maximum number of blobs to return (default: 5000)",
        example = "100"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_keys: Option<u32>,

    #[field(
        display_name = "Continuation Token",
        description = "Marker for paginating through results"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Blobs Output")]
pub struct ListFilesOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Files",
        description = "List of blob objects with key, size, last_modified, etag"
    )]
    pub files: Vec<BlobInfo>,

    #[field(display_name = "Count", description = "Number of blobs returned")]
    pub count: u32,

    #[field(
        display_name = "Next Token",
        description = "Marker for fetching the next page"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_continuation_token: Option<String>,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
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

    Ok(if resp.status == 200 {
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
    })
}

// ============================================================================
// Get Blob Info (storage_get_file_info)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Blob Info Input")]
pub struct GetFileInfoInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Container", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/2026-05-16.csv")]
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Blob Info Output")]
pub struct GetFileInfoOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Content Type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    #[field(display_name = "Size", description = "Blob size in bytes")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,

    #[field(display_name = "ETag")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,

    #[field(display_name = "Last Modified")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    let path = blob_path(&input.bucket, &input.key);
    let resp = azure_request("HEAD", &path, &conn.connection_id, &[], None)?;

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
            error: Some(format!("HeadBlob failed (status {})", resp.status)),
        }
    })
}

// ============================================================================
// Delete Blob (storage_delete_file)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Blob Input")]
pub struct DeleteFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Container", example = "uploads")]
    pub bucket: String,

    #[field(display_name = "Key", example = "reports/old-report.csv")]
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Delete Blob Output")]
pub struct DeleteFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    let path = blob_path(&input.bucket, &input.key);
    let resp = azure_request("DELETE", &path, &conn.connection_id, &[], None)?;

    Ok(match resp.status {
        // 404 (already gone) is treated as success — matches legacy behaviour.
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
    })
}

// ============================================================================
// Copy Blob (storage_copy_file)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Copy Blob Input")]
pub struct CopyFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Copy Blob Output")]
pub struct CopyFileOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
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

    // Azure Copy Blob: PUT on the destination with x-ms-copy-source pointing
    // at the source. The proxy rewrites the absolute URL from the relative
    // source path.
    let dst_path = blob_path(&input.destination_bucket, &input.destination_key);
    let copy_source = format!("/{}/{}", input.source_bucket, input.source_key);

    let resp = azure_request(
        "PUT",
        &dst_path,
        &conn.connection_id,
        &[("x-ms-copy-source", &copy_source)],
        None,
    )?;

    Ok(match resp.status {
        200..=202 => CopyFileOutput {
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
    })
}

// ============================================================================
// Generate Presigned (SAS) URL (storage_generate_presigned_url)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Generate SAS URL Input")]
pub struct GeneratePresignedUrlInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
#[capability_output(display_name = "Generate SAS URL Output")]
pub struct GeneratePresignedUrlOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "URL", description = "Time-limited SAS URL")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    #[field(
        display_name = "Expires In (seconds)",
        description = "Actual lifetime of the URL after server-side clamping"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,

    #[field(display_name = "Error")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

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

    Ok(
        match runtara_http::presign(
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
        ("ListBucketsInput", &__INPUT_META_ListBucketsInput),
        ("DeleteBucketInput", &__INPUT_META_DeleteBucketInput),
        ("UploadFileInput", &__INPUT_META_UploadFileInput),
        ("DownloadFileInput", &__INPUT_META_DownloadFileInput),
        ("ListFilesInput", &__INPUT_META_ListFilesInput),
        ("GetFileInfoInput", &__INPUT_META_GetFileInfoInput),
        ("DeleteFileInput", &__INPUT_META_DeleteFileInput),
        ("CopyFileInput", &__INPUT_META_CopyFileInput),
        (
            "GeneratePresignedUrlInput",
            &__INPUT_META_GeneratePresignedUrlInput,
        ),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "CreateBucketOutput",
            &__OUTPUT_META_CreateBucketOutput as &OutputTypeMeta,
        ),
        ("ListBucketsOutput", &__OUTPUT_META_ListBucketsOutput),
        ("DeleteBucketOutput", &__OUTPUT_META_DeleteBucketOutput),
        ("UploadFileOutput", &__OUTPUT_META_UploadFileOutput),
        ("DownloadFileOutput", &__OUTPUT_META_DownloadFileOutput),
        ("ListFilesOutput", &__OUTPUT_META_ListFilesOutput),
        ("GetFileInfoOutput", &__OUTPUT_META_GetFileInfoOutput),
        ("DeleteFileOutput", &__OUTPUT_META_DeleteFileOutput),
        ("CopyFileOutput", &__OUTPUT_META_CopyFileOutput),
        (
            "GeneratePresignedUrlOutput",
            &__OUTPUT_META_GeneratePresignedUrlOutput,
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
        id: "azure_blob_storage".into(),
        name: "Azure Blob Storage".into(),
        description: "Azure Blob Storage: container and blob management, plus SAS URL generation. \
                      Credentials are injected server-side by the runtara HTTP proxy — the \
                      component never handles Shared Key material."
            .into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["azure_blob_storage".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_azure_blob_storage::capabilities::{
    ConnectionInfo, ErrorInfo, Guest,
};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(
        capability_id: String,
        input: Vec<u8>,
        connection: Option<ConnectionInfo>,
    ) -> Result<Vec<u8>, ErrorInfo> {
        let mut value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        // Inject the WIT `connection` arg into the input JSON under `_connection`
        // so the macro-generated executor can deserialize it into the
        // capability input struct's `_connection: Option<RawConnection>` field.
        if let Some(c) = connection.as_ref() {
            if let serde_json::Value::Object(ref mut obj) = value {
                let parameters = serde_json::from_str::<serde_json::Value>(&c.parameters)
                    .unwrap_or(serde_json::Value::Null);
                let rate_limit_config = c
                    .rate_limit_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
                obj.insert(
                    "_connection".into(),
                    serde_json::json!({
                        "connection_id": c.connection_id,
                        "integration_id": c.integration_id,
                        "connection_subtype": c.connection_subtype,
                        "parameters": parameters,
                        "rate_limit_config": rate_limit_config,
                    }),
                );
            }
        }

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
                    message: format!("azure_blob_storage agent has no capability `{other}`"),
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_containers_extracts_name() {
        let xml = r#"<?xml version="1.0"?>
<EnumerationResults>
  <Containers>
    <Container>
      <Name>uploads</Name>
      <Properties>
        <Last-Modified>Mon, 27 Jul 2026 12:00:00 GMT</Last-Modified>
      </Properties>
    </Container>
    <Container>
      <Name>reports</Name>
      <Properties>
        <Last-Modified>Tue, 28 Jul 2026 10:00:00 GMT</Last-Modified>
      </Properties>
    </Container>
  </Containers>
</EnumerationResults>"#;
        let containers = parse_list_containers_xml(xml);
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].name, "uploads");
        assert_eq!(containers[1].name, "reports");
    }

    #[test]
    fn parse_list_blobs_extracts_size_and_next_marker() {
        let xml = r#"<?xml version="1.0"?>
<EnumerationResults>
  <Blobs>
    <Blob>
      <Name>file.txt</Name>
      <Properties>
        <Content-Length>42</Content-Length>
        <Last-Modified>Mon, 27 Jul 2026 12:00:00 GMT</Last-Modified>
        <Etag>"abc123"</Etag>
      </Properties>
    </Blob>
  </Blobs>
  <NextMarker>page2</NextMarker>
</EnumerationResults>"#;
        let (blobs, marker) = parse_list_blobs_xml(xml);
        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs[0].key, "file.txt");
        assert_eq!(blobs[0].size, 42);
        assert_eq!(marker.as_deref(), Some("page2"));
    }

    #[test]
    fn parse_azure_error_extracts_code_and_message() {
        let body = r#"<?xml version="1.0"?>
<Error><Code>ContainerNotFound</Code><Message>The specified container does not exist.</Message></Error>"#;
        assert_eq!(
            parse_azure_error(body),
            "ContainerNotFound: The specified container does not exist."
        );
    }
}
