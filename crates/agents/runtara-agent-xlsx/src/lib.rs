//! XLSX agent — thin wrapper component.
//!
//! Excel/OpenDocument spreadsheet parsing relies on `calamine` (read) and
//! `rust_xlsxwriter` (write) — neither builds cleanly to `wasm32-wasip2`, so
//! this component is a *native wrapper*: each capability is a forwarding stub
//! that POSTs the typed input JSON to the host's internal native-agent endpoint
//! at `$RUNTARA_AGENT_SERVICE_URL/xlsx/{capability_id}` (typically
//! `http://127.0.0.1:7002/api/internal/agents/xlsx/{capability_id}`). The host
//! runs the real work and returns the result JSON, which we deserialize back
//! into the typed output and hand to the dispatcher.
//!
//! Capability metadata still travels through `#[capability_input]` /
//! `#[capability]` / `#[capability_output]` annotations on the same Rust types
//! and functions that the wasm cdylib's `invoke` dispatcher calls into. The
//! workspace binary `runtara-agent-bundle-emit` reads these macro-emitted
//! `&'static` statics on the host architecture and writes
//! `runtara_agent_xlsx.meta.json` next to the `.wasm` — the JSON is a build
//! artifact, never hand-edited.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings {
    // Bindings are generated at compile time by the wit-bindgen macro (no
    // committed bindings.rs, no cargo-component). `path` lists the shared
    // `runtara:agent` package first (dependency), then this crate's
    // build.rs-generated `wit/agent.wit`.
    wit_bindgen::generate!({
        path: ["../../runtara-agent-wit/wit", "wit"],
        world: "runtara:agent-xlsx/agent",
        // Sync impls of the async-TYPED invoke (sync lift; see
        // docs/wasip3-parallelism.md ABI v2 + spikes/wit-bindgen-async-typed).
        async: false,
        generate_all,
    });
}

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
}

impl From<serde_json::Error> for AgentError {
    fn from(err: serde_json::Error) -> Self {
        AgentError::permanent("XLSX_JSON_ERROR", err.to_string())
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
// The xlsx agent itself doesn't use connections (`supports_connections: false`),
// but the macro-derived dispatcher path still pipes the optional `_connection`
// field through input deserialization, and `forward_to_native` re-serializes it
// when shipping the request to the host. We keep the shape consistent with the
// other migrated HTTP agents so any future capability that does take a
// connection slots in without surgery.

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
// FileData / XlsxDataInput
// ============================================================================
//
// The wasm component has no filesystem access, so spreadsheet bytes always
// arrive base64-encoded inside the input JSON. We mirror the legacy shapes from
// `crates/runtara-agents/src/agents/xlsx.rs` so the host's native handler — which
// reuses the same legacy struct definitions — deserializes our forwarded body
// unchanged.

#[derive(Debug, Serialize, Deserialize)]
pub struct FileData {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Flexible spreadsheet data input supporting raw bytes or base64 encoded file
/// structures. Untagged so the JSON shape matches the legacy agent verbatim.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum XlsxDataInput {
    /// Raw bytes
    Bytes(Vec<u8>),
    /// File data with base64 content
    File(FileData),
    /// Plain base64 string
    Base64String(String),
}

// ============================================================================
// Parse Spreadsheet (from-xlsx)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Parse Spreadsheet Input")]
pub struct FromXlsxInput {
    /// Connection data injected by the wasm Guest::invoke wrapper before
    /// dispatching to the capability executor. `#[field(skip)]` keeps this
    /// out of the capability metadata (xlsx has no connection, but the
    /// dispatcher pipeline still flows through this field).
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Spreadsheet Data",
        description = "Spreadsheet data as bytes, base64 encoded string, or file data object"
    )]
    pub data: XlsxDataInput,

    #[field(
        display_name = "Sheet",
        description = "Sheet name or index (e.g. '#0' for first sheet, '#2' for third). Default: first sheet",
        example = "Sheet1"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,

