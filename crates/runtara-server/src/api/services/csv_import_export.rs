//! CSV Import/Export Service
//!
//! Business logic for exporting object model instances as CSV
//! and importing CSV data into object model schemas.

use crate::api::dto::csv_import_export::*;
use crate::api::dto::object_model::{ColumnDefinition, ColumnType};
use crate::api::repositories::object_model::ObjectStoreManager;
use crate::api::services::object_model::{ServiceError, get_store};
use runtara_connections::ConnectionsFacade;
use runtara_object_store::FilterRequest as StoreFilterRequest;
use std::collections::HashMap;
use std::sync::Arc;

/// Page size for paginating through instances during export
const EXPORT_PAGE_SIZE: i64 = 1000;

// ============================================================================
// CSV Export
// ============================================================================

/// Export instances as CSV bytes.
///
/// Fetches all matching instances (paginated), selects requested columns,
/// and serializes to CSV format.
pub async fn export_csv(
    manager: &ObjectStoreManager,
    facade: &Arc<ConnectionsFacade>,
    tenant_id: &str,
    schema_name: &str,
    request: CsvExportRequest,
    connection_id: Option<&str>,
) -> Result<Vec<u8>, ServiceError> {
    let store = get_store(manager, Some(facade.as_ref()), connection_id, tenant_id).await?;

    // Fetch schema to get column definitions
    let schema = store
        .get_schema(schema_name)
        .await
        .map_err(|e| ServiceError::DatabaseError(format!("Failed to get schema: {}", e)))?
        .ok_or_else(|| ServiceError::NotFound(format!("Schema '{}' not found", schema_name)))?;

    let columns: Vec<ColumnDefinition> = schema
        .columns
        .into_iter()
        .map(ColumnDefinition::from)
        .collect();

    // Determine which columns to export. Generated columns (e.g. tsvector)
    // are skipped: their printed form is internal noise, not user data.
    let export_columns: Vec<&str> = match &request.columns {
        Some(selected) => {
            // Validate all requested columns exist and aren't generated.
            for col_name in selected {
                let col = columns.iter().find(|c| c.name == *col_name);
                match col {
                    None => {
                        return Err(ServiceError::ValidationError(format!(
                            "Column '{}' not found in schema '{}'",
                            col_name, schema_name
                        )));
                    }
                    Some(c)
                        if matches!(
                            c.column_type,
                            crate::api::dto::object_model::ColumnType::Tsvector { .. }
                        ) =>
                    {
                        return Err(ServiceError::ValidationError(format!(
                            "Column '{}' is a generated tsvector and cannot be exported",
                            col_name
                        )));
                    }
                    _ => {}
                }
            }
            selected.iter().map(|s| s.as_str()).collect()
        }
        None => columns
            .iter()
            .filter(|c| {
                !matches!(
                    c.column_type,
                    crate::api::dto::object_model::ColumnType::Tsvector { .. }
                )
            })
            .map(|c| c.name.as_str())
            .collect(),
    };

    // Build CSV header
    let mut header: Vec<String> = Vec::new();
    if request.include_system_columns {
        header.extend_from_slice(&[
            "id".to_string(),
            "created_at".to_string(),
            "updated_at".to_string(),
        ]);
    }
    header.extend(export_columns.iter().map(|s| s.to_string()));

    let mut writer = csv::Writer::from_writer(Vec::new());
    writer
        .write_record(&header)
        .map_err(|e| ServiceError::DatabaseError(format!("CSV write error: {}", e)))?;

    // Paginate through all matching rows
    let mut offset: i64 = 0;
    loop {
        let filter = StoreFilterRequest {
            offset,
            limit: EXPORT_PAGE_SIZE,
            condition: request.condition.clone().map(|c| c.into()),
            sort_by: request.sort_by.clone(),
            sort_order: request.sort_order.clone(),
            score_expression: None,
            order_by: None,
        };

        let (instances, _total_count) =
            store
                .filter_instances(schema_name, filter)
                .await
                .map_err(|e| {
                    ServiceError::DatabaseError(format!("Failed to fetch instances: {}", e))
                })?;

        let page_len = instances.len() as i64;

        for instance in &instances {
            let mut row: Vec<String> = Vec::new();

            if request.include_system_columns {
                row.push(instance.id.clone());
                row.push(instance.created_at.clone());
                row.push(instance.updated_at.clone());
            }

            for col_name in &export_columns {
                let value = instance.properties.get(*col_name);
                row.push(json_value_to_csv_string(value));
            }

            writer
                .write_record(&row)
                .map_err(|e| ServiceError::DatabaseError(format!("CSV write error: {}", e)))?;
        }

        offset += page_len;
        if page_len < EXPORT_PAGE_SIZE {
            break;
        }
    }

    writer
        .into_inner()
        .map_err(|e| ServiceError::DatabaseError(format!("CSV flush error: {}", e)))
}

