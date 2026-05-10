//! Object Model DTOs
//!
//! Data transfer objects for schema and instance management

use schemars::JsonSchema;
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

    /// Generated `tsvector` column derived from another text column on the
    /// same schema. Read-only; populated automatically.
    Tsvector {
        /// Name of the text column to derive the tsvector from. Must be a
        /// `String` or `Enum` column declared in the same schema.
        #[serde(rename = "sourceColumn")]
        source_column: String,
        /// Postgres text-search configuration. Defaults to `"english"`.
        #[serde(default = "default_tsvector_language")]
        language: String,
    },

    /// pgvector `vector(N)` column for storing embedding vectors. Populated
    /// at ingest time (typically via the `openai-create-embedding`
    /// capability composed in a workflow). Queryable via the four distance
    /// ExprFns: `COSINE_DISTANCE`, `L2_DISTANCE`, `INNER_PRODUCT`.
    Vector {
        /// Number of dimensions. Range: 1..=16000.
        dimension: u32,
        /// Optional approximate-nearest-neighbor index. None ⇒ no index.
        #[serde(
            default,
            rename = "indexMethod",
            skip_serializing_if = "Option::is_none"
        )]
        index_method: Option<VectorIndexMethod>,
    },
}

/// Mirror of `runtara_object_store::VectorIndexMethod` for the HTTP DTO.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum VectorIndexMethod {
    /// HNSW. Default for embedding workloads.
    Hnsw,
    /// IVFFlat with `lists` inverted lists.
    IvfFlat { lists: u32 },
}

fn default_precision() -> u8 {
    19
}

fn default_scale() -> u8 {
    4
}

fn default_tsvector_language() -> String {
    "english".to_string()
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
            ColumnType::Tsvector { .. } => "TSVECTOR".to_string(),
            ColumnType::Vector { dimension, .. } => format!("vector({})", dimension),
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
            (ColumnType::Tsvector { .. }, _) => {
                Err("Generated tsvector columns are read-only; do not set a value".to_string())
            }
            (ColumnType::Vector { dimension, .. }, serde_json::Value::Array(arr)) => {
                if arr.len() as u32 != *dimension {
                    return Err(format!(
                        "Vector dimension mismatch: expected {}, got {}",
                        dimension,
                        arr.len()
                    ));
                }
                for (i, v) in arr.iter().enumerate() {
                    let f = v
                        .as_f64()
                        .ok_or_else(|| format!("Vector element at index {} is not a number", i))?;
                    if !f.is_finite() {
                        return Err(format!(
                            "Vector element at index {} is not finite ({})",
                            i, f
                        ));
                    }
                }
                Ok(())
            }
            _ => Err(format!(
                "Type mismatch: expected {:?}, got {:?}",
                self, value
            )),
        }
    }
}

/// Optional secondary text-index annotation for string-typed columns.
///
/// `Trigram` causes a `gin_trgm_ops` GIN index to be created alongside the
/// table, which speeds up `SIMILARITY_GTE` and `similarity()` scoring.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TextIndexKind {
    #[default]
    None,
    Trigram,
}