    #[field(
        display_name = "Has Headers",
        description = "Whether the first row contains column headers",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub has_headers: bool,

    #[field(
        display_name = "Skip Empty Rows",
        description = "Whether to skip rows where all cells are empty",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub skip_empty_rows: bool,
}

fn default_true() -> bool {
    true
}

#[capability(
    module = "xlsx",
    display_name = "Parse Spreadsheet",
    description = "Parse a spreadsheet sheet into a JSON array of objects or arrays. Supports XLSX, XLS, XLSB, and ODS formats.",
    module_display_name = "Spreadsheet",
    module_description = "Parse Excel and OpenDocument spreadsheets (XLSX, XLS, XLSB, ODS). Runs on the host via the native agent service.",
    errors(
        permanent("XLSX_DECODE_ERROR", "Failed to decode base64 or file data"),
        permanent("XLSX_PARSE_ERROR", "Failed to open or parse the spreadsheet file"),
        permanent(
            "XLSX_SHEET_NOT_FOUND",
            "The requested sheet was not found in the workbook"
        ),
    )
)]
pub fn from_xlsx(input: FromXlsxInput) -> Result<Value, AgentError> {
    let input_value = serde_json::to_value(&input)?;
    let raw = forward_to_native("from-xlsx", input._connection.as_ref(), &input_value)?;
    // The legacy capability returns `Vec<Value>` (rows as JSON values). We keep
    // it as untyped `Value` to avoid a redundant Vec<Value> ↔ Value round-trip.
    Ok(raw)
}

// ============================================================================
// List Sheets (get-sheets)
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Sheets Input")]
pub struct GetSheetsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Spreadsheet Data",
        description = "Spreadsheet data as bytes, base64 encoded string, or file data object"
    )]
    pub data: XlsxDataInput,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Sheet Info",
    description = "Metadata about a single sheet in a workbook"
)]
pub struct SheetInfo {
    #[field(display_name = "Name", description = "Sheet name")]
    pub name: String,

    #[field(display_name = "Index", description = "Zero-based sheet index")]
    pub index: usize,

    #[field(display_name = "Rows", description = "Number of rows in the sheet")]
    pub rows: usize,

    #[field(
        display_name = "Columns",
        description = "Number of columns in the sheet"
    )]
    pub columns: usize,
}

#[capability(
    module = "xlsx",
    display_name = "List Sheets",
    description = "List all sheet names and dimensions from a spreadsheet workbook",
    errors(
        permanent("XLSX_DECODE_ERROR", "Failed to decode base64 or file data"),
        permanent("XLSX_PARSE_ERROR", "Failed to open or parse the spreadsheet file"),
    )
)]
pub fn get_sheets(input: GetSheetsInput) -> Result<Vec<SheetInfo>, AgentError> {
    let input_value = serde_json::to_value(&input)?;
    let raw = forward_to_native("get-sheets", input._connection.as_ref(), &input_value)?;
    serde_json::from_value::<Vec<SheetInfo>>(raw).map_err(|e| {
        AgentError::permanent("XLSX_OUTPUT_DESERIALIZATION_ERROR", e.to_string())
            .with_attr("capability", "get-sheets")
    })
}

// ============================================================================
// Forwarding to the host's native xlsx handler
// ============================================================================

