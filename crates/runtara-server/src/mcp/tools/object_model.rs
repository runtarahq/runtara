use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::super::server::SmoMcpServer;
use super::internal_api::{
    api_delete, api_delete_with_body, api_get, api_patch, api_post, api_put, encode_path_param,
    validate_identifier_param, validate_path_param,
};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

const LARGE_RESULT_ROW_WARNING: usize = 1_000;
const LARGE_RESULT_BYTES_WARNING: usize = 1_000_000;
const MCP_REQUEST_PAYLOAD_GUIDANCE_BYTES: usize = 8 * 1024 * 1024;

fn json_result_with_guidance(
    mut value: serde_json::Value,
    guidance: Vec<String>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    if !guidance.is_empty() {
        let guidance_value = serde_json::json!({
            "warnings": guidance,
            "nextActions": [
                "Use limit and offset to page through results.",
                "Use query_aggregate for grouped summaries instead of fetching rows and folding client-side.",
            ],
        });
        match &mut value {
            Value::Object(map) => {
                map.insert("_mcpGuidance".to_string(), guidance_value);
            }
            _ => {
                value = serde_json::json!({
                    "result": value,
                    "_mcpGuidance": guidance_value,
                });
            }
        }
    }
    json_result(value)
}

fn extract_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_i64()))
}

fn extract_array_len(value: &Value, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_array()).map(Vec::len))
}

fn result_size_guidance(
    value: &Value,
    tool_name: &str,
    requested_limit: Option<i64>,
    result_key: &[&str],
    total_key: &[&str],
) -> Vec<String> {
    let returned = extract_array_len(value, result_key);
    let total = extract_i64(value, total_key);
    let mut warnings = Vec::new();

    if let (Some(returned), Some(total)) = (returned, total) {
        if total > returned as i64 {
            warnings.push(format!(
                "{} returned {} of {} matching rows. Use limit/offset to fetch subsequent pages.",
                tool_name, returned, total
            ));
        }
        if returned >= LARGE_RESULT_ROW_WARNING {
            warnings.push(format!(
                "{} returned {} rows in one MCP response. Prefer a smaller limit for interactive use.",
                tool_name, returned
            ));
        }
    }

    let likely_large_without_limit = requested_limit.is_none()
        && (returned.is_some_and(|n| n >= 100)
            || total.is_some_and(|n| n >= 100)
            || serde_json::to_vec(value)
                .map(|bytes| bytes.len() >= LARGE_RESULT_BYTES_WARNING)
                .unwrap_or(false));
    if likely_large_without_limit {
        warnings.push(format!(
            "{} was called without an explicit limit. Set limit/offset deliberately for large schemas.",
            tool_name
        ));
    }

    if serde_json::to_vec(value)
        .map(|bytes| bytes.len() >= LARGE_RESULT_BYTES_WARNING)
        .unwrap_or(false)
    {
        warnings.push(format!(
            "{} produced a large JSON response. Narrow the condition, lower limit, or aggregate before returning data to MCP.",
            tool_name
        ));
    }

    warnings
}

fn ensure_request_payload_reasonable(tool_name: &str, body: &Value) -> Result<(), rmcp::ErrorData> {
    let Ok(bytes) = serde_json::to_vec(body) else {
        return Ok(());
    };
    if bytes.len() <= MCP_REQUEST_PAYLOAD_GUIDANCE_BYTES {
        return Ok(());
    }

    Err(rmcp::ErrorData::invalid_params(
        format!(
            "{} request payload is {} bytes, which is likely too large for MCP transport. Split the request into smaller batches; for bulk_create_instances prefer columns/rows plus constants to avoid repeating field names.",
            tool_name,
            bytes.len()
        ),
        None,
    ))
}

fn with_connection_id_query(
    path: &str,
    connection_id: Option<&str>,
) -> Result<String, rmcp::ErrorData> {
    match connection_id {
        Some(connection_id) => {
            validate_identifier_param("connection_id", connection_id)?;
            Ok(format!(
                "{}?connectionId={}",
                path,
                encode_path_param(connection_id)
            ))
        }
        None => Ok(path.to_string()),
    }
}

fn sql_params_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "array",
        "description": "Typed positional SQL parameters. Items are bound in array order: first item is $1, second is $2. Use Postgres/SQLx positional placeholders; named parameters are not supported.",
        "items": {
            "type": "object",
            "required": ["type", "value"],
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["string", "integer", "decimal", "boolean", "timestamp", "json", "enum", "vector"],
                    "description": "Object Model column type used to validate and bind the value."
                },
                "value": {
                    "description": "Parameter value. Use null for SQL NULL. Timestamp values must be RFC3339 strings. Vector values must be number arrays matching dimension."
                },
                "precision": {
                    "type": "integer",
                    "description": "Decimal precision; optional for type=decimal."
                },
                "scale": {
                    "type": "integer",
                    "description": "Decimal scale; optional for type=decimal."
                },
                "values": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Allowed values; required for type=enum."
                },
                "dimension": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 16000,
                    "description": "Vector dimension; required for type=vector."
                }
            },
            "additionalProperties": true
        }
    })
}