/// Convert a JSON value to its CSV string representation.
fn json_value_to_csv_string(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(v @ serde_json::Value::Object(_)) | Some(v @ serde_json::Value::Array(_)) => {
            serde_json::to_string(v).unwrap_or_default()
        }
    }
}

// ============================================================================
// Import Preview
// ============================================================================

/// Parse CSV headers and sample rows, auto-suggest column mappings.
pub async fn preview_import(
    manager: &ObjectStoreManager,
    facade: &Arc<ConnectionsFacade>,
    tenant_id: &str,
    schema_name: &str,
    csv_data: &[u8],
    connection_id: Option<&str>,
) -> Result<ImportPreviewResponse, ServiceError> {
    let store = get_store(manager, Some(facade.as_ref()), connection_id, tenant_id).await?;

    // Fetch schema
    let schema = store
        .get_schema(schema_name)
        .await
        .map_err(|e| ServiceError::DatabaseError(format!("Failed to get schema: {}", e)))?
        .ok_or_else(|| ServiceError::NotFound(format!("Schema '{}' not found", schema_name)))?;

    let columns: Vec<ColumnDefinition> = schema
        .columns
        .into_iter()
        .map(ColumnDefinition::from)
        .collect();

    // Parse CSV
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(csv_data);

    let csv_headers: Vec<String> = reader
        .headers()
        .map_err(|e| ServiceError::ValidationError(format!("Failed to parse CSV headers: {}", e)))?
        .iter()
        .map(|s| s.to_string())
        .collect();

    if csv_headers.is_empty() {
        return Err(ServiceError::ValidationError(
            "CSV file has no headers".to_string(),
        ));
    }

    // Read sample rows (up to 5)
    let mut sample_rows: Vec<Vec<String>> = Vec::new();
    let mut total_rows: usize = 0;
    for result in reader.records() {
        let record =
            result.map_err(|e| ServiceError::ValidationError(format!("CSV parse error: {}", e)))?;
        total_rows += 1;
        if sample_rows.len() < 5 {
            sample_rows.push(record.iter().map(|s| s.to_string()).collect());
        }
    }

    // Build schema column info
    let schema_columns: Vec<SchemaColumnInfo> = columns
        .iter()
        .map(|col| SchemaColumnInfo {
            name: col.name.clone(),
            column_type: column_type_name(&col.column_type),
            nullable: col.nullable,
            unique: col.unique,
        })
        .collect();

    // Collect unique columns (from column-level UNIQUE constraints and unique indexes)
    let mut unique_columns: Vec<String> = columns
        .iter()
        .filter(|col| col.unique)
        .map(|col| col.name.clone())
        .collect();

    // Also include columns from single-column unique indexes
    if let Some(indexes) = &schema.indexes {
        for idx in indexes {
            if idx.unique && idx.columns.len() == 1 {
                let col_name = &idx.columns[0];
                if !unique_columns.contains(col_name) {
                    unique_columns.push(col_name.clone());
                }
            }
        }
    }

    // Auto-suggest mappings
    let suggested_mappings = suggest_column_mappings(&csv_headers, &columns);

    Ok(ImportPreviewResponse {
        success: true,
        csv_headers,
        sample_rows,
        schema_columns,
        suggested_mappings,
        unique_columns,
        total_rows,
    })
}

