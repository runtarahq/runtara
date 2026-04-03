//! CSV Import/Export DTOs
//!
//! Data transfer objects for CSV import and export of object model instances.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

use super::object_model::Condition;

// ============================================================================
// Export DTOs
// ============================================================================

/// Request body for CSV export
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CsvExportRequest {
    /// Columns to include in export (default: all schema columns)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,

    /// Filter condition (reuses existing Condition type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,

    /// Sort fields
    #[serde(rename = "sortBy", skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<Vec<String>>,

    /// Sort order for each field ("asc" or "desc")
    #[serde(rename = "sortOrder", skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<Vec<String>>,

    /// Include system columns (id, created_at, updated_at). Default: true
    #[serde(
        rename = "includeSystemColumns",
        default = "default_true",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub include_system_columns: bool,
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Import Preview DTOs
// ============================================================================

/// JSON request body for import preview (base64-encoded CSV)
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CsvPreviewJsonRequest {
    /// Base64-encoded CSV data
    pub data: String,
}

/// Column info from the schema, returned in preview
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SchemaColumnInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub column_type: String,
    pub nullable: bool,
    /// Whether the column has a UNIQUE constraint
    #[serde(default)]
    pub unique: bool,
}

/// Response from the import preview endpoint
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ImportPreviewResponse {
    pub success: bool,
    #[serde(rename = "csvHeaders")]
    pub csv_headers: Vec<String>,
    #[serde(rename = "sampleRows")]
    pub sample_rows: Vec<Vec<String>>,
    #[serde(rename = "schemaColumns")]
    pub schema_columns: Vec<SchemaColumnInfo>,
    #[serde(rename = "suggestedMappings")]
    pub suggested_mappings: HashMap<String, Option<String>>,
    /// Columns that can be used as conflict columns for upsert mode
    /// (columns with UNIQUE constraints or belonging to unique indexes)
    #[serde(rename = "uniqueColumns")]
    pub unique_columns: Vec<String>,
    #[serde(rename = "totalRows")]
    pub total_rows: usize,
}

// ============================================================================
// Import DTOs
// ============================================================================

/// JSON request body for CSV import (base64-encoded CSV with mapping)
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CsvImportJsonRequest {
    /// Base64-encoded CSV data
    pub data: String,

    /// Column mapping: CSV header → schema column name
    #[serde(rename = "columnMapping")]
    pub column_mapping: HashMap<String, String>,

    /// Import mode: "create" or "upsert"
    #[serde(default = "default_create")]
    pub mode: String,

    /// Conflict columns for upsert mode
    #[serde(rename = "conflictColumns", skip_serializing_if = "Option::is_none")]
    pub conflict_columns: Option<Vec<String>>,

    /// Error handling: "abort" (reject all on any error) or "skip" (import valid rows, skip invalid)
    #[serde(rename = "onError", default = "default_abort")]
    pub on_error: String,
}

fn default_create() -> String {
    "create".to_string()
}

fn default_abort() -> String {
    "abort".to_string()
}

/// Import configuration (extracted from multipart or JSON request)
#[derive(Debug, Serialize, Deserialize)]
pub struct CsvImportConfig {
    /// Column mapping: CSV header → schema column name
    #[serde(rename = "columnMapping")]
    pub column_mapping: HashMap<String, String>,

    /// Import mode: "create" or "upsert"
    #[serde(default = "default_create")]
    pub mode: String,

    /// Conflict columns for upsert mode
    #[serde(rename = "conflictColumns", skip_serializing_if = "Option::is_none")]
    pub conflict_columns: Option<Vec<String>>,

    /// Error handling: "abort" (reject all on any error) or "skip" (import valid rows, skip invalid)
    #[serde(rename = "onError", default = "default_abort")]
    pub on_error: String,
}

/// Successful import response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CsvImportResponse {
    pub success: bool,
    #[serde(rename = "affectedRows")]
    pub affected_rows: i64,
    pub mode: String,
    pub message: String,
    /// Number of rows skipped due to validation errors (only present in "skip" mode)
    #[serde(rename = "skippedRows", skip_serializing_if = "Option::is_none")]
    pub skipped_rows: Option<i64>,
    /// Validation errors for skipped rows (only present in "skip" mode)
    #[serde(rename = "validationErrors", skip_serializing_if = "Option::is_none")]
    pub validation_errors: Option<Vec<CsvValidationError>>,
}

/// Per-row validation error
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CsvValidationError {
    /// 1-indexed row number (excluding header)
    pub row: usize,
    pub column: String,
    pub error: String,
}

/// Validation failure response (HTTP 400)
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CsvImportValidationErrorResponse {
    pub success: bool,
    pub error: String,
    #[serde(rename = "validationErrors")]
    pub validation_errors: Vec<CsvValidationError>,
}
