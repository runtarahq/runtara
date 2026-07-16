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
pub struct FileData {
    #[field(display_name = "Content", description = "Base64-encoded file content")]
    pub content: String,
    #[field(
        display_name = "Filename",
        description = "Optional filename (e.g. \"report.zip\")"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    // Wire name is `mimeType` to match the host's shared `runtara_agents::types::FileData`
    // (which renames this one field). Every other field is snake_case, matching the
    // capability metadata the validator authors against.
    #[field(
        display_name = "MIME Type",
        description = "Optional content-type (e.g. \"application/zip\")"
    )]
    #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
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
    let body_text = String::from_utf8_lossy(&response.body).to_string();
    if !(200..300).contains(&status) {
        return Err(AgentError::permanent(
            format!("NATIVE_AGENT_HTTP_{status}"),
            format!("native agent compression/{capability_id} returned {status}: {body_text}"),
        ));
    }

    parse_native_envelope(capability_id, &body_text)
}

/// Unwrap the `{ success, output | error }` envelope the internal native-agent
/// endpoint wraps every response in (see `internal_agents::run_agent`).
///
/// On `success: true` we return the inner `output` payload — the caller then
/// deserializes THAT into the typed output struct. Returning the whole envelope
/// (the previous behaviour) made every capability fail with a misleading
/// `missing field 'content'`, because the envelope has no `content` key — only
/// `success` and `output`. The sibling sftp wrapper already unwraps identically.
///
/// On `success: false` the `error` field is a JSON-encoded `AgentError`; we
/// parse it back so the workflow sees the real native error code (e.g.
/// `ARCHIVE_FILE_NOT_FOUND`) and retry category instead of an opaque wrapper.
///
/// Pure (no I/O) so the envelope contract is unit-tested without a live host.
fn parse_native_envelope(capability_id: &str, body_text: &str) -> Result<Value, AgentError> {
    let envelope: Value = serde_json::from_str(body_text).map_err(|e| {
        AgentError::permanent(
            "COMPRESSION_OUTPUT_PARSE_ERROR",
            format!(
                "invalid JSON envelope from native compression/{capability_id}: {e}: {body_text}"
            ),
        )
    })?;

    if envelope
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        Ok(envelope.get("output").cloned().unwrap_or(Value::Null))
    } else {
        Err(native_error_to_agent_error(envelope.get("error")))
    }
}

