//! CSV agent — parse and generate CSV — as a WebAssembly Component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]`
//! annotations on the same Rust types and functions that the wasm cdylib's
//! `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_csv.meta.json` next to the
//! `.wasm` — the JSON is a build artifact, never hand-edited.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use runtara_agent_macro::{CapabilityInput, capability};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// -----------------------------------------------------------------------------
// Locally-defined FileData (replaces legacy `crate::types::FileData`).
// The wasm component has no filesystem access, so the file content is always
// base64-encoded and shipped inline through the JSON envelope.
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct FileData {
    pub content: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub filename: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub mime_type: Option<String>,
}

// -----------------------------------------------------------------------------
// Inputs (with capability macros so meta.json can be derived)
// -----------------------------------------------------------------------------

/// Flexible CSV data input supporting raw bytes or base64 encoded file structures
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CsvDataInput {
    /// Raw bytes (existing behavior)
    Bytes(Vec<u8>),
    /// File data with base64 content
    File(FileData),
    /// Plain base64 string
    Base64String(String),
}

impl CsvDataInput {
    /// Convert any supported input into raw bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            CsvDataInput::Bytes(b) => Ok(b.clone()),
            CsvDataInput::File(f) => BASE64.decode(&f.content).map_err(|e| {
                err_json(
                    "CSV_DECODE_ERROR",
                    format!("Failed to decode FileData base64 content: {e}"),
                )
            }),
            CsvDataInput::Base64String(s) => BASE64.decode(s).map_err(|e| {
                err_json(
                    "CSV_DECODE_ERROR",
                    format!("Failed to decode base64 CSV content: {e}"),
                )
            }),
        }
    }
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Parse CSV Input")]
pub struct FromCsvInput {
    /// Raw CSV data as bytes
    #[field(
        display_name = "CSV Data",
        description = "Raw CSV data as bytes, base64 encoded string, or file data object"
    )]
    pub data: CsvDataInput,

    /// Character encoding (default: "UTF-8")
    #[field(
        display_name = "Encoding",
        description = "Character encoding of the CSV data",
        example = "UTF-8",
        default = "UTF-8"
    )]
    #[serde(default = "default_encoding")]
    pub encoding: String,

    /// Column delimiter (default: ',')
    #[field(
        display_name = "Delimiter",
        description = "Column delimiter character",
        example = ",",
        default = ","
    )]
    #[serde(default = "default_delimiter")]
    pub delimiter: char,

    /// Quote character (default: '"')
    #[field(
        display_name = "Quote Character",
        description = "Character used to quote fields containing delimiters",
        example = "\"",
        default = "\""
    )]
    #[serde(default = "default_quote_char")]
    pub quote_char: char,

    /// Escape character (default: empty = no escape)
    #[field(
        display_name = "Escape Character",
        description = "Character used to escape special characters (optional)"
    )]
    #[serde(default)]
    pub escape_char: Option<char>,

    /// Whether the first row contains headers (default: true)
    #[field(
        display_name = "Use Header",
        description = "Whether the first row contains column headers",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub use_header: bool,

    /// Skip empty lines (default: true)
    #[field(
        display_name = "Skip Empty Lines",
        description = "Whether to skip empty lines in the CSV",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub skip_empty_lines: bool,

    /// Trim whitespace from fields (default: false)
    #[field(
        display_name = "Trim Whitespace",
        description = "Whether to trim whitespace from field values",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub trim_whitespace: bool,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Generate CSV Input")]
pub struct ToCsvInput {
    /// Data to convert to CSV (can be array of objects, array of arrays, or single object)
    #[field(
        display_name = "Value",
        description = "Data to convert to CSV (array of objects, array of arrays, or single object)",
        example = r#"[{"name": "Alice", "age": 30}]"#
    )]
    pub value: Value,

    /// Character encoding (default: "UTF-8")
    #[field(
        display_name = "Encoding",
        description = "Character encoding for the output CSV",
        example = "UTF-8",
        default = "UTF-8"
    )]
    #[serde(default = "default_encoding")]
    #[allow(dead_code)]
    pub encoding: String,

    /// Column delimiter (default: ',')
    #[field(
        display_name = "Delimiter",
        description = "Column delimiter character",
        example = ",",
        default = ","
    )]
    #[serde(default = "default_delimiter")]
    pub delimiter: char,

    /// Quote character (default: '"')
    #[field(
        display_name = "Quote Character",
        description = "Character used to quote fields containing delimiters",
        example = "\"",
        default = "\""
    )]
    #[serde(default = "default_quote_char")]
    pub quote_char: char,

    /// Escape character (default: empty = no escape)
    #[field(
        display_name = "Escape Character",
        description = "Character used to escape special characters (optional)"
    )]
    #[serde(default)]
    pub escape_char: Option<char>,

    /// Whether to include header row (default: true)
    #[field(
        display_name = "Use Header",
        description = "Whether to include a header row in the output",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub use_header: bool,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get CSV Header Input")]
