//! Schema Validation for Dynamic Object Model
//!
//! Validates schema definitions including table names, column names, and data types
//! to ensure safety and PostgreSQL compatibility.

use crate::api::dto::object_model::{ColumnDefinition, IndexDefinition};
use runtara_object_store::sql::validate_identifier;
use std::collections::HashSet;

/// Auto-managed column names that users cannot define
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

        // Cross-column validation: tsvector columns reference another column
        // on the same schema, so we have to wait until every column has been
        // visited before checking those references.
        Self::validate_cross_column(columns)?;

        // Validate indexes
        if let Some(idx_list) = indexes {
            let mut index_names = HashSet::new();
            for idx in idx_list {
                if !index_names.insert(idx.name.as_str()) {
                    return Err(ValidationError::InvalidIndex(format!(
                        "Duplicate index name: {}",
                        idx.name
                    )));
                }
                Self::validate_index(idx, columns)?;
            }
        }

        Ok(())
    }

    /// Cross-column checks that need to see every column at once.
    fn validate_cross_column(columns: &[ColumnDefinition]) -> Result<(), ValidationError> {
        for col in columns {
            if let crate::api::dto::object_model::ColumnType::Tsvector {
                source_column,
                language,
            } = &col.column_type
            {
                if language.trim().is_empty() {
                    return Err(ValidationError::UnsupportedType(format!(
                        "Tsvector column '{}' has an empty language",
                        col.name
                    )));
                }
                let src = columns.iter().find(|c| c.name == *source_column);
                match src {
                    None => {
                        return Err(ValidationError::UnsupportedType(format!(
                            "Tsvector column '{}' references unknown source column '{}'",
                            col.name, source_column
                        )));
                    }
                    Some(c) => match &c.column_type {
                        crate::api::dto::object_model::ColumnType::String
                        | crate::api::dto::object_model::ColumnType::Enum { .. } => {}
                        other => {
                            return Err(ValidationError::UnsupportedType(format!(
                                "Tsvector column '{}' source '{}' must be a string/enum column, got {:?}",
                                col.name, source_column, other
                            )));
                        }
                    },
                }
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

        // Vector columns: dimension must be in pgvector's supported range and
        // any IVFFlat index needs a positive `lists` parameter.
        if let crate::api::dto::object_model::ColumnType::Vector {
            dimension,
            index_method,
        } = &col.column_type
        {
            if *dimension == 0 || *dimension > 16000 {
                return Err(ValidationError::UnsupportedType(format!(
                    "Vector column '{}' dimension {} is out of range; pgvector supports 1..=16000",
                    col.name, dimension
                )));
            }
            if let Some(crate::api::dto::object_model::VectorIndexMethod::IvfFlat { lists }) =
                index_method
                && *lists == 0
            {
                return Err(ValidationError::UnsupportedType(format!(
                    "Vector column '{}' IVFFlat index requires lists > 0",
                    col.name
                )));
            }
        }

        // Validate text-index annotation only on string-compatible types.
        if matches!(
            col.text_index,
            crate::api::dto::object_model::TextIndexKind::Trigram
        ) {
            match &col.column_type {
                crate::api::dto::object_model::ColumnType::String
                | crate::api::dto::object_model::ColumnType::Enum { .. } => {}
                other => {
                    return Err(ValidationError::UnsupportedType(format!(
                        "Trigram text index is only supported on string/enum columns; \
                         column '{}' has type {:?}",
                        col.name, other
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
                text_index: crate::api::dto::object_model::TextIndexKind::None,
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
                text_index: crate::api::dto::object_model::TextIndexKind::None,
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
                text_index: crate::api::dto::object_model::TextIndexKind::None,
            },
            ColumnDefinition {
                name: "sku".to_string(), // duplicate
                column_type: ColumnType::String,
                unique: false,
                nullable: true,
                default_value: None,
                text_index: crate::api::dto::object_model::TextIndexKind::None,
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
            text_index: crate::api::dto::object_model::TextIndexKind::None,
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
            text_index: crate::api::dto::object_model::TextIndexKind::None,
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
    fn test_validate_duplicate_index_name() {
        let columns = vec![ColumnDefinition {
            name: "sku".to_string(),
            column_type: ColumnType::String,
            unique: false,
            nullable: true,
            default_value: None,
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }];

        let indexes = Some(vec![
            IndexDefinition {
                name: "idx_sku".to_string(),
                columns: vec!["sku".to_string()],
                unique: false,
            },
            IndexDefinition {
                name: "idx_sku".to_string(),
                columns: vec!["sku".to_string()],
                unique: true,
            },
        ]);

        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &indexes),
            Err(ValidationError::InvalidIndex(_))
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
            text_index: crate::api::dto::object_model::TextIndexKind::None,
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
            text_index: crate::api::dto::object_model::TextIndexKind::None,
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
            text_index: crate::api::dto::object_model::TextIndexKind::None,
        }];

        assert!(matches!(
            SchemaValidator::validate_schema("products", &columns, &None),
            Err(ValidationError::UnsupportedType(_))
        ));
    }
}