/// Reconstruct an `AgentError` from the envelope's `error` field. The native
/// side serializes a full `AgentError` to a JSON string, so we parse it to
/// preserve `code`/`message`/`category`; anything unparseable falls back to a
/// generic permanent error carrying the raw text.
fn native_error_to_agent_error(error: Option<&Value>) -> AgentError {
    let raw = match error {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "unknown native agent error".to_string(),
    };

    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&raw) {
        let code = map
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("COMPRESSION_NATIVE_AGENT_ERROR")
            .to_string();
        let message = map
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or(&raw)
            .to_string();
        if map.get("category").and_then(|v| v.as_str()) == Some("transient") {
            AgentError::transient(code, message)
        } else {
            AgentError::permanent(code, message)
        }
    } else {
        AgentError::permanent("COMPRESSION_NATIVE_AGENT_ERROR", raw)
    }
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
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
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
            capability_to_api_with_types(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
                &output_types,
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
use bindings::exports::runtara::agent_compression::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

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

// ============================================================================
// Tests (host-only; exercise the pure envelope + wire-contract logic)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Bug #1: the native envelope must be unwrapped ---------------------

    #[test]
    fn success_envelope_unwraps_to_output() {
        // The internal endpoint wraps output in `{ success, output }`. We must
        // return the inner `output` so the caller can deserialize FileData —
        // returning the whole envelope caused the reported `missing field
        // 'content'` (the envelope has no `content`, only `success`/`output`).
        let body = json!({
            "success": true,
            "output": { "content": "d29ybGQ=", "filename": "hello.txt", "mimeType": "text/plain" }
        })
        .to_string();

        let output = parse_native_envelope("extract-file", &body).expect("envelope unwraps");
        // The unwrapped output deserializes cleanly into the typed FileData.
        let file: FileData = deserialize_output("extract-file", output).expect("deserializes");
        assert_eq!(file.content, "d29ybGQ=");
        assert_eq!(file.filename.as_deref(), Some("hello.txt"));
        assert_eq!(file.mime_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn failure_envelope_preserves_native_error_code() {
        // `error` is a JSON-encoded AgentError; the real code/category must
        // survive so workflow onError branches can switch on it.
        let inner = json!({
            "code": "ARCHIVE_FILE_NOT_FOUND",
            "message": "File 'x' not found in archive",
            "category": "permanent",
            "severity": "error"
        })
        .to_string();
        let body = json!({ "success": false, "error": inner }).to_string();

        let err = parse_native_envelope("extract-file", &body).expect_err("surfaces error");
        assert_eq!(err.code, "ARCHIVE_FILE_NOT_FOUND");
        assert_eq!(err.category, "permanent");
        assert!(err.message.contains("not found"));
    }

    #[test]
    fn malformed_envelope_is_a_parse_error_not_missing_content() {
        let err = parse_native_envelope("extract-file", "not json").expect_err("parse fails");
        assert_eq!(err.code, "COMPRESSION_OUTPUT_PARSE_ERROR");
    }

    #[test]
    fn plain_string_error_falls_back_gracefully() {
        let body = json!({ "success": false, "error": "boom" }).to_string();
        let err = parse_native_envelope("list-archive", &body).expect_err("surfaces error");
        assert_eq!(err.code, "COMPRESSION_NATIVE_AGENT_ERROR");
        assert_eq!(err.message, "boom");
    }

    // ---- Bug #2: input field names are snake_case on the wire --------------

    #[test]
    fn extract_file_input_deserializes_snake_case_file_path() {
        // The validator/metadata advertise `file_path`; the runtime must accept
        // exactly that (previously `rename_all = camelCase` demanded `filePath`).
        let input: ExtractFileInput = serde_json::from_value(json!({
            "archive": "UEsDBAoAAAAAAA==",
            "file_path": "data/report.csv"
        }))
        .expect("snake_case input deserializes");
        assert_eq!(input.file_path, "data/report.csv");
    }

    #[test]
    fn extract_file_input_rejects_camel_case_file_path() {
        // Guard against re-introducing the camelCase wire contract.
        let err = serde_json::from_value::<ExtractFileInput>(json!({
            "archive": "UEsDBAoAAAAAAA==",
            "filePath": "data/report.csv"
        }))
        .expect_err("camelCase must NOT satisfy the required field");
        assert!(err.to_string().contains("file_path"));
    }

    #[test]
    fn create_archive_input_uses_snake_case_multiword_fields() {
        let input: CreateArchiveInput = serde_json::from_value(json!({
            "files": [{ "file": "aGk=", "path": "a.txt" }],
            "compression_level": 9,
            "archive_name": "out.zip"
        }))
        .expect("snake_case multi-word fields deserialize");
        assert_eq!(input.compression_level, 9);
        assert_eq!(input.archive_name.as_deref(), Some("out.zip"));
    }

    // ---- FileData output stays wire-compatible with the host FileData ------

    #[test]
    fn file_data_serializes_mime_type_as_mimetype() {
        let file = FileData {
            content: "aGk=".into(),
            filename: Some("a.txt".into()),
            mime_type: Some("text/plain".into()),
        };
        let v = serde_json::to_value(&file).unwrap();
        assert_eq!(
            v.get("mimeType").and_then(|x| x.as_str()),
            Some("text/plain")
        );
        assert!(
            v.get("mime_type").is_none(),
            "must not emit snake_case mime_type"
        );
        assert!(v.get("content").is_some());
    }
}