fn sql_result_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "array",
        "description": "Expected result columns for typed SQL reads. Each item names a selected SQL column and its Object Model type.",
        "items": {
            "type": "object",
            "required": ["name", "type"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Column name or SQL alias in the SELECT result."
                },
                "type": {
                    "type": "string",
                    "enum": ["string", "integer", "decimal", "boolean", "timestamp", "json", "enum", "vector"],
                    "description": "Expected Object Model column type for decoding this result column."
                },
                "nullable": {
                    "type": "boolean",
                    "default": false,
                    "description": "Whether SQL NULL is allowed for this column."
                },
                "precision": {
                    "type": "integer",
                    "description": "Decimal precision; optional for type=decimal."
                },
                "scale": {
                    "type": "integer",
                    "description": "Decimal scale; optional for type=decimal."
                },
                "values": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Allowed values; required for type=enum."
                },
                "dimension": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 16000,
                    "description": "Vector dimension; required for type=vector."
                }
            },
            "additionalProperties": true
        }
    })
}

fn with_payload_too_large_guidance<T>(
    result: Result<T, rmcp::ErrorData>,
    tool_name: &str,
) -> Result<T, rmcp::ErrorData> {
    result.map_err(|err| {
        let msg = format!("{:?}", err);
        let lower = msg.to_lowercase();
        if lower.contains("413")
            || lower.contains("payload too large")
            || lower.contains("request body too large")
            || lower.contains("body limit")
            || lower.contains("exceeds limit")
        {
            rmcp::ErrorData::invalid_params(
                format!(
                    "{} request was too large for the object-model API. Split it into smaller batches; for bulk_create_instances prefer columns/rows plus constants, and for reads use limit/offset or query_aggregate.",
                    tool_name
                ),
                None,
            )
        } else {
            err
        }
    })
}

fn condition_op_wire_name(op: &str) -> String {
    let mut out = String::new();
    let mut prev_was_lower_or_digit = false;
    for ch in op.chars() {
        if ch == '-' || ch == ' ' {
            if !out.ends_with('_') {
                out.push('_');
            }
            prev_was_lower_or_digit = false;
            continue;
        }
        if ch == '_' {
            if !out.ends_with('_') {
                out.push('_');
            }
            prev_was_lower_or_digit = false;
            continue;
        }
        if ch.is_uppercase() && prev_was_lower_or_digit {
            out.push('_');
        }
        out.extend(ch.to_uppercase());
        prev_was_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
    }
    out
}

fn mapping_value_to_condition_arg(value: Value) -> Value {
    let Value::Object(mut map) = value else {
        return value;
    };

    match map.get("valueType").and_then(Value::as_str) {
        Some("reference") | Some("template") => map.remove("value").unwrap_or(Value::Null),
        Some("immediate") => map.remove("value").unwrap_or(Value::Null),
        Some("composite") => map.remove("value").unwrap_or(Value::Object(map)),
        _ => Value::Object(map),
    }
}

fn normalize_condition_argument(value: Value) -> Result<Value, rmcp::ErrorData> {
    if value.get("op").is_some() || value.get("type").and_then(Value::as_str) == Some("operation") {
        normalize_condition(value)
    } else {
        Ok(mapping_value_to_condition_arg(value))
    }
}

