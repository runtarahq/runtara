//! CSV agent — parse and generate CSV — as a WebAssembly Component.
//!
//! Schema parity with `runtara-agents/src/agents/csv.rs`.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CsvDataInput {
    Bytes(Vec<u8>),
    File(FileData),
    Base64String(String),
}

#[derive(Debug, Deserialize)]
struct FileData {
    content: String,
    #[serde(default)]
    #[allow(dead_code)]
    filename: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    mime_type: Option<String>,
}

impl CsvDataInput {
    fn to_bytes(&self) -> Result<Vec<u8>, ErrorInfo> {
        match self {
            CsvDataInput::Bytes(b) => Ok(b.clone()),
            CsvDataInput::File(f) => BASE64
                .decode(&f.content)
                .map_err(|e| permanent_err("CSV_DECODE_ERROR", e.to_string())),
            CsvDataInput::Base64String(s) => BASE64
                .decode(s)
                .map_err(|e| permanent_err("CSV_DECODE_ERROR", e.to_string())),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FromCsvInput {
    data: CsvDataInput,
    #[serde(default = "default_encoding")]
    encoding: String,
    #[serde(default = "default_delimiter")]
    delimiter: char,
    #[serde(default = "default_quote_char")]
    quote_char: char,
    #[serde(default)]
    escape_char: Option<char>,
    #[serde(default = "default_true")]
    use_header: bool,
    #[serde(default = "default_true")]
    skip_empty_lines: bool,
    #[serde(default)]
    trim_whitespace: bool,
}

#[derive(Debug, Deserialize)]
struct ToCsvInput {
    value: Value,
    #[serde(default = "default_encoding")]
    #[allow(dead_code)]
    encoding: String,
    #[serde(default = "default_delimiter")]
    delimiter: char,
    #[serde(default = "default_quote_char")]
    quote_char: char,
    #[serde(default)]
    escape_char: Option<char>,
    #[serde(default = "default_true")]
    use_header: bool,
}

#[derive(Debug, Deserialize)]
struct GetHeaderInput {
    data: CsvDataInput,
    #[serde(default = "default_encoding")]
    encoding: String,
    #[serde(default = "default_delimiter")]
    delimiter: char,
    #[serde(default = "default_quote_char")]
    quote_char: char,
    #[serde(default)]
    escape_char: Option<char>,
    #[serde(default = "default_true")]
    use_header: bool,
    #[serde(default)]
    trim_whitespace: bool,
}

fn default_encoding() -> String {
    "UTF-8".into()
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

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "csv".into(),
            display_name: "CSV".into(),
            description: "CSV parsing and generation.".into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "from-csv",
                "from_csv",
                "Parse CSV",
                "Parse CSV bytes into a JSON array of objects or arrays.",
                FROM_CSV_INPUT_SCHEMA,
                FROM_CSV_OUTPUT_SCHEMA,
            ),
            cap(
                "to-csv",
                "to_csv",
                "Generate CSV",
                "Convert JSON data to CSV bytes.",
                TO_CSV_INPUT_SCHEMA,
                TO_CSV_OUTPUT_SCHEMA,
            ),
            cap(
                "get-header",
                "get_header",
                "Get CSV Header",
                "Extract CSV headers with type inference from the first data row.",
                GET_HEADER_INPUT_SCHEMA,
                GET_HEADER_OUTPUT_SCHEMA,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        _connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "from-csv" => from_csv(&input),
            "to-csv" => to_csv(&input),
            "get-header" => get_header(&input),
            other => Err(permanent_err(
                "UNKNOWN_CAPABILITY",
                format!("csv agent has no capability `{other}`"),
            )),
        }
    }
}

fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects: false,
        is_idempotent: true,
        rate_limited: false,
        tags: vec!["csv".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

fn from_csv(input_json: &str) -> Result<String, ErrorInfo> {
    let input: FromCsvInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let data = input.data.to_bytes()?;
    let csv_string = decode_bytes(&data, &input.encoding)?;

    let mut builder = csv::ReaderBuilder::new();
    builder
        .delimiter(input.delimiter as u8)
        .quote(input.quote_char as u8)
        .has_headers(input.use_header)
        .trim(if input.trim_whitespace {
            csv::Trim::All
        } else {
            csv::Trim::None
        });
    if let Some(esc) = input.escape_char {
        builder.escape(Some(esc as u8));
    }

    let mut reader = builder.from_reader(csv_string.as_bytes());
    let mut result = Vec::new();

    if input.use_header {
        let headers = reader
            .headers()
            .map_err(|e| permanent_err("CSV_PARSE_ERROR", format!("read headers: {e}")))?
            .clone();
        for rec in reader.records() {
            let record =
                rec.map_err(|e| permanent_err("CSV_PARSE_ERROR", format!("read record: {e}")))?;
            if input.skip_empty_lines && is_empty_record(&record) {
                continue;
            }
            let mut row_map = Map::new();
            for (i, field) in record.iter().enumerate() {
                let col = headers
                    .get(i)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| i.to_string());
                if !col.is_empty() {
                    row_map.insert(col, Value::String(field.into()));
                }
            }
            result.push(Value::Object(row_map));
        }
    } else {
        for rec in reader.records() {
            let record =
                rec.map_err(|e| permanent_err("CSV_PARSE_ERROR", format!("read record: {e}")))?;
            if input.skip_empty_lines && is_empty_record(&record) {
                continue;
            }
            let row: Vec<Value> = record.iter().map(|f| Value::String(f.into())).collect();
            result.push(Value::Array(row));
        }
    }

    serde_json::to_string(&Value::Array(result))
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn to_csv(input_json: &str) -> Result<String, ErrorInfo> {
    let input: ToCsvInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;

    let mut builder = csv::WriterBuilder::new();
    builder
        .delimiter(input.delimiter as u8)
        .quote(input.quote_char as u8);
    if let Some(esc) = input.escape_char {
        builder.escape(esc as u8);
    }

    let mut output = Vec::new();
    {
        let mut writer = builder.from_writer(&mut output);
        match &input.value {
            Value::Array(arr) => {
                if !arr.is_empty() {
                    if input.use_header {
                        let headers = get_header_names(&arr[0]);
                        writer.write_record(&headers).map_err(|e| {
                            permanent_err("CSV_WRITE_ERROR", format!("header: {e}"))
                        })?;
                    }
                    for item in arr {
                        let row = value_to_csv_row(item);
                        writer
                            .write_record(&row)
                            .map_err(|e| permanent_err("CSV_WRITE_ERROR", format!("row: {e}")))?;
                    }
                }
            }
            single => {
                if input.use_header {
                    let headers = get_header_names(single);
                    writer
                        .write_record(&headers)
                        .map_err(|e| permanent_err("CSV_WRITE_ERROR", format!("header: {e}")))?;
                }
                let row = value_to_csv_row(single);
                writer
                    .write_record(&row)
                    .map_err(|e| permanent_err("CSV_WRITE_ERROR", format!("row: {e}")))?;
            }
        }
        writer
            .flush()
            .map_err(|e| permanent_err("CSV_WRITE_ERROR", format!("flush: {e}")))?;
    }

    // Output is serialized as a JSON array of byte integers (matching the
    // legacy `Vec<u8>` serde behavior).
    serde_json::to_string(&output)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn get_header(input_json: &str) -> Result<String, ErrorInfo> {
    let input: GetHeaderInput = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    let data = input.data.to_bytes()?;
    let csv_string = decode_bytes(&data, &input.encoding)?;

    let mut builder = csv::ReaderBuilder::new();
    builder
        .delimiter(input.delimiter as u8)
        .quote(input.quote_char as u8)
        .has_headers(input.use_header)
        .trim(if input.trim_whitespace {
            csv::Trim::All
        } else {
            csv::Trim::None
        });
    if let Some(esc) = input.escape_char {
        builder.escape(Some(esc as u8));
    }

    let mut reader = builder.from_reader(csv_string.as_bytes());
    let mut result: HashMap<String, String> = HashMap::new();

    let headers: Vec<String> = if input.use_header {
        reader
            .headers()
            .map_err(|e| permanent_err("CSV_PARSE_ERROR", format!("headers: {e}")))?
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
        let first = reader
            .records()
            .next()
            .ok_or_else(|| permanent_err("CSV_EMPTY_FILE", "CSV file is empty"))?
            .map_err(|e| permanent_err("CSV_PARSE_ERROR", format!("first record: {e}")))?;
        (0..first.len())
            .map(|i| format!("Column {}", i + 1))
            .collect()
    };

    let first_data_row = reader.records().next();
    if let Some(rec) = first_data_row {
        let record = rec.map_err(|e| permanent_err("CSV_PARSE_ERROR", format!("data row: {e}")))?;
        for (i, header) in headers.iter().enumerate() {
            let inferred = record
                .get(i)
                .map(infer_type)
                .unwrap_or_else(|| "String".into());
            result.insert(header.clone(), inferred);
        }
    } else {
        for header in headers {
            result.insert(header, "String".into());
        }
    }

    serde_json::to_string(&result)
        .map_err(|e| permanent_err("OUTPUT_SERIALIZATION_ERROR", e.to_string()))
}

fn decode_bytes(data: &[u8], encoding: &str) -> Result<String, ErrorInfo> {
    match encoding.to_uppercase().as_str() {
        "UTF-8" | "UTF8" => String::from_utf8(data.to_vec())
            .map_err(|e| permanent_err("CSV_ENCODING_ERROR", format!("UTF-8: {e}"))),
        "LATIN-1" | "LATIN1" | "ISO-8859-1" | "ISO88591" | "WINDOWS-1252" | "CP1252" => {
            let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(data);
            Ok(decoded.into_owned())
        }
        _ => {
            if let Some(enc) = encoding_rs::Encoding::for_label(encoding.as_bytes()) {
                let (decoded, _, _) = enc.decode(data);
                Ok(decoded.into_owned())
            } else {
                Ok(String::from_utf8_lossy(data).into_owned())
            }
        }
    }
}

fn is_empty_record(record: &csv::StringRecord) -> bool {
    record.is_empty() || (record.len() == 1 && record.get(0).is_some_and(|s| s.trim().is_empty()))
}

fn get_header_names(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => map.keys().map(|k| k.to_string()).collect(),
        Value::Array(arr) => (1..=arr.len()).map(|i| format!("Column {i}")).collect(),
        _ => vec!["value".into()],
    }
}

fn value_to_csv_row(value: &Value) -> Vec<String> {
    match value {
        Value::Object(map) => map.values().map(value_to_string).collect(),
        Value::Array(arr) => arr.iter().map(value_to_string).collect(),
        _ => vec![value_to_string(value)],
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn infer_type(value: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(value) {
        match v {
            Value::Bool(_) => return "Boolean".into(),
            Value::Number(n) if n.is_i64() => return "Integer".into(),
            Value::Number(n) if n.is_f64() => return "Double".into(),
            _ => {}
        }
    }
    "String".into()
}

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

const CSV_DATA_SCHEMA: &str = r#"{
    "description": "Raw CSV data as bytes, base64-encoded string, or FileData object",
    "oneOf": [
        { "type": "array", "items": { "type": "integer" } },
        { "type": "string" },
        {
            "type": "object",
            "required": ["content"],
            "properties": {
                "content":   { "type": "string" },
                "filename":  { "type": "string" },
                "mime_type": { "type": "string" }
            }
        }
    ]
}"#;

const FROM_CSV_INPUT_SCHEMA: &str = const_format::concatcp!(
    r#"{
    "type": "object",
    "required": ["data"],
    "properties": {
        "data": "#,
    CSV_DATA_SCHEMA,
    r#",
        "encoding":         { "type": "string", "default": "UTF-8" },
        "delimiter":        { "type": "string", "default": "," },
        "quote_char":       { "type": "string", "default": "\"" },
        "escape_char":      { "type": "string" },
        "use_header":       { "type": "boolean", "default": true },
        "skip_empty_lines": { "type": "boolean", "default": true },
        "trim_whitespace":  { "type": "boolean", "default": false }
    }
}"#
);

const FROM_CSV_OUTPUT_SCHEMA: &str = r#"{ "type": "array", "description": "Array of row objects (with headers) or arrays (without)" }"#;

const TO_CSV_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value"],
    "properties": {
        "value":       { "description": "Data to convert to CSV (array of objects, array of arrays, or single object)" },
        "encoding":    { "type": "string", "default": "UTF-8" },
        "delimiter":   { "type": "string", "default": "," },
        "quote_char":  { "type": "string", "default": "\"" },
        "escape_char": { "type": "string" },
        "use_header":  { "type": "boolean", "default": true }
    }
}"#;

const TO_CSV_OUTPUT_SCHEMA: &str = r#"{ "type": "array", "items": { "type": "integer" }, "description": "CSV bytes (JSON array of byte integers)" }"#;

const GET_HEADER_INPUT_SCHEMA: &str = const_format::concatcp!(
    r#"{
    "type": "object",
    "required": ["data"],
    "properties": {
        "data": "#,
    CSV_DATA_SCHEMA,
    r#",
        "encoding":        { "type": "string", "default": "UTF-8" },
        "delimiter":       { "type": "string", "default": "," },
        "quote_char":      { "type": "string", "default": "\"" },
        "escape_char":     { "type": "string" },
        "use_header":      { "type": "boolean", "default": true },
        "trim_whitespace": { "type": "boolean", "default": false }
    }
}"#
);

const GET_HEADER_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "additionalProperties": { "type": "string", "enum": ["String", "Integer", "Double", "Boolean"] }
}"#;

bindings::export!(Component with_types_in bindings);
