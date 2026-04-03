//! Core type definitions for Object Store
//!
//! Includes column types, column definitions, and index definitions.

use serde::{Deserialize, Serialize};

// ============================================================================
// Typed Column Definitions (for dynamic schema)
// ============================================================================

/// Column type definition with validation and SQL mapping
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ColumnType {
    /// Text field (unlimited length, maps to TEXT)
    String,

    /// Integer field (maps to BIGINT for 64-bit range)
    Integer,

    /// Decimal field with precision and scale (maps to NUMERIC)
    Decimal {
        /// Total number of digits (default: 19)
        #[serde(default = "default_precision")]
        precision: u8,
        /// Number of digits after decimal point (default: 4)
        #[serde(default = "default_scale")]
        scale: u8,
    },

    /// Boolean field (maps to BOOLEAN)
    Boolean,

    /// Timestamp field, always stored in UTC (maps to TIMESTAMP WITH TIME ZONE)
    Timestamp,

    /// JSON field, stored as binary JSON (maps to JSONB)
    Json,

    /// Enum field with allowed values
    Enum {
        /// List of allowed string values
        values: Vec<String>,
    },
}

fn default_precision() -> u8 {
    19
}

fn default_scale() -> u8 {
    4
}

impl ColumnType {
    /// Create a Decimal type with specified precision and scale
    pub fn decimal(precision: u8, scale: u8) -> Self {
        ColumnType::Decimal { precision, scale }
    }

    /// Convert column type to PostgreSQL type string
    pub fn to_sql_type(&self, column_name: &str) -> String {
        match self {
            ColumnType::String => "TEXT".to_string(),
            ColumnType::Integer => "BIGINT".to_string(),
            ColumnType::Decimal { precision, scale } => {
                format!("NUMERIC({},{})", precision, scale)
            }
            ColumnType::Boolean => "BOOLEAN".to_string(),
            ColumnType::Timestamp => "TIMESTAMP WITH TIME ZONE".to_string(),
            ColumnType::Json => "JSONB".to_string(),
            ColumnType::Enum { values } => {
                // For enum, we use TEXT with CHECK constraint
                format!(
                    "TEXT CHECK ({} IN ({}))",
                    column_name,
                    values
                        .iter()
                        .map(|v| format!("'{}'", v.replace("'", "''")))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }

    /// Validate that a JSON value is compatible with this column type
    pub fn validate_value(&self, value: &serde_json::Value) -> Result<(), String> {
        // Handle null values first (for all types)
        if value.is_null() {
            // Null is handled by nullable flag, not type validation
            return Ok(());
        }

        match (self, value) {
            (ColumnType::String, serde_json::Value::String(_)) => Ok(()),
            (ColumnType::Integer, serde_json::Value::Number(n)) if n.is_i64() => Ok(()),
            // Allow string-to-integer coercion (common when importing from CSV)
            (ColumnType::Integer, serde_json::Value::String(s)) => s
                .parse::<i64>()
                .map(|_| ())
                .map_err(|_| format!("Cannot convert '{}' to integer", s)),
            (ColumnType::Decimal { .. }, serde_json::Value::Number(_)) => Ok(()),
            // Allow string-to-decimal coercion (common when importing from CSV)
            (ColumnType::Decimal { .. }, serde_json::Value::String(s)) => s
                .parse::<f64>()
                .map(|_| ())
                .map_err(|_| format!("Cannot convert '{}' to decimal", s)),
            (ColumnType::Boolean, serde_json::Value::Bool(_)) => Ok(()),
            // Allow string-to-boolean coercion
            (ColumnType::Boolean, serde_json::Value::String(s)) => {
                match s.to_lowercase().as_str() {
                    "true" | "false" | "1" | "0" | "yes" | "no" => Ok(()),
                    _ => Err(format!("Cannot convert '{}' to boolean", s)),
                }
            }
            (ColumnType::Timestamp, serde_json::Value::String(s)) => {
                // Validate ISO 8601 timestamp format
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|_| ())
                    .map_err(|e| format!("Invalid timestamp format: {}", e))
            }
            (ColumnType::Json, _) => Ok(()), // Any JSON value is valid
            (ColumnType::Enum { values }, serde_json::Value::String(s)) => {
                if values.contains(s) {
                    Ok(())
                } else {
                    Err(format!("Value '{}' not in enum values: {:?}", s, values))
                }
            }
            _ => Err(format!(
                "Type mismatch: expected {:?}, got {:?}",
                self, value
            )),
        }
    }
}

fn default_nullable() -> bool {
    true
}

/// Column definition for dynamic schema
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColumnDefinition {
    /// Column name (must be valid PostgreSQL identifier)
    pub name: String,

    /// Column type with validation rules
    #[serde(flatten)]
    pub column_type: ColumnType,

    /// Whether the column allows NULL values (default: true)
    #[serde(default = "default_nullable")]
    pub nullable: bool,

    /// Whether the column has a UNIQUE constraint (default: false)
    #[serde(default)]
    pub unique: bool,

    /// Default value (SQL expression, e.g., "0", "NOW()", "'active'")
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "default")]
    pub default_value: Option<String>,
}