fn normalize_condition(value: Value) -> Result<Value, rmcp::ErrorData> {
    let Value::Object(map) = value else {
        return Err(rmcp::ErrorData::invalid_params(
            "condition must be an object".to_string(),
            None,
        ));
    };

    if map.get("valueType").is_some() || map.get("type").and_then(Value::as_str) == Some("value") {
        return Ok(serde_json::json!({
            "op": "IS_DEFINED",
            "arguments": [mapping_value_to_condition_arg(Value::Object(map))],
        }));
    }

    let op = map.get("op").and_then(Value::as_str).ok_or_else(|| {
        rmcp::ErrorData::invalid_params(
            "condition must include `op` or use the workflow condition shape {type:'operation', op, arguments}".to_string(),
            None,
        )
    })?;

    let normalized_op = condition_op_wire_name(op);
    let mut normalized = serde_json::Map::new();
    normalized.insert("op".to_string(), Value::String(normalized_op));

    if let Some(arguments) = map.get("arguments") {
        let args = arguments.as_array().ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "condition.arguments must be an array".to_string(),
                None,
            )
        })?;
        let normalized_args = args
            .iter()
            .cloned()
            .map(normalize_condition_argument)
            .collect::<Result<Vec<_>, _>>()?;
        normalized.insert("arguments".to_string(), Value::Array(normalized_args));
    }

    Ok(Value::Object(normalized))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetObjectSchemaParams {
    #[schemars(description = "Schema name")]
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateObjectSchemaParams {
    #[schemars(description = "Schema name")]
    pub name: String,
    #[schemars(description = "Schema description")]
    pub description: Option<String>,
    #[schemars(description = "Database table name (auto-derived from name if omitted)")]
    pub table_name: Option<String>,
    #[schemars(description = "Column definitions as JSON array")]
    pub columns: Vec<Value>,
    #[schemars(description = "Index definitions as JSON array (optional)")]
    pub indexes: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateObjectSchemaParams {
    #[schemars(description = "Existing schema name to update")]
    pub name: String,
    #[schemars(description = "New schema name (rename). Omit to keep current name.")]
    pub new_name: Option<String>,
    #[schemars(description = "New description. Omit to keep current.")]
    pub description: Option<String>,
    #[schemars(
        description = "FULL replacement column list. The server diffs this against the \
                       current schema and emits ALTER TABLE ADD/DROP/ALTER COLUMN. To \
                       add columns without losing existing ones, fetch the current \
                       schema with get_object_schema, append your new columns, and pass \
                       the merged array. Omit to leave columns unchanged."
    )]
    pub columns: Option<Vec<Value>>,
    #[schemars(
        description = "FULL replacement index list (same diff semantics as columns). \
                       Omit to leave indexes unchanged."
    )]
    pub indexes: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteObjectSchemaParams {
    #[schemars(description = "Schema name to delete")]
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListObjectInstancesParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
    #[schemars(
        description = "Max results. Defaults to the API page size; set explicitly for large schemas."
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination offset")]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryObjectInstancesParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
    #[schemars(
        description = "Filter condition as a JSON object: { op: string, arguments?: any[] }. \
                              Supported ops: AND, OR, NOT, EQ, NE, GT, LT, GTE, LTE, CONTAINS, \
                              IN, NOT_IN, IS_EMPTY, IS_NOT_EMPTY, IS_DEFINED. Compound filters \
                              nest child Conditions inside arguments of AND/OR."
    )]
    pub condition: Option<serde_json::Value>,
    #[schemars(
        description = "Optional computed score expression object, not an escaped JSON string. \
                       For vector nearest-neighbor search, use {alias:'distance', expression:{fn:'COSINE_DISTANCE', \
                       arguments:[{valueType:'reference', value:'embedding'}, \
                       {valueType:'immediate', value:[number,...]}]}} and order by \
                       that alias ASC."
    )]
    pub score_expression: Option<serde_json::Value>,
    #[schemars(
        description = "Optional structured order: [{expression:{kind:'alias'|'column', \
                       name}, direction:'ASC'|'DESC'}]. Use an alias target matching \
                       score_expression.alias for vector nearest-neighbor search."
    )]
    pub order_by: Option<serde_json::Value>,
    #[schemars(
        description = "Max results. Set explicitly for large schemas; use offset for paging."
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination offset")]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryAggregateParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
    #[schemars(
        description = "Filter condition (same DSL as query_object_instances): \
                       { op, arguments?: any[] }. Supported ops: AND, OR, NOT, \
                       EQ, NE, GT, LT, GTE, LTE, CONTAINS, IN, NOT_IN, \
                       IS_EMPTY, IS_NOT_EMPTY, IS_DEFINED."
    )]
    pub condition: Option<serde_json::Value>,
    #[schemars(
        description = "Columns to GROUP BY. Empty/omitted → one output row over \
                       the whole filtered set."
    )]
    pub group_by: Option<Vec<String>>,
    #[schemars(
        description = "Array of aggregate specs: [{alias, fn, column?, distinct?, \
                       orderBy?, expression?, percentile?}]. `fn` is one of COUNT, \
                       SUM, AVG, MIN, MAX, FIRST_VALUE, LAST_VALUE, STDDEV_SAMP, \
                       VAR_SAMP, PERCENTILE_CONT, PERCENTILE_DISC, EXPR. Each alias \
                       must be a unique [a-zA-Z_][a-zA-Z0-9_]* identifier. `column` is \
                       optional for COUNT (→ COUNT(*)) and required for SUM/AVG/MIN/\
                       MAX/FIRST_VALUE/LAST_VALUE/STDDEV_SAMP/VAR_SAMP; must be omitted \
                       for EXPR and PERCENTILE_*. `distinct: true` is valid only with \
                       COUNT + column. FIRST_VALUE/LAST_VALUE require non-empty \
                       orderBy: [{column, direction: ASC|DESC}]. PERCENTILE_CONT/\
                       PERCENTILE_DISC require `percentile` in [0.0, 1.0] and exactly \
                       one numeric orderBy entry (the value column). EXPR requires \
                       `expression`: a tree over previously-declared aliases and \
                       constants. Operators: arithmetic (ADD, SUB, MUL, DIV, NEG, \
                       ABS, COALESCE; DIV returns NULL on divide-by-zero), comparison \
                       (EQ, NE, GT, GTE, LT, LTE), logical (AND, OR, NOT), and \
                       nullability (IS_DEFINED, IS_EMPTY, IS_NOT_EMPTY). Operand forms \
                       inside an expression: {valueType:'alias', value:'<prior_alias>'} \
                       (resolves to a prior aggregate's value), {valueType:'immediate', \
                       value:<number|bool|string|null>} (literal). Field references \
                       ({valueType:'reference', ...}) are rejected inside EXPR. Max \
                       tree depth is 8."
    )]
    pub aggregates: serde_json::Value,
    #[schemars(
        description = "Optional top-level sort: [{column, direction}] where column \
                       must match a group_by column or aggregate alias, or \
                       [{expression:{fn:'COSINE_DISTANCE'|'L2_DISTANCE', field, \
                       value:[number,...]}, direction:'ASC'|'DESC'}] for vector \
                       nearest-neighbor ordering against a vector field."
    )]
    pub order_by: Option<serde_json::Value>,
    #[schemars(
        description = "Max result rows (server caps at 100000). Omit to let the \
                       server return all rows — if the natural result exceeds the \
                       cap the request is rejected. Set this explicitly for interactive MCP use."
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination offset")]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuerySqlParams {
    #[schemars(
        description = "SQL string using Postgres/SQLx positional placeholders ($1, $2, ...). Named parameters are not supported. Include LIMIT/OFFSET directly in SELECT statements for interactive MCP reads."
    )]
    pub sql: String,
    #[schemars(schema_with = "sql_params_schema")]
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
    #[schemars(schema_with = "sql_result_schema")]
    #[serde(rename = "resultSchema", alias = "result_schema")]
    pub result_schema: Vec<serde_json::Value>,
    #[schemars(
        description = "Optional connection ID for database selection. Omit to use the default object-model database."
    )]
    #[serde(rename = "connectionId", alias = "connection_id")]
    pub connection_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuerySqlRawParams {
    #[schemars(
        description = "SQL string using Postgres/SQLx positional placeholders ($1, $2, ...). Named parameters are not supported. Include LIMIT/OFFSET directly in SELECT statements for interactive MCP reads."
    )]
    pub sql: String,
    #[schemars(schema_with = "sql_params_schema")]
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
    #[schemars(
        description = "Optional connection ID for database selection. Omit to use the default object-model database."
    )]
    #[serde(rename = "connectionId", alias = "connection_id")]
    pub connection_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteSqlParams {
    #[schemars(
        description = "SQL command using Postgres/SQLx positional placeholders ($1, $2, ...). Named parameters are not supported. Execute is for commands and returns rowsAffected. Postgres prepared statements accept one SQL statement per call."
    )]
    pub sql: String,
    #[schemars(schema_with = "sql_params_schema")]
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
    #[schemars(
        description = "Optional connection ID for database selection. Omit to use the default object-model database."
    )]
    #[serde(rename = "connectionId", alias = "connection_id")]
    pub connection_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateObjectInstanceParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
    #[schemars(description = "Instance properties as JSON object")]
    pub properties: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateObjectInstanceParams {
    #[schemars(description = "Schema ID")]
    pub schema_id: String,
    #[schemars(description = "Instance ID")]
    pub instance_id: String,
    #[schemars(description = "Updated properties as JSON object")]
    pub properties: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkCreateInstancesParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
    #[schemars(
        description = "Object form — array of instance objects, one per record. Mutually \
                       exclusive with `columns`/`rows`."
    )]
    pub instances: Option<Vec<serde_json::Value>>,
    #[schemars(
        description = "Columnar form — column names (length N). Pair with `rows`. Use for \
                       large uniform payloads."
    )]
    pub columns: Option<Vec<String>>,
    #[schemars(
        description = "Columnar form — each row is an array of values aligned to `columns`."
    )]
    pub rows: Option<Vec<Vec<serde_json::Value>>>,
    #[schemars(
        description = "Columnar form — fields merged into every row. Row cell values take \
                       precedence over constants."
    )]
    pub constants: Option<serde_json::Map<String, serde_json::Value>>,
    #[schemars(
        description = "Columnar form — when true, empty strings in non-string columns are \
                       converted to null before validation."
    )]
    #[serde(rename = "nullifyEmptyStrings")]
    pub nullify_empty_strings: Option<bool>,
    #[schemars(
        description = "Behavior on unique-key conflict: error (default), skip (ON CONFLICT \
                       DO NOTHING), or upsert (ON CONFLICT DO UPDATE)."
    )]
    #[serde(rename = "onConflict")]
    pub on_conflict: Option<String>,
    #[schemars(
        description = "Behavior on per-row validation failure: stop (default — abort whole \
                       bulk on first failure) or skip (collect into errors[], keep going)."
    )]
    #[serde(rename = "onError")]
    pub on_error: Option<String>,
    #[schemars(
        description = "Columns used to detect conflicts. Required when onConflict is skip \
                       or upsert."
    )]
    #[serde(rename = "conflictColumns")]
    pub conflict_columns: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkUpdateInstancesParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
    #[schemars(
        description = "Update mode: byCondition (apply same `properties` to every row \
                       matching `condition`) or byIds (per-row `properties` for each \
                       listed id via `updates`)."
    )]
    pub mode: String,
    #[schemars(
        description = "byCondition: filter condition (same DSL as query_object_instances). \
                       Required when mode=byCondition."
    )]
    pub condition: Option<serde_json::Value>,
    #[schemars(
        description = "byCondition: flat object of column → new_value applied to every \
                       matched row. Required when mode=byCondition."
    )]
    pub properties: Option<serde_json::Value>,
    #[schemars(
        description = "byIds: array of {id, properties} entries. Required when mode=byIds."
    )]
    pub updates: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkDeleteInstancesParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
    #[schemars(description = "Instance IDs to delete")]
    #[serde(rename = "instanceIds")]
    pub instance_ids: Vec<String>,
}

