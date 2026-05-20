//! Compression agent — WebAssembly component (native-wrapper).
//!
//! Zip/unzip operations rely on native libraries that don't compile to
//! wasm32-wasip2 cleanly, so this component is a thin forwarder: every
//! capability serializes its input to JSON, POSTs it to the host's internal
//! native-agent endpoint at
//! `$RUNTARA_AGENT_SERVICE_URL/<module>/<capability>` (i.e.
//! `http://127.0.0.1:7002/api/internal/agents/compression/<capability_id>` in
//! local dev), then deserializes the response back into the typed output
//! struct. The host runs the actual zip work.
//!
//! Capability metadata still travels through the same `#[capability_input]` /
//! `#[capability]` / `#[capability_output]` annotations used by every other
//! migrated agent — `runtara-agent-bundle-emit` walks the macro-emitted
//! `&'static` statics on the host architecture and writes
//! `runtara_agent_compression.meta.json` next to the `.wasm`.
//!
//! See `docs/wasm-components-migration-plan.md § 5.6` for the wrapper pattern.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use strum::{Display, EnumString};

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
// version here. Mirrors the shim in `runtara-agent-mailgun` /
// `runtara-agent-transform`.

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
}

impl From<serde_json::Error> for AgentError {
    fn from(err: serde_json::Error) -> Self {
        AgentError::permanent("COMPRESSION_JSON_ERROR", err.to_string())
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
// the input JSON under the `_connection` key. Compression itself does not
// require a connection (supports_connections=false), but we keep the shape
// identical to the other migrated agents so the dispatcher contract stays
// uniform — the native side simply ignores `_connection` for this module.

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
// Shared types
// ============================================================================
//
// `FileData` is the same shape used by the host crate's `runtara_agents::types`
// — we mirror it here so the wasm component is self-contained. Content is
// always base64-encoded for transport through the JSON envelope (wasm has no
// filesystem access).

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "File Data",
    description = "Binary file content with optional filename and MIME type, transported as base64."
)]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    #[field(display_name = "Content", description = "Base64-encoded file content")]
    pub content: String,
    #[field(
        display_name = "Filename",
        description = "Optional filename (e.g. \"report.zip\")"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[field(
        display_name = "MIME Type",
        description = "Optional content-type (e.g. \"application/zip\")"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Supported archive formats. Only ZIP today; placeholder for tar/gzip/etc.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Display, EnumString, PartialEq)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ArchiveFormat {
    #[default]
    Zip,
}

/// Flexible input for archive data — accepts a `FileData` object or a raw
/// base64 string. Forwarded as-is to the native handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArchiveDataInput {
    FileData(FileData),
    Base64String(String),
}

/// A file entry to be added to an archive. Matches the legacy
/// `ArchiveFileEntry` shape so the native handler can decode the same JSON.
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Archive File Entry",
    description = "A file to add to an archive with optional path"
)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveFileEntry {
    #[field(
        display_name = "File",
        description = "The file content to add to the archive (FileData or base64 string)"
    )]
    pub file: ArchiveDataInput,

    #[field(
        display_name = "Path",
        description = "Path within the archive (e.g. \"data/report.csv\")"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

// ============================================================================
// create-archive
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Create Archive Input",
    description = "Input for creating an archive from files"
)]
#[serde(rename_all = "camelCase")]
pub struct CreateArchiveInput {
    /// Connection data injected by the wasm Guest::invoke wrapper. Compression
    /// doesn't use a connection, but the field is kept for uniformity with
    /// other migrated agents.
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Files",
        description = "List of files to include in the archive"
    )]
    pub files: Vec<ArchiveFileEntry>,

    #[field(
        display_name = "Format",
        description = "Archive format: 'zip' (default)",
        default = "zip"
    )]
    #[serde(default)]
    pub format: ArchiveFormat,

    #[field(
        display_name = "Compression Level",
        description = "Compression level from 0 (none) to 9 (maximum)",
        default = "6"
    )]
    #[serde(default = "default_compression_level")]
    pub compression_level: u8,

    #[field(
        display_name = "Archive Name",
        description = "Filename for the output archive (e.g. \"data.zip\")"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_name: Option<String>,
}

fn default_compression_level() -> u8 {
    6
}

#[capability(
    module = "compression",
    display_name = "Create Archive",
    description = "Create an archive from one or more files. Runs on the host via the native agent service.",
    module_display_name = "Compression",
    module_description = "ZIP archive create/extract/list operations. Runs on the host via the native agent service.",
    module_has_side_effects = false,
    module_supports_connections = false,
    errors(
        permanent(
            "ARCHIVE_NO_FILES",
            "At least one file is required to create an archive"
        ),
        permanent("ARCHIVE_DECODE_ERROR", "Failed to decode file data"),
        permanent("ARCHIVE_WRITE_ERROR", "Failed to write or finalize archive"),
    )
)]
pub fn create_archive(input: CreateArchiveInput) -> Result<FileData, AgentError> {
    let value = serde_json::to_value(&input)?;
    forward_to_native("create-archive", &value)
        .and_then(|v| deserialize_output("create-archive", v))
}