fn is_default_text_index(t: &TextIndexKind) -> bool {
    matches!(t, TextIndexKind::None)
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

    /// Optional secondary text index. Only valid for `string` / `enum`
    /// columns; rejected at validation otherwise.
    #[serde(
        default,
        rename = "textIndex",
        skip_serializing_if = "is_default_text_index"
    )]
    pub text_index: TextIndexKind,
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

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, JsonSchema)]
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
    /// Optional computed score column. Adds `<expression> AS <alias>` to
    /// the SELECT and surfaces under `instance.computed[alias]`.
    #[serde(
        rename = "scoreExpression",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub score_expression: Option<ScoreExpression>,
    /// Optional ORDER BY entries. When set, supersedes `sortBy` / `sortOrder`.
    #[serde(rename = "orderBy", skip_serializing_if = "Option::is_none", default)]
    pub order_by: Option<Vec<OrderByEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScoreExpression {
    /// Output alias. Must be `[a-zA-Z_][a-zA-Z0-9_]*`.
    pub alias: String,
    /// Expression tree. Same shape as aggregate `EXPR`, plus the
    /// whitelisted function-call form `{fn: "SIMILARITY"|"GREATEST"|"LEAST",
    /// arguments: [...]}`.
    pub expression: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum OrderByTarget {
    Column { name: String },
    Alias { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OrderByEntry {
    pub expression: OrderByTarget,
    #[serde(default)]
    pub direction: SortDirection,
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
    /// Computed columns (e.g. `score_expression` output). Absent when
    /// no score expression was requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computed: Option<serde_json::Map<String, serde_json::Value>>,
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

/// Behavior on unique-key conflict for bulk-create.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BulkConflictMode {
    /// Any unique conflict aborts the transaction (default).
    #[default]
    Error,
    /// Conflicting rows are silently skipped (`ON CONFLICT DO NOTHING`).
    Skip,
    /// Conflicting rows are updated with the incoming values (`ON CONFLICT DO UPDATE`).
    Upsert,
}

/// Behavior on per-row validation failure for bulk-create.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BulkValidationMode {
    /// First validation failure aborts the whole bulk (default).
    #[default]
    Stop,
    /// Invalid rows are reported in `errors` and skipped; valid rows still insert.
    Skip,
}

/// Bulk create request supporting two input shapes.
///
/// **Object form** — each record as a JSON object:
/// ```jsonc
/// { "instances": [ { "sku": "A", "qty": 1 }, ... ] }
/// ```
///
/// **Columnar form** — column names once, rows as arrays of values. Optional
/// `constants` are merged into every row (row values win on overlap). Use for
/// large, uniform payloads (snapshots, CSV-style writes) to avoid repeating
/// column keys.
/// ```jsonc
/// {
///   "columns": ["sku", "qty"],
///   "rows":    [["A", 1], ["B", 2]],
///   "constants":          { "snapshot_date": "2026-04-18" },
///   "nullifyEmptyStrings": true
/// }
/// ```
///
/// Exactly one of (`instances`) or (`columns` + `rows`) must be provided.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkCreateRequest {
    /// Object form — array of JSON objects, one per record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<Vec<serde_json::Value>>,

    /// Columnar form — column names (length N). Must be paired with `rows`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,

    /// Columnar form — each row is an array of values aligned to `columns`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<serde_json::Value>>>,

    /// Columnar form — fields merged into every row. Row cell values take
    /// precedence over constants when both provide the same column.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub constants: serde_json::Map<String, serde_json::Value>,

    /// Columnar form — when true, empty strings in non-string columns are
    /// converted to `null` before validation. Useful when ingesting from
    /// sources (CSV, SFTP) where missing values come through as "".
    #[serde(default, rename = "nullifyEmptyStrings")]
    pub nullify_empty_strings: bool,

    /// How to handle unique-key conflicts (default `error`).
    #[serde(default, rename = "onConflict")]
    pub on_conflict: BulkConflictMode,

    /// How to handle per-row validation failures (default `stop`).
    #[serde(default, rename = "onError")]
    pub on_error: BulkValidationMode,

    /// Columns used to detect conflicts. Required when `onConflict` is `skip` or `upsert`.
    #[serde(default, rename = "conflictColumns")]
    pub conflict_columns: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkRowError {
    pub index: usize,
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkCreateResponse {
    pub success: bool,
    #[serde(rename = "createdCount")]
    pub created_count: i64,
    #[serde(rename = "skippedCount", default)]
    pub skipped_count: i64,
    #[serde(default)]
    pub errors: Vec<BulkRowError>,
    pub message: String,
}

/// Bulk update request. The `mode` field selects between two semantics:
/// - `byCondition` — apply the same `properties` to every row matching `condition`.
/// - `byIds` — apply per-row `properties` to each listed `id`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum BulkUpdateRequest {
    ByCondition {
        properties: serde_json::Value,
        condition: Condition,
    },
    ByIds {
        updates: Vec<BulkUpdateByIdEntry>,
    },
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkUpdateByIdEntry {
    pub id: String,
    pub properties: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkUpdateResponse {
    pub success: bool,
    #[serde(rename = "updatedCount")]
    pub updated_count: i64,
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
// Aggregate (GROUP BY) DTOs
// ============================================================================

/// Aggregate function. JSON encoding is SCREAMING_SNAKE_CASE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AggregateFn {
    Count,
    Sum,
    /// Arithmetic mean over a numeric column. Returns NULL on an empty set.
    Avg,
    Min,
    Max,
    FirstValue,
    LastValue,
    /// Continuous percentile (PostgreSQL `percentile_cont`). Requires
    /// `percentile` ∈ [0.0, 1.0] and exactly one numeric `orderBy` entry.
    PercentileCont,
    /// Discrete percentile (PostgreSQL `percentile_disc`). Same shape as
    /// `PERCENTILE_CONT`.
    PercentileDisc,
    /// Sample standard deviation over a numeric column.
    StddevSamp,
    /// Sample variance over a numeric column.
    VarSamp,
    /// Computed column — the value is derived from prior aliases via an
    /// `expression` tree. Reads no DB column. See v1.1 spec.
    Expr,
}

/// Sort direction. JSON encoding is UPPERCASE (`"ASC"` / `"DESC"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AggregateOrderBy {
    pub column: String,
    #[serde(default)]
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AggregateSpec {
    /// Output column name. Must match `[a-zA-Z_][a-zA-Z0-9_]*` and be unique.
    pub alias: String,
    /// Aggregate function. One of COUNT, SUM, MIN, MAX, FIRST_VALUE, LAST_VALUE, EXPR.
    #[serde(rename = "fn")]
    pub fn_: AggregateFn,
    /// Source column. Optional for COUNT (COUNT(*)); required otherwise.
    /// Must be omitted for EXPR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Apply DISTINCT. Only valid with `fn = COUNT` and a non-null `column`.
    #[serde(default)]
    pub distinct: bool,
    /// Required for FIRST_VALUE / LAST_VALUE; rejected for others.
    #[serde(default, rename = "orderBy", alias = "order_by")]
    pub order_by: Vec<AggregateOrderBy>,
    /// Required for EXPR — an expression tree referencing prior aliases and
    /// constants. Rejected for every other function.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<serde_json::Value>,
    /// Fraction in `[0.0, 1.0]` for `PERCENTILE_CONT` / `PERCENTILE_DISC`.
    /// Required for those functions, rejected otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentile: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AggregateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    #[serde(default, rename = "groupBy", alias = "group_by")]
    pub group_by: Vec<String>,
    pub aggregates: Vec<AggregateSpec>,
    #[serde(default, rename = "orderBy", alias = "order_by")]
    pub order_by: Vec<AggregateOrderBy>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AggregateResponse {
    pub success: bool,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    #[serde(rename = "groupCount")]
    pub group_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---- Conversions to store-level types ----

impl From<SortDirection> for runtara_object_store::SortDirection {
    fn from(d: SortDirection) -> Self {
        match d {
            SortDirection::Asc => runtara_object_store::SortDirection::Asc,
            SortDirection::Desc => runtara_object_store::SortDirection::Desc,
        }
    }
}

impl From<AggregateFn> for runtara_object_store::AggregateFn {
    fn from(f: AggregateFn) -> Self {
        match f {
            AggregateFn::Count => runtara_object_store::AggregateFn::Count,
            AggregateFn::Sum => runtara_object_store::AggregateFn::Sum,
            AggregateFn::Avg => runtara_object_store::AggregateFn::Avg,
            AggregateFn::Min => runtara_object_store::AggregateFn::Min,
            AggregateFn::Max => runtara_object_store::AggregateFn::Max,
            AggregateFn::FirstValue => runtara_object_store::AggregateFn::FirstValue,
            AggregateFn::LastValue => runtara_object_store::AggregateFn::LastValue,
            AggregateFn::PercentileCont => runtara_object_store::AggregateFn::PercentileCont,
            AggregateFn::PercentileDisc => runtara_object_store::AggregateFn::PercentileDisc,
            AggregateFn::StddevSamp => runtara_object_store::AggregateFn::StddevSamp,
            AggregateFn::VarSamp => runtara_object_store::AggregateFn::VarSamp,
            AggregateFn::Expr => runtara_object_store::AggregateFn::Expr,
        }
    }
}

impl From<AggregateOrderBy> for runtara_object_store::AggregateOrderBy {
    fn from(o: AggregateOrderBy) -> Self {
        runtara_object_store::AggregateOrderBy {
            column: o.column,
            direction: o.direction.into(),
        }
    }
}

impl From<AggregateSpec> for runtara_object_store::AggregateSpec {
    fn from(s: AggregateSpec) -> Self {
        runtara_object_store::AggregateSpec {
            alias: s.alias,
            fn_: s.fn_.into(),
            column: s.column,
            distinct: s.distinct,
            order_by: s.order_by.into_iter().map(Into::into).collect(),
            expression: s.expression,
            percentile: s.percentile,
        }
    }
}

impl From<AggregateRequest> for runtara_object_store::AggregateRequest {
    fn from(r: AggregateRequest) -> Self {
        runtara_object_store::AggregateRequest {
            condition: r.condition.map(Into::into),
            group_by: r.group_by,
            aggregates: r.aggregates.into_iter().map(Into::into).collect(),
            order_by: r.order_by.into_iter().map(Into::into).collect(),
            limit: r.limit,
            offset: r.offset,
        }
    }
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
            computed: store_instance.computed,
        }
    }
}

impl From<TextIndexKind> for runtara_object_store::TextIndexKind {
    fn from(t: TextIndexKind) -> Self {
        match t {
            TextIndexKind::None => runtara_object_store::TextIndexKind::None,
            TextIndexKind::Trigram => runtara_object_store::TextIndexKind::Trigram,
        }
    }
}

impl From<runtara_object_store::TextIndexKind> for TextIndexKind {
    fn from(t: runtara_object_store::TextIndexKind) -> Self {
        match t {
            runtara_object_store::TextIndexKind::None => TextIndexKind::None,
            runtara_object_store::TextIndexKind::Trigram => TextIndexKind::Trigram,
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
            text_index: col.text_index.into(),
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
            text_index: col.text_index.into(),
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
            StoreColumnType::Tsvector {
                source_column,
                language,
            } => ColumnType::Tsvector {
                source_column,
                language,
            },
            StoreColumnType::Vector {
                dimension,
                index_method,
            } => ColumnType::Vector {
                dimension,
                index_method: index_method.map(VectorIndexMethod::from),
            },
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
            ColumnType::Tsvector {
                source_column,
                language,
            } => StoreColumnType::Tsvector {
                source_column,
                language,
            },
            ColumnType::Vector {
                dimension,
                index_method,
            } => StoreColumnType::Vector {
                dimension,
                index_method: index_method.map(runtara_object_store::VectorIndexMethod::from),
            },
        }
    }
}

impl From<runtara_object_store::VectorIndexMethod> for VectorIndexMethod {
    fn from(m: runtara_object_store::VectorIndexMethod) -> Self {
        match m {
            runtara_object_store::VectorIndexMethod::Hnsw => VectorIndexMethod::Hnsw,
            runtara_object_store::VectorIndexMethod::IvfFlat { lists } => {
                VectorIndexMethod::IvfFlat { lists }
            }
        }
    }
}

impl From<VectorIndexMethod> for runtara_object_store::VectorIndexMethod {
    fn from(m: VectorIndexMethod) -> Self {
        match m {
            VectorIndexMethod::Hnsw => runtara_object_store::VectorIndexMethod::Hnsw,
            VectorIndexMethod::IvfFlat { lists } => {
                runtara_object_store::VectorIndexMethod::IvfFlat { lists }
            }
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
            score_expression: req.score_expression.map(|s| s.into()),
            order_by: req
                .order_by
                .map(|entries| entries.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<ScoreExpression> for runtara_object_store::ScoreExpression {
    fn from(s: ScoreExpression) -> Self {
        runtara_object_store::ScoreExpression {
            alias: s.alias,
            expression: s.expression,
        }
    }
}

impl From<OrderByTarget> for runtara_object_store::OrderByTarget {
    fn from(t: OrderByTarget) -> Self {
        match t {
            OrderByTarget::Column { name } => runtara_object_store::OrderByTarget::Column { name },
            OrderByTarget::Alias { name } => runtara_object_store::OrderByTarget::Alias { name },
        }
    }
}

impl From<OrderByEntry> for runtara_object_store::OrderByEntry {
    fn from(e: OrderByEntry) -> Self {
        // Reuse the SortDirection conversion — both DTO and store use the
        // same uppercase enum.
        let direction = match e.direction {
            SortDirection::Asc => runtara_object_store::SortDirection::Asc,
            SortDirection::Desc => runtara_object_store::SortDirection::Desc,
        };
        runtara_object_store::OrderByEntry {
            expression: e.expression.into(),
            direction,
        }
    }
}