/// Get a human-readable type name for a column type.
fn column_type_name(ct: &ColumnType) -> String {
    match ct {
        ColumnType::String => "string".to_string(),
        ColumnType::Integer => "integer".to_string(),
        ColumnType::Decimal { .. } => "decimal".to_string(),
        ColumnType::Boolean => "boolean".to_string(),
        ColumnType::Timestamp => "timestamp".to_string(),
        ColumnType::Json => "json".to_string(),
        ColumnType::Enum { .. } => "enum".to_string(),
        ColumnType::Tsvector { .. } => "tsvector".to_string(),
    }
}

/// Auto-suggest column mappings from CSV headers to schema columns.
///
/// Strategy:
/// 1. Case-insensitive exact match
/// 2. Normalized match (replace spaces/hyphens/dots with underscores, lowercase)
fn suggest_column_mappings(
    csv_headers: &[String],
    columns: &[ColumnDefinition],
) -> HashMap<String, Option<String>> {
    let mut mappings = HashMap::new();

    for header in csv_headers {
        let normalized_header = normalize_column_name(header);

        let matched = columns.iter().find(|col| {
            // Exact case-insensitive match
            col.name.eq_ignore_ascii_case(header)
            // Normalized match
            || normalize_column_name(&col.name) == normalized_header
        });

        mappings.insert(header.clone(), matched.map(|col| col.name.clone()));
    }

    mappings
}

/// Normalize a column name for fuzzy matching:
/// replace spaces, hyphens, dots with underscores, then lowercase.
fn normalize_column_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ' ' | '-' | '.' => '_',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

// ============================================================================
// Import
// ============================================================================

