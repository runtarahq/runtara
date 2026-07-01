//! SFTP agent — thin native-wrapper WebAssembly component.
//!
//! SFTP requires libssh2 (a C library) which doesn't compile to wasm32-wasip2,
//! so this component does NOT do real SFTP work. Each capability call is
//! forwarded as a JSON envelope to the host's internal native agent endpoint
//! at `/api/internal/agents/sftp/{capability_id}`. The host owns the real SFTP
//! code (in `runtara-agents::sftp`) and executes it natively.
//!
//! What DOES live in this wasm component:
//! - The macro-derived input / output struct definitions and field metadata.
//! - The `#[capability]`-decorated stub functions that the macro turns into
//!   `&'static CapabilityMeta` items consumed by `runtara-agent-bundle-emit`
//!   to write `runtara_agent_sftp.meta.json` next to the `.wasm`.
//! - A thin forwarder (`forward_to_native`) that POSTs the input JSON to the
//!   internal endpoint and unwraps the `{success, output|error}` envelope.
//!
//! Routing model differs from HTTP-style agents: there is no proxy /
//! `X-Runtara-Connection-Id` hop. The host resolves the connection from the
//! opaque `connection_id` we forward inside `_connection` and runs the
//! capability in-process (see `internal_agents::run_agent`). Credentials never
//! enter this sandbox: we forward only the id — never parameters — and the host
//! overwrites `_connection` with the authoritative resolved values.
#![allow(clippy::result_large_err)]

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

/// Serialize into the canonical JSON envelope so the `#[capability]` macro
/// executor passes us straight through to `error_string_to_error_info` on the
/// wasm side (which parses the JSON back into a typed `ErrorInfo`).
impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

impl From<serde_json::Error> for AgentError {
    fn from(e: serde_json::Error) -> Self {
        AgentError::permanent("SFTP_JSON_ERROR", e.to_string())
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================
//
// Same rationale as in mailgun: `runtara-agents` is host-only, so we mirror
// just the struct shape the macro-derived executor needs for deserializing
// the `_connection` blob injected by Guest::invoke.

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
// Native-stub forwarder
// ============================================================================
//
// Each capability body is a one-liner that hands its serialized input + the
// resolved connection to this function. We POST to the host's internal native
// agent endpoint, which runs `runtara_agents::registry::execute_capability`
// in-process and replies with `{ "success": true, "output": <typed json> }`
// or `{ "success": false, "error": "..." }`. We unwrap the envelope so the
// caller can `serde_json::from_value` straight into the typed output struct.

fn forward_to_native(
    capability_id: &str,
    connection: &Option<RawConnection>,
    input: &Value,
) -> Result<Value, AgentError> {
    let base = std::env::var("RUNTARA_AGENT_SERVICE_URL").map_err(|_| {
        AgentError::permanent(
            "SFTP_AGENT_SERVICE_URL_MISSING",
            "RUNTARA_AGENT_SERVICE_URL not set; native wrapper cannot forward",
        )
    })?;
    let url = format!("{}/sftp/{capability_id}", base.trim_end_matches('/'));

    // The macro-derived executor strips `_connection` from the parsed input
    // before it reaches us. Re-inject it for the native side so the host
    // doesn't need to do a connection-service round trip.
    let mut envelope = input.clone();
    if let Value::Object(ref mut map) = envelope
        && let Some(conn) = connection
    {
        map.insert("_connection".into(), serde_json::to_value(conn)?);
    }

    let body = serde_json::to_vec(&envelope)?;
    let tenant_id = std::env::var("RUNTARA_TENANT_ID").unwrap_or_default();

    let client = runtara_http::HttpClient::with_timeout(Duration::from_secs(120));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Org-Id", &tenant_id)
        .body_bytes(&body)
        .call()
        .map_err(|e| {
            AgentError::transient(
                "SFTP_NATIVE_AGENT_NETWORK_ERROR",
                format!("native agent call failed: {e}"),
            )
        })?;

    let status = response.status;
    let body_text = String::from_utf8_lossy(&response.body).to_string();
    if !(200..300).contains(&status) {
        return Err(AgentError::permanent(
            format!("SFTP_NATIVE_AGENT_HTTP_{status}"),
            format!("native agent sftp/{capability_id} returned {status}: {body_text}"),
        ));
    }

    // The internal endpoint wraps every response in `{ success, output|error }`.
    let envelope: Value = serde_json::from_str(&body_text).map_err(|e| {
        AgentError::permanent(
            "SFTP_NATIVE_AGENT_PARSE_ERROR",
            format!("invalid JSON envelope from native agent: {e}: {body_text}"),
        )
    })?;

    if envelope
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        Ok(envelope.get("output").cloned().unwrap_or(Value::Null))
    } else {
        let err = envelope
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown native agent error")
            .to_string();
        Err(AgentError::permanent("SFTP_NATIVE_AGENT_ERROR", err))
    }
}

/// Run a capability stub: serialize input → forward → deserialize output.
fn run_capability<I, O>(
    capability_id: &str,
    connection: &Option<RawConnection>,
    input: &I,
) -> Result<O, AgentError>
where
    I: Serialize,
    O: for<'de> Deserialize<'de>,
{
    let input_value = serde_json::to_value(input)?;
    let output_value = forward_to_native(capability_id, connection, &input_value)?;
    serde_json::from_value(output_value).map_err(|e| {
        AgentError::permanent("SFTP_OUTPUT_DESERIALIZATION_ERROR", e.to_string())
            .with_attr("capability", capability_id)
    })
}

// ============================================================================
// File info (shared output type for list)
// ============================================================================

/// File information returned by list operations
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "File Info",
    description = "Information about a file or directory from SFTP listing"
)]
pub struct FileInfo {
    #[field(
        display_name = "Name",
        description = "The name of the file or directory",
        example = "document.txt"
    )]
    pub name: String,

    #[field(
        display_name = "Path",
        description = "The full path to the file or directory",
        example = "/home/user/documents/document.txt"
    )]
    pub path: String,

    #[field(
        display_name = "Size",
        description = "The size of the file in bytes",
        example = "1024"
    )]
    pub size: u64,

    #[field(
        display_name = "Is Directory",
        description = "Whether this entry is a directory",
        example = "false"
    )]
    pub is_directory: bool,

    #[field(
        display_name = "Modified Time",
        description = "The last modified timestamp (Unix epoch seconds)"
    )]
    pub modified_time: Option<i64>,
}

