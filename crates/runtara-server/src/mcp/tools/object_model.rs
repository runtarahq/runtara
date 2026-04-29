use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{
    api_delete, api_delete_with_body, api_get, api_patch, api_post, api_put, validate_path_param,
};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
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
    pub columns: serde_json::Value,
    #[schemars(description = "Index definitions as JSON array (optional)")]
    pub indexes: Option<serde_json::Value>,
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
    pub columns: Option<serde_json::Value>,
    #[schemars(
        description = "FULL replacement index list (same diff semantics as columns). \
                       Omit to leave indexes unchanged."
    )]
    pub indexes: Option<serde_json::Value>,
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
    #[schemars(description = "Max results")]
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
        description = "Optional top-level sort: [{column, direction}]. Each column \
                       must match a group_by column or an aggregate alias."
    )]
    pub order_by: Option<serde_json::Value>,
    #[schemars(
        description = "Max result rows (server caps at 100000). Omit to let the \
                       server return all rows — if the natural result exceeds the \
                       cap the request is rejected."
    )]
    pub limit: Option<i64>,
    #[schemars(description = "Pagination offset")]
    pub offset: Option<i64>,
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
        body["indexes"] = indexes;
    }
    let result = api_post(server, "/api/runtime/object-model/schemas", Some(body)).await?;
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
        body["columns"] = c;
    }
    if let Some(i) = params.indexes {
        body["indexes"] = i;
    }
    let result = api_put(
        server,
        &format!("/api/runtime/object-model/schemas/{}", id),
        Some(body),
    )
    .await?;
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
    let result = api_get(
        server,
        &format!(
            "/api/runtime/object-model/instances/schema/name/{}",
            params.schema_name
        ),
    )
    .await?;
    json_result(result)
}

pub async fn query_object_instances(
    server: &SmoMcpServer,
    params: QueryObjectInstancesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("schema_name", &params.schema_name)?;
    let mut body = serde_json::json!({});
    if let Some(condition) = params.condition {
        body["condition"] = condition;
    }
    if let Some(limit) = params.limit {
        body["limit"] = serde_json::json!(limit);
    }
    if let Some(offset) = params.offset {
        body["offset"] = serde_json::json!(offset);
    }
    let result = api_post(
        server,
        &format!(
            "/api/runtime/object-model/instances/schema/{}/filter",
            params.schema_name
        ),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn query_aggregate(
    server: &SmoMcpServer,
    params: QueryAggregateParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("schema_name", &params.schema_name)?;
    let mut body = serde_json::json!({ "aggregates": params.aggregates });
    if let Some(condition) = params.condition {
        body["condition"] = condition;
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
    let result = api_post(
        server,
        &format!(
            "/api/runtime/object-model/instances/schema/{}/aggregate",
            params.schema_name
        ),
        Some(body),
    )
    .await?;
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
    let result = api_post(server, "/api/runtime/object-model/instances", Some(body)).await?;
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
    let result = api_put(
        server,
        &format!(
            "/api/runtime/object-model/instances/{}/{}",
            params.schema_id, params.instance_id
        ),
        Some(body),
    )
    .await?;
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

    let result = api_post(
        server,
        &format!("/api/runtime/object-model/instances/{}/bulk", schema_id),
        Some(serde_json::Value::Object(body)),
    )
    .await?;
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

    let result = api_patch(
        server,
        &format!("/api/runtime/object-model/instances/{}/bulk", schema_id),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn bulk_delete_instances(
    server: &SmoMcpServer,
    params: BulkDeleteInstancesParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let schema_id = resolve_schema_id_by_name(server, &params.schema_name).await?;
    let body = serde_json::json!({ "instanceIds": params.instance_ids });
    let result = api_delete_with_body(
        server,
        &format!("/api/runtime/object-model/instances/{}/bulk", schema_id),
        Some(body),
    )
    .await?;
    json_result(result)
}
