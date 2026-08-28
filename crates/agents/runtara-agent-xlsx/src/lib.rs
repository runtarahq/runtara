//! XLSX agent — WebAssembly component.
//!
//! Excel / OpenDocument spreadsheet parsing (XLSX, XLS, XLSB, ODS), executed
//! entirely inside the wasm sandbox. This agent used to be a thin forwarder to
//! a native host handler at `$RUNTARA_AGENT_SERVICE_URL/xlsx/{capability}`;
//! that hop is gone. `calamine` is pure Rust and builds for wasm32-wasip2 with
//! its default features, and these capabilities only ever used its in-memory
//! reader (`open_workbook_auto_from_rs`) — no filesystem access is involved.
//! `rust_xlsxwriter`, named in the old header as the other blocker, was never
//! actually a dependency: there is no write capability.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]`
//! / `#[capability_output]` annotations on the same Rust types and functions
//! that the wasm `invoke` dispatcher calls into. `runtara-agent-bundle-emit`
//! reads the macro-emitted `&'static` statics on the host architecture and
//! writes `runtara_agent_xlsx.meta.json` next to the `.wasm`.
#![allow(clippy::result_large_err)]

use base64::{Engine as _, engine::general_purpose};
use calamine::{Data, Reader, open_workbook_auto_from_rs};
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Cursor;

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
        // spikes/wit-bindgen-async-typed).
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

impl FileData {
    /// Decode the base64 `content` into raw bytes.
    pub fn decode(&self) -> Result<Vec<u8>, AgentError> {
        general_purpose::STANDARD
            .decode(&self.content)
            .map_err(|e| {
                AgentError::permanent(
                    "XLSX_DECODE_ERROR",
                    format!("Failed to decode base64 file content: {}", e),
                )
            })
    }
}

impl XlsxDataInput {
    /// Normalize any accepted shape into raw spreadsheet bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, AgentError> {
        match self {
            XlsxDataInput::Bytes(b) => Ok(b.clone()),
            XlsxDataInput::File(f) => f.decode(),
            XlsxDataInput::Base64String(s) => general_purpose::STANDARD.decode(s).map_err(|e| {
                AgentError::permanent(
                    "XLSX_DECODE_ERROR",
                    format!("Failed to decode base64 spreadsheet content: {}", e),
                )
            }),
        }
    }
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
    module_description = "Parse Excel and OpenDocument spreadsheets (XLSX, XLS, XLSB, ODS).",
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
    let bytes = input.data.to_bytes()?;
    let cursor = Cursor::new(bytes);

    let mut workbook = open_workbook_auto_from_rs(cursor).map_err(|e| {
        AgentError::permanent(
            "XLSX_PARSE_ERROR",
            format!("Failed to open spreadsheet: {}", e),
        )
    })?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    if sheet_names.is_empty() {
        return Err(AgentError::permanent(
            "XLSX_PARSE_ERROR",
            "Workbook contains no sheets",
        ));
    }

    let sheet_name = resolve_sheet_name(&sheet_names, input.sheet.as_deref())?;

    let range = workbook.worksheet_range(&sheet_name).map_err(|e| {
        AgentError::permanent(
            "XLSX_PARSE_ERROR",
            format!("Failed to read sheet '{}': {}", sheet_name, e),
        )
    })?;

    let mut rows_iter = range.rows();
    let mut result: Vec<Value> = Vec::new();

    if input.has_headers {
        let headers: Vec<String> = match rows_iter.next() {
            Some(row) => row.iter().map(cell_to_header_string).collect(),
            None => return Ok(Value::Array(result)), // Empty sheet
        };

        for row in rows_iter {
            if input.skip_empty_rows && row.iter().all(|c| matches!(c, Data::Empty)) {
                continue;
            }

            let mut obj = serde_json::Map::new();
            for (i, cell) in row.iter().enumerate() {
                let key = headers.get(i).cloned().unwrap_or_else(|| i.to_string());
                if !key.is_empty() {
                    obj.insert(key, cell_to_value(cell));
                }
            }
            result.push(Value::Object(obj));
        }
    } else {
        for row in rows_iter {
            if input.skip_empty_rows && row.iter().all(|c| matches!(c, Data::Empty)) {
                continue;
            }

            let arr: Vec<Value> = row.iter().map(cell_to_value).collect();
            result.push(Value::Array(arr));
        }
    }

    // The capability's declared output is an untyped `Value` holding the row
    // array — unchanged from the forwarding version, which passed the host's
    // `Vec<Value>` through verbatim.
    Ok(Value::Array(result))
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
    let bytes = input.data.to_bytes()?;
    let cursor = Cursor::new(bytes);

    let mut workbook = open_workbook_auto_from_rs(cursor).map_err(|e| {
        AgentError::permanent(
            "XLSX_PARSE_ERROR",
            format!("Failed to open spreadsheet: {}", e),
        )
    })?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut sheets = Vec::with_capacity(sheet_names.len());

    for (index, name) in sheet_names.iter().enumerate() {
        let (rows, columns) = match workbook.worksheet_range(name) {
            Ok(range) => (range.height(), range.width()),
            Err(_) => (0, 0),
        };

        sheets.push(SheetInfo {
            name: name.clone(),
            index,
            rows,
            columns,
        });
    }

    Ok(sheets)
}

// ============================================================================
// Helper functions
// ============================================================================
//
// Ported verbatim from the former host-side `runtara_agents::xlsx`.