pub struct GetHeaderInput {
    /// Raw CSV data as bytes
    #[field(
        display_name = "CSV Data",
        description = "Raw CSV data as bytes, base64 encoded string, or file data object"
    )]
    pub data: CsvDataInput,

    /// Character encoding (default: "UTF-8")
    #[field(
        display_name = "Encoding",
        description = "Character encoding of the CSV data",
        example = "UTF-8",
        default = "UTF-8"
    )]
    #[serde(default = "default_encoding")]
    pub encoding: String,

    /// Column delimiter (default: ',')
    #[field(
        display_name = "Delimiter",
        description = "Column delimiter character",
        example = ",",
        default = ","
    )]
    #[serde(default = "default_delimiter")]
    pub delimiter: char,

    /// Quote character (default: '"')
    #[field(
        display_name = "Quote Character",
        description = "Character used to quote fields containing delimiters",
        example = "\"",
        default = "\""
    )]
    #[serde(default = "default_quote_char")]
    pub quote_char: char,

    /// Escape character (default: empty = no escape)
    #[field(
        display_name = "Escape Character",
        description = "Character used to escape special characters (optional)"
    )]
    #[serde(default)]
    pub escape_char: Option<char>,

    /// Whether the first row contains headers (default: true)
    #[field(
        display_name = "Use Header",
        description = "Whether the first row contains column headers",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub use_header: bool,

    /// Trim whitespace from fields (default: false)
    #[field(
        display_name = "Trim Whitespace",
        description = "Whether to trim whitespace from field values",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub trim_whitespace: bool,
}

// Default value functions
fn default_encoding() -> String {
    "UTF-8".to_string()
}

fn default_delimiter() -> char {
    ','
}

fn default_quote_char() -> char {
    '"'
}

fn default_true() -> bool {
    true
}

// -----------------------------------------------------------------------------
// Capabilities — annotated for metadata; the `__executor_*` fns the macro emits
// are what the wasm Guest impl dispatches to.
// -----------------------------------------------------------------------------

/// Parses CSV bytes into a JSON array
/// - With headers: Returns Vec<Map<String, String>>
/// - Without headers: Returns Vec<Vec<String>>
#[capability(
    id = "from-csv",
    module = "csv",
    module_display_name = "CSV",
    module_description = "CSV parsing and generation.",
    display_name = "Parse CSV",
    description = "Parse CSV bytes into a JSON array of objects or arrays"
)]
pub fn from_csv(input: FromCsvInput) -> Result<Vec<Value>, String> {
    // Convert bytes to string using specified encoding
    let data = input.data.to_bytes()?;
    let csv_string = decode_bytes(&data, &input.encoding)?;

    // Build CSV reader
    let mut reader_builder = csv::ReaderBuilder::new();
    reader_builder
        .delimiter(input.delimiter as u8)
        .quote(input.quote_char as u8)
        .has_headers(input.use_header)
        .trim(if input.trim_whitespace {
            csv::Trim::All
        } else {
            csv::Trim::None
        });

    if let Some(escape) = input.escape_char {
        reader_builder.escape(Some(escape as u8));
    }

    let mut reader = reader_builder.from_reader(csv_string.as_bytes());
    let mut result = Vec::new();

    if input.use_header {
        // Parse with headers - return array of objects
        let headers = reader
            .headers()
            .map_err(|e| {
                err_json(
                    "CSV_PARSE_ERROR",
                    format!("Failed to read CSV headers: {e}"),
                )
            })?
            .clone();

        for record_result in reader.records() {
            let record = record_result.map_err(|e| {
                err_json("CSV_PARSE_ERROR", format!("Failed to read CSV record: {e}"))
            })?;

            // Skip empty lines if configured
            if input.skip_empty_lines && is_empty_record(&record) {
                continue;
            }

            let mut row_map = Map::new();
            for (i, field) in record.iter().enumerate() {
                let column_name = headers
                    .get(i)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| i.to_string());

                if !column_name.is_empty() {
                    row_map.insert(column_name, Value::String(field.to_string()));
                }
            }

            result.push(Value::Object(row_map));
        }
    } else {
        // Parse without headers - return array of arrays
        for record_result in reader.records() {
            let record = record_result.map_err(|e| {
                err_json("CSV_PARSE_ERROR", format!("Failed to read CSV record: {e}"))
            })?;

            // Skip empty lines if configured
            if input.skip_empty_lines && is_empty_record(&record) {
                continue;
            }

            let row: Vec<Value> = record
                .iter()
                .map(|field| Value::String(field.to_string()))
                .collect();

            result.push(Value::Array(row));
        }
    }

    Ok(result)
}