/// Import CSV data into a schema.
///
/// Validates all rows first (atomic), then bulk inserts or upserts.
pub async fn import_csv(
    manager: &ObjectStoreManager,
    facade: &Arc<ConnectionsFacade>,
    tenant_id: &str,
    schema_name: &str,
    csv_data: &[u8],
    config: CsvImportConfig,
    connection_id: Option<&str>,
) -> Result<CsvImportResponse, CsvImportError> {
    // Validate mode
    if config.mode != "create" && config.mode != "upsert" {
        return Err(CsvImportError::Service(ServiceError::ValidationError(
            format!(
                "Invalid mode '{}'. Must be 'create' or 'upsert'",
                config.mode
            ),
        )));
    }

    let skip_invalid = config.on_error == "skip";

    if config.mode == "upsert" {
        match &config.conflict_columns {
            None => {
                return Err(CsvImportError::Service(ServiceError::ValidationError(
                    "conflictColumns is required for upsert mode".to_string(),
                )));
            }
            Some(cols) if cols.is_empty() => {
                return Err(CsvImportError::Service(ServiceError::ValidationError(
                    "conflictColumns must not be empty for upsert mode".to_string(),
                )));
            }
            _ => {}
        }
    }

    let store = get_store(manager, Some(facade.as_ref()), connection_id, tenant_id)
        .await
        .map_err(CsvImportError::Service)?;

    // Fetch schema
    let schema = store
        .get_schema(schema_name)
        .await
        .map_err(|e| {
            CsvImportError::Service(ServiceError::DatabaseError(format!(
                "Failed to get schema: {}",
                e
            )))
        })?
        .ok_or_else(|| {
            CsvImportError::Service(ServiceError::NotFound(format!(
                "Schema '{}' not found",
                schema_name
            )))
        })?;

    let columns: Vec<ColumnDefinition> = schema
        .columns
        .into_iter()
        .map(ColumnDefinition::from)
        .collect();

    // Build a lookup from schema column name → ColumnDefinition
    let column_map: HashMap<&str, &ColumnDefinition> =
        columns.iter().map(|c| (c.name.as_str(), c)).collect();

    // Validate that all mapping targets exist in the schema
    for (csv_header, schema_col) in &config.column_mapping {
        if !column_map.contains_key(schema_col.as_str()) {
            return Err(CsvImportError::Service(ServiceError::ValidationError(
                format!(
                    "Mapping target '{}' (from CSV header '{}') not found in schema",
                    schema_col, csv_header
                ),
            )));
        }
    }

    // Validate conflict columns exist in schema (for upsert)
    if let Some(conflict_cols) = &config.conflict_columns {
        for col in conflict_cols {
            if !column_map.contains_key(col.as_str()) {
                return Err(CsvImportError::Service(ServiceError::ValidationError(
                    format!("Conflict column '{}' not found in schema", col),
                )));
            }
        }
    }

    // Parse CSV
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(csv_data);

    let csv_headers: Vec<String> = reader
        .headers()
        .map_err(|e| {
            CsvImportError::Service(ServiceError::ValidationError(format!(
                "Failed to parse CSV headers: {}",
                e
            )))
        })?
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Build index mapping: for each CSV column index, the target schema column (if mapped)
    let header_to_schema: Vec<Option<&str>> = csv_headers
        .iter()
        .map(|h| config.column_mapping.get(h).map(|s| s.as_str()))
        .collect();

    // Parse and validate all rows
    let mut instances: Vec<serde_json::Value> = Vec::new();
    let mut errors: Vec<CsvValidationError> = Vec::new();
    let mut failed_row_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (row_idx, result) in reader.records().enumerate() {
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                errors.push(CsvValidationError {
                    row: row_idx + 1,
                    column: String::new(),
                    error: format!("CSV parse error: {}", e),
                });
                failed_row_indices.insert(row_idx);
                continue;
            }
        };

        let mut obj = serde_json::Map::new();
        let mut row_has_error = false;

        for (col_idx, field_value) in record.iter().enumerate() {
            let Some(schema_col_name) = header_to_schema.get(col_idx).copied().flatten() else {
                // This CSV column is not mapped, skip
                continue;
            };

            let col_def = column_map[schema_col_name];
            match csv_string_to_json_value(field_value, col_def) {
                Ok(json_val) => {
                    obj.insert(schema_col_name.to_string(), json_val);
                }
                Err(err_msg) => {
                    row_has_error = true;
                    errors.push(CsvValidationError {
                        row: row_idx + 1,
                        column: schema_col_name.to_string(),
                        error: err_msg,
                    });
                }
            }
        }

        // Check for non-nullable columns without default that are missing from the row
        for (col_name, col_def) in &column_map {
            if !col_def.nullable && col_def.default_value.is_none() && !obj.contains_key(*col_name)
            {
                // Check if this column is even mapped
                let is_mapped = config.column_mapping.values().any(|v| v == *col_name);
                if is_mapped {
                    row_has_error = true;
                    errors.push(CsvValidationError {
                        row: row_idx + 1,
                        column: col_name.to_string(),
                        error: "Required column is missing from CSV row".to_string(),
                    });
                }
            }
        }

        if row_has_error {
            failed_row_indices.insert(row_idx);
        }

        instances.push(serde_json::Value::Object(obj));
    }

    if !errors.is_empty() {
        if skip_invalid {
            // Remove invalid rows (iterate in reverse to preserve indices)
            let mut sorted_indices: Vec<usize> = failed_row_indices.iter().copied().collect();
            sorted_indices.sort_unstable_by(|a, b| b.cmp(a));
            for idx in sorted_indices {
                if idx < instances.len() {
                    instances.remove(idx);
                }
            }
        } else {
            let error_count = errors.len();
            let row_count = failed_row_indices.len();
            return Err(CsvImportError::Validation(
                CsvImportValidationErrorResponse {
                    success: false,
                    error: format!(
                        "CSV validation failed: {} error(s) in {} row(s)",
                        error_count, row_count
                    ),
                    validation_errors: errors,
                },
            ));
        }
    }

    let skipped_count = failed_row_indices.len() as i64;

    if instances.is_empty() {
        return Ok(CsvImportResponse {
            success: true,
            affected_rows: 0,
            mode: config.mode,
            message: if skipped_count > 0 {
                format!("No valid rows to import ({} skipped)", skipped_count)
            } else {
                "No rows to import".to_string()
            },
            skipped_rows: if skip_invalid {
                Some(skipped_count)
            } else {
                None
            },
            validation_errors: if skip_invalid && !errors.is_empty() {
                Some(errors)
            } else {
                None
            },
        });
    }

    let row_count = instances.len();

    // Execute bulk operation
    let affected_rows = match config.mode.as_str() {
        "upsert" => {
            let conflict_columns = config.conflict_columns.unwrap_or_default();
            store
                .upsert_instances(schema_name, instances, conflict_columns)
                .await
                .map_err(|e| {
                    CsvImportError::Service(ServiceError::DatabaseError(format!(
                        "Upsert failed: {}",
                        e
                    )))
                })?
        }
        _ => store
            .create_instances(schema_name, instances)
            .await
            .map_err(|e| {
                CsvImportError::Service(ServiceError::DatabaseError(format!(
                    "Bulk create failed: {}",
                    e
                )))
            })?,
    };

    Ok(CsvImportResponse {
        success: true,
        affected_rows,
        mode: config.mode,
        message: if skipped_count > 0 {
            format!(
                "Successfully imported {} row(s) ({} affected, {} skipped)",
                row_count, affected_rows, skipped_count
            )
        } else {
            format!(
                "Successfully imported {} row(s) ({} affected)",
                row_count, affected_rows
            )
        },
        skipped_rows: if skip_invalid {
            Some(skipped_count)
        } else {
            None
        },
        validation_errors: if skip_invalid && !errors.is_empty() {
            Some(errors)
        } else {
            None
        },
    })
}

