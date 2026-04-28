use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::server::SmoMcpServer;
use super::internal_api::{
    api_delete, api_delete_with_body, api_get, api_patch, api_post, api_put, validate_path_param,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthoringSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
struct AuthoringIssue {
    severity: AuthoringSeverity,
    path: String,
    code: &'static str,
    message: String,
}

fn json_result(value: Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetReportAuthoringSchemaParams {
    #[schemars(
        description = "Optional Object Model schema name. When provided, the response includes its fields so report blocks can reference valid source/table/chart fields."
    )]
    pub object_schema: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListReportsParams {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetReportParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateReportParams {
    pub name: String,
    #[schemars(description = "URL-safe report slug. Omit to derive from name.")]
    pub slug: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    #[schemars(
        description = "Report status: draft, published, or archived. Defaults to published."
    )]
    pub status: Option<String>,
    #[schemars(
        description = "Full report definition: {definitionVersion, markdown, filters, blocks}. Every block must include a stable id."
    )]
    pub definition: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateReportParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    #[schemars(description = "Report status: draft, published, or archived.")]
    pub status: Option<String>,
    #[schemars(
        description = "Full replacement report definition: {definitionVersion, markdown, filters, blocks}. Use block mutation tools for atomic block edits."
    )]
    pub definition: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteReportParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateReportParams {
    #[schemars(description = "Report definition to validate before saving.")]
    pub definition: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderReportParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Global report filter values keyed by filter id.")]
    pub filters: Option<Value>,
    #[schemars(
        description = "Optional array of block data requests: [{id, page?, sort?, search?, blockFilters?}]. Omit to render non-lazy blocks."
    )]
    pub blocks: Option<Value>,
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetReportBlockDataParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable block id")]
    pub block_id: String,
    #[schemars(description = "Global report filter values keyed by filter id.")]
    pub filters: Option<Value>,
    #[schemars(description = "Pagination request: {offset, size}.")]
    pub page: Option<Value>,
    #[schemars(description = "Sort array: [{field, direction}].")]
    pub sort: Option<Value>,
    #[schemars(description = "Table search request: {query, fields?}.")]
    pub search: Option<Value>,
    #[schemars(description = "Per-block filter values keyed by filter id.")]
    pub block_filters: Option<Value>,
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddReportBlockParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Full block definition. The block must include a unique stable id.")]
    pub block: Value,
    #[schemars(
        description = "Insert at zero-based block index. Mutually exclusive with before_block_id and after_block_id."
    )]
    pub index: Option<usize>,
    #[schemars(
        description = "Insert before this block id. Mutually exclusive with index and after_block_id."
    )]
    pub before_block_id: Option<String>,
    #[schemars(
        description = "Insert after this block id. Mutually exclusive with index and before_block_id."
    )]
    pub after_block_id: Option<String>,
    #[schemars(
        description = "Also insert {{ block.<id> }} into report markdown. Defaults to true."
    )]
    pub insert_markdown_placeholder: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceReportBlockParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable block id to replace. The replacement block id must match.")]
    pub block_id: String,
    #[schemars(description = "Full replacement block definition.")]
    pub block: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchReportBlockParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable block id to update.")]
    pub block_id: String,
    #[schemars(
        description = "RFC 7386-style JSON merge patch applied to the block definition. The id field cannot be changed."
    )]
    pub patch: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveReportBlockParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable block id to move.")]
    pub block_id: String,
    #[schemars(
        description = "Move to zero-based block index. Mutually exclusive with before_block_id and after_block_id."
    )]
    pub index: Option<usize>,
    #[schemars(
        description = "Move before this block id. Mutually exclusive with index and after_block_id."
    )]
    pub before_block_id: Option<String>,
    #[schemars(
        description = "Move after this block id. Mutually exclusive with index and before_block_id."
    )]
    pub after_block_id: Option<String>,
    #[schemars(
        description = "Also move the existing {{ block.<id> }} markdown placeholder when present. Defaults to true."
    )]
    pub move_markdown_placeholder: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveReportBlockParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable block id to remove.")]
    pub block_id: String,
    #[schemars(
        description = "Also remove {{ block.<id> }} from report markdown. Defaults to true."
    )]
    pub remove_markdown_placeholder: Option<bool>,
}

// ===== Tool Implementations =====