impl ColumnDefinition {
    /// Create a new column definition with a name and type
    pub fn new(name: impl Into<String>, column_type: ColumnType) -> Self {
        Self {
            name: name.into(),
            column_type,
            nullable: true,
            unique: false,
            default_value: None,
        }
    }

    /// Set the column as non-nullable
    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }

    /// Set the column as unique
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Set a default value
    pub fn default(mut self, value: impl Into<String>) -> Self {
        self.default_value = Some(value.into());
        self
    }
}

/// Index definition for dynamic schema
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexDefinition {
    /// Index name
    pub name: String,

    /// Columns included in the index
    pub columns: Vec<String>,

    /// Whether this is a UNIQUE index (default: false)
    #[serde(default)]
    pub unique: bool,
}

impl IndexDefinition {
    /// Create a new index definition
    pub fn new(name: impl Into<String>, columns: Vec<String>) -> Self {
        Self {
            name: name.into(),
            columns,
            unique: false,
        }
    }

    /// Set the index as unique
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // ColumnType SQL Generation Tests
    // =========================================================================

    #[test]
    fn test_column_type_string_sql() {
        assert_eq!(ColumnType::String.to_sql_type("name"), "TEXT");
    }

    #[test]
    fn test_column_type_integer_sql() {
        assert_eq!(ColumnType::Integer.to_sql_type("count"), "BIGINT");
    }

    #[test]
    fn test_column_type_decimal_sql() {
        let decimal = ColumnType::Decimal {
            precision: 10,
            scale: 2,
        };
        assert_eq!(decimal.to_sql_type("price"), "NUMERIC(10,2)");
    }

    #[test]
    fn test_column_type_decimal_default_values() {
        let decimal = ColumnType::Decimal {
            precision: default_precision(),
            scale: default_scale(),
        };
        assert_eq!(decimal.to_sql_type("amount"), "NUMERIC(19,4)");
    }

    #[test]
    fn test_column_type_boolean_sql() {
        assert_eq!(ColumnType::Boolean.to_sql_type("active"), "BOOLEAN");
    }

    #[test]
    fn test_column_type_timestamp_sql() {
        assert_eq!(
            ColumnType::Timestamp.to_sql_type("created_at"),
            "TIMESTAMP WITH TIME ZONE"
        );
    }

    #[test]
    fn test_column_type_json_sql() {
        assert_eq!(ColumnType::Json.to_sql_type("metadata"), "JSONB");
    }