// ============================================================================
// List Files
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SFTP List Files Input")]
pub struct SftpListFilesInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (the UI/runtime fills it from the
    /// configured connection, not from user input).
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Directory Path",
        description = "Path to the directory to list (use \"/\" for root)",
        example = "/data/uploads"
    )]
    pub path: String,
}

#[capability(
    module = "sftp",
    display_name = "List Files",
    description = "List files and directories in an SFTP directory",
    module_display_name = "SFTP",
    module_description = "SFTP file operations (list, upload, download, delete). The wasm component forwards each call to the host's native SFTP handler (libssh2).",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "sftp",
    module_secure = true
)]
pub fn sftp_list_files(input: SftpListFilesInput) -> Result<Vec<FileInfo>, AgentError> {
    run_capability("sftp-list-files", &input._connection, &input)
}

// ============================================================================
// Download File
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SFTP Download File Input")]
pub struct SftpDownloadFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "File Path",
        description = "Full path to the file to download",
        example = "/data/uploads/document.pdf"
    )]
    pub path: String,

    #[field(
        display_name = "Response Format",
        description = "Format for the downloaded content: \"text\" for text files, \"base64\" for binary files",
        example = "text",
        default = "text"
    )]
    #[serde(default = "default_response_format")]
    pub response_format: String,
}

fn default_response_format() -> String {
    "text".to_string()
}

#[capability(
    module = "sftp",
    display_name = "Download File",
    description = "Download a file from SFTP and return its content"
)]
pub fn sftp_download_file(input: SftpDownloadFileInput) -> Result<String, AgentError> {
    run_capability("sftp-download-file", &input._connection, &input)
}

// ============================================================================
// Upload File
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SFTP Upload File Input")]
pub struct SftpUploadFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Destination Path",
        description = "Full path where the file should be uploaded",
        example = "/data/uploads/new-file.txt"
    )]
    pub path: String,

    #[field(
        display_name = "File Content",
        description = "Content to upload (plain text or base64-encoded binary)",
        example = "Hello, World!"
    )]
    pub content: String,

    #[field(
        display_name = "Content Format",
        description = "Format of the content: \"text\" for plain text, \"base64\" for binary data",
        example = "text",
        default = "text"
    )]
    #[serde(default = "default_content_format")]
    pub content_format: String,
}