// ===== Tool Implementations =====

pub async fn list_object_schemas(server: &SmoMcpServer) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_get(server, "/api/runtime/object-model/schemas").await?;
    json_result(result)
}

pub async fn get_object_schema(
    server: &SmoMcpServer,
    params: GetObjectSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("name", &params.name)?;
    let result = api_get(
        server,
        &format!("/api/runtime/object-model/schemas/name/{}", params.name),
    )
    .await?;
    json_result(result)
}

pub async fn create_object_schema(
    server: &SmoMcpServer,
    params: CreateObjectSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    // Derive tableName from name: "ShopifyProduct" → "shopify_product"
    let table_name = params
        .table_name
        .unwrap_or_else(|| to_snake_case(&params.name));
    let mut body = serde_json::json!({
        "name": params.name,
        "tableName": table_name,
        "columns": params.columns,
    });
    if let Some(desc) = params.description {
        body["description"] = serde_json::Value::String(desc);
    }
    if let Some(indexes) = params.indexes {
        body["indexes"] = Value::Array(indexes);
    }
    ensure_request_payload_reasonable("create_object_schema", &body)?;
    let result = with_payload_too_large_guidance(
        api_post(server, "/api/runtime/object-model/schemas", Some(body)).await,
        "create_object_schema",
    )?;
    json_result(result)
}

/// Convert PascalCase/camelCase to snake_case for table names.
fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_lowercase().next().unwrap());
        } else {
            result.push(ch);
        }
    }
    result
}