pub async fn get_report_authoring_schema(
    server: &SmoMcpServer,
    params: GetReportAuthoringSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut result = report_authoring_schema();

    if let Some(object_schema) = params.object_schema {
        validate_path_param("object_schema", &object_schema)?;
        let schema = api_get(
            server,
            &format!("/api/runtime/object-model/schemas/name/{}", object_schema),
        )
        .await?;
        result["objectSchema"] = schema.get("schema").cloned().unwrap_or(schema);
    }

    json_result(result)
}

pub async fn list_reports(
    server: &SmoMcpServer,
    _params: ListReportsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_get(server, "/api/runtime/reports").await?;
    json_result(result)
}

pub async fn get_report(
    server: &SmoMcpServer,
    params: GetReportParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    let result = api_get(
        server,
        &format!("/api/runtime/reports/{}", params.report_id),
    )
    .await?;
    json_result(result)
}

pub async fn create_report(
    server: &SmoMcpServer,
    params: CreateReportParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let issues = collect_report_definition_authoring_issues(&params.definition);
    if authoring_errors(&issues).next().is_some() {
        return Err(authoring_invalid_params(issues));
    }

    let mut body = json!({
        "name": params.name,
        "definition": params.definition,
        "tags": params.tags.unwrap_or_default(),
    });
    if let Some(slug) = params.slug {
        body["slug"] = Value::String(slug);
    }
    if let Some(description) = params.description {
        body["description"] = Value::String(description);
    }
    if let Some(status) = params.status {
        body["status"] = Value::String(status);
    }

    let result = api_post(server, "/api/runtime/reports", Some(body)).await?;
    json_result(result)
}

