// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
// CSV agents for workflow execution
//
// This module provides CSV parsing and generation operations:
// - from_csv: Parse CSV bytes into JSON array of objects
// - to_csv: Convert JSON data to CSV bytes
// - get_header: Extract CSV headers with type inference
//
// All operations work with raw byte arrays instead of files

#[allow(unused_imports)]
use base64::{Engine as _, engine::general_purpose};

use crate::types::AgentError;
pub use crate::types::FileData;
use runtara_agent_macro::{CapabilityInput, capability};
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::Value;
#[allow(unused_imports)]
use std::collections::HashMap;

// ============================================================================
// Input/Output Types
// ============================================================================

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
    pub fn to_bytes(&self) -> Result<Vec<u8>, AgentError> {
        match self {
            CsvDataInput::Bytes(b) => Ok(b.clone()),
            CsvDataInput::File(f) => f
                .decode()
                .map_err(|e| AgentError::permanent("CSV_DECODE_ERROR", e)),
            CsvDataInput::Base64String(s) => general_purpose::STANDARD.decode(s).map_err(|e| {
                AgentError::permanent(
                    "CSV_DECODE_ERROR",
                    format!("Failed to decode base64 CSV content: {}", e),
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

#[derive(Debug, Serialize)]
pub struct HeaderInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
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

// ============================================================================
// Operations
// ============================================================================

/// Parses CSV bytes into a JSON array
/// - With headers: Returns Vec<Map<String, String>>
/// - Without headers: Returns Vec<Vec<String>>
#[capability(
    module = "csv",
    display_name = "Parse CSV",
    description = "Parse CSV bytes into a JSON array of objects or arrays",
    errors(
        permanent("CSV_DECODE_ERROR", "Failed to decode base64 or file data"),
        permanent("CSV_ENCODING_ERROR", "Failed to decode with specified encoding"),
        permanent("CSV_PARSE_ERROR", "Failed to parse CSV data or read records"),
    )
)]
pub fn from_csv(input: FromCsvInput) -> Result<Vec<Value>, AgentError> {
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
                AgentError::permanent(
                    "CSV_PARSE_ERROR",
                    format!("Failed to read CSV headers: {}", e),
                )
            })?
            .clone();

        for record_result in reader.records() {
            let record = record_result.map_err(|e| {
                AgentError::permanent(
                    "CSV_PARSE_ERROR",
                    format!("Failed to read CSV record: {}", e),
                )
            })?;

            // Skip empty lines if configured
            if input.skip_empty_lines && is_empty_record(&record) {
                continue;
            }

            let mut row_map = serde_json::Map::new();
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
                AgentError::permanent(
                    "CSV_PARSE_ERROR",
                    format!("Failed to read CSV record: {}", e),
                )
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
    module = "csv",
    display_name = "Generate CSV",
    description = "Convert JSON data to CSV bytes",
    errors(permanent("CSV_WRITE_ERROR", "Failed to write CSV header or data row"),)
)]
pub fn to_csv(input: ToCsvInput) -> Result<Vec<u8>, AgentError> {
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
                            AgentError::permanent(
                                "CSV_WRITE_ERROR",
                                format!("Failed to write CSV header: {}", e),
                            )
                        })?;
                    }

                    // Write rows
                    for item in arr {
                        let row = value_to_csv_row(item);
                        writer.write_record(&row).map_err(|e| {
                            AgentError::permanent(
                                "CSV_WRITE_ERROR",
                                format!("Failed to write CSV row: {}", e),
                            )
                        })?;
                    }
                }
            }
            single_value => {
                // Single object - write as one row
                if input.use_header {
                    let headers = get_header_names(single_value);
                    writer.write_record(&headers).map_err(|e| {
                        AgentError::permanent(
                            "CSV_WRITE_ERROR",
                            format!("Failed to write CSV header: {}", e),
                        )
                    })?;
                }

                let row = value_to_csv_row(single_value);
                writer.write_record(&row).map_err(|e| {
                    AgentError::permanent(
                        "CSV_WRITE_ERROR",
                        format!("Failed to write CSV row: {}", e),
                    )
                })?;
            }
        }

        writer.flush().map_err(|e| {
            AgentError::permanent(
                "CSV_WRITE_ERROR",
                format!("Failed to flush CSV writer: {}", e),
            )
        })?;
    }
    // Writer is dropped here, releasing the borrow on output

    Ok(output)
}