/// Resolve a schema name to its UUID by calling the in-process get-by-name endpoint.
async fn resolve_schema_id_by_name(
    server: &SmoMcpServer,
    name: &str,
) -> Result<String, rmcp::ErrorData> {
    validate_path_param("name", name)?;
    let resp = api_get(
        server,
        &format!("/api/runtime/object-model/schemas/name/{}", name),
    )
    .await?;
    resp.get("schema")
        .and_then(|s| s.get("id"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                format!("schema '{}' not found or response missing schema.id", name),
                None,
            )
        })
}

pub async fn update_object_schema(
    server: &SmoMcpServer,
    params: UpdateObjectSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let id = resolve_schema_id_by_name(server, &params.name).await?;
    let mut body = serde_json::json!({});
    if let Some(n) = params.new_name {
        body["name"] = serde_json::Value::String(n);
    }
    if let Some(d) = params.description {
        body["description"] = serde_json::Value::String(d);
    }
    if let Some(c) = params.columns {
        body["columns"] = Value::Array(c);
    }
    if let Some(i) = params.indexes {
        body["indexes"] = Value::Array(i);
    }
    ensure_request_payload_reasonable("update_object_schema", &body)?;
    let result = with_payload_too_large_guidance(
        api_put(
            server,
            &format!("/api/runtime/object-model/schemas/{}", id),
            Some(body),
        )
        .await,
        "update_object_schema",
    )?;
    json_result(result)
}

pub async fn delete_object_schema(
    server: &SmoMcpServer,
    params: DeleteObjectSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let id = resolve_schema_id_by_name(server, &params.name).await?;
    let result = api_delete(server, &format!("/api/runtime/object-model/schemas/{}", id)).await?;
    json_result(result)
}

pub async fn list_object_instances(
    server: &SmoMcpServer,
    params: ListObjectInstancesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("schema_name", &params.schema_name)?;
    let mut path = format!(
        "/api/runtime/object-model/instances/schema/name/{}",
        params.schema_name
    );
    let mut query = Vec::new();
    if let Some(offset) = params.offset {
        query.push(format!("offset={}", offset));
    }
    if let Some(limit) = params.limit {
        query.push(format!("limit={}", limit));
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }

    let mut result =
        with_payload_too_large_guidance(api_get(server, &path).await, "list_object_instances")?;
    let omitted_vector_columns =
        omit_vector_columns_from_instances(server, &params.schema_name, &mut result).await?;
    let guidance = result_size_guidance(
        &result,
        "list_object_instances",
        params.limit,
        &["instances"],
        &["totalCount", "total_count"],
    )
    .into_iter()
    .chain(vector_omission_guidance(
        "list_object_instances",
        omitted_vector_columns,
    ))
    .collect();
    json_result_with_guidance(result, guidance)
}

pub async fn query_object_instances(
    server: &SmoMcpServer,
    params: QueryObjectInstancesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("schema_name", &params.schema_name)?;
    let body = build_query_object_instances_body(&params)?;
    let mut result = with_payload_too_large_guidance(
        api_post(
            server,
            &format!(
                "/api/runtime/object-model/instances/schema/{}/filter",
                params.schema_name
            ),
            Some(body),
        )
        .await,
        "query_object_instances",
    )?;
    let omitted_vector_columns =
        omit_vector_columns_from_instances(server, &params.schema_name, &mut result).await?;
    let guidance = result_size_guidance(
        &result,
        "query_object_instances",
        params.limit,
        &["instances"],
        &["totalCount", "total_count"],
    )
    .into_iter()
    .chain(vector_omission_guidance(
        "query_object_instances",
        omitted_vector_columns,
    ))
    .collect();
    json_result_with_guidance(result, guidance)
}

async fn omit_vector_columns_from_instances(
    server: &SmoMcpServer,
    schema_name: &str,
    result: &mut Value,
) -> Result<Vec<String>, rmcp::ErrorData> {
    let schema = api_get(
        server,
        &format!(
            "/api/runtime/object-model/schemas/name/{}",
            encode_path_param(schema_name)
        ),
    )
    .await?;
    let vector_columns = vector_columns_from_schema_response(&schema);
    if vector_columns.is_empty() {
        return Ok(Vec::new());
    }

    let Some(instances) = result.get_mut("instances").and_then(Value::as_array_mut) else {
        return Ok(Vec::new());
    };

    let mut omitted = false;
    for instance in instances {
        if let Some(props) = instance
            .get_mut("properties")
            .and_then(Value::as_object_mut)
        {
            for column in &vector_columns {
                omitted |= props.remove(column).is_some();
            }
        }
        if let Some(obj) = instance.as_object_mut() {
            for column in &vector_columns {
                omitted |= obj.remove(column).is_some();
            }
        }
    }

    if omitted {
        Ok(vector_columns)
    } else {
        Ok(Vec::new())
    }
}

