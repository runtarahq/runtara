//! Object Model DTOs
//!
//! Data transfer objects for schema and instance management

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// Re-export runtara-object-store types for internal use
pub use runtara_object_store::{
    ColumnDefinition as StoreColumnDefinition, ColumnType as StoreColumnType,
    FilterRequest as StoreFilterRequest, IndexDefinition as StoreIndexDefinition,
    Instance as StoreInstance, Schema as StoreSchema,
};

// ============================================================================
// Typed Column Definitions (for dynamic schema)
// ============================================================================

/// Column type definition with validation and SQL mapping
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
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
                // For enum, we'll use TEXT with CHECK constraint
                // The CHECK constraint will be added separately in DDL
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

/// Column definition for dynamic schema
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
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

fn default_nullable() -> bool {
    true
}

/// Index definition for dynamic schema
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct IndexDefinition {
    /// Index name
    pub name: String,

    /// Columns included in the index
    pub columns: Vec<String>,

    /// Whether this is a UNIQUE index (default: false)
    #[serde(default)]
    pub unique: bool,
}

// ============================================================================
// Condition-based Filtering Structures
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Condition {
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FilterRequest {
    #[serde(default = "default_offset")]
    pub offset: i64,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    /// Fields to sort by (e.g., ["createdAt", "name"]). Supports system fields (id, createdAt, updatedAt) and schema-defined columns.
    #[serde(rename = "sortBy", skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<Vec<String>>,
    /// Sort order for each field (e.g., ["desc", "asc"]). Defaults to "asc" for unspecified fields.
    #[serde(rename = "sortOrder", skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<Vec<String>>,
}

fn default_offset() -> i64 {
    0
}

fn default_limit() -> i64 {
    100
}

// ============================================================================
// Query Parameters
// ============================================================================

/// Query parameters for object model endpoints with optional connection support
#[derive(Debug, Deserialize)]
pub struct ObjectModelQueryParams {
    #[serde(default = "default_offset")]
    pub offset: i64,
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Optional connection ID to use a specific database instead of the default.
    /// If not provided, uses the default OBJECT_MODEL_DATABASE_URL.
    #[serde(rename = "connectionId")]
    pub connection_id: Option<String>,
}

/// Query parameters for endpoints that only need connection_id (no pagination)
#[derive(Debug, Deserialize)]
pub struct ConnectionQueryParams {
    /// Optional connection ID to use a specific database instead of the default.
    /// If not provided, uses the default OBJECT_MODEL_DATABASE_URL.
    #[serde(rename = "connectionId")]
    pub connection_id: Option<String>,
}

// ============================================================================
// Schema DTOs
// ============================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Schema {
    pub id: String,
    #[serde(rename = "tenantId")]
    pub tenant_id: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "tableName")]
    pub table_name: String,
    pub columns: Vec<ColumnDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexes: Option<Vec<IndexDefinition>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateSchemaRequest {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "tableName")]
    pub table_name: String,
    pub columns: Vec<ColumnDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexes: Option<Vec<IndexDefinition>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateSchemaRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub columns: Option<Vec<ColumnDefinition>>,
    pub indexes: Option<Vec<IndexDefinition>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListSchemasResponse {
    pub success: bool,
    pub schemas: Vec<Schema>,
    #[serde(rename = "totalCount")]
    pub total_count: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GetSchemaResponse {
    pub success: bool,
    pub schema: Schema,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateSchemaResponse {
    pub success: bool,
    #[serde(rename = "schemaId")]
    pub schema_id: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateSchemaResponse {
    pub success: bool,
    pub message: String,
}

// ============================================================================
// Instance DTOs
// ============================================================================

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Instance {
    pub id: String,
    #[serde(rename = "tenantId")]
    pub tenant_id: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "schemaId")]
    pub schema_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "schemaName")]
    pub schema_name: Option<String>,
    pub properties: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateInstanceRequest {
    /// Schema ID (UUID) - use this OR schemaName
    #[serde(rename = "schemaId", skip_serializing_if = "Option::is_none")]
    pub schema_id: Option<String>,
    /// Schema name - use this OR schemaId (more convenient)
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    pub properties: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateInstanceRequest {
    pub properties: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListInstancesResponse {
    pub success: bool,
    pub instances: Vec<Instance>,
    #[serde(rename = "totalCount")]
    pub total_count: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GetInstanceResponse {
    pub success: bool,
    pub instance: Instance,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateInstanceResponse {
    pub success: bool,
    #[serde(rename = "instanceId")]
    pub instance_id: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateInstanceResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkDeleteRequest {
    #[serde(rename = "instanceIds")]
    pub instance_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkDeleteResponse {
    pub success: bool,
    #[serde(rename = "deletedCount")]
    pub deleted_count: usize,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FilterInstancesResponse {
    pub success: bool,
    pub instances: Vec<Instance>,
    #[serde(rename = "totalCount")]
    pub total_count: i64,
    pub offset: i64,
    pub limit: i64,
}

// ============================================================================
// Conversions from runtara-object-store types
// ============================================================================

impl Schema {
    /// Create a Schema DTO from a runtara-object-store Schema with tenant_id
    pub fn from_store(store_schema: StoreSchema, tenant_id: String) -> Self {
        Self {
            id: store_schema.id,
            tenant_id,
            created_at: store_schema.created_at,
            updated_at: store_schema.updated_at,
            name: store_schema.name,
            description: store_schema.description,
            table_name: store_schema.table_name,
            columns: store_schema
                .columns
                .into_iter()
                .map(ColumnDefinition::from)
                .collect(),
            indexes: store_schema
                .indexes
                .map(|idxs| idxs.into_iter().map(IndexDefinition::from).collect()),
        }
    }
}

impl Instance {
    /// Create an Instance DTO from a runtara-object-store Instance with tenant_id
    pub fn from_store(store_instance: StoreInstance, tenant_id: String) -> Self {
        Self {
            id: store_instance.id,
            tenant_id,
            created_at: store_instance.created_at,
            updated_at: store_instance.updated_at,
            schema_id: store_instance.schema_id,
            schema_name: store_instance.schema_name,
            properties: store_instance.properties,
        }
    }
}

impl From<StoreColumnDefinition> for ColumnDefinition {
    fn from(col: StoreColumnDefinition) -> Self {
        Self {
            name: col.name,
            column_type: ColumnType::from(col.column_type),
            nullable: col.nullable,
            unique: col.unique,
            default_value: col.default_value,
        }
    }
}

impl From<ColumnDefinition> for StoreColumnDefinition {
    fn from(col: ColumnDefinition) -> Self {
        Self {
            name: col.name,
            column_type: StoreColumnType::from(col.column_type),
            nullable: col.nullable,
            unique: col.unique,
            default_value: col.default_value,
        }
    }
}

impl From<StoreColumnType> for ColumnType {
    fn from(ct: StoreColumnType) -> Self {
        match ct {
            StoreColumnType::String => ColumnType::String,
            StoreColumnType::Integer => ColumnType::Integer,
            StoreColumnType::Decimal { precision, scale } => {
                ColumnType::Decimal { precision, scale }
            }
            StoreColumnType::Boolean => ColumnType::Boolean,
            StoreColumnType::Timestamp => ColumnType::Timestamp,
            StoreColumnType::Json => ColumnType::Json,
            StoreColumnType::Enum { values } => ColumnType::Enum { values },
        }
    }
}

impl From<ColumnType> for StoreColumnType {
    fn from(ct: ColumnType) -> Self {
        match ct {
            ColumnType::String => StoreColumnType::String,
            ColumnType::Integer => StoreColumnType::Integer,
            ColumnType::Decimal { precision, scale } => {
                StoreColumnType::Decimal { precision, scale }
            }
            ColumnType::Boolean => StoreColumnType::Boolean,
            ColumnType::Timestamp => StoreColumnType::Timestamp,
            ColumnType::Json => StoreColumnType::Json,
            ColumnType::Enum { values } => StoreColumnType::Enum { values },
        }
    }
}

impl From<StoreIndexDefinition> for IndexDefinition {
    fn from(idx: StoreIndexDefinition) -> Self {
        Self {
            name: idx.name,
            columns: idx.columns,
            unique: idx.unique,
        }
    }
}

impl From<IndexDefinition> for StoreIndexDefinition {
    fn from(idx: IndexDefinition) -> Self {
        Self {
            name: idx.name,
            columns: idx.columns,
            unique: idx.unique,
        }
    }
}

impl From<CreateSchemaRequest> for runtara_object_store::CreateSchemaRequest {
    fn from(req: CreateSchemaRequest) -> Self {
        Self {
            name: req.name,
            description: req.description,
            table_name: req.table_name,
            columns: req.columns.into_iter().map(|c| c.into()).collect(),
            indexes: req
                .indexes
                .map(|idxs| idxs.into_iter().map(|i| i.into()).collect()),
        }
    }
}

impl From<UpdateSchemaRequest> for runtara_object_store::UpdateSchemaRequest {
    fn from(req: UpdateSchemaRequest) -> Self {
        Self {
            name: req.name,
            description: req.description,
            columns: req
                .columns
                .map(|cols| cols.into_iter().map(|c| c.into()).collect()),
            indexes: req
                .indexes
                .map(|idxs| idxs.into_iter().map(|i| i.into()).collect()),
        }
    }
}

impl From<Condition> for runtara_object_store::Condition {
    fn from(cond: Condition) -> Self {
        Self {
            op: cond.op,
            arguments: cond.arguments,
        }
    }
}

impl From<FilterRequest> for StoreFilterRequest {
    fn from(req: FilterRequest) -> Self {
        Self {
            offset: req.offset,
            limit: req.limit,
            condition: req.condition.map(|c| c.into()),
            sort_by: req.sort_by,
            sort_order: req.sort_order,
        }
    }
}