/// Convert a CSV string field to a JSON value based on the schema column type.
fn csv_string_to_json_value(
    value: &str,
    col_def: &ColumnDefinition,
) -> Result<serde_json::Value, String> {
    // Empty string handling
    if value.is_empty() {
        if col_def.nullable {
            return Ok(serde_json::Value::Null);
        }
        if col_def.default_value.is_some() {
            // The database will use the default value
            return Ok(serde_json::Value::Null);
        }
        return Err(format!(
            "Empty value for non-nullable column '{}'",
            col_def.name
        ));
    }

    match &col_def.column_type {
        ColumnType::String => Ok(serde_json::Value::String(value.to_string())),

        ColumnType::Integer => {
            // Pass as string — runtara-object-store handles string→integer coercion
            Ok(serde_json::Value::String(value.to_string()))
        }

        ColumnType::Decimal { .. } => {
            // Pass as string — runtara-object-store handles string→decimal coercion
            Ok(serde_json::Value::String(value.to_string()))
        }

        ColumnType::Boolean => match value.to_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(serde_json::Value::Bool(true)),
            "false" | "0" | "no" => Ok(serde_json::Value::Bool(false)),
            _ => Err(format!("Cannot convert '{}' to boolean", value)),
        },

        ColumnType::Timestamp => {
            // Validate ISO 8601 format
            chrono::DateTime::parse_from_rfc3339(value)
                .map(|_| serde_json::Value::String(value.to_string()))
                .map_err(|e| format!("Invalid timestamp '{}': {}", value, e))
        }

        ColumnType::Json => {
            serde_json::from_str(value).map_err(|e| format!("Invalid JSON '{}': {}", value, e))
        }

        ColumnType::Enum { values } => {
            if values.contains(&value.to_string()) {
                Ok(serde_json::Value::String(value.to_string()))
            } else {
                Err(format!(
                    "Value '{}' not in enum values: {:?}",
                    value, values
                ))
            }
        }

        ColumnType::Tsvector { .. } => {
            Err("tsvector columns are generated; they cannot be set from a CSV import".to_string())
        }
    }
}

/// Error type for import that can be either a service error or a validation error.
pub enum CsvImportError {
    Service(ServiceError),
    Validation(CsvImportValidationErrorResponse),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn string_col(name: &str, nullable: bool) -> ColumnDefinition {
        ColumnDefinition {
            name: name.to_string(),
            column_type: ColumnType::String,
            nullable,
            unique: false,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }
    }

    fn integer_col(name: &str, nullable: bool) -> ColumnDefinition {
        ColumnDefinition {
            name: name.to_string(),
            column_type: ColumnType::Integer,
            nullable,
            unique: false,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }
    }

    fn decimal_col(name: &str, nullable: bool) -> ColumnDefinition {
        ColumnDefinition {
            name: name.to_string(),
            column_type: ColumnType::Decimal {
                precision: 10,
                scale: 2,
            },
            nullable,
            unique: false,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }
    }

    fn boolean_col(name: &str, nullable: bool) -> ColumnDefinition {
        ColumnDefinition {
            name: name.to_string(),
            column_type: ColumnType::Boolean,
            nullable,
            unique: false,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }
    }