pub async fn update_report(
    server: &SmoMcpServer,
    params: UpdateReportParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    let issues = collect_report_definition_authoring_issues(&params.definition);
    if authoring_errors(&issues).next().is_some() {
        return Err(authoring_invalid_params(issues));
    }

    let mut body = json!({
        "name": params.name,
        "slug": params.slug,
        "description": params.description,
        "tags": params.tags.unwrap_or_default(),
        "definition": params.definition,
    });
    if let Some(status) = params.status {
        body["status"] = Value::String(status);
    }

    let result = api_put(
        server,
        &format!("/api/runtime/reports/{}", params.report_id),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn delete_report(
    server: &SmoMcpServer,
    params: DeleteReportParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    let result = api_delete(
        server,
        &format!("/api/runtime/reports/{}", params.report_id),
    )
    .await?;
    json_result(result)
}

pub async fn validate_report(
    server: &SmoMcpServer,
    params: ValidateReportParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let issues = collect_report_definition_authoring_issues(&params.definition);
    if authoring_errors(&issues).next().is_some() {
        return json_result(authoring_validation_response(issues));
    }

    let result = api_post(
        server,
        "/api/runtime/reports/validate",
        Some(json!({ "definition": params.definition })),
    )
    .await?;
    let mut result = result;
    merge_authoring_issues(&mut result, issues);
    json_result(result)
}

pub async fn render_report(
    server: &SmoMcpServer,
    params: RenderReportParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    let mut body = json!({});
    if let Some(filters) = params.filters {
        body["filters"] = filters;
    }
    if let Some(blocks) = params.blocks {
        body["blocks"] = blocks;
    }
    if let Some(timezone) = params.timezone {
        body["timezone"] = Value::String(timezone);
    }

    let result = api_post(
        server,
        &format!("/api/runtime/reports/{}/render", params.report_id),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn get_report_block_data(
    server: &SmoMcpServer,
    params: GetReportBlockDataParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("block_id", &params.block_id)?;
    let mut body = json!({});
    if let Some(filters) = params.filters {
        body["filters"] = filters;
    }
    if let Some(page) = params.page {
        body["page"] = page;
    }
    if let Some(sort) = params.sort {
        body["sort"] = sort;
    }
    if let Some(search) = params.search {
        body["search"] = search;
    }
    if let Some(block_filters) = params.block_filters {
        body["blockFilters"] = block_filters;
    }
    if let Some(timezone) = params.timezone {
        body["timezone"] = Value::String(timezone);
    }

    let result = api_post(
        server,
        &format!(
            "/api/runtime/reports/{}/blocks/{}/data",
            params.report_id, params.block_id
        ),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn add_report_block(
    server: &SmoMcpServer,
    params: AddReportBlockParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    let mut issues = Vec::new();
    collect_report_block_authoring_issues("$.block", &params.block, true, &mut issues);
    if authoring_errors(&issues).next().is_some() {
        return Err(authoring_invalid_params(issues));
    }

    let mut body = json!({
        "block": params.block,
        "position": position_body(params.index, params.before_block_id, params.after_block_id),
    });
    if let Some(insert_markdown_placeholder) = params.insert_markdown_placeholder {
        body["insertMarkdownPlaceholder"] = Value::Bool(insert_markdown_placeholder);
    }

    let result = api_post(
        server,
        &format!("/api/runtime/reports/{}/blocks", params.report_id),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn replace_report_block(
    server: &SmoMcpServer,
    params: ReplaceReportBlockParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("block_id", &params.block_id)?;
    let mut issues = Vec::new();
    collect_report_block_authoring_issues("$.block", &params.block, true, &mut issues);
    if authoring_errors(&issues).next().is_some() {
        return Err(authoring_invalid_params(issues));
    }

    let result = api_put(
        server,
        &format!(
            "/api/runtime/reports/{}/blocks/{}",
            params.report_id, params.block_id
        ),
        Some(json!({ "block": params.block })),
    )
    .await?;
    json_result(result)
}

pub async fn patch_report_block(
    server: &SmoMcpServer,
    params: PatchReportBlockParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("block_id", &params.block_id)?;
    let mut issues = Vec::new();
    collect_report_block_authoring_issues("$.patch", &params.patch, false, &mut issues);
    if authoring_errors(&issues).next().is_some() {
        return Err(authoring_invalid_params(issues));
    }

    let result = api_patch(
        server,
        &format!(
            "/api/runtime/reports/{}/blocks/{}",
            params.report_id, params.block_id
        ),
        Some(json!({ "patch": params.patch })),
    )
    .await?;
    json_result(result)
}

pub async fn move_report_block(
    server: &SmoMcpServer,
    params: MoveReportBlockParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("block_id", &params.block_id)?;
    let mut body = json!({
        "position": position_body(params.index, params.before_block_id, params.after_block_id),
    });
    if let Some(move_markdown_placeholder) = params.move_markdown_placeholder {
        body["moveMarkdownPlaceholder"] = Value::Bool(move_markdown_placeholder);
    }

    let result = api_post(
        server,
        &format!(
            "/api/runtime/reports/{}/blocks/{}/move",
            params.report_id, params.block_id
        ),
        Some(body),
    )
    .await?;
    json_result(result)
}

pub async fn remove_report_block(
    server: &SmoMcpServer,
    params: RemoveReportBlockParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("block_id", &params.block_id)?;
    let mut body = json!({});
    if let Some(remove_markdown_placeholder) = params.remove_markdown_placeholder {
        body["removeMarkdownPlaceholder"] = Value::Bool(remove_markdown_placeholder);
    }

    let result = api_delete_with_body(
        server,
        &format!(
            "/api/runtime/reports/{}/blocks/{}",
            params.report_id, params.block_id
        ),
        Some(body),
    )
    .await?;
    json_result(result)
}

fn position_body(
    index: Option<usize>,
    before_block_id: Option<String>,
    after_block_id: Option<String>,
) -> Value {
    let mut position = serde_json::Map::new();
    if let Some(index) = index {
        position.insert("index".to_string(), json!(index));
    }
    if let Some(before_block_id) = before_block_id {
        position.insert("beforeBlockId".to_string(), Value::String(before_block_id));
    }
    if let Some(after_block_id) = after_block_id {
        position.insert("afterBlockId".to_string(), Value::String(after_block_id));
    }
    Value::Object(position)
}

fn report_authoring_schema() -> Value {
    json!({
        "definitionVersion": 1,
        "purpose": "Canonical MCP contract for authoring Runtara reports. Call this before create_report, update_report, add_report_block, replace_report_block, or patch_report_block.",
        "relatedTools": [
            "list_object_schemas",
            "get_object_schema",
            "query_aggregate",
            "validate_report",
            "render_report",
            "get_report_block_data"
        ],
        "definitionShape": {
            "definitionVersion": 1,
            "markdown": "Markdown body. Render data blocks with placeholders like {{ block.daily_qty }}.",
            "filters": "Optional global filter presets. Each filter can apply to one or more block/source fields.",
            "blocks": "Array of typed block definitions. Every block must have a stable id for MCP block mutations."
        },
        "blockShape": {
            "common": {
                "id": "Stable id, unique within the report. Referenced as {{ block.<id> }} in markdown.",
                "type": "table | chart | metric | markdown",
                "title": "Optional UI title.",
                "lazy": "Optional boolean. Lazy blocks fetch only when requested.",
                "source": "Object Model data source and query plan.",
                "filters": "Optional per-block filter presets."
            },
            "table": {
                "type": "table",
                "configKey": "table",
                "columnsPath": "table.columns",
                "columns": [{"field": "sku", "label": "SKU", "format": "optional formatter"}],
                "defaultSort": [{"field": "sku", "direction": "asc"}],
                "pagination": {"defaultPageSize": 50, "allowedPageSizes": [25, 50, 100]},
                "note": "Tables currently render row-level Object Model data through source.mode='filter'. Configure visible/searchable/sortable fields in table.columns."
            },
            "chart": {
                "type": "chart",
                "configKey": "chart",
                "required": {
                    "chart.kind": "line | bar | area | pie | donut",
                    "chart.x": "Output field for the x/name axis, usually a source.groupBy field.",
                    "chart.series": "Array of output value fields, usually aggregate aliases."
                }
            },
            "metric": {
                "type": "metric",
                "configKey": "metric",
                "required": {
                    "metric.valueField": "Output field to display, usually an aggregate alias."
                }
            }
        },
        "sourceShape": {
            "schema": "Object Model schema name. Use get_object_schema to inspect valid fields.",
            "connectionId": "Optional connection id for connection-scoped schemas.",
            "mode": "filter | aggregate",
            "condition": "Optional Object Model condition DSL.",
            "filterMappings": "Optional mappings from global filter ids to source fields.",
            "groupBy": "Aggregate output grouping fields.",
            "aggregates": "Aggregate specs. Report aggregate specs use {alias, op, field?, distinct?, orderBy?, expression?}. Use op/field here, not fn/column.",
            "orderBy": "Sort array using {field, direction}. Field must be a row field, groupBy field, or aggregate alias depending on source mode.",
            "limit": "Optional row/group cap."
        },
        "aggregateOps": {
            "core": ["count", "sum", "avg", "min", "max", "first_value", "last_value", "expr"],
            "expr": {
                "canonical": {"op": "SUB", "arguments": [{"valueType": "alias", "value": "last_qty"}, {"valueType": "alias", "value": "first_qty"}]},
                "notes": [
                    "EXPR can reference earlier aggregate aliases and immediate constants.",
                    "Workflow-style operation nodes and value_type/valueType casing are normalized by the report API, but canonical report JSON should use op + valueType."
                ]
            }
        },
        "filterShape": {
            "types": ["select", "multi_select", "radio", "checkbox", "time_range", "number_range", "text", "search"],
            "example": {
                "id": "vendor",
                "label": "Vendor",
                "type": "select",
                "options": [{"label": "TD Synnex", "value": "TD Synnex"}],
                "appliesTo": [{"blockId": "products", "field": "vendor", "op": "eq"}]
            }
        },
        "fieldRules": [
            "For table.columns, use Object Model row fields when source.mode='filter'.",
            "For chart.x, use an aggregate output field, usually a source.groupBy field.",
            "For chart.series[].field and metric.valueField, use aggregate aliases from source.aggregates.",
            "For source.orderBy and table.defaultSort, use field, not column."
        ],
        "commonMistakes": [
            "Do not put columns at block.columns, block.fields, or source.columns. Use block.table.columns.",
            "Do not put chartType, x, or y at block top-level. Use block.chart.kind, block.chart.x, and block.chart.series[].field.",
            "Do not use metric.valueAlias or top-level valueAlias. Use block.metric.valueField.",
            "Do not copy query_aggregate specs directly: report aggregates use op/field while query_aggregate uses fn/column.",
            "Always run validate_report before saving or mutating report blocks."
        ],
        "examples": {
            "table": {
                "id": "products",
                "type": "table",
                "title": "Products",
                "source": {
                    "schema": "TDProduct",
                    "mode": "filter",
                    "orderBy": [{"field": "sku", "direction": "asc"}],
                    "limit": 100
                },
                "table": {
                    "columns": [
                        {"field": "sku", "label": "SKU"},
                        {"field": "vendor", "label": "Vendor"}
                    ],
                    "defaultSort": [{"field": "sku", "direction": "asc"}],
                    "pagination": {"defaultPageSize": 50, "allowedPageSizes": [25, 50, 100]}
                }
            },
            "chart": {
                "id": "daily_qty",
                "type": "chart",
                "title": "Daily quantity",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "aggregate",
                    "groupBy": ["snapshot_date"],
                    "aggregates": [{"alias": "qty_total", "op": "sum", "field": "qty"}],
                    "orderBy": [{"field": "snapshot_date", "direction": "asc"}]
                },
                "chart": {
                    "kind": "line",
                    "x": "snapshot_date",
                    "series": [{"field": "qty_total", "label": "Qty"}]
                }
            },
            "metric": {
                "id": "total_snaps",
                "type": "metric",
                "title": "Stock snapshots",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "aggregate",
                    "aggregates": [{"alias": "n", "op": "count"}]
                },
                "metric": {"valueField": "n", "label": "Stock snapshots", "format": "number"}
            },
            "exprAggregate": {
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "aggregate",
                    "groupBy": ["sku"],
                    "aggregates": [
                        {"alias": "first_qty", "op": "first_value", "field": "qty", "orderBy": [{"field": "snapshot_date", "direction": "asc"}]},
                        {"alias": "last_qty", "op": "last_value", "field": "qty", "orderBy": [{"field": "snapshot_date", "direction": "asc"}]},
                        {"alias": "delta", "op": "expr", "expression": {"op": "SUB", "arguments": [{"valueType": "alias", "value": "last_qty"}, {"valueType": "alias", "value": "first_qty"}]}},
                        {"alias": "delta_abs", "op": "expr", "expression": {"op": "ABS", "arguments": [{"valueType": "alias", "value": "delta"}]}}
                    ],
                    "orderBy": [{"field": "delta_abs", "direction": "desc"}],
                    "limit": 100
                }
            }
        }
    })
}

fn collect_report_definition_authoring_issues(definition: &Value) -> Vec<AuthoringIssue> {
    let mut issues = Vec::new();
    collect_unknown_keys(
        "$",
        definition,
        &["definitionVersion", "markdown", "filters", "blocks"],
        &mut issues,
    );

    if let Some(blocks) = definition.get("blocks") {
        match blocks.as_array() {
            Some(blocks) => {
                for (index, block) in blocks.iter().enumerate() {
                    collect_report_block_authoring_issues(
                        &format!("$.blocks[{index}]"),
                        block,
                        true,
                        &mut issues,
                    );
                }
            }
            None => issues.push(error(
                "$.blocks",
                "INVALID_BLOCKS",
                "Report definition blocks must be an array.",
            )),
        }
    }

    issues
}

fn collect_report_block_authoring_issues(
    path: &str,
    block: &Value,
    full_block: bool,
    issues: &mut Vec<AuthoringIssue>,
) {
    let Some(block_object) = block.as_object() else {
        issues.push(error(
            path,
            "INVALID_BLOCK",
            "Report block must be an object.",
        ));
        return;
    };

    for key in block_object.keys() {
        if [
            "id", "type", "title", "lazy", "source", "table", "chart", "metric", "filters",
        ]
        .contains(&key.as_str())
        {
            continue;
        }

        let key_path = format!("{path}.{key}");
        match key.as_str() {
            "columns" | "fields" => issues.push(error(
                &key_path,
                "MISPLACED_TABLE_COLUMNS",
                "Table columns must be configured at table.columns. Top-level columns/fields are ignored and render as an empty table.",
            )),
            "chartType" | "x" | "y" => issues.push(error(
                &key_path,
                "MISPLACED_CHART_CONFIG",
                "Chart configuration must be nested under chart: use chart.kind, chart.x, and chart.series[].field.",
            )),
            "label" | "valueAlias" | "valueField" | "format" => issues.push(error(
                &key_path,
                "MISPLACED_METRIC_CONFIG",
                "Metric display configuration must be nested under metric. Use metric.valueField, metric.label, and metric.format.",
            )),
            _ => issues.push(error(
                &key_path,
                "UNKNOWN_REPORT_BLOCK_FIELD",
                format!("Unknown report block field '{key}'. The report API ignores unknown block fields; use get_report_authoring_schema for the canonical shape."),
            )),
        }
    }

    if full_block {
        if block
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            issues.push(error(
                path,
                "MISSING_BLOCK_ID",
                "Report block must include a non-empty stable id.",
            ));
        }
        if block.get("type").and_then(Value::as_str).is_none() {
            issues.push(error(
                path,
                "MISSING_BLOCK_TYPE",
                "Report block must include type: table, chart, metric, or markdown.",
            ));
        }
        match block.get("source") {
            Some(source) if source.is_object() => {
                if source
                    .get("schema")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    issues.push(error(
                        format!("{path}.source.schema"),
                        "MISSING_SOURCE_SCHEMA",
                        "Report block source must include an Object Model schema name.",
                    ));
                }
            }
            _ => issues.push(error(
                format!("{path}.source"),
                "MISSING_BLOCK_SOURCE",
                "Report block must include source with at least {schema}.",
            )),
        }
    }

    if let Some(source) = block.get("source") {
        collect_source_issues(&format!("{path}.source"), source, issues);
    }
    if let Some(table) = block.get("table") {
        collect_table_issues(&format!("{path}.table"), table, issues);
    }
    if let Some(chart) = block.get("chart") {
        collect_chart_issues(&format!("{path}.chart"), chart, issues);
    }
    if let Some(metric) = block.get("metric") {
        collect_metric_issues(&format!("{path}.metric"), metric, issues);
    }

    if !full_block {
        return;
    }

    match block.get("type").and_then(Value::as_str) {
        Some("table") => {
            let has_columns = block
                .get("table")
                .and_then(|table| table.get("columns"))
                .and_then(Value::as_array)
                .is_some_and(|columns| !columns.is_empty());
            if !has_columns {
                issues.push(error(
                    format!("{path}.table.columns"),
                    "MISSING_TABLE_COLUMNS",
                    "Table blocks must define table.columns; otherwise the UI renders 'This table has no configured columns.'",
                ));
            }
            if block
                .get("source")
                .and_then(|source| source.get("mode"))
                .and_then(Value::as_str)
                == Some("aggregate")
            {
                issues.push(warning(
                    format!("{path}.source.mode"),
                    "AGGREGATE_TABLE_RENDERING",
                    "Table blocks currently render row-level Object Model data. Use chart or metric for aggregate sources, or add aggregate table support before relying on this shape.",
                ));
            }
        }
        Some("chart") => {
            let Some(chart) = block.get("chart").filter(|chart| chart.is_object()) else {
                issues.push(error(
                    format!("{path}.chart"),
                    "MISSING_CHART_CONFIG",
                    "Chart blocks must include chart.kind, chart.x, and preferably chart.series.",
                ));
                return;
            };
            if chart.get("kind").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.chart.kind"),
                    "MISSING_CHART_KIND",
                    "Chart blocks must set chart.kind: line, bar, area, pie, or donut.",
                ));
            }
            if chart.get("x").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.chart.x"),
                    "MISSING_CHART_X",
                    "Chart blocks must set chart.x to an output field, usually a source.groupBy field.",
                ));
            }
            if block
                .get("source")
                .and_then(|source| source.get("aggregates"))
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
            {
                issues.push(error(
                    format!("{path}.source.aggregates"),
                    "MISSING_CHART_AGGREGATES",
                    "Chart blocks need source.aggregates so the renderer has value series to plot.",
                ));
            }
        }
        Some("metric") => {
            let Some(metric) = block.get("metric").filter(|metric| metric.is_object()) else {
                issues.push(error(
                    format!("{path}.metric"),
                    "MISSING_METRIC_CONFIG",
                    "Metric blocks must include metric.valueField.",
                ));
                return;
            };
            if metric.get("valueField").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.metric.valueField"),
                    "MISSING_METRIC_VALUE_FIELD",
                    "Metric blocks must set metric.valueField to an aggregate alias.",
                ));
            }
            if block
                .get("source")
                .and_then(|source| source.get("aggregates"))
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
            {
                issues.push(error(
                    format!("{path}.source.aggregates"),
                    "MISSING_METRIC_AGGREGATES",
                    "Metric blocks need source.aggregates so metric.valueField has a value.",
                ));
            }
        }
        _ => {}
    }
}