// ============================================================================
// extract-archive
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Extract Archive Input",
    description = "Input for extracting all files from an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ExtractArchiveInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Archive", description = "The archive file to extract")]
    pub archive: ArchiveDataInput,

    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Extracted File",
    description = "A file extracted from an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedFile {
    #[field(
        display_name = "File",
        description = "The extracted file data (base64-encoded)"
    )]
    pub file: FileData,

    #[field(
        display_name = "Path",
        description = "Original path of the file within the archive"
    )]
    pub path: String,

    #[field(display_name = "Size", description = "Uncompressed file size in bytes")]
    pub size: u64,

    #[field(
        display_name = "Is Directory",
        description = "True if this entry is a directory"
    )]
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Extract Archive Output",
    description = "Result of extracting all files from an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ExtractArchiveOutput {
    #[field(display_name = "Files", description = "List of all extracted files")]
    pub files: Vec<ExtractedFile>,

    #[field(
        display_name = "Count",
        description = "Total number of files extracted"
    )]
    pub count: usize,
}

#[capability(
    module = "compression",
    display_name = "Extract Archive",
    description = "Extract all files from an archive. Runs on the host via the native agent service.",
    errors(
        permanent("ARCHIVE_DECODE_ERROR", "Failed to decode archive data"),
        permanent("ARCHIVE_READ_ERROR", "Failed to read archive or archive entry"),
    )
)]
pub fn extract_archive(input: ExtractArchiveInput) -> Result<ExtractArchiveOutput, AgentError> {
    let value = serde_json::to_value(&input)?;
    forward_to_native("extract-archive", &value)
        .and_then(|v| deserialize_output("extract-archive", v))
}

// ============================================================================
// extract-file
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "Extract File Input",
    description = "Input for extracting a single file from an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ExtractFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Archive",
        description = "The archive file containing the target file"
    )]
    pub archive: ArchiveDataInput,

    #[field(
        display_name = "File Path",
        description = "Path of the file to extract (e.g. \"data/report.csv\")"
    )]
    pub file_path: String,

    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

#[capability(
    module = "compression",
    display_name = "Extract File",
    description = "Extract a single file from an archive by its path. Runs on the host via the native agent service.",
    errors(
        permanent("ARCHIVE_DECODE_ERROR", "Failed to decode archive data"),
        permanent("ARCHIVE_READ_ERROR", "Failed to read archive"),
        permanent("ARCHIVE_FILE_NOT_FOUND", "Specified file not found in archive"),
        permanent("ARCHIVE_IS_DIRECTORY", "Specified path is a directory, not a file"),
    )
)]
pub fn extract_file(input: ExtractFileInput) -> Result<FileData, AgentError> {
    let value = serde_json::to_value(&input)?;
    forward_to_native("extract-file", &value).and_then(|v| deserialize_output("extract-file", v))
}

// ============================================================================
// list-archive
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityInput)]
#[capability_input(
    display_name = "List Archive Input",
    description = "Input for listing archive contents"
)]
#[serde(rename_all = "camelCase")]
pub struct ListArchiveInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Archive",
        description = "The archive file to list contents of"
    )]
    pub archive: ArchiveDataInput,

    #[field(
        display_name = "Format",
        description = "Archive format (auto-detected from content if not specified)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ArchiveFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Archive Entry Info",
    description = "Information about a file in an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEntryInfo {
    #[field(
        display_name = "Path",
        description = "Path of the file within the archive"
    )]
    pub path: String,

    #[field(display_name = "Size", description = "Uncompressed file size in bytes")]
    pub size: u64,

    #[field(
        display_name = "Compressed Size",
        description = "Compressed file size in bytes"
    )]
    pub compressed_size: u64,

    #[field(
        display_name = "Is Directory",
        description = "True if this entry is a directory"
    )]
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "List Archive Output",
    description = "Contents of an archive"
)]
#[serde(rename_all = "camelCase")]
pub struct ListArchiveOutput {
    #[field(
        display_name = "Entries",
        description = "List of files and directories"
    )]
    pub entries: Vec<ArchiveEntryInfo>,

    #[field(display_name = "Total Count", description = "Total number of entries")]
    pub total_count: usize,

    #[field(
        display_name = "Total Size",
        description = "Total uncompressed size in bytes"
    )]
    pub total_size: u64,

    #[field(display_name = "Format", description = "Archive format")]
    pub format: ArchiveFormat,
}

#[capability(
    module = "compression",
    display_name = "List Archive",
    description = "List all files and directories in an archive without extracting. Runs on the host via the native agent service.",
    errors(
        permanent("ARCHIVE_DECODE_ERROR", "Failed to decode archive data"),
        permanent("ARCHIVE_READ_ERROR", "Failed to read archive or archive entry"),
    )
)]
pub fn list_archive(input: ListArchiveInput) -> Result<ListArchiveOutput, AgentError> {
    let value = serde_json::to_value(&input)?;
    forward_to_native("list-archive", &value).and_then(|v| deserialize_output("list-archive", v))
}