    #[test]
    fn test_column_type_enum_sql() {
        let enum_type = ColumnType::Enum {
            values: vec![
                "pending".to_string(),
                "active".to_string(),
                "done".to_string(),
            ],
        };
        let sql = enum_type.to_sql_type("status");
        assert!(sql.contains("TEXT CHECK"));
        assert!(sql.contains("'pending'"));
        assert!(sql.contains("'active'"));
        assert!(sql.contains("'done'"));
    }

    #[test]
    fn test_column_type_enum_sql_escapes_quotes() {
        let enum_type = ColumnType::Enum {
            values: vec!["it's".to_string(), "normal".to_string()],
        };
        let sql = enum_type.to_sql_type("test");
        assert!(sql.contains("'it''s'")); // Escaped single quote
    }

    // =========================================================================
    // ColumnType Serialization Tests
    // =========================================================================

    #[test]
    fn test_column_type_string_serialization() {
        let col = ColumnType::String;
        let json = serde_json::to_string(&col).unwrap();
        assert_eq!(json, r#"{"type":"string"}"#);
    }

    #[test]
    fn test_column_type_decimal_serialization() {
        let col = ColumnType::Decimal {
            precision: 10,
            scale: 2,
        };
        let json = serde_json::to_string(&col).unwrap();
        assert!(json.contains("\"type\":\"decimal\""));
        assert!(json.contains("\"precision\":10"));
        assert!(json.contains("\"scale\":2"));
    }

    #[test]
    fn test_column_type_enum_serialization() {
        let col = ColumnType::Enum {
            values: vec!["a".to_string(), "b".to_string()],
        };
        let json = serde_json::to_string(&col).unwrap();
        assert!(json.contains("\"type\":\"enum\""));
        assert!(json.contains("\"values\""));
    }

    #[test]
    fn test_column_type_deserialization() {
        let json = r#"{"type":"decimal","precision":15,"scale":3}"#;
        let col: ColumnType = serde_json::from_str(json).unwrap();
        match col {
            ColumnType::Decimal { precision, scale } => {
                assert_eq!(precision, 15);
                assert_eq!(scale, 3);
            }
            _ => panic!("Expected Decimal type"),
        }
    }

    // =========================================================================
    // Value Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_string_valid() {
        let t = ColumnType::String;
        assert!(t.validate_value(&serde_json::json!("hello")).is_ok());
        assert!(t.validate_value(&serde_json::json!("")).is_ok());
        assert!(
            t.validate_value(&serde_json::json!("unicode: 日本語"))
                .is_ok()
        );
    }

    #[test]
    fn test_validate_string_invalid() {
        let t = ColumnType::String;
        assert!(t.validate_value(&serde_json::json!(123)).is_err());
        assert!(t.validate_value(&serde_json::json!(true)).is_err());
        assert!(
            t.validate_value(&serde_json::json!({"key": "value"}))
                .is_err()
        );
    }

    #[test]
    fn test_validate_integer_valid() {
        let t = ColumnType::Integer;
        assert!(t.validate_value(&serde_json::json!(0)).is_ok());
        assert!(t.validate_value(&serde_json::json!(123)).is_ok());
        assert!(t.validate_value(&serde_json::json!(-456)).is_ok());
        assert!(
            t.validate_value(&serde_json::json!(9223372036854775807_i64))
                .is_ok()
        );
    }

    #[test]
    fn test_validate_integer_coercion() {
        let t = ColumnType::Integer;
        assert!(t.validate_value(&serde_json::json!("123")).is_ok());
        assert!(t.validate_value(&serde_json::json!("-456")).is_ok());
        assert!(t.validate_value(&serde_json::json!("abc")).is_err());
        assert!(t.validate_value(&serde_json::json!("12.34")).is_err());
    }

    #[test]
    fn test_validate_decimal_valid() {
        let t = ColumnType::Decimal {
            precision: 10,
            scale: 2,
        };
        assert!(t.validate_value(&serde_json::json!(0)).is_ok());
        assert!(t.validate_value(&serde_json::json!(123.45)).is_ok());
        assert!(t.validate_value(&serde_json::json!(-99.99)).is_ok());
    }