/// Converts JSON data to CSV bytes
#[capability(
    id = "to-csv",
    module = "csv",
    display_name = "Generate CSV",
    description = "Convert JSON data to CSV bytes"
)]
pub fn to_csv(input: ToCsvInput) -> Result<Vec<u8>, String> {
    let mut writer_builder = csv::WriterBuilder::new();
    writer_builder
        .delimiter(input.delimiter as u8)
        .quote(input.quote_char as u8);

    if let Some(escape) = input.escape_char {
        writer_builder.escape(escape as u8);
    }

    let mut output = Vec::new();

    {
        let mut writer = writer_builder.from_writer(&mut output);

        match &input.value {
            Value::Array(arr) => {
                if !arr.is_empty() {
                    // Write header if requested
                    if input.use_header {
                        let headers = get_header_names(&arr[0]);
                        writer.write_record(&headers).map_err(|e| {
                            err_json(
                                "CSV_WRITE_ERROR",
                                format!("Failed to write CSV header: {e}"),
                            )
                        })?;
                    }

                    // Write rows
                    for item in arr {
                        let row = value_to_csv_row(item);
                        writer.write_record(&row).map_err(|e| {
                            err_json("CSV_WRITE_ERROR", format!("Failed to write CSV row: {e}"))
                        })?;
                    }
                }
            }
            single_value => {
                // Single object - write as one row
                if input.use_header {
                    let headers = get_header_names(single_value);
                    writer.write_record(&headers).map_err(|e| {
                        err_json(
                            "CSV_WRITE_ERROR",
                            format!("Failed to write CSV header: {e}"),
                        )
                    })?;
                }

                let row = value_to_csv_row(single_value);
                writer.write_record(&row).map_err(|e| {
                    err_json("CSV_WRITE_ERROR", format!("Failed to write CSV row: {e}"))
                })?;
            }
        }

        writer.flush().map_err(|e| {
            err_json(
                "CSV_WRITE_ERROR",
                format!("Failed to flush CSV writer: {e}"),
            )
        })?;
    }
    // Writer is dropped here, releasing the borrow on output

    Ok(output)
}

/// Extracts CSV headers with type inference from the first data row
#[capability(
    id = "get-header",
    module = "csv",
    display_name = "Get CSV Header",
    description = "Extract CSV headers with type inference from the first data row"
)]
pub fn get_header(input: GetHeaderInput) -> Result<HashMap<String, String>, String> {
    // Convert bytes to string using specified encoding
    let data = input.data.to_bytes()?;
    let csv_string = decode_bytes(&data, &input.encoding)?;

    // Build CSV reader
    let mut reader_builder = csv::ReaderBuilder::new();
    reader_builder
        .delimiter(input.delimiter as u8)
        .quote(input.quote_char as u8)
        .has_headers(input.use_header)
        .trim(if input.trim_whitespace {
            csv::Trim::All
        } else {
            csv::Trim::None
        });

    if let Some(escape) = input.escape_char {
        reader_builder.escape(Some(escape as u8));
    }

    let mut reader = reader_builder.from_reader(csv_string.as_bytes());
    let mut result: HashMap<String, String> = HashMap::new();

    // Get headers
    let headers: Vec<String> = if input.use_header {
        reader
            .headers()
            .map_err(|e| {
                err_json(
                    "CSV_PARSE_ERROR",
                    format!("Failed to read CSV headers: {e}"),
                )
            })?
            .iter()
            .enumerate()
            .map(|(i, h)| {
                if h.is_empty() {
                    i.to_string()
                } else {
                    h.to_string()
                }
            })
            .collect()
    } else {
        // If no header, peek at first row to determine column count
        let first_record = reader
            .records()
            .next()
            .ok_or_else(|| err_json("CSV_EMPTY_FILE", "CSV file is empty"))?
            .map_err(|e| {
                err_json(
                    "CSV_PARSE_ERROR",
                    format!("Failed to read first record: {e}"),
                )
            })?;

        (0..first_record.len())
            .map(|i| format!("Column {}", i + 1))
            .collect()
    };

    // Get first data row for type inference
    let first_data_row = reader.records().next();

    if let Some(record_result) = first_data_row {
        let record = record_result
            .map_err(|e| err_json("CSV_PARSE_ERROR", format!("Failed to read data row: {e}")))?;

        for (i, header) in headers.iter().enumerate() {
            let inferred_type = if let Some(value) = record.get(i) {
                infer_type(value)
            } else {
                "String".to_string()
            };

            result.insert(header.clone(), inferred_type);
        }
    } else {
        // No data rows - default all to String
        for header in headers {
            result.insert(header, "String".to_string());
        }
    }

    Ok(result)
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Decodes bytes to string using specified encoding
fn decode_bytes(data: &[u8], encoding: &str) -> Result<String, String> {
    match encoding.to_uppercase().as_str() {
        "UTF-8" | "UTF8" => String::from_utf8(data.to_vec())
            .map_err(|e| err_json("CSV_ENCODING_ERROR", format!("Failed to decode UTF-8: {e}"))),
        "LATIN-1" | "LATIN1" | "ISO-8859-1" | "ISO88591" | "WINDOWS-1252" | "CP1252" => {
            // Use encoding_rs for Latin-1/Windows-1252 encoding
            let (decoded, _, _had_errors) = encoding_rs::WINDOWS_1252.decode(data);
            Ok(decoded.into_owned())
        }
        _ => {
            // Try to use encoding_rs for other encodings
            if let Some(enc) = encoding_rs::Encoding::for_label(encoding.as_bytes()) {
                let (decoded, _, _) = enc.decode(data);
                Ok(decoded.into_owned())
            } else {
                // Fall back to lossy UTF-8 conversion
                Ok(String::from_utf8_lossy(data).into_owned())
            }
        }
    }
}

/// Checks if a CSV record is empty
fn is_empty_record(record: &csv::StringRecord) -> bool {
    record.is_empty() || (record.len() == 1 && record.get(0).is_some_and(|s| s.trim().is_empty()))
}

/// Gets header names from a JSON value
fn get_header_names(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => map.keys().map(|k| k.to_string()).collect(),
        Value::Array(arr) => (1..=arr.len()).map(|i| format!("Column {i}")).collect(),
        _ => vec!["value".to_string()],
    }
}