fn vector_columns_from_schema_response(schema: &Value) -> Vec<String> {
    schema
        .get("schema")
        .and_then(|schema| schema.get("columns"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|column| {
            let is_vector = column.get("type").and_then(Value::as_str) == Some("vector");
            is_vector
                .then(|| column.get("name").and_then(Value::as_str))
                .flatten()
                .map(str::to_string)
        })
        .collect()
}

fn vector_omission_guidance(tool_name: &str, columns: Vec<String>) -> Vec<String> {
    if columns.is_empty() {
        return Vec::new();
    }

    vec![format!(
        "{} omitted vector columns from instance properties to keep MCP responses small: {}.",
        tool_name,
        columns.join(", ")
    )]
}

fn build_query_object_instances_body(
    params: &QueryObjectInstancesParams,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let mut body = serde_json::json!({});
    if let Some(condition) = params.condition.clone() {
        body["condition"] = normalize_condition(condition)?;
    }
    if let Some(score_expression) = &params.score_expression {
        body["scoreExpression"] = score_expression.clone();
    }
    if let Some(order_by) = &params.order_by {
        body["orderBy"] = order_by.clone();
    }
    if let Some(limit) = params.limit {
        body["limit"] = serde_json::json!(limit);
    }
    if let Some(offset) = params.offset {
        body["offset"] = serde_json::json!(offset);
    }
    ensure_request_payload_reasonable("query_object_instances", &body)?;
    Ok(body)
}

pub async fn query_aggregate(
    server: &SmoMcpServer,
    params: QueryAggregateParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("schema_name", &params.schema_name)?;
    let mut body = serde_json::json!({ "aggregates": params.aggregates });
    if let Some(condition) = params.condition {
        body["condition"] = normalize_condition(condition)?;
    }
    if let Some(group_by) = params.group_by {
        body["groupBy"] = serde_json::json!(group_by);
    }
    if let Some(order_by) = params.order_by {
        body["orderBy"] = order_by;
    }
    if let Some(limit) = params.limit {
        body["limit"] = serde_json::json!(limit);
    }
    if let Some(offset) = params.offset {
        body["offset"] = serde_json::json!(offset);
    }
    ensure_request_payload_reasonable("query_aggregate", &body)?;
    let result = with_payload_too_large_guidance(
        api_post(
            server,
            &format!(
                "/api/runtime/object-model/instances/schema/{}/aggregate",
                params.schema_name
            ),
            Some(body),
        )
        .await,
        "query_aggregate",
    )?;
    let guidance = result_size_guidance(
        &result,
        "query_aggregate",
        params.limit,
        &["rows"],
        &["groupCount", "group_count"],
    );
    json_result_with_guidance(result, guidance)
}

fn build_query_sql_body(params: &QuerySqlParams) -> serde_json::Value {
    serde_json::json!({
        "sql": params.sql.clone(),
        "params": params.params.clone(),
        "resultSchema": params.result_schema.clone(),
    })
}

fn build_query_sql_raw_body(params: &QuerySqlRawParams) -> serde_json::Value {
    serde_json::json!({
        "sql": params.sql.clone(),
        "params": params.params.clone(),
    })
}

fn build_execute_sql_body(params: &ExecuteSqlParams) -> serde_json::Value {
    serde_json::json!({
        "sql": params.sql.clone(),
        "params": params.params.clone(),
    })
}

pub async fn query_sql(
    server: &SmoMcpServer,
    params: QuerySqlParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let path = with_connection_id_query(
        "/api/runtime/object-model/sql/query",
        params.connection_id.as_deref(),
    )?;
    let body = build_query_sql_body(&params);
    ensure_request_payload_reasonable("query_sql", &body)?;
    let result =
        with_payload_too_large_guidance(api_post(server, &path, Some(body)).await, "query_sql")?;
    let guidance = result_size_guidance(
        &result,
        "query_sql",
        None,
        &["rows"],
        &["rowCount", "row_count"],
    );
    json_result_with_guidance(result, guidance)
}

pub async fn query_sql_one(
    server: &SmoMcpServer,
    params: QuerySqlParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let path = with_connection_id_query(
        "/api/runtime/object-model/sql/query-one",
        params.connection_id.as_deref(),
    )?;
    let body = build_query_sql_body(&params);
    ensure_request_payload_reasonable("query_sql_one", &body)?;
    let result = with_payload_too_large_guidance(
        api_post(server, &path, Some(body)).await,
        "query_sql_one",
    )?;
    json_result(result)
}

pub async fn query_sql_raw(
    server: &SmoMcpServer,
    params: QuerySqlRawParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let path = with_connection_id_query(
        "/api/runtime/object-model/sql/query-raw",
        params.connection_id.as_deref(),
    )?;
    let body = build_query_sql_raw_body(&params);
    ensure_request_payload_reasonable("query_sql_raw", &body)?;
    let result = with_payload_too_large_guidance(
        api_post(server, &path, Some(body)).await,
        "query_sql_raw",
    )?;
    let guidance = result_size_guidance(
        &result,
        "query_sql_raw",
        None,
        &["rows"],
        &["rowCount", "row_count"],
    );
    json_result_with_guidance(result, guidance)
}

pub async fn execute_sql(
    server: &SmoMcpServer,
    params: ExecuteSqlParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let path = with_connection_id_query(
        "/api/runtime/object-model/sql/execute",
        params.connection_id.as_deref(),
    )?;
    let body = build_execute_sql_body(&params);
    ensure_request_payload_reasonable("execute_sql", &body)?;
    let result =
        with_payload_too_large_guidance(api_post(server, &path, Some(body)).await, "execute_sql")?;
    json_result(result)
}

pub async fn create_object_instance(
    server: &SmoMcpServer,
    params: CreateObjectInstanceParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let body = serde_json::json!({
        "schemaName": params.schema_name,
        "properties": params.properties,
    });
    ensure_request_payload_reasonable("create_object_instance", &body)?;
    let result = with_payload_too_large_guidance(
        api_post(server, "/api/runtime/object-model/instances", Some(body)).await,
        "create_object_instance",
    )?;
    json_result(result)
}

pub async fn update_object_instance(
    server: &SmoMcpServer,
    params: UpdateObjectInstanceParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("schema_id", &params.schema_id)?;
    validate_path_param("instance_id", &params.instance_id)?;
    let body = serde_json::json!({
        "properties": params.properties,
    });
    ensure_request_payload_reasonable("update_object_instance", &body)?;
    let result = with_payload_too_large_guidance(
        api_put(
            server,
            &format!(
                "/api/runtime/object-model/instances/{}/{}",
                params.schema_id, params.instance_id
            ),
            Some(body),
        )
        .await,
        "update_object_instance",
    )?;
    json_result(result)
}

pub async fn bulk_create_instances(
    server: &SmoMcpServer,
    params: BulkCreateInstancesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let schema_id = resolve_schema_id_by_name(server, &params.schema_name).await?;

    let mut body = serde_json::Map::new();
    if let Some(instances) = params.instances {
        body.insert("instances".to_string(), serde_json::Value::Array(instances));
    }
    if let Some(columns) = params.columns {
        body.insert("columns".to_string(), serde_json::json!(columns));
    }
    if let Some(rows) = params.rows {
        body.insert("rows".to_string(), serde_json::json!(rows));
    }
    if let Some(constants) = params.constants {
        body.insert(
            "constants".to_string(),
            serde_json::Value::Object(constants),
        );
    }
    if let Some(b) = params.nullify_empty_strings {
        body.insert(
            "nullifyEmptyStrings".to_string(),
            serde_json::Value::Bool(b),
        );
    }
    if let Some(s) = params.on_conflict {
        body.insert("onConflict".to_string(), serde_json::Value::String(s));
    }
    if let Some(s) = params.on_error {
        body.insert("onError".to_string(), serde_json::Value::String(s));
    }
    if let Some(cols) = params.conflict_columns {
        body.insert("conflictColumns".to_string(), serde_json::json!(cols));
    }

    let body = serde_json::Value::Object(body);
    ensure_request_payload_reasonable("bulk_create_instances", &body)?;
    let result = with_payload_too_large_guidance(
        api_post(
            server,
            &format!("/api/runtime/object-model/instances/{}/bulk", schema_id),
            Some(body),
        )
        .await,
        "bulk_create_instances",
    )?;
    json_result(result)
}

pub async fn bulk_update_instances(
    server: &SmoMcpServer,
    params: BulkUpdateInstancesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let schema_id = resolve_schema_id_by_name(server, &params.schema_name).await?;

    let body = match params.mode.as_str() {
        "byCondition" => {
            let condition = params.condition.ok_or_else(|| {
                rmcp::ErrorData::invalid_params(
                    "mode=byCondition requires `condition`".to_string(),
                    None,
                )
            })?;
            let condition = normalize_condition(condition)?;
            let properties = params.properties.ok_or_else(|| {
                rmcp::ErrorData::invalid_params(
                    "mode=byCondition requires `properties`".to_string(),
                    None,
                )
            })?;
            serde_json::json!({
                "mode": "byCondition",
                "condition": condition,
                "properties": properties,
            })
        }
        "byIds" => {
            let updates = params.updates.ok_or_else(|| {
                rmcp::ErrorData::invalid_params("mode=byIds requires `updates`".to_string(), None)
            })?;
            serde_json::json!({
                "mode": "byIds",
                "updates": updates,
            })
        }
        other => {
            return Err(rmcp::ErrorData::invalid_params(
                format!("unknown mode '{}': expected byCondition or byIds", other),
                None,
            ));
        }
    };

    ensure_request_payload_reasonable("bulk_update_instances", &body)?;
    let result = with_payload_too_large_guidance(
        api_patch(
            server,
            &format!("/api/runtime/object-model/instances/{}/bulk", schema_id),
            Some(body),
        )
        .await,
        "bulk_update_instances",
    )?;
    json_result(result)
}

pub async fn bulk_delete_instances(
    server: &SmoMcpServer,
    params: BulkDeleteInstancesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let schema_id = resolve_schema_id_by_name(server, &params.schema_name).await?;
    let body = serde_json::json!({ "instanceIds": params.instance_ids });
    ensure_request_payload_reasonable("bulk_delete_instances", &body)?;
    let result = with_payload_too_large_guidance(
        api_delete_with_body(
            server,
            &format!("/api/runtime/object-model/instances/{}/bulk", schema_id),
            Some(body),
        )
        .await,
        "bulk_delete_instances",
    )?;
    json_result(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn generated_property_schema<T: JsonSchema>(property: &str) -> Value {
        let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        schema
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|properties| properties.get(property))
            .cloned()
            .unwrap_or_else(|| panic!("missing property schema for {property}: {schema:#}"))
    }

    fn schema_allows_array(schema: &Value) -> bool {
        let has_array_type = match schema.get("type") {
            Some(Value::String(schema_type)) => schema_type == "array",
            Some(Value::Array(schema_types)) => schema_types.iter().any(|t| t == "array"),
            _ => false,
        };
        if has_array_type {
            return true;
        }

        ["anyOf", "oneOf", "allOf"].iter().any(|keyword| {
            schema
                .get(*keyword)
                .and_then(Value::as_array)
                .is_some_and(|variants| variants.iter().any(schema_allows_array))
        })
    }

    #[test]
    fn object_schema_tool_params_generate_array_schemas_for_lists() {
        let create_columns = generated_property_schema::<CreateObjectSchemaParams>("columns");
        let create_indexes = generated_property_schema::<CreateObjectSchemaParams>("indexes");
        let update_columns = generated_property_schema::<UpdateObjectSchemaParams>("columns");
        let update_indexes = generated_property_schema::<UpdateObjectSchemaParams>("indexes");

        assert!(
            schema_allows_array(&create_columns),
            "create columns schema should allow arrays: {create_columns:#}"
        );
        assert!(
            schema_allows_array(&create_indexes),
            "create indexes schema should allow arrays: {create_indexes:#}"
        );
        assert!(
            schema_allows_array(&update_columns),
            "update columns schema should allow arrays: {update_columns:#}"
        );
        assert!(
            schema_allows_array(&update_indexes),
            "update indexes schema should allow arrays: {update_indexes:#}"
        );
    }

    #[test]
    fn sql_tool_params_generate_array_schemas_for_typed_params() {
        let sql_params = generated_property_schema::<QuerySqlParams>("params");
        let raw_params = generated_property_schema::<QuerySqlRawParams>("params");
        let execute_params = generated_property_schema::<ExecuteSqlParams>("params");
        let result_schema = generated_property_schema::<QuerySqlParams>("resultSchema");

        assert!(
            schema_allows_array(&sql_params),
            "query_sql params schema should allow arrays: {sql_params:#}"
        );
        assert!(
            schema_allows_array(&raw_params),
            "query_sql_raw params schema should allow arrays: {raw_params:#}"
        );
        assert!(
            schema_allows_array(&execute_params),
            "execute_sql params schema should allow arrays: {execute_params:#}"
        );
        assert!(
            schema_allows_array(&result_schema),
            "query_sql resultSchema schema should allow arrays: {result_schema:#}"
        );
    }

    #[test]
    fn query_sql_body_uses_http_result_schema_wire_name() {
        let params = QuerySqlParams {
            sql: "SELECT $1 AS label".to_string(),
            params: vec![json!({"type": "string", "value": "alpha"})],
            result_schema: vec![json!({"name": "label", "type": "string"})],
            connection_id: Some("conn-1".to_string()),
        };

        assert_eq!(
            build_query_sql_body(&params),
            json!({
                "sql": "SELECT $1 AS label",
                "params": [{"type": "string", "value": "alpha"}],
                "resultSchema": [{"name": "label", "type": "string"}]
            })
        );
    }

    #[test]
    fn normalize_condition_preserves_canonical_condition() {
        let condition = json!({
            "op": "EQ",
            "arguments": ["status", "active"]
        });

        assert_eq!(
            normalize_condition(condition).unwrap(),
            json!({
                "op": "EQ",
                "arguments": ["status", "active"]
            })
        );
    }

    #[test]
    fn normalize_condition_accepts_workflow_condition_expression_shape() {
        let condition = json!({
            "type": "operation",
            "op": "Eq",
            "arguments": [
                {"valueType": "reference", "value": "status"},
                {"valueType": "immediate", "value": "active"}
            ]
        });

        assert_eq!(
            normalize_condition(condition).unwrap(),
            json!({
                "op": "EQ",
                "arguments": ["status", "active"]
            })
        );
    }

    #[test]
    fn normalize_condition_accepts_nested_workflow_condition_expression_shape() {
        let condition = json!({
            "type": "operation",
            "op": "Or",
            "arguments": [
                {
                    "type": "operation",
                    "op": "IsDefined",
                    "arguments": [{"valueType": "reference", "value": "email"}]
                },
                {
                    "type": "operation",
                    "op": "Eq",
                    "arguments": [
                        {"valueType": "reference", "value": "status"},
                        {"valueType": "immediate", "value": "pending"}
                    ]
                }
            ]
        });

        assert_eq!(
            normalize_condition(condition).unwrap(),
            json!({
                "op": "OR",
                "arguments": [
                    {"op": "IS_DEFINED", "arguments": ["email"]},
                    {"op": "EQ", "arguments": ["status", "pending"]}
                ]
            })
        );
    }

    #[test]
    fn query_object_instances_body_carries_score_expression_and_order_by() {
        let params = QueryObjectInstancesParams {
            schema_name: "UnspscNode".to_string(),
            condition: None,
            score_expression: Some(json!({
                "alias": "distance",
                "expression": {
                    "fn": "COSINE_DISTANCE",
                    "arguments": [
                        {"valueType": "reference", "value": "embedding"},
                        {"valueType": "immediate", "value": [0.1, 0.2, 0.3]}
                    ]
                }
            })),
            order_by: Some(json!([{
                "expression": {"kind": "alias", "name": "distance"},
                "direction": "ASC"
            }])),
            limit: Some(25),
            offset: Some(0),
        };

        let body = build_query_object_instances_body(&params).unwrap();

        assert_eq!(body["scoreExpression"]["alias"], "distance");
        assert_eq!(body["orderBy"][0]["expression"]["name"], "distance");
        assert_eq!(body["limit"], 25);
    }

    #[test]
    fn vector_columns_from_schema_response_finds_vector_columns() {
        let schema = json!({
            "success": true,
            "schema": {
                "columns": [
                    {"name": "commodityTitle", "type": "string"},
                    {"name": "embedding", "type": "vector", "dimension": 1536}
                ]
            }
        });

        assert_eq!(
            vector_columns_from_schema_response(&schema),
            vec!["embedding".to_string()]
        );
    }

    #[test]
    fn result_size_guidance_warns_for_partial_page() {
        let result = json!({
            "instances": [{ "id": "one" }],
            "totalCount": 2,
            "limit": 1,
            "offset": 0
        });

        let warnings = result_size_guidance(
            &result,
            "query_object_instances",
            Some(1),
            &["instances"],
            &["totalCount"],
        );

        assert!(warnings.iter().any(|warning| warning.contains("1 of 2")));
    }

    #[test]
    fn request_payload_guidance_rejects_oversized_payload() {
        let body = json!({ "blob": "x".repeat(MCP_REQUEST_PAYLOAD_GUIDANCE_BYTES + 1) });

        assert!(ensure_request_payload_reasonable("bulk_create_instances", &body).is_err());
    }
}