fn collect_source_issues(path: &str, source: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys_with_messages(
        path,
        source,
        &[
            "schema",
            "connectionId",
            "mode",
            "condition",
            "filterMappings",
            "groupBy",
            "aggregates",
            "orderBy",
            "limit",
        ],
        |key| match key {
            "columns" => Some((
                "MISPLACED_TABLE_COLUMNS",
                "Table columns must be configured at table.columns, not source.columns.",
            )),
            "group_by" => Some(("MISNAMED_SOURCE_GROUP_BY", "Use groupBy, not group_by.")),
            "order_by" => Some(("MISNAMED_SOURCE_ORDER_BY", "Use orderBy, not order_by.")),
            _ => None,
        },
        issues,
    );

    if let Some(aggregates) = source.get("aggregates").and_then(Value::as_array) {
        for (index, aggregate) in aggregates.iter().enumerate() {
            collect_aggregate_issues(&format!("{path}.aggregates[{index}]"), aggregate, issues);
        }
    }

    if let Some(order_by) = source.get("orderBy").and_then(Value::as_array) {
        for (index, order) in order_by.iter().enumerate() {
            collect_order_by_issues(&format!("{path}.orderBy[{index}]"), order, issues);
        }
    }
}

fn collect_aggregate_issues(path: &str, aggregate: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys_with_messages(
        path,
        aggregate,
        &[
            "alias",
            "op",
            "field",
            "distinct",
            "orderBy",
            "expression",
            "percentile",
        ],
        |key| match key {
            "fn" => Some(("MISNAMED_AGGREGATE_OP", "Report aggregates use op, not fn.")),
            "column" => Some((
                "MISNAMED_AGGREGATE_FIELD",
                "Report aggregates use field, not column.",
            )),
            "order_by" => Some(("MISNAMED_AGGREGATE_ORDER_BY", "Use orderBy, not order_by.")),
            _ => None,
        },
        issues,
    );

    if let Some(order_by) = aggregate.get("orderBy").and_then(Value::as_array) {
        for (index, order) in order_by.iter().enumerate() {
            collect_order_by_issues(&format!("{path}.orderBy[{index}]"), order, issues);
        }
    }
}