    fn timestamp_col(name: &str, nullable: bool) -> ColumnDefinition {
        ColumnDefinition {
            name: name.to_string(),
            column_type: ColumnType::Timestamp,
            nullable,
            unique: false,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }
    }

    fn enum_col(name: &str, values: Vec<&str>, nullable: bool) -> ColumnDefinition {
        ColumnDefinition {
            name: name.to_string(),
            column_type: ColumnType::Enum {
                values: values.into_iter().map(|s| s.to_string()).collect(),
            },
            nullable,
            unique: false,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }
    }

    fn json_col(name: &str, nullable: bool) -> ColumnDefinition {
        ColumnDefinition {
            name: name.to_string(),
            column_type: ColumnType::Json,
            nullable,
            unique: false,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }
    }

    // ========================================================================
    // csv_string_to_json_value tests
    // ========================================================================

    #[test]
    fn test_string_conversion() {
        let col = string_col("name", false);
        assert_eq!(
            csv_string_to_json_value("hello", &col).unwrap(),
            serde_json::Value::String("hello".to_string())
        );
    }

    #[test]
    fn test_integer_conversion() {
        let col = integer_col("count", false);
        // Integers are passed as strings for the store's coercion
        assert_eq!(
            csv_string_to_json_value("123", &col).unwrap(),
            serde_json::Value::String("123".to_string())
        );
    }

    #[test]
    fn test_decimal_conversion() {
        let col = decimal_col("price", false);
        assert_eq!(
            csv_string_to_json_value("12.34", &col).unwrap(),
            serde_json::Value::String("12.34".to_string())
        );
    }