    #[test]
    fn test_validate_decimal_coercion() {
        let t = ColumnType::Decimal {
            precision: 10,
            scale: 2,
        };
        assert!(t.validate_value(&serde_json::json!("123.45")).is_ok());
        assert!(t.validate_value(&serde_json::json!("-99.99")).is_ok());
        assert!(
            t.validate_value(&serde_json::json!("not a number"))
                .is_err()
        );
    }

    #[test]
    fn test_validate_boolean_valid() {
        let t = ColumnType::Boolean;
        assert!(t.validate_value(&serde_json::json!(true)).is_ok());
        assert!(t.validate_value(&serde_json::json!(false)).is_ok());
    }

    #[test]
    fn test_validate_boolean_coercion() {
        let t = ColumnType::Boolean;
        assert!(t.validate_value(&serde_json::json!("true")).is_ok());
        assert!(t.validate_value(&serde_json::json!("false")).is_ok());
        assert!(t.validate_value(&serde_json::json!("TRUE")).is_ok());
        assert!(t.validate_value(&serde_json::json!("FALSE")).is_ok());
        assert!(t.validate_value(&serde_json::json!("1")).is_ok());
        assert!(t.validate_value(&serde_json::json!("0")).is_ok());
        assert!(t.validate_value(&serde_json::json!("yes")).is_ok());
        assert!(t.validate_value(&serde_json::json!("no")).is_ok());
        assert!(t.validate_value(&serde_json::json!("maybe")).is_err());
    }

    #[test]
    fn test_validate_timestamp_valid() {
        let t = ColumnType::Timestamp;
        assert!(
            t.validate_value(&serde_json::json!("2024-01-15T10:30:00Z"))
                .is_ok()
        );
        assert!(
            t.validate_value(&serde_json::json!("2024-01-15T10:30:00+05:00"))
                .is_ok()
        );
    }

    #[test]
    fn test_validate_timestamp_invalid() {
        let t = ColumnType::Timestamp;
        assert!(t.validate_value(&serde_json::json!("2024-01-15")).is_err());
        assert!(t.validate_value(&serde_json::json!("not a date")).is_err());
        assert!(t.validate_value(&serde_json::json!(123456789)).is_err());
    }

    #[test]
    fn test_validate_json_accepts_any() {
        let t = ColumnType::Json;
        assert!(t.validate_value(&serde_json::json!(null)).is_ok());
        assert!(t.validate_value(&serde_json::json!("string")).is_ok());
        assert!(t.validate_value(&serde_json::json!(123)).is_ok());
        assert!(t.validate_value(&serde_json::json!(true)).is_ok());
        assert!(
            t.validate_value(&serde_json::json!({"key": "value"}))
                .is_ok()
        );
        assert!(t.validate_value(&serde_json::json!([1, 2, 3])).is_ok());
    }

    #[test]
    fn test_validate_enum_valid() {
        let t = ColumnType::Enum {
            values: vec![
                "pending".to_string(),
                "active".to_string(),
                "done".to_string(),
            ],
        };
        assert!(t.validate_value(&serde_json::json!("pending")).is_ok());
        assert!(t.validate_value(&serde_json::json!("active")).is_ok());
        assert!(t.validate_value(&serde_json::json!("done")).is_ok());
    }

    #[test]
    fn test_validate_enum_invalid() {
        let t = ColumnType::Enum {
            values: vec!["pending".to_string(), "active".to_string()],
        };
        assert!(t.validate_value(&serde_json::json!("invalid")).is_err());
        assert!(t.validate_value(&serde_json::json!("PENDING")).is_err()); // case sensitive
    }