fn collect_table_issues(path: &str, table: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys_with_messages(
        path,
        table,
        &["columns", "defaultSort", "pagination"],
        |key| match key {
            "fields" => Some((
                "MISNAMED_TABLE_COLUMNS",
                "Use table.columns, not table.fields.",
            )),
            "default_sort" => Some(("MISNAMED_TABLE_SORT", "Use defaultSort, not default_sort.")),
            _ => None,
        },
        issues,
    );

    if let Some(columns) = table.get("columns").and_then(Value::as_array) {
        for (index, column) in columns.iter().enumerate() {
            collect_unknown_keys(
                &format!("{path}.columns[{index}]"),
                column,
                &["field", "label", "format"],
                issues,
            );
            if column.get("field").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.columns[{index}].field"),
                    "MISSING_TABLE_COLUMN_FIELD",
                    "Each table column must include field.",
                ));
            }
        }
    }

    if let Some(default_sort) = table.get("defaultSort").and_then(Value::as_array) {
        for (index, order) in default_sort.iter().enumerate() {
            collect_order_by_issues(&format!("{path}.defaultSort[{index}]"), order, issues);
        }
    }

    if let Some(pagination) = table.get("pagination") {
        collect_unknown_keys(
            &format!("{path}.pagination"),
            pagination,
            &["defaultPageSize", "allowedPageSizes"],
            issues,
        );
    }
}

