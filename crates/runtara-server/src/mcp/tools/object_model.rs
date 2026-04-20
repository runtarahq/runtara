use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post, api_put, validate_path_param};

fn json_result(value: serde_json::Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetObjectSchemaParams {
    #[schemars(description = "Schema name")]
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
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
pub struct ListObjectInstancesParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
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
                       orderBy?, expression?}]. `fn` is one of COUNT, SUM, MIN, MAX, \
                       FIRST_VALUE, LAST_VALUE, EXPR. Each alias must be a unique \
                       [a-zA-Z_][a-zA-Z0-9_]* identifier. `column` is optional for \
                       COUNT (→ COUNT(*)) and required for SUM/MIN/MAX/FIRST_VALUE/\
                       LAST_VALUE; must be omitted for EXPR. `distinct: true` is \
                       valid only with COUNT + column. FIRST_VALUE/LAST_VALUE require \
                       non-empty orderBy: [{column, direction: ASC|DESC}]. EXPR \
                       requires `expression`: a tree over previously-declared aliases \
                       and constants. Operators: arithmetic (ADD, SUB, MUL, DIV, NEG, \
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
pub struct CreateObjectInstanceParams {
    #[schemars(description = "Schema name")]
    pub schema_name: String,
    #[schemars(description = "Instance properties as JSON object")]
    pub properties: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateObjectInstanceParams {
    #[schemars(description = "Schema ID")]
    pub schema_id: String,
    #[schemars(description = "Instance ID")]
    pub instance_id: String,
    #[schemars(description = "Updated properties as JSON object")]
    pub properties: serde_json::Value,
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
