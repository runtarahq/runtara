// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
// XLSX/XLS agents for workflow execution
//
// This module provides spreadsheet parsing operations:
// - from_xlsx: Parse spreadsheet into JSON array of objects or arrays
// - get_sheets: List sheet names and dimensions from a workbook
//
// Supports XLSX, XLS, XLSB, and ODS formats via calamine.
// All operations work with raw byte arrays, base64, or FileData.

#[allow(unused_imports)]
use base64::{Engine as _, engine::general_purpose};

use crate::types::AgentError;
pub use crate::types::FileData;
use calamine::{Data, Reader, open_workbook_auto_from_rs};
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::Value;
use std::io::Cursor;

// ============================================================================
// Input/Output Types
// ============================================================================

/// Flexible spreadsheet data input supporting raw bytes or base64 encoded file structures
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum XlsxDataInput {
    /// Raw bytes
    Bytes(Vec<u8>),
    /// File data with base64 content
    File(FileData),
    /// Plain base64 string
    Base64String(String),
}

impl XlsxDataInput {
    /// Convert any supported input into raw bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, AgentError> {
        match self {
            XlsxDataInput::Bytes(b) => Ok(b.clone()),
            XlsxDataInput::File(f) => f
                .decode()
                .map_err(|e| AgentError::permanent("XLSX_DECODE_ERROR", e.to_string())),
            XlsxDataInput::Base64String(s) => general_purpose::STANDARD.decode(s).map_err(|e| {
                AgentError::permanent(
                    "XLSX_DECODE_ERROR",
                    format!("Failed to decode base64 spreadsheet content: {}", e),
                )
            }),
        }
    }
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Parse Spreadsheet Input")]
pub struct FromXlsxInput {
    /// Spreadsheet data
    #[field(
        display_name = "Spreadsheet Data",
        description = "Spreadsheet data as bytes, base64 encoded string, or file data object"
    )]
    pub data: XlsxDataInput,

    /// Sheet to read — name or "#0" for index (default: first sheet)
    #[field(
        display_name = "Sheet",
        description = "Sheet name or index (e.g. '#0' for first sheet, '#2' for third). Default: first sheet",
        example = "Sheet1"
    )]
    #[serde(default)]
    pub sheet: Option<String>,

    /// Whether the first row contains headers (default: true)
    #[field(
        display_name = "Has Headers",
        description = "Whether the first row contains column headers",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub has_headers: bool,

    /// Skip rows that are entirely empty (default: true)
    #[field(
        display_name = "Skip Empty Rows",
        description = "Whether to skip rows where all cells are empty",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    pub skip_empty_rows: bool,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Sheets Input")]
pub struct GetSheetsInput {
    /// Spreadsheet data
    #[field(
        display_name = "Spreadsheet Data",
        description = "Spreadsheet data as bytes, base64 encoded string, or file data object"
    )]
    pub data: XlsxDataInput,
}

#[derive(Debug, Serialize, CapabilityOutput)]
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

// Default value functions
fn default_true() -> bool {
    true
}

// ============================================================================
// Operations
// ============================================================================