/// POST `input_value` (with `_connection` re-attached when present) to
/// `$RUNTARA_AGENT_SERVICE_URL/xlsx/{capability_id}` and return the response
/// JSON. Mirrors the request shape the original wasm xlsx stub used so the host
/// handler reads us identically — only the typed-input/typed-output framing
/// changed.
fn forward_to_native(
    capability_id: &str,
    connection: Option<&RawConnection>,
    input_value: &Value,
) -> Result<Value, AgentError> {
    let base = std::env::var("RUNTARA_AGENT_SERVICE_URL").map_err(|_| {
        AgentError::permanent(
            "AGENT_SERVICE_URL_MISSING",
            "RUNTARA_AGENT_SERVICE_URL not set; native wrapper cannot forward",
        )
    })?;
    let url = format!("{}/xlsx/{capability_id}", base.trim_end_matches('/'));

    // Build the outgoing body — start from the typed input, drop `_connection`
    // (we re-attach it under the canonical shape the host expects so the
    // serialization matches whatever the legacy direct invocation produced).
    let mut body_value = input_value.clone();
    if let Value::Object(ref mut map) = body_value {
        map.remove("_connection");
        if let Some(conn) = connection {
            map.insert(
                "_connection".into(),
                serde_json::json!({
                    "connection_id": conn.connection_id,
                    "integration_id": conn.integration_id,
                    "connection_subtype": conn.connection_subtype,
                    "parameters": conn.parameters,
                    "rate_limit_config": conn.rate_limit_config,
                }),
            );
        }
    }

    let body = serde_json::to_vec(&body_value)
        .map_err(|e| AgentError::permanent("XLSX_INPUT_SERIALIZATION_ERROR", e.to_string()))?;
    let tenant_id = std::env::var("RUNTARA_TENANT_ID").unwrap_or_default();

    let response = runtara_http::HttpClient::with_timeout(Duration::from_secs(120))
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Org-Id", &tenant_id)
        .body_bytes(&body)
        .call()
        .map_err(|e| {
            AgentError::transient(
                "XLSX_NETWORK_ERROR",
                format!("native agent call failed: {e}"),
            )
        })?;

    let status = response.status;
    let body_text = String::from_utf8_lossy(&response.body).to_string();
    if !(200..300).contains(&status) {
        // Try to lift a structured error from the host (canonical AgentError
        // JSON shape). Fall back to a permanent error wrapping the raw body.
        if let Ok(value) = serde_json::from_str::<Value>(&body_text)
            && value.get("code").and_then(|v| v.as_str()).is_some()
        {
            let category = value
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("permanent");
            let code = value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("XLSX_NATIVE_ERROR")
                .to_string();
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&body_text)
                .to_string();
            let mut err = if category == "transient" {
                AgentError::transient(code, message)
            } else {
                AgentError::permanent(code, message)
            };
            err = err
                .with_attr("status_code", status.to_string())
                .with_attr("capability", capability_id);
            return Err(err);
        }
        return Err(AgentError::permanent(
            format!("XLSX_NATIVE_HTTP_{status}"),
            format!("native xlsx/{capability_id} returned {status}: {body_text}"),
        )
        .with_attr("status_code", status.to_string()));
    }

    serde_json::from_str::<Value>(&body_text).map_err(|e| {
        AgentError::permanent(
            "XLSX_OUTPUT_DESERIALIZATION_ERROR",
            format!("failed to parse native xlsx response: {e}"),
        )
        .with_attr("capability", capability_id)
    })
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================

#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] =
        &[&__CAPABILITY_META_FROM_XLSX, &__CAPABILITY_META_GET_SHEETS];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "FromXlsxInput",
            &__INPUT_META_FromXlsxInput as &InputTypeMeta,
        ),
        (
            "GetSheetsInput",
            &__INPUT_META_GetSheetsInput as &InputTypeMeta,
        ),
    ]
    .into_iter()
    .collect();
    // `from-xlsx` returns a raw `Value`, so only `get-sheets`'s `Vec<SheetInfo>`
    // contributes an `OutputTypeMeta`.
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> =
        [("SheetInfo", &__OUTPUT_META_SheetInfo as &OutputTypeMeta)]
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
        id: "xlsx".into(),
        name: "Spreadsheet".into(),
        description:
            "Parse Excel and OpenDocument spreadsheets (XLSX, XLS, XLSB, ODS). Runs on the host via the native agent service."
                .into(),
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
use bindings::exports::runtara::agent_xlsx::capabilities::{ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(capability_id: String, input: Vec<u8>) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        let executor_result = match capability_id.as_str() {
            "from-xlsx" => __executor_from_xlsx(value),
            "get-sheets" => __executor_get_sheets(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("xlsx agent has no capability `{other}`"),
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