    #[test]
    fn test_boolean_conversion() {
        let col = boolean_col("active", false);
        assert_eq!(
            csv_string_to_json_value("true", &col).unwrap(),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            csv_string_to_json_value("1", &col).unwrap(),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            csv_string_to_json_value("yes", &col).unwrap(),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            csv_string_to_json_value("false", &col).unwrap(),
            serde_json::Value::Bool(false)
        );
        assert_eq!(
            csv_string_to_json_value("0", &col).unwrap(),
            serde_json::Value::Bool(false)
        );
        assert_eq!(
            csv_string_to_json_value("no", &col).unwrap(),
            serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn test_boolean_conversion_invalid() {
        let col = boolean_col("active", false);
        assert!(csv_string_to_json_value("maybe", &col).is_err());
    }

    #[test]
    fn test_timestamp_conversion() {
        let col = timestamp_col("created", false);
        assert_eq!(
            csv_string_to_json_value("2025-01-15T10:00:00Z", &col).unwrap(),
            serde_json::Value::String("2025-01-15T10:00:00Z".to_string())
        );
    }

    #[test]
    fn test_timestamp_conversion_invalid() {
        let col = timestamp_col("created", false);
        assert!(csv_string_to_json_value("not-a-date", &col).is_err());
    }

    #[test]
    fn test_enum_conversion() {
        let col = enum_col("status", vec!["active", "inactive"], false);
        assert_eq!(
            csv_string_to_json_value("active", &col).unwrap(),
            serde_json::Value::String("active".to_string())
        );
    }

    #[test]
    fn test_enum_conversion_invalid() {
        let col = enum_col("status", vec!["active", "inactive"], false);
        assert!(csv_string_to_json_value("unknown", &col).is_err());
    }

    #[test]
    fn test_json_conversion() {
        let col = json_col("metadata", false);
        let result = csv_string_to_json_value(r#"{"key": "value"}"#, &col).unwrap();
        assert_eq!(result, serde_json::json!({"key": "value"}));
    }

    #[test]
    fn test_json_conversion_invalid() {
        let col = json_col("metadata", false);
        assert!(csv_string_to_json_value("not json", &col).is_err());
    }

    #[test]
    fn test_empty_nullable() {
        let col = string_col("name", true);
        assert_eq!(
            csv_string_to_json_value("", &col).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn test_empty_non_nullable_no_default() {
        let col = string_col("name", false);
        assert!(csv_string_to_json_value("", &col).is_err());
    }

    #[test]
    fn test_empty_non_nullable_with_default() {
        let mut col = string_col("name", false);
        col.default_value = Some("'default'".to_string());
        assert_eq!(
            csv_string_to_json_value("", &col).unwrap(),
            serde_json::Value::Null
        );
    }

    // ========================================================================
    // json_value_to_csv_string tests
    // ========================================================================

    #[test]
    fn test_json_to_csv_null() {
        assert_eq!(json_value_to_csv_string(None), "");
        assert_eq!(json_value_to_csv_string(Some(&serde_json::Value::Null)), "");
    }

    #[test]
    fn test_json_to_csv_string() {
        let v = serde_json::Value::String("hello".to_string());
        assert_eq!(json_value_to_csv_string(Some(&v)), "hello");
    }

    #[test]
    fn test_json_to_csv_number() {
        let v = serde_json::json!(42);
        assert_eq!(json_value_to_csv_string(Some(&v)), "42");
    }

    #[test]
    fn test_json_to_csv_bool() {
        let v = serde_json::json!(true);
        assert_eq!(json_value_to_csv_string(Some(&v)), "true");
    }

    #[test]
    fn test_json_to_csv_object() {
        let v = serde_json::json!({"key": "value"});
        assert_eq!(json_value_to_csv_string(Some(&v)), r#"{"key":"value"}"#);
    }

    // ========================================================================
    // suggest_column_mappings tests
    // ========================================================================

    #[test]
    fn test_suggest_exact_case_insensitive() {
        let headers = vec!["SKU".to_string(), "Name".to_string()];
        let columns = vec![string_col("sku", false), string_col("name", false)];
        let mappings = suggest_column_mappings(&headers, &columns);
        assert_eq!(mappings["SKU"], Some("sku".to_string()));
        assert_eq!(mappings["Name"], Some("name".to_string()));
    }

    #[test]
    fn test_suggest_normalized() {
        let headers = vec![
            "Product Name".to_string(),
            "Unit-Price".to_string(),
            "Stock.Count".to_string(),
        ];
        let columns = vec![
            string_col("product_name", false),
            decimal_col("unit_price", false),
            integer_col("stock_count", false),
        ];
        let mappings = suggest_column_mappings(&headers, &columns);
        assert_eq!(mappings["Product Name"], Some("product_name".to_string()));
        assert_eq!(mappings["Unit-Price"], Some("unit_price".to_string()));
        assert_eq!(mappings["Stock.Count"], Some("stock_count".to_string()));
    }

    #[test]
    fn test_suggest_no_match() {
        let headers = vec!["Unknown Column".to_string()];
        let columns = vec![string_col("sku", false)];
        let mappings = suggest_column_mappings(&headers, &columns);
        assert_eq!(mappings["Unknown Column"], None);
    }

    // ========================================================================
    // normalize_column_name tests
    // ========================================================================

    #[test]
    fn test_normalize_spaces() {
        assert_eq!(normalize_column_name("Product Name"), "product_name");
    }

    #[test]
    fn test_normalize_hyphens() {
        assert_eq!(normalize_column_name("unit-price"), "unit_price");
    }

    #[test]
    fn test_normalize_dots() {
        assert_eq!(normalize_column_name("stock.count"), "stock_count");
    }

    #[test]
    fn test_normalize_already_normalized() {
        assert_eq!(normalize_column_name("product_name"), "product_name");
    }

    // ========================================================================
    // CSV export formatting test
    // ========================================================================

    #[test]
    fn test_export_csv_formatting() {
        let mut writer = csv::Writer::from_writer(Vec::new());
        writer.write_record(["id", "name", "price"]).unwrap();
        writer.write_record(["1", "Widget", "9.99"]).unwrap();
        writer
            .write_record(["2", "Gadget with, comma", "19.99"])
            .unwrap();
        let csv_bytes = writer.into_inner().unwrap();
        let csv_string = String::from_utf8(csv_bytes).unwrap();
        assert!(csv_string.contains("id,name,price"));
        assert!(csv_string.contains("1,Widget,9.99"));
        // Commas in values should be quoted
        assert!(csv_string.contains("\"Gadget with, comma\""));
    }
}