/// Parses a spreadsheet into a JSON array
/// - With headers: Returns array of objects (header → value)
/// - Without headers: Returns array of arrays
#[capability(
    module = "xlsx",
    module_display_name = "Spreadsheet",
    module_description = "Parse Excel and OpenDocument spreadsheets (XLSX, XLS, XLSB, ODS)",
    display_name = "Parse Spreadsheet",
    description = "Parse a spreadsheet sheet into a JSON array of objects or arrays. Supports XLSX, XLS, XLSB, and ODS formats.",
    errors(
        permanent("XLSX_DECODE_ERROR", "Failed to decode base64 or file data"),
        permanent("XLSX_PARSE_ERROR", "Failed to open or parse the spreadsheet file"),
        permanent(
            "XLSX_SHEET_NOT_FOUND",
            "The requested sheet was not found in the workbook"
        ),
    )
)]
pub fn from_xlsx(input: FromXlsxInput) -> Result<Vec<Value>, AgentError> {
    let bytes = input.data.to_bytes()?;
    let cursor = Cursor::new(bytes);

    let mut workbook = open_workbook_auto_from_rs(cursor).map_err(|e| {
        AgentError::permanent(
            "XLSX_PARSE_ERROR",
            format!("Failed to open spreadsheet: {}", e),
        )
    })?;

    // Resolve sheet name
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
    let mut result = Vec::new();

    if input.has_headers {
        // First row = headers
        let headers: Vec<String> = match rows_iter.next() {
            Some(row) => row.iter().map(cell_to_header_string).collect(),
            None => return Ok(result), // Empty sheet
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

    Ok(result)
}

/// Lists all sheets in a workbook with their dimensions
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
            Ok(range) => {
                let height = range.height();
                let width = range.width();
                (height, width)
            }
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
// Helper Functions
// ============================================================================

/// Resolve a sheet selector to a concrete sheet name.
/// - None → first sheet
/// - "#N" → sheet at index N
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

/// Convert a cell to a JSON value
fn cell_to_value(cell: &Data) -> Value {
    match cell {
        Data::Empty => Value::Null,
        Data::String(s) => Value::String(s.clone()),
        Data::Int(n) => Value::Number((*n).into()),
        Data::Float(f) => {
            // Check if it's actually a whole number
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

/// Convert a cell to a string for use as a header name
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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: create a minimal XLSX file in memory using a simple builder.
    /// For tests we use calamine to verify parsing, but we need to create test files.
    /// We'll use the `simple_excel_writer` approach via raw ZIP+XML construction.
    fn create_test_xlsx(sheets: &[(&str, &[&[&str]])]) -> Vec<u8> {
        use std::io::Write;
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        let mut buf = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buf));
            let options = SimpleFileOptions::default();

            // [Content_Types].xml
            zip.start_file("[Content_Types].xml", options).unwrap();
            let mut content_types = String::from(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
            );
            for (i, _) in sheets.iter().enumerate() {
                content_types.push_str(&format!(
                    r#"
  <Override PartName="/xl/worksheets/sheet{}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
                    i + 1
                ));
            }
            content_types.push_str("\n  <Override PartName=\"/xl/sharedStrings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml\"/>");
            content_types.push_str("\n</Types>");
            zip.write_all(content_types.as_bytes()).unwrap();

            // _rels/.rels
            zip.start_file("_rels/.rels", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
            )
            .unwrap();

            // Collect all unique strings for shared strings table
            let mut all_strings: Vec<String> = Vec::new();
            let mut string_index = std::collections::HashMap::new();
            for (_name, rows) in sheets {
                for row in *rows {
                    for cell in *row {
                        let s = cell.to_string();
                        if !string_index.contains_key(&s) {
                            string_index.insert(s.clone(), all_strings.len());
                            all_strings.push(s);
                        }
                    }
                }
            }

            // xl/sharedStrings.xml
            zip.start_file("xl/sharedStrings.xml", options).unwrap();
            let mut ss = format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{}" uniqueCount="{}">"#,
                all_strings.len(),
                all_strings.len()
            );
            for s in &all_strings {
                // Escape XML special characters
                let escaped = s
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                ss.push_str(&format!("<si><t>{}</t></si>", escaped));
            }
            ss.push_str("</sst>");
            zip.write_all(ss.as_bytes()).unwrap();

            // xl/workbook.xml
            zip.start_file("xl/workbook.xml", options).unwrap();
            let mut wb = String::from(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>"#,
            );
            for (i, (name, _)) in sheets.iter().enumerate() {
                wb.push_str(&format!(
                    r#"
    <sheet name="{}" sheetId="{}" r:id="rId{}"/>"#,
                    name,
                    i + 1,
                    i + 1
                ));
            }
            wb.push_str(
                r#"
  </sheets>
</workbook>"#,
            );
            zip.write_all(wb.as_bytes()).unwrap();

            // xl/_rels/workbook.xml.rels
            zip.start_file("xl/_rels/workbook.xml.rels", options)
                .unwrap();
            let mut wb_rels = String::from(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            );
            for (i, _) in sheets.iter().enumerate() {
                wb_rels.push_str(&format!(
                    r#"
  <Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{}.xml"/>"#,
                    i + 1,
                    i + 1
                ));
            }
            wb_rels.push_str(&format!(
                r#"
  <Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>"#,
                sheets.len() + 1
            ));
            wb_rels.push_str("\n</Relationships>");
            zip.write_all(wb_rels.as_bytes()).unwrap();

            // xl/worksheets/sheet{N}.xml
            for (i, (_name, rows)) in sheets.iter().enumerate() {
                zip.start_file(format!("xl/worksheets/sheet{}.xml", i + 1), options)
                    .unwrap();

                let mut sheet_xml = String::from(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>"#,
                );

                for (row_idx, row) in rows.iter().enumerate() {
                    sheet_xml.push_str(&format!("\n    <row r=\"{}\">", row_idx + 1));
                    for (col_idx, cell) in row.iter().enumerate() {
                        let col_letter = col_index_to_letter(col_idx);
                        let cell_ref = format!("{}{}", col_letter, row_idx + 1);
                        let si = string_index[&cell.to_string()];
                        sheet_xml
                            .push_str(&format!("<c r=\"{}\" t=\"s\"><v>{}</v></c>", cell_ref, si));
                    }
                    sheet_xml.push_str("</row>");
                }

                sheet_xml.push_str(
                    r#"
  </sheetData>
</worksheet>"#,
                );
                zip.write_all(sheet_xml.as_bytes()).unwrap();
            }

            zip.finish().unwrap();
        }
        buf
    }

    fn col_index_to_letter(idx: usize) -> String {
        let mut result = String::new();
        let mut n = idx;
        loop {
            result.insert(0, (b'A' + (n % 26) as u8) as char);
            if n < 26 {
                break;
            }
            n = n / 26 - 1;
        }
        result
    }

    #[test]
    fn test_from_xlsx_with_headers() {
        let xlsx = create_test_xlsx(&[(
            "Sheet1",
            &[
                &["name", "age", "city"],
                &["Alice", "30", "NYC"],
                &["Bob", "25", "LA"],
            ],
        )]);
        let input = FromXlsxInput {
            data: XlsxDataInput::Bytes(xlsx),
            sheet: None,
            has_headers: true,
            skip_empty_rows: true,
        };

        let result = from_xlsx(input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["name"], "Alice");
        assert_eq!(result[0]["age"], "30");
        assert_eq!(result[0]["city"], "NYC");
        assert_eq!(result[1]["name"], "Bob");
    }

    #[test]
    fn test_from_xlsx_without_headers() {
        let xlsx = create_test_xlsx(&[("Sheet1", &[&["Alice", "30"], &["Bob", "25"]])]);
        let input = FromXlsxInput {
            data: XlsxDataInput::Bytes(xlsx),
            sheet: None,
            has_headers: false,
            skip_empty_rows: true,
        };

        let result = from_xlsx(input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], json!(["Alice", "30"]));
        assert_eq!(result[1], json!(["Bob", "25"]));
    }

    #[test]
    fn test_from_xlsx_sheet_by_name() {
        let xlsx = create_test_xlsx(&[
            ("Products", &[&["id", "name"], &["1", "Widget"]]),
            ("Orders", &[&["id", "total"], &["100", "59.99"]]),
        ]);
        let input = FromXlsxInput {
            data: XlsxDataInput::Bytes(xlsx),
            sheet: Some("Orders".to_string()),
            has_headers: true,
            skip_empty_rows: true,
        };

        let result = from_xlsx(input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["id"], "100");
        assert_eq!(result[0]["total"], "59.99");
    }

    #[test]
    fn test_from_xlsx_sheet_by_index() {
        let xlsx = create_test_xlsx(&[
            ("First", &[&["a", "b"], &["1", "2"]]),
            ("Second", &[&["x", "y"], &["3", "4"]]),
        ]);
        let input = FromXlsxInput {
            data: XlsxDataInput::Bytes(xlsx),
            sheet: Some("#1".to_string()),
            has_headers: true,
            skip_empty_rows: true,
        };

        let result = from_xlsx(input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["x"], "3");
        assert_eq!(result[0]["y"], "4");
    }

    #[test]
    fn test_from_xlsx_sheet_not_found() {
        let xlsx = create_test_xlsx(&[("Sheet1", &[&["a"], &["1"]])]);
        let input = FromXlsxInput {
            data: XlsxDataInput::Bytes(xlsx),
            sheet: Some("NonExistent".to_string()),
            has_headers: true,
            skip_empty_rows: true,
        };

        let err = from_xlsx(input).unwrap_err();
        assert_eq!(err.code, "XLSX_SHEET_NOT_FOUND");
        assert!(err.message.contains("NonExistent"));
    }

    #[test]
    fn test_from_xlsx_sheet_index_out_of_range() {
        let xlsx = create_test_xlsx(&[("Sheet1", &[&["a"], &["1"]])]);
        let input = FromXlsxInput {
            data: XlsxDataInput::Bytes(xlsx),
            sheet: Some("#5".to_string()),
            has_headers: true,
            skip_empty_rows: true,
        };

        let err = from_xlsx(input).unwrap_err();
        assert_eq!(err.code, "XLSX_SHEET_NOT_FOUND");
    }

    #[test]
    fn test_from_xlsx_base64_input() {
        let xlsx = create_test_xlsx(&[("Sheet1", &[&["name"], &["Alice"]])]);
        let encoded = general_purpose::STANDARD.encode(&xlsx);
        let input = FromXlsxInput {
            data: XlsxDataInput::Base64String(encoded),
            sheet: None,
            has_headers: true,
            skip_empty_rows: true,
        };

        let result = from_xlsx(input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "Alice");
    }

    #[test]
    fn test_from_xlsx_file_data_input() {
        let xlsx = create_test_xlsx(&[("Sheet1", &[&["name"], &["Bob"]])]);
        let file_data = FileData::from_bytes(xlsx, Some("test.xlsx".to_string()), None);
        let input = FromXlsxInput {
            data: XlsxDataInput::File(file_data),
            sheet: None,
            has_headers: true,
            skip_empty_rows: true,
        };

        let result = from_xlsx(input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "Bob");
    }

    #[test]
    fn test_from_xlsx_empty_sheet() {
        let xlsx = create_test_xlsx(&[("Empty", &[])]);
        let input = FromXlsxInput {
            data: XlsxDataInput::Bytes(xlsx),
            sheet: None,
            has_headers: true,
            skip_empty_rows: true,
        };

        let result = from_xlsx(input).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_from_xlsx_invalid_data() {
        let input = FromXlsxInput {
            data: XlsxDataInput::Bytes(b"not a spreadsheet".to_vec()),
            sheet: None,
            has_headers: true,
            skip_empty_rows: true,
        };

        let err = from_xlsx(input).unwrap_err();
        assert_eq!(err.code, "XLSX_PARSE_ERROR");
    }

    #[test]
    fn test_get_sheets() {
        let xlsx = create_test_xlsx(&[
            (
                "Products",
                &[&["id", "name"], &["1", "Widget"], &["2", "Gadget"]],
            ),
            ("Orders", &[&["id"], &["100"]]),
        ]);
        let input = GetSheetsInput {
            data: XlsxDataInput::Bytes(xlsx),
        };

        let result = get_sheets(input).unwrap();
        assert_eq!(result.len(), 2);

        assert_eq!(result[0].name, "Products");
        assert_eq!(result[0].index, 0);
        assert_eq!(result[0].rows, 3);
        assert_eq!(result[0].columns, 2);

        assert_eq!(result[1].name, "Orders");
        assert_eq!(result[1].index, 1);
        assert_eq!(result[1].rows, 2);
        assert_eq!(result[1].columns, 1);
    }

    #[test]
    fn test_get_sheets_invalid_data() {
        let input = GetSheetsInput {
            data: XlsxDataInput::Bytes(b"garbage".to_vec()),
        };

        let err = get_sheets(input).unwrap_err();
        assert_eq!(err.code, "XLSX_PARSE_ERROR");
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_cell_to_value_types() {
        assert_eq!(cell_to_value(&Data::Empty), Value::Null);
        assert_eq!(cell_to_value(&Data::String("hello".into())), json!("hello"));
        assert_eq!(cell_to_value(&Data::Int(42)), json!(42));
        assert_eq!(cell_to_value(&Data::Float(3.14)), json!(3.14));
        assert_eq!(cell_to_value(&Data::Bool(true)), json!(true));
        // Whole-number floats become integers
        assert_eq!(cell_to_value(&Data::Float(100.0)), json!(100));
    }

    #[test]
    fn test_col_index_to_letter() {
        assert_eq!(col_index_to_letter(0), "A");
        assert_eq!(col_index_to_letter(1), "B");
        assert_eq!(col_index_to_letter(25), "Z");
        assert_eq!(col_index_to_letter(26), "AA");
        assert_eq!(col_index_to_letter(27), "AB");
    }
}