fn collect_chart_issues(path: &str, chart: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys_with_messages(
        path,
        chart,
        &["kind", "x", "series"],
        |key| match key {
            "chartType" => Some((
                "MISNAMED_CHART_KIND",
                "Use chart.kind, not chart.chartType.",
            )),
            "y" => Some((
                "MISNAMED_CHART_SERIES",
                "Use chart.series[].field for y values.",
            )),
            _ => None,
        },
        issues,
    );

    if let Some(series) = chart.get("series").and_then(Value::as_array) {
        for (index, entry) in series.iter().enumerate() {
            collect_unknown_keys(
                &format!("{path}.series[{index}]"),
                entry,
                &["field", "label"],
                issues,
            );
            if entry.get("field").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.series[{index}].field"),
                    "MISSING_CHART_SERIES_FIELD",
                    "Each chart series entry must include field.",
                ));
            }
        }
    }
}

fn collect_metric_issues(path: &str, metric: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys_with_messages(
        path,
        metric,
        &["valueField", "label", "format"],
        |key| match key {
            "valueAlias" => Some((
                "MISNAMED_METRIC_VALUE_FIELD",
                "Use metric.valueField, not metric.valueAlias.",
            )),
            _ => None,
        },
        issues,
    );
}

fn collect_order_by_issues(path: &str, order_by: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys_with_messages(
        path,
        order_by,
        &["field", "direction"],
        |key| match key {
            "column" => Some((
                "MISNAMED_ORDER_FIELD",
                "Report orderBy entries use field, not column.",
            )),
            _ => None,
        },
        issues,
    );
}