fn default_content_format() -> String {
    "text".to_string()
}

#[capability(
    module = "sftp",
    display_name = "Upload File",
    description = "Upload a file to SFTP",
    side_effects = true
)]
pub fn sftp_upload_file(input: SftpUploadFileInput) -> Result<usize, AgentError> {
    run_capability("sftp-upload-file", &input._connection, &input)
}

// ============================================================================
// Delete File
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "SFTP Delete File Input")]
pub struct SftpDeleteFileInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "File Path",
        description = "Full path to the file to delete",
        example = "/data/uploads/old-file.txt"
    )]
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Delete File Response",
    description = "Response from deleting a file via SFTP"
)]
pub struct DeleteFileResponse {
    #[field(
        display_name = "Success",
        description = "Whether the deletion was successful",
        example = "true"
    )]
    pub success: bool,

    #[field(
        display_name = "Path",
        description = "The path of the deleted file",
        example = "/home/user/documents/old-file.txt"
    )]
    pub path: String,
}

#[capability(
    module = "sftp",
    display_name = "Delete File",
    description = "Delete a file from SFTP",
    side_effects = true
)]
pub fn sftp_delete_file(input: SftpDeleteFileInput) -> Result<DeleteFileResponse, AgentError> {
    run_capability("sftp-delete-file", &input._connection, &input)
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
        &__CAPABILITY_META_SFTP_LIST_FILES,
        &__CAPABILITY_META_SFTP_DOWNLOAD_FILE,
        &__CAPABILITY_META_SFTP_UPLOAD_FILE,
        &__CAPABILITY_META_SFTP_DELETE_FILE,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "SftpListFilesInput",
            &__INPUT_META_SftpListFilesInput as &InputTypeMeta,
        ),
        ("SftpDownloadFileInput", &__INPUT_META_SftpDownloadFileInput),
        ("SftpUploadFileInput", &__INPUT_META_SftpUploadFileInput),
        ("SftpDeleteFileInput", &__INPUT_META_SftpDeleteFileInput),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        ("FileInfo", &__OUTPUT_META_FileInfo as &OutputTypeMeta),
        ("DeleteFileResponse", &__OUTPUT_META_DeleteFileResponse),
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
        id: "sftp".into(),
        name: "SFTP".into(),
        description: "SFTP file operations (list, upload, download, delete). The wasm component forwards each call to the host's native SFTP handler (libssh2).".into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["sftp".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_sftp::capabilities::{ConnectionInfo, ErrorInfo, Guest};

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
        // so the macro-generated executor can deserialize it into the capability
        // input struct's `_connection: Option<RawConnection>` field.
        //
        // Credentials never cross the wasm boundary: we forward ONLY the opaque
        // `connection_id`. The host resolves the real parameters from it
        // (`internal_agents::run_agent`) and overwrites `_connection` wholesale,
        // so `c.parameters` is deliberately ignored — nothing secret flows
        // through here, and there is no path to reintroduce one.
        if let Some(c) = connection.as_ref() {
            if let serde_json::Value::Object(ref mut obj) = value {
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
                        "parameters": serde_json::Value::Object(Default::default()),
                        "rate_limit_config": rate_limit_config,
                    }),
                );
            }
        }

        let executor_result = match capability_id.as_str() {
            "sftp-list-files" => __executor_sftp_list_files(value),
            "sftp-download-file" => __executor_sftp_download_file(value),
            "sftp-upload-file" => __executor_sftp_upload_file(value),
            "sftp-delete-file" => __executor_sftp_delete_file(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("sftp agent has no capability `{other}`"),
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
    fn test_file_info_serialization() {
        let file = FileInfo {
            name: "test.txt".to_string(),
            path: "/data/test.txt".to_string(),
            size: 1024,
            is_directory: false,
            modified_time: Some(1609459200),
        };

        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("test.txt"));
        assert!(json.contains("1024"));
    }

    #[test]
    fn test_default_formats() {
        assert_eq!(default_response_format(), "text");
        assert_eq!(default_content_format(), "text");
    }
}