// ============================================================================
// Native-stub forwarder
// ============================================================================

/// POST the input JSON to `$RUNTARA_AGENT_SERVICE_URL/compression/<capability_id>`
/// — the host's internal native-agent endpoint owns the actual zip work and
/// returns the JSON-encoded output.
///
/// Matches the request shape of `runtara-agent-sftp` and the original
/// `runtara-agent-compression` wrapper: `Content-Type: application/json`,
/// `X-Org-Id` from `RUNTARA_TENANT_ID`, 120s timeout.
fn forward_to_native(capability_id: &str, input_value: &Value) -> Result<Value, AgentError> {
    let base = std::env::var("RUNTARA_AGENT_SERVICE_URL").map_err(|_| {
        AgentError::permanent(
            "AGENT_SERVICE_URL_MISSING",
            "RUNTARA_AGENT_SERVICE_URL not set; native wrapper cannot forward",
        )
    })?;
    let url = format!("{}/compression/{capability_id}", base.trim_end_matches('/'));

    let body = serde_json::to_vec(input_value)
        .map_err(|e| AgentError::permanent("INPUT_SERIALIZATION_ERROR", e.to_string()))?;

    let tenant_id = std::env::var("RUNTARA_TENANT_ID").unwrap_or_default();

    let client = runtara_http::HttpClient::with_timeout(Duration::from_secs(120));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Org-Id", &tenant_id)
        .body_bytes(&body)
        .call()
        .map_err(|e| {
            AgentError::transient("NETWORK_ERROR", format!("native agent call failed: {e}"))
        })?;

    let status = response.status;
    if !(200..300).contains(&status) {
        let body_text = String::from_utf8_lossy(&response.body).to_string();
        return Err(AgentError::permanent(
            format!("NATIVE_AGENT_HTTP_{status}"),
            format!("native agent compression/{capability_id} returned {status}: {body_text}"),
        ));
    }

    serde_json::from_slice::<Value>(&response.body).map_err(|e| {
        AgentError::permanent(
            "COMPRESSION_OUTPUT_PARSE_ERROR",
            format!("invalid JSON from native handler: {e}"),
        )
    })
}

fn deserialize_output<T: for<'de> Deserialize<'de>>(
    capability_id: &str,
    value: Value,
) -> Result<T, AgentError> {
    serde_json::from_value(value).map_err(|e| {
        AgentError::permanent(
            "COMPRESSION_OUTPUT_DESERIALIZATION_ERROR",
            format!("failed to deserialize {capability_id} output: {e}"),
        )
    })
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
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_CREATE_ARCHIVE,
        &__CAPABILITY_META_EXTRACT_ARCHIVE,
        &__CAPABILITY_META_EXTRACT_FILE,
        &__CAPABILITY_META_LIST_ARCHIVE,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "CreateArchiveInput",
            &__INPUT_META_CreateArchiveInput as &InputTypeMeta,
        ),
        ("ExtractArchiveInput", &__INPUT_META_ExtractArchiveInput),
        ("ExtractFileInput", &__INPUT_META_ExtractFileInput),
        ("ListArchiveInput", &__INPUT_META_ListArchiveInput),
        // ArchiveFileEntry isn't directly used as a capability input, but the
        // macro derive emits its `InputTypeMeta` static (because the type
        // carries `#[derive(CapabilityInput)]` for nested-field metadata). We
        // don't register it in the lookup table since no capability references
        // it as its top-level input type.
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        ("FileData", &__OUTPUT_META_FileData as &OutputTypeMeta),
        ("ExtractedFile", &__OUTPUT_META_ExtractedFile),
        ("ExtractArchiveOutput", &__OUTPUT_META_ExtractArchiveOutput),
        ("ArchiveEntryInfo", &__OUTPUT_META_ArchiveEntryInfo),
        ("ListArchiveOutput", &__OUTPUT_META_ListArchiveOutput),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
            )
        })
        .collect();

    AgentInfo {
        id: "compression".into(),
        name: "Compression".into(),
        description: "ZIP archive create/extract/list operations. Runs on the host via the native agent service.".into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_compression::capabilities::{
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
        // Compression doesn't currently use connections, but the wrapper keeps
        // the same shape as the other migrated agents so the dispatcher
        // contract stays uniform.
        if let Some(c) = connection.as_ref()
            && let serde_json::Value::Object(ref mut obj) = value
        {
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

        let executor_result = match capability_id.as_str() {
            "create-archive" => __executor_create_archive(value),
            "extract-archive" => __executor_extract_archive(value),
            "extract-file" => __executor_extract_file(value),
            "list-archive" => __executor_list_archive(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("compression agent has no capability `{other}`"),
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