fn collect_unknown_keys(
    path: &str,
    value: &Value,
    allowed: &[&str],
    issues: &mut Vec<AuthoringIssue>,
) {
    collect_unknown_keys_with_messages(path, value, allowed, |_| None, issues);
}

fn collect_unknown_keys_with_messages<F>(
    path: &str,
    value: &Value,
    allowed: &[&str],
    message_for_key: F,
    issues: &mut Vec<AuthoringIssue>,
) where
    F: Fn(&str) -> Option<(&'static str, &'static str)>,
{
    let Some(object) = value.as_object() else {
        return;
    };

    for key in object.keys() {
        if allowed.contains(&key.as_str()) {
            continue;
        }

        let key_path = format!("{path}.{key}");
        if let Some((code, message)) = message_for_key(key) {
            issues.push(error(&key_path, code, message));
        } else {
            issues.push(error(
                &key_path,
                "UNKNOWN_REPORT_FIELD",
                format!("Unknown report field '{key}'. Use get_report_authoring_schema for the canonical shape."),
            ));
        }
    }
}

fn authoring_errors(issues: &[AuthoringIssue]) -> impl Iterator<Item = &AuthoringIssue> {
    issues
        .iter()
        .filter(|issue| issue.severity == AuthoringSeverity::Error)
}

