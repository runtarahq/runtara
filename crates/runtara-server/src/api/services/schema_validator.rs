//! Schema Validation for Dynamic Object Model
//!
//! Validates schema definitions including table names, column names, and data types
//! to ensure safety and PostgreSQL compatibility.

use crate::api::dto::object_model::{ColumnDefinition, IndexDefinition};
use runtara_object_store::sql::validate_identifier;
use std::collections::HashSet;

/// SMO-specific auto-managed column names that users cannot define
const AUTO_MANAGED_COLUMNS: &[&str] = &["id", "created_at", "updated_at", "deleted", "tenant_id"];

/// Schema validation errors
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Invalid table name: {0}")]
    InvalidTableName(String),

    #[error("Invalid column name: {0}")]
    InvalidColumnName(String),

    #[error("Unsupported column type: {0}")]
    UnsupportedType(String),

    #[error("Duplicate column name: {0}")]
    DuplicateColumn(String),

    #[error("Schema must have at least one user-defined column")]
    NoColumns,

    #[error("Invalid index definition: {0}")]
    InvalidIndex(String),

    #[error("Index references non-existent column: {0}")]
    IndexColumnNotFound(String),
}

/// Schema validator
pub struct SchemaValidator;

impl SchemaValidator {
    /// Validate a complete schema definition
    ///
    /// # Arguments
    /// * `table_name` - User-defined table name
    /// * `columns` - User-defined column definitions
    /// * `indexes` - Optional index definitions
    ///
    /// # Returns
    /// Ok(()) if valid, Err with validation details if invalid
    pub fn validate_schema(
        table_name: &str,
        columns: &[ColumnDefinition],
        indexes: &Option<Vec<IndexDefinition>>,
    ) -> Result<(), ValidationError> {
        // Validate table name
        Self::validate_table_name(table_name)?;

        // Must have at least one column
        if columns.is_empty() {
            return Err(ValidationError::NoColumns);
        }

        // Validate each column
        let mut column_names = HashSet::new();
        for col in columns {
            Self::validate_column(col, &mut column_names)?;
        }

        // Validate indexes
        if let Some(idx_list) = indexes {
            for idx in idx_list {
                Self::validate_index(idx, columns)?;
            }
        }

        Ok(())
    }

    /// Validate a table name
    ///
    /// # Arguments
    /// * `name` - Table name to validate
    ///
    /// # Returns
    /// Ok(()) if valid, Err with details if invalid
    pub fn validate_table_name(name: &str) -> Result<(), ValidationError> {
        validate_identifier(name, &[]).map_err(ValidationError::InvalidTableName)
    }

    /// Validate a single column definition
    ///
    /// # Arguments
    /// * `col` - Column definition
    /// * `seen_names` - Set of already-seen column names (for duplicate detection)
    ///
    /// # Returns
    /// Ok(()) if valid, Err with details if invalid
    fn validate_column(
        col: &ColumnDefinition,
        seen_names: &mut HashSet<String>,
    ) -> Result<(), ValidationError> {
        // Validate column name (check auto-managed columns)
        validate_identifier(&col.name, AUTO_MANAGED_COLUMNS)
            .map_err(ValidationError::InvalidColumnName)?;

        // Check for duplicate column names
        if !seen_names.insert(col.name.clone()) {
            return Err(ValidationError::DuplicateColumn(col.name.clone()));
        }

        // Validate column type (all ColumnType variants are valid by construction)
        // Additional validation for enum values
        if let crate::api::dto::object_model::ColumnType::Enum { values } = &col.column_type {
            if values.is_empty() {
                return Err(ValidationError::UnsupportedType(
                    "Enum type must have at least one value".to_string(),
                ));
            }
            // Validate enum values don't contain single quotes (prevent SQL injection)
            for value in values {
                if value.contains('\'') {
                    return Err(ValidationError::UnsupportedType(format!(
                        "Enum value '{}' contains invalid character: '",
                        value
                    )));
                }
            }
        }

        Ok(())
    }