/// Converts a JSON value to a CSV row (array of strings)
fn value_to_csv_row(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => map.values().map(value_to_string).collect(),
        Value::Array(arr) => arr.iter().map(value_to_string).collect(),
        _ => vec![value_to_string(value)],
    }
}

/// Converts a JSON value to string representation
fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => {
            // Serialize complex types as JSON
            serde_json::to_string(value).unwrap_or_default()
        }
    }
}

/// Infers the type of a CSV field value
fn infer_type(value: &str) -> String {
    // Try parsing as JSON to infer type
    if let Ok(json_value) = serde_json::from_str::<Value>(value) {
        match json_value {
            Value::Bool(_) => return "Boolean".to_string(),
            Value::Number(n) if n.is_i64() => return "Integer".to_string(),
            Value::Number(n) if n.is_f64() => return "Double".to_string(),
            _ => {}
        }
    }

    "String".to_string()
}

/// Build the JSON-string error envelope the `#[capability]` macro round-trips
/// back to the wasm host via `error_string_to_error_info`.
fn err_json(code: &str, message: impl Into<String>) -> String {
    serde_json::json!({
        "code": code,
        "message": message.into(),
        "category": "permanent",
        "severity": "error",
    })
    .to_string()
}

// -----------------------------------------------------------------------------
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// -----------------------------------------------------------------------------

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
        &__CAPABILITY_META_FROM_CSV,
        &__CAPABILITY_META_TO_CSV,
        &__CAPABILITY_META_GET_HEADER,
    ];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        ("FromCsvInput", &__INPUT_META_FromCsvInput as &InputTypeMeta),
        ("ToCsvInput", &__INPUT_META_ToCsvInput),
        ("GetHeaderInput", &__INPUT_META_GetHeaderInput),
    ]
    .into_iter()
    .collect();
    // CSV capabilities return `Vec<Value>`, `Vec<u8>` and
    // `HashMap<String, String>` — none are user-defined output structs, so there
    // is no `OutputTypeMeta` to register.
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = HashMap::new();

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
        id: "csv".into(),
        name: "CSV".into(),
        description: "CSV parsing and generation.".into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities,
    }
}

// -----------------------------------------------------------------------------
// Wasm component plumbing
// -----------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_csv::capabilities::{ConnectionInfo, ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(
        capability_id: String,
        input: Vec<u8>,
        _connection: Option<ConnectionInfo>,
    ) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;
        let executor_result = match capability_id.as_str() {
            "from-csv" => __executor_from_csv(value),
            "to-csv" => __executor_to_csv(value),
            "get-header" => __executor_get_header(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("csv agent has no capability `{other}`"),
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
/// `{ code, message, category, severity }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
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
            category: value
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("permanent")
                .into(),
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable: value
                .get("retryable")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
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