/// Resolve a sheet selector to a concrete sheet name.
/// - `None` → first sheet
/// - `"#N"` → sheet at index N
/// - other → sheet by name
fn resolve_sheet_name(
    sheet_names: &[String],
    selector: Option<&str>,
) -> Result<String, AgentError> {
    match selector {
        None => Ok(sheet_names[0].clone()),
        Some(s) if s.starts_with('#') => {
            let idx: usize = s[1..].parse().map_err(|_| {
                AgentError::permanent(
                    "XLSX_SHEET_NOT_FOUND",
                    format!("Invalid sheet index: '{}'", s),
                )
            })?;
            sheet_names.get(idx).cloned().ok_or_else(|| {
                AgentError::permanent(
                    "XLSX_SHEET_NOT_FOUND",
                    format!(
                        "Sheet index {} out of range (workbook has {} sheets)",
                        idx,
                        sheet_names.len()
                    ),
                )
            })
        }
        Some(name) => {
            if sheet_names.iter().any(|n| n == name) {
                Ok(name.to_string())
            } else {
                Err(AgentError::permanent(
                    "XLSX_SHEET_NOT_FOUND",
                    format!(
                        "Sheet '{}' not found. Available sheets: {}",
                        name,
                        sheet_names.join(", ")
                    ),
                ))
            }
        }
    }
}

/// Convert a cell to a JSON value.
fn cell_to_value(cell: &Data) -> Value {
    match cell {
        Data::Empty => Value::Null,
        Data::String(s) => Value::String(s.clone()),
        Data::Int(n) => Value::Number((*n).into()),
        Data::Float(f) => {
            // Whole floats come back as integers, matching the host behaviour.
            if f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 {
                Value::Number((*f as i64).into())
            } else {
                serde_json::Number::from_f64(*f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(dt) => Value::String(dt.to_string()),
        Data::DateTimeIso(s) => Value::String(s.clone()),
        Data::DurationIso(s) => Value::String(s.clone()),
        Data::Error(e) => Value::String(format!("#ERROR: {:?}", e)),
    }
}

/// Convert a cell to a string for use as a header name.
fn cell_to_header_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(n) => n.to_string(),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("{:?}", e),
    }
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
        description: "Parse Excel and OpenDocument spreadsheets (XLSX, XLS, XLSB, ODS).".into(),
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal three-row, three-column XLSX workbook with one sheet named
    /// "Orders" (header row + two data rows, inline strings and numeric cells).
    /// Built as a raw OOXML package so the fixture needs no writer dependency.
    const ORDERS_XLSX_B64: &str = concat!(
        "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",
        "AA==",
    );

    fn orders() -> XlsxDataInput {
        XlsxDataInput::Base64String(ORDERS_XLSX_B64.to_string())
    }

    #[test]
    fn parses_rows_into_objects_using_the_header_row() {
        let out = from_xlsx(FromXlsxInput {
            _connection: None,
            data: orders(),
            sheet: None,
            has_headers: true,
            skip_empty_rows: true,
        })
        .expect("workbook parses");

        let rows = out.as_array().expect("rows are an array");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["sku"], Value::String("ABC-1".into()));
        // Whole floats collapse to integers, matching the host behaviour.
        assert_eq!(rows[0]["qty"], Value::Number(4.into()));
        assert_eq!(rows[0]["price"].as_f64(), Some(9.5));
        assert_eq!(rows[1]["sku"], Value::String("XYZ-9".into()));
        assert_eq!(rows[1]["price"].as_f64(), Some(0.25));
    }

    #[test]
    fn without_headers_every_row_is_an_array() {
        let out = from_xlsx(FromXlsxInput {
            _connection: None,
            data: orders(),
            sheet: None,
            has_headers: false,
            skip_empty_rows: true,
        })
        .expect("workbook parses");

        let rows = out.as_array().expect("rows are an array");
        assert_eq!(rows.len(), 3); // header row is data now
        assert_eq!(rows[0][0], Value::String("sku".into()));
    }

    #[test]
    fn get_sheets_reports_names_and_dimensions() {
        let sheets = get_sheets(GetSheetsInput {
            _connection: None,
            data: orders(),
        })
        .expect("workbook opens");

        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].name, "Orders");
        assert_eq!(sheets[0].index, 0);
        assert_eq!(sheets[0].rows, 3);
        assert_eq!(sheets[0].columns, 3);
    }

    #[test]
    fn sheet_selector_accepts_a_name_and_an_index() {
        for selector in ["Orders", "#0"] {
            let out = from_xlsx(FromXlsxInput {
                _connection: None,
                data: orders(),
                sheet: Some(selector.to_string()),
                has_headers: true,
                skip_empty_rows: true,
            })
            .expect("selector resolves");
            assert_eq!(
                out.as_array().expect("array").len(),
                2,
                "selector {selector}"
            );
        }
    }

    #[test]
    fn unknown_sheet_is_a_typed_not_found_error() {
        let err = from_xlsx(FromXlsxInput {
            _connection: None,
            data: orders(),
            sheet: Some("Nope".to_string()),
            has_headers: true,
            skip_empty_rows: true,
        })
        .expect_err("missing sheet errors");

        assert_eq!(err.code, "XLSX_SHEET_NOT_FOUND");
        assert!(err.message.contains("Orders"), "lists available sheets");
    }

    #[test]
    fn undecodable_input_surfaces_a_decode_error() {
        let err = get_sheets(GetSheetsInput {
            _connection: None,
            data: XlsxDataInput::Base64String("!!not base64!!".to_string()),
        })
        .expect_err("bad base64 errors");

        assert_eq!(err.code, "XLSX_DECODE_ERROR");
    }

    #[test]
    fn a_non_spreadsheet_payload_is_a_parse_error() {
        let err = get_sheets(GetSheetsInput {
            _connection: None,
            data: XlsxDataInput::Base64String("aGVsbG8gd29ybGQ=".to_string()),
        })
        .expect_err("garbage errors");

        assert_eq!(err.code, "XLSX_PARSE_ERROR");
    }
}