    /// Validate an index definition
    ///
    /// # Arguments
    /// * `index` - Index definition
    /// * `columns` - Schema column definitions
    ///
    /// # Returns
    /// Ok(()) if valid, Err with details if invalid
    fn validate_index(
        index: &IndexDefinition,
        columns: &[ColumnDefinition],
    ) -> Result<(), ValidationError> {
        // Index must have a name
        if index.name.is_empty() {
            return Err(ValidationError::InvalidIndex(
                "Index name cannot be empty".to_string(),
            ));
        }

        // Index name must be valid identifier
        validate_identifier(&index.name, &[]).map_err(ValidationError::InvalidIndex)?;

        // Index must have at least one column
        if index.columns.is_empty() {
            return Err(ValidationError::InvalidIndex(
                "Index must reference at least one column".to_string(),
            ));
        }

        // All index columns must exist in schema
        let column_names: HashSet<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        for idx_col in &index.columns {
            if !column_names.contains(idx_col.as_str()) {
                return Err(ValidationError::IndexColumnNotFound(idx_col.clone()));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::object_model::ColumnType;

    #[test]
    fn test_validate_table_name_valid() {
        assert!(SchemaValidator::validate_table_name("products").is_ok());
        assert!(SchemaValidator::validate_table_name("my_table_123").is_ok());
    }

    #[test]
    fn test_validate_table_name_invalid() {
        assert!(SchemaValidator::validate_table_name("1products").is_err());
        assert!(SchemaValidator::validate_table_name("my-table").is_err());
        assert!(SchemaValidator::validate_table_name("select").is_err()); // reserved
    }

    #[test]
    fn test_validate_schema_valid() {
        let columns = vec![
            ColumnDefinition {
                name: "sku".to_string(),
                column_type: ColumnType::String,
                unique: true,
                nullable: false,
                default_value: None,
            },
            ColumnDefinition {
                name: "price".to_string(),
                column_type: ColumnType::Decimal {
                    precision: 10,
                    scale: 2,
                },
                unique: false,
                nullable: true,
                default_value: None,
            },
        ];

        let indexes = Some(vec![IndexDefinition {
            name: "idx_sku".to_string(),
            columns: vec!["sku".to_string()],
            unique: true,
        }]);

        assert!(SchemaValidator::validate_schema("products", &columns, &indexes).is_ok());
    }

    #[test]
    fn test_validate_schema_no_columns() {
        let columns = vec![];
        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &None),
            Err(ValidationError::NoColumns)
        ));
    }

    #[test]
    fn test_validate_schema_duplicate_column() {
        let columns = vec![
            ColumnDefinition {
                name: "sku".to_string(),
                column_type: ColumnType::String,
                unique: false,
                nullable: true,
                default_value: None,
            },
            ColumnDefinition {
                name: "sku".to_string(), // duplicate
                column_type: ColumnType::String,
                unique: false,
                nullable: true,
                default_value: None,
            },
        ];

        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &None),
            Err(ValidationError::DuplicateColumn(_))
        ));
    }

    #[test]
    fn test_validate_schema_auto_managed_column() {
        let columns = vec![ColumnDefinition {
            name: "id".to_string(), // reserved auto-managed column
            column_type: ColumnType::String,
            unique: false,
            nullable: true,
            default_value: None,
        }];

        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &None),
            Err(ValidationError::InvalidColumnName(_))
        ));
    }

    #[test]
    fn test_validate_index_column_not_found() {
        let columns = vec![ColumnDefinition {
            name: "sku".to_string(),
            column_type: ColumnType::String,
            unique: false,
            nullable: true,
            default_value: None,
        }];

        let indexes = Some(vec![IndexDefinition {
            name: "idx_price".to_string(),
            columns: vec!["price".to_string()], // doesn't exist
            unique: false,
        }]);

        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &indexes),
            Err(ValidationError::IndexColumnNotFound(_))
        ));
    }

    #[test]
    fn test_validate_index_empty_columns() {
        let columns = vec![ColumnDefinition {
            name: "sku".to_string(),
            column_type: ColumnType::String,
            unique: false,
            nullable: true,
            default_value: None,
        }];

        let indexes = Some(vec![IndexDefinition {
            name: "idx_bad".to_string(),
            columns: vec![], // empty
            unique: false,
        }]);

        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &indexes),
            Err(ValidationError::InvalidIndex(_))
        ));
    }

    #[test]
    fn test_validate_enum_empty_values() {
        let columns = vec![ColumnDefinition {
            name: "status".to_string(),
            column_type: ColumnType::Enum { values: vec![] }, // empty enum
            unique: false,
            nullable: false,
            default_value: None,
        }];

        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &None),
            Err(ValidationError::UnsupportedType(_))
        ));
    }

    #[test]
    fn test_validate_enum_invalid_characters() {
        let columns = vec![ColumnDefinition {
            name: "status".to_string(),
            column_type: ColumnType::Enum {
                values: vec!["active".to_string(), "in'active".to_string()], // contains quote
            },
            unique: false,
            nullable: false,
            default_value: None,
        }];

        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &None),
            Err(ValidationError::UnsupportedType(_))
        ));
    }
}