/// Extracts CSV headers with type inference from the first data row
#[capability(
    module = "csv",
    display_name = "Get CSV Header",
    description = "Extract CSV headers with type inference from the first data row",
    errors(
        permanent("CSV_DECODE_ERROR", "Failed to decode base64 or file data"),
        permanent("CSV_ENCODING_ERROR", "Failed to decode with specified encoding"),
        permanent("CSV_PARSE_ERROR", "Failed to parse CSV headers or records"),
        permanent("CSV_EMPTY_FILE", "CSV file is empty"),
    )
)]
pub fn get_header(input: GetHeaderInput) -> Result<HashMap<String, String>, AgentError> {
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
    let mut result = HashMap::new();

    // Get headers
    let headers: Vec<String> = if input.use_header {
        reader
            .headers()
            .map_err(|e| {
                AgentError::permanent(
                    "CSV_PARSE_ERROR",
                    format!("Failed to read CSV headers: {}", e),
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
            .ok_or_else(|| AgentError::permanent("CSV_EMPTY_FILE", "CSV file is empty"))?
            .map_err(|e| {
                AgentError::permanent(
                    "CSV_PARSE_ERROR",
                    format!("Failed to read first record: {}", e),
                )
            })?;

        (0..first_record.len())
            .map(|i| format!("Column {}", i + 1))
            .collect()
    };

    // Get first data row for type inference
    let first_data_row = reader.records().next();

    if let Some(record_result) = first_data_row {
        let record = record_result.map_err(|e| {
            AgentError::permanent("CSV_PARSE_ERROR", format!("Failed to read data row: {}", e))
        })?;

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

// ============================================================================
// Helper Functions
// ============================================================================

/// Decodes bytes to string using specified encoding
fn decode_bytes(data: &[u8], encoding: &str) -> Result<String, AgentError> {
    match encoding.to_uppercase().as_str() {
        "UTF-8" | "UTF8" => String::from_utf8(data.to_vec()).map_err(|e| {
            AgentError::permanent(
                "CSV_ENCODING_ERROR",
                format!("Failed to decode UTF-8: {}", e),
            )
        }),
        "LATIN-1" | "LATIN1" | "ISO-8859-1" | "ISO88591" | "WINDOWS-1252" | "CP1252" => {
            // Use encoding_rs for Latin-1/Windows-1252 encoding
            let (decoded, _, had_errors) = encoding_rs::WINDOWS_1252.decode(data);
            if had_errors {
                // Even with errors, we got a result - just warn but continue
                Ok(decoded.into_owned())
            } else {
                Ok(decoded.into_owned())
            }
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
        Value::Array(arr) => (1..=arr.len()).map(|i| format!("Column {}", i)).collect(),
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
            serde_json::to_string(value).unwrap_or_else(|_| String::new())
        }
    }
}

/// Infers the type of a CSV field value
fn infer_type(value: &str) -> String {
    // Try parsing as JSON to infer type
    if let Ok(json_value) = serde_json::from_str::<Value>(value) {
        match json_value {
            Value::Bool(_) => return "Boolean".to_string(),
            Value::Number(n) => {
                if n.is_i64() {
                    return "Integer".to_string();
                } else if n.is_f64() {
                    return "Double".to_string();
                }
            }
            _ => {}
        }
    }

    "String".to_string()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_from_csv_with_headers() {
        let csv_data = b"name,age,active\nAlice,30,true\nBob,25,false";
        let input = FromCsvInput {
            data: CsvDataInput::Bytes(csv_data.to_vec()),
            encoding: "UTF-8".to_string(),
            delimiter: ',',
            quote_char: '"',
            escape_char: None,
            use_header: true,
            skip_empty_lines: true,
            trim_whitespace: false,
        };

        let result = from_csv(input).unwrap();
        assert_eq!(result.len(), 2);

        assert_eq!(result[0]["name"], "Alice");
        assert_eq!(result[0]["age"], "30");
        assert_eq!(result[0]["active"], "true");

        assert_eq!(result[1]["name"], "Bob");
        assert_eq!(result[1]["age"], "25");
    }

    #[test]
    fn test_from_csv_without_headers() {
        let csv_data = b"Alice,30\nBob,25";
        let input = FromCsvInput {
            data: CsvDataInput::Bytes(csv_data.to_vec()),
            encoding: "UTF-8".to_string(),
            delimiter: ',',
            quote_char: '"',
            escape_char: None,
            use_header: false,
            skip_empty_lines: true,
            trim_whitespace: false,
        };

        let result = from_csv(input).unwrap();
        assert_eq!(result.len(), 2);

        if let Value::Array(row1) = &result[0] {
            assert_eq!(row1[0], "Alice");
            assert_eq!(row1[1], "30");
        } else {
            panic!("Expected array");
        }
    }

    #[test]
    fn test_from_csv_custom_delimiter() {
        let csv_data = b"name;age\nAlice;30\nBob;25";
        let input = FromCsvInput {
            data: CsvDataInput::Bytes(csv_data.to_vec()),
            encoding: "UTF-8".to_string(),
            delimiter: ';',
            quote_char: '"',
            escape_char: None,
            use_header: true,
            skip_empty_lines: true,
            trim_whitespace: false,
        };

        let result = from_csv(input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["name"], "Alice");
    }

    #[test]
    fn test_from_csv_skip_empty_lines() {
        let csv_data = b"name,age\n\nAlice,30\n\nBob,25\n";
        let input = FromCsvInput {
            data: CsvDataInput::Bytes(csv_data.to_vec()),
            encoding: "UTF-8".to_string(),
            delimiter: ',',
            quote_char: '"',
            escape_char: None,
            use_header: true,
            skip_empty_lines: true,
            trim_whitespace: false,
        };

        let result = from_csv(input).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_from_csv_base64_string() {
        let csv_data = b"name,age\nAlice,30";
        let encoded = base64::engine::general_purpose::STANDARD.encode(csv_data);
        let input = FromCsvInput {
            data: CsvDataInput::Base64String(encoded),
            encoding: "UTF-8".to_string(),
            delimiter: ',',
            quote_char: '"',
            escape_char: None,
            use_header: true,
            skip_empty_lines: true,
            trim_whitespace: false,
        };

        let result = from_csv(input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "Alice");
    }

    #[test]
    fn test_to_csv_array_of_objects() {
        let data = json!([
            {"name": "Alice", "age": 30},
            {"name": "Bob", "age": 25}
        ]);

        let input = ToCsvInput {
            value: data,
            encoding: "UTF-8".to_string(),
            delimiter: ',',
            quote_char: '"',
            escape_char: None,
            use_header: true,
        };

        let result = to_csv(input).unwrap();
        let csv_string = String::from_utf8(result).unwrap();

        assert!(csv_string.contains("name"));
        assert!(csv_string.contains("age"));
        assert!(csv_string.contains("Alice"));
        assert!(csv_string.contains("30"));
    }

    #[test]
    fn test_to_csv_without_header() {
        let data = json!([
            {"name": "Alice", "age": 30}
        ]);

        let input = ToCsvInput {
            value: data,
            encoding: "UTF-8".to_string(),
            delimiter: ',',
            quote_char: '"',
            escape_char: None,
            use_header: false,
        };

        let result = to_csv(input).unwrap();
        let csv_string = String::from_utf8(result).unwrap();

        // Should not contain header row
        assert!(!csv_string.starts_with("name"));
        assert!(csv_string.contains("Alice"));
    }

    #[test]
    fn test_to_csv_custom_delimiter() {
        let data = json!([{"name": "Alice", "age": 30}]);

        let input = ToCsvInput {
            value: data,
            encoding: "UTF-8".to_string(),
            delimiter: ';',
            quote_char: '"',
            escape_char: None,
            use_header: true,
        };

        let result = to_csv(input).unwrap();
        let csv_string = String::from_utf8(result).unwrap();

        assert!(csv_string.contains(';'));
    }

    #[test]
    fn test_get_header_with_type_inference() {
        let csv_data = b"name,age,active\nAlice,30,true";
        let input = GetHeaderInput {
            data: CsvDataInput::Bytes(csv_data.to_vec()),
            encoding: "UTF-8".to_string(),
            delimiter: ',',
            quote_char: '"',
            escape_char: None,
            use_header: true,
            skip_empty_lines: true,
            trim_whitespace: false,
        };

        let result = get_header(input).unwrap();

        assert_eq!(result.get("name"), Some(&"String".to_string()));
        assert_eq!(result.get("age"), Some(&"Integer".to_string()));
        assert_eq!(result.get("active"), Some(&"Boolean".to_string()));
    }

    #[test]
    fn test_get_header_without_headers() {
        let csv_data = b"Alice,30,true\nBob,25,false";
        let input = GetHeaderInput {
            data: CsvDataInput::Bytes(csv_data.to_vec()),
            encoding: "UTF-8".to_string(),
            delimiter: ',',
            quote_char: '"',
            escape_char: None,
            use_header: false,
            skip_empty_lines: true,
            trim_whitespace: false,
        };

        let result = get_header(input).unwrap();

        // Should generate column names
        assert!(result.contains_key("Column 1"));
        assert!(result.contains_key("Column 2"));
        assert!(result.contains_key("Column 3"));
    }

    #[test]
    fn test_infer_type() {
        assert_eq!(infer_type("true"), "Boolean");
        assert_eq!(infer_type("false"), "Boolean");
        assert_eq!(infer_type("42"), "Integer");
        assert_eq!(infer_type("3.14"), "Double");
        assert_eq!(infer_type("hello"), "String");
        assert_eq!(infer_type(""), "String");
    }
}