    #[test]
    fn test_validate_null_always_ok() {
        // Null validation is handled by nullable flag, not type
        assert!(
            ColumnType::String
                .validate_value(&serde_json::json!(null))
                .is_ok()
        );
        assert!(
            ColumnType::Integer
                .validate_value(&serde_json::json!(null))
                .is_ok()
        );
        assert!(
            ColumnType::Boolean
                .validate_value(&serde_json::json!(null))
                .is_ok()
        );
    }

    // =========================================================================
    // ColumnDefinition Tests
    // =========================================================================

    #[test]
    fn test_column_definition_new() {
        let col = ColumnDefinition::new("name", ColumnType::String);
        assert_eq!(col.name, "name");
        assert!(matches!(col.column_type, ColumnType::String));
        assert!(col.nullable); // default
        assert!(!col.unique); // default
        assert!(col.default_value.is_none());
    }

    #[test]
    fn test_column_definition_not_null() {
        let col = ColumnDefinition::new("id", ColumnType::String).not_null();
        assert!(!col.nullable);
    }

    #[test]
    fn test_column_definition_unique() {
        let col = ColumnDefinition::new("email", ColumnType::String).unique();
        assert!(col.unique);
    }

    #[test]
    fn test_column_definition_default() {
        let col = ColumnDefinition::new("status", ColumnType::String).default("'active'");
        assert_eq!(col.default_value, Some("'active'".to_string()));
    }

    #[test]
    fn test_column_definition_chained_builders() {
        let col = ColumnDefinition::new("sku", ColumnType::String)
            .not_null()
            .unique()
            .default("''");

        assert_eq!(col.name, "sku");
        assert!(!col.nullable);
        assert!(col.unique);
        assert_eq!(col.default_value, Some("''".to_string()));
    }

    #[test]
    fn test_column_definition_serialization() {
        let col = ColumnDefinition::new(
            "price",
            ColumnType::Decimal {
                precision: 10,
                scale: 2,
            },
        )
        .not_null();

        let json = serde_json::to_string(&col).unwrap();
        assert!(json.contains("\"name\":\"price\""));
        assert!(json.contains("\"type\":\"decimal\""));
        assert!(json.contains("\"nullable\":false"));
    }

    #[test]
    fn test_column_definition_deserialization() {
        let json = r#"{"name":"count","type":"integer","nullable":false,"unique":true}"#;
        let col: ColumnDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(col.name, "count");
        assert!(matches!(col.column_type, ColumnType::Integer));
        assert!(!col.nullable);
        assert!(col.unique);
    }

    // =========================================================================
    // IndexDefinition Tests
    // =========================================================================

    #[test]
    fn test_index_definition_new() {
        let idx = IndexDefinition::new("idx_name", vec!["name".to_string()]);
        assert_eq!(idx.name, "idx_name");
        assert_eq!(idx.columns, vec!["name"]);
        assert!(!idx.unique);
    }

    #[test]
    fn test_index_definition_unique() {
        let idx = IndexDefinition::new("idx_email", vec!["email".to_string()]).unique();
        assert!(idx.unique);
    }

    #[test]
    fn test_index_definition_multi_column() {
        let idx = IndexDefinition::new(
            "idx_composite",
            vec!["tenant_id".to_string(), "created_at".to_string()],
        );
        assert_eq!(idx.columns.len(), 2);
    }

    #[test]
    fn test_index_definition_serialization() {
        let idx = IndexDefinition::new("idx_sku", vec!["sku".to_string()]).unique();
        let json = serde_json::to_string(&idx).unwrap();
        assert!(json.contains("\"name\":\"idx_sku\""));
        assert!(json.contains("\"unique\":true"));
    }

    #[test]
    fn test_decimal_helper() {
        let decimal = ColumnType::decimal(12, 4);
        match decimal {
            ColumnType::Decimal { precision, scale } => {
                assert_eq!(precision, 12);
                assert_eq!(scale, 4);
            }
            _ => panic!("Expected Decimal"),
        }
    }
}