fn authoring_invalid_params(issues: Vec<AuthoringIssue>) -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params(
        "Report definition does not match the MCP report authoring schema. Call get_report_authoring_schema and fix the reported paths.",
        Some(authoring_validation_response(issues)),
    )
}

fn authoring_validation_response(issues: Vec<AuthoringIssue>) -> Value {
    let (errors, warnings) = split_authoring_issues(issues);
    json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "hint": "Call get_report_authoring_schema for canonical table/chart/metric block shapes and use get_object_schema for valid Object Model fields."
    })
}

fn merge_authoring_issues(result: &mut Value, issues: Vec<AuthoringIssue>) {
    let (errors, warnings) = split_authoring_issues(issues);
    if !errors.is_empty() {
        result["valid"] = Value::Bool(false);
    }
    append_validation_issues(result, "errors", errors);
    append_validation_issues(result, "warnings", warnings);
}

fn append_validation_issues(result: &mut Value, key: &str, issues: Vec<Value>) {
    if issues.is_empty() {
        return;
    }
    if !result.get(key).is_some_and(Value::is_array) {
        result[key] = json!([]);
    }
    if let Some(existing) = result.get_mut(key).and_then(Value::as_array_mut) {
        existing.extend(issues);
    }
}

fn split_authoring_issues(issues: Vec<AuthoringIssue>) -> (Vec<Value>, Vec<Value>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    for issue in issues {
        let value = json!({
            "path": issue.path,
            "code": issue.code,
            "message": issue.message,
        });
        match issue.severity {
            AuthoringSeverity::Error => errors.push(value),
            AuthoringSeverity::Warning => warnings.push(value),
        }
    }

    (errors, warnings)
}

fn error(
    path: impl Into<String>,
    code: &'static str,
    message: impl Into<String>,
) -> AuthoringIssue {
    AuthoringIssue {
        severity: AuthoringSeverity::Error,
        path: path.into(),
        code,
        message: message.into(),
    }
}

fn warning(
    path: impl Into<String>,
    code: &'static str,
    message: impl Into<String>,
) -> AuthoringIssue {
    AuthoringIssue {
        severity: AuthoringSeverity::Warning,
        path: path.into(),
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_authoring_lints_misplaced_table_columns() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.products }}",
            "blocks": [{
                "id": "products",
                "type": "table",
                "columns": [{"field": "sku"}],
                "source": {"schema": "TDProduct", "mode": "filter"}
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"MISPLACED_TABLE_COLUMNS"));
        assert!(codes.contains(&"MISSING_TABLE_COLUMNS"));
    }

    #[test]
    fn report_authoring_lints_chart_top_level_axes() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.daily_qty }}",
            "blocks": [{
                "id": "daily_qty",
                "type": "chart",
                "chartType": "line",
                "x": "snapshot_date",
                "y": "qty_total",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "aggregate",
                    "groupBy": ["snapshot_date"],
                    "aggregates": [{"alias": "qty_total", "op": "sum", "field": "qty"}]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"MISPLACED_CHART_CONFIG"));
        assert!(codes.contains(&"MISSING_CHART_CONFIG"));
    }

    #[test]
    fn report_authoring_accepts_canonical_chart_shape() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.daily_qty }}",
            "blocks": [{
                "id": "daily_qty",
                "type": "chart",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "aggregate",
                    "groupBy": ["snapshot_date"],
                    "aggregates": [{"alias": "qty_total", "op": "sum", "field": "qty"}],
                    "orderBy": [{"field": "snapshot_date", "direction": "asc"}]
                },
                "chart": {
                    "kind": "line",
                    "x": "snapshot_date",
                    "series": [{"field": "qty_total", "label": "Qty"}]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);

        assert!(authoring_errors(&issues).next().is_none());
    }

    fn issue_codes(issues: &[AuthoringIssue]) -> Vec<&'static str> {
        issues.iter().map(|issue| issue.code).collect()
    }
}
