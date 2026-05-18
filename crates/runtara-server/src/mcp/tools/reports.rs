use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::services::reports::ReportService;

use super::super::server::SmoMcpServer;
use super::internal_api::{
    api_delete, api_get, api_post, api_put, normalize_json_arg, validate_path_param,
};

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
pub struct GetReportDefinitionSchemaParams {}

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
        description = "Full report definition: {definitionVersion, layout?, filters, datasets?, blocks}. For BI reports, define datasets and let blocks reference them. Every block must include a stable id; every layout node must include a stable id."
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
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
        description = "Full replacement report definition: {definitionVersion, layout?, filters, datasets?, blocks}. Use datasets for BI reports and block/layout mutation tools for atomic edits."
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub definition: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteReportParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportValidationMode {
    Syntax,
    Semantic,
    All,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidateReportParams {
    #[schemars(description = "Report definition to validate before saving.")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub definition: Value,
    #[schemars(
        description = "Validation mode: syntax runs JSON Schema only; semantic runs backend tenant-reference checks; all runs MCP authoring checks plus backend validation. Defaults to all."
    )]
    pub mode: Option<ReportValidationMode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderReportParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Global report filter values keyed by filter id.")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub filters: Option<Value>,
    #[schemars(
        description = "Optional array of block data requests: [{id, page?, sort?, search?, blockFilters?}]. Omit to render non-lazy blocks."
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_array_schema")]
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
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub filters: Option<Value>,
    #[schemars(description = "Pagination request: {offset, size}.")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub page: Option<Value>,
    #[schemars(description = "Sort array: [{field, direction}].")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_array_schema")]
    pub sort: Option<Value>,
    #[schemars(description = "Table search request: {query, fields?}.")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub search: Option<Value>,
    #[schemars(description = "Per-block filter values keyed by filter id.")]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_object_schema")]
    pub block_filters: Option<Value>,
    pub timezone: Option<String>,
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

pub async fn get_report_definition_schema(
    _server: &SmoMcpServer,
    _params: GetReportDefinitionSchemaParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    json_result(json!({
        "success": true,
        "kind": "json_schema",
        "schema": ReportService::report_definition_json_schema(),
        "hint": "This is the machine JSON Schema for report.definition. Use get_report_authoring_schema for AI-oriented examples and validate_report mode='all' before saving."
    }))
}

pub async fn list_reports(
    server: &SmoMcpServer,
    _params: ListReportsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_get(server, "/api/runtime/reports").await?;
    json_result(result)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListReportsNeedingReAuthoringParams {}

/// Returns only reports whose stored definition failed to deserialize
/// into the current `ReportDefinition` shape. Each entry includes the
/// repo's reported parse error in `needsReAuthoring` so the operator
/// can decide whether to delete or re-author through MCP.
pub async fn list_reports_needing_re_authoring(
    server: &SmoMcpServer,
    _params: ListReportsNeedingReAuthoringParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_get(server, "/api/runtime/reports").await?;
    let reports = result
        .get("reports")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let unsupported: Vec<Value> = reports
        .into_iter()
        .filter(|report| {
            report
                .get("needsReAuthoring")
                .map(|value| !value.is_null())
                .unwrap_or(false)
        })
        .collect();
    json_result(json!({
        "success": true,
        "count": unsupported.len(),
        "reports": unsupported,
        "hint": "Each entry's `needsReAuthoring` field carries the parser error. Use get_report to fetch the raw stored JSON, then call create_report or update_report with a re-authored ReportDefinition."
    }))
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
    let definition = normalize_json_arg(params.definition, "definition")?;

    let mut body = json!({
        "name": params.name,
        "definition": definition,
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
    let definition = normalize_json_arg(params.definition, "definition")?;

    let mut body = json!({
        "name": params.name,
        "slug": params.slug,
        "description": params.description,
        "tags": params.tags.unwrap_or_default(),
        "definition": definition,
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
    let mode = params.mode.unwrap_or(ReportValidationMode::All);
    let definition = normalize_json_arg(params.definition, "definition")?;
    if mode == ReportValidationMode::Syntax {
        let errors = ReportService::validate_report_definition_json_syntax_issues(&definition)
            .map_err(|err| {
                rmcp::ErrorData::internal_error(
                    "Could not run report JSON Schema validation.",
                    Some(json!({ "message": err.to_string() })),
                )
            })?;
        return json_result(json!({
            "valid": errors.is_empty(),
            "mode": "syntax",
            "errors": errors,
            "warnings": [],
            "hint": "Syntax mode validates the generated JSON Schema only. Use mode='all' before saving."
        }));
    }

    let mut result = api_post(
        server,
        "/api/runtime/reports/validate",
        Some(json!({ "definition": definition })),
    )
    .await?;
    result["mode"] = json!(match mode {
        ReportValidationMode::Syntax => "syntax",
        ReportValidationMode::Semantic => "semantic",
        ReportValidationMode::All => "all",
    });
    if mode == ReportValidationMode::All {
        let lint_warnings = runtara_report_dsl::lint::lint(&definition);
        if !lint_warnings.is_empty() {
            let warning_values = lint_warnings
                .iter()
                .map(|issue| {
                    json!({
                        "path": issue.path,
                        "code": issue.code,
                        "message": issue.message,
                        "hint": issue.hint,
                    })
                })
                .collect::<Vec<_>>();
            if !result.get("warnings").is_some_and(Value::is_array) {
                result["warnings"] = json!([]);
            }
            if let Some(existing) = result.get_mut("warnings").and_then(Value::as_array_mut) {
                existing.extend(warning_values);
            }
        }
    }
    json_result(result)
}

pub async fn render_report(
    server: &SmoMcpServer,
    params: RenderReportParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    let mut body = json!({});
    if let Some(filters) = params.filters {
        body["filters"] = normalize_json_object_arg(filters, "filters")?;
    }
    if let Some(blocks) = params.blocks {
        body["blocks"] = normalize_json_array_arg(blocks, "blocks")?;
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
        body["filters"] = normalize_json_object_arg(filters, "filters")?;
    }
    if let Some(page) = params.page {
        body["page"] = normalize_json_object_arg(page, "page")?;
    }
    if let Some(sort) = params.sort {
        body["sort"] = normalize_json_array_arg(sort, "sort")?;
    }
    if let Some(search) = params.search {
        body["search"] = normalize_json_object_arg(search, "search")?;
    }
    if let Some(block_filters) = params.block_filters {
        body["blockFilters"] = normalize_json_object_arg(block_filters, "block_filters")?;
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditReportParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(
        description = "Batch of ReportEditOp objects applied atomically. Each op is \
                       `{ kind: add_block | replace_block | patch_block | move_block | \
                       remove_block | add_layout_node | replace_layout_node | \
                       patch_layout_node | move_layout_node | remove_layout_node, ... }`. \
                       See get_report_authoring_schema for the op shapes."
    )]
    #[schemars(schema_with = "crate::mcp::tools::internal_api::json_array_schema")]
    pub ops: Value,
}

/// Phase 6 canonical edit-report MCP tool — accepts a batch of edit ops
/// and POSTs them to `/api/runtime/reports/{id}/edit`. This is the only
/// authoring path for targeted block + layout mutations; the per-op
/// wrapper tools (add_report_block, add_report_layout_node, …) were
/// removed in the Phase 6/8 collapse. Callers compose multi-op batches
/// directly.
pub async fn edit_report(
    server: &SmoMcpServer,
    params: EditReportParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    let ops = normalize_json_array_arg(params.ops, "ops")?;
    let result = api_post(
        server,
        &format!("/api/runtime/reports/{}/edit", params.report_id),
        Some(json!({ "ops": ops })),
    )
    .await?;
    json_result(result)
}

fn normalize_json_object_arg(value: Value, field: &str) -> Result<Value, rmcp::ErrorData> {
    let normalized = normalize_json_arg(value, field)?;
    if normalized.is_object() {
        Ok(normalized)
    } else {
        Err(rmcp::ErrorData::invalid_params(
            format!(
                "{} must be a JSON object. If your MCP gateway stringifies arguments, pass a JSON-encoded object string.",
                field
            ),
            None,
        ))
    }
}

fn normalize_json_array_arg(value: Value, field: &str) -> Result<Value, rmcp::ErrorData> {
    let normalized = normalize_json_arg(value, field)?;
    if normalized.is_array() {
        Ok(normalized)
    } else {
        Err(rmcp::ErrorData::invalid_params(
            format!(
                "{} must be a JSON array. If your MCP gateway stringifies arguments, pass a JSON-encoded array string.",
                field
            ),
            None,
        ))
    }
}

fn report_authoring_schema() -> Value {
    let mut result: Value = serde_json::from_str(r###"{
        "definitionVersion": 1,
        "purpose": "Canonical MCP contract for authoring Runtara reports. Call this before create_report, update_report, or edit_report (for targeted block + layout mutations).",
        "relatedTools": [
            "get_report_definition_schema",
            "list_workflows",
            "list_executions",
            "get_execution",
            "list_object_schemas",
            "get_object_schema",
            "query_aggregate",
            "validate_report",
            "render_report",
            "get_report_block_data",
            "edit_report"
        ],
        "definitionShape": {
            "definitionVersion": 1,
            "layout": "Mandatory single root grid (Phase 10). Every block must live inside its items[]. The root grid has a stable id (default 'root'), optional title/description, and required columns/items. Nested grids carry the legacy 'grid' type tag; the root grid omits it because the field is typed directly.",
            "views": "Optional named report views for master/detail navigation. Each view has an id, optional title/titleFrom/titleFromBlock, parentViewId + clearFiltersOnBack for generated breadcrumbs, optional manual breadcrumb override, and its own root-grid layout.",
            "filters": "Optional global filter presets. Each filter can apply to one or more block/source fields.",
            "datasets": "Optional semantic BI datasets. Prefer defining datasets for aggregate BI reports so blocks reference named dimensions/measures instead of raw aggregate specs.",
            "blocks": "Array of typed block definitions. Every block must have a stable id for MCP block mutations."
        },
        "biGuidance": {
            "currentContract": [
                "For BI-style reports, define definition.datasets first, then use block.dataset with selected dimensions/measures.",
                "Use raw block.source only for lower-level Object Model queries, custom joins, or reports that have not moved to datasets yet.",
                "For BI-style reports, define global filters with object_model-backed options so viewers can self-serve without raw SQL.",
                "Use filter.options.source='object_model' with schema, field, optional labelField, search=true, and dependsOn for cascading filter option lists.",
                "Use block.interactions for drill/cross-filter behavior. Supported UI events are point_click on charts and row_click/cell_click on tables.",
                "Use set_filter actions to update global filters from clicked chart/table data, e.g. valueFrom='datum.category'.",
                "Use navigate_view with set_filter for master/detail navigation, e.g. row click sets case_id and opens the detail view. Omit navigate_view for inline dependent content.",
                "For nested detail navigation, set parentViewId on each child view and clearFiltersOnBack to the filters that should be cleared when returning to the parent; breadcrumb can still be supplied manually as an override.",
                "For navigation-driven filters (set by row-click), mark them strictWhenReferenced=true so detail-view blocks render an explicit 'filter not set' empty state instead of silently falling back to an unfiltered query when someone hits the detail URL without the filter populated.",
                "Use showWhen on layout nodes or blocks to show dependent content only after a filter is selected.",
                "Keep exploration governed: only expose dimensions and measures declared in datasets and report blocks/filters/interactions that the report author intentionally configured."
            ],
            "datasetExample": {
                "id": "stock_snapshots",
                "label": "Stock snapshots",
                "source": {"schema": "StockSnapshot", "connectionId": null},
                "timeDimension": "snapshot_date",
                "dimensions": [
                    {"field": "sku", "label": "SKU", "type": "string"},
                    {"field": "vendor", "label": "Vendor", "type": "string"},
                    {"field": "category", "label": "Category", "type": "string"}
                ],
                "measures": [
                    {"id": "snapshot_count", "label": "Snapshots", "op": "count", "format": "number"},
                    {"id": "qty_total", "label": "Total quantity", "op": "sum", "field": "qty", "format": "number"},
                    {"id": "qty_avg", "label": "Average quantity", "op": "avg", "field": "qty", "format": "decimal"}
                ]
            },
            "dynamicFilterExample": {
                "id": "vendor",
                "label": "Vendor",
                "type": "multi_select",
                "options": {"source": "object_model", "schema": "StockSnapshot", "field": "vendor", "search": true},
                "appliesTo": [{"field": "vendor", "op": "in"}]
            },
            "drillInteractionExample": {
                "interactions": [
                    {
                        "id": "drill_category",
                        "trigger": {"event": "point_click"},
                        "actions": [{"type": "set_filter", "filterId": "category", "valueFrom": "datum.category"}]
                    }
                ]
            },
            "masterDetailNavigationExample": {
                "filters": [{"id": "case_id", "label": "Case", "type": "text", "strictWhenReferenced": true}],
                "views": [
                    {"id": "list", "title": "Review cases", "layout": {"id": "view_list_root", "columns": 1, "items": [{"id": "view_list_root_i0", "child": {"id": "cases_node", "type": "block", "blockId": "cases"}}]}},
                    {"id": "detail", "titleFrom": "filters.case_id", "parentViewId": "list", "clearFiltersOnBack": ["case_id"], "layout": {"id": "view_detail_root", "columns": 1, "items": [{"id": "view_detail_root_i0", "child": {"id": "case_summary_node", "type": "block", "blockId": "case_summary"}}]}}
                ],
                "interaction": {"id": "open_case", "trigger": {"event": "row_click"}, "actions": [{"type": "set_filter", "filterId": "case_id", "valueFrom": "datum.case_id"}, {"type": "navigate_view", "viewId": "detail"}]}
            }
        },
        "layoutGuidance": {
            "currentContract": [
                "definition.layout is a single mandatory root grid (Phase 10). All blocks must live inside its items[] (directly, or via nested grids).",
                "The root grid has a stable id (default 'root') and carries optional title/description and columns/rows/columnWidths. It cannot be removed or replaced with a block.",
                "Phase 9 collapsed the layout vocabulary to two node types: 'block' (leaf reference to a block by id) and 'grid' (recursive container with columns/rows + items).",
                "Every container (single-column section, multi-column row, metric strip, 2D dashboard) is expressed as a grid with different columns/columnWidths/colSpan/rowSpan. Single column = section, columns=N = side-by-side, columns=4 with metric-blocks inside = metric row.",
                "Every layout node has a stable id. Use edit_report with add_layout_node / replace_layout_node / patch_layout_node / move_layout_node / remove_layout_node ops for targeted edits. LayoutTarget.parentNodeId picks the destination grid; null resolves to the root grid.",
                "Grid items wrap their child as { id, colSpan?, rowSpan?, child: <ReportLayoutNode> } — child is itself a block or nested grid.",
                "Use type='block' layout nodes to reference blocks (markdown / table / chart / metric / card / actions). Do not put Markdown content directly in layout nodes."
            ],
            "rootShape": {
                "definition.layout": {
                    "id": "root",
                    "columns": 1,
                    "rows": 1,
                    "items": [
                        {"id": "root_i0", "child": {"id": "intro_node", "type": "block", "blockId": "intro"}}
                    ]
                },
                "note": "The root grid wire form omits the `type` field — it is implicitly a grid. Nested layout nodes inside `items[].child` must carry `type: 'block' | 'grid'`."
            },
            "layoutNodes": {
                "blockLeaf": {"id": "records_node", "type": "block", "blockId": "records"},
                "sectionAsSingleColumnNestedGrid": {
                    "id": "summary_section",
                    "type": "grid",
                    "title": "Summary",
                    "description": "Optional context.",
                    "columns": 1,
                    "items": [
                        {"id": "summary_metrics_item", "child": {"id": "summary_metrics", "type": "grid", "columns": 3, "items": [
                            {"id": "m1_item", "child": {"id": "m1_node", "type": "block", "blockId": "total_records"}},
                            {"id": "m2_item", "child": {"id": "m2_node", "type": "block", "blockId": "open_records"}},
                            {"id": "m3_item", "child": {"id": "m3_node", "type": "block", "blockId": "closed_records"}}
                        ]}}
                    ]
                },
                "twoColumnComparison": {
                    "id": "comparison",
                    "type": "grid",
                    "columns": 2,
                    "columnWidths": [1, 1],
                    "items": [
                        {"id": "left_item", "child": {"id": "left_chart_node", "type": "block", "blockId": "left_chart"}},
                        {"id": "right_item", "child": {"id": "right_table_node", "type": "block", "blockId": "right_table"}}
                    ]
                },
                "dashboardGridInsideRoot": {
                    "note": "When the root grid is just a holder, drop a 12-column nested grid inside it for dashboard-style layouts.",
                    "rootSnippet": {
                        "id": "root",
                        "columns": 1,
                        "items": [
                            {"id": "root_i0", "child": {
                                "id": "dashboard_grid",
                                "type": "grid",
                                "columns": 12,
                                "items": [
                                    {"id": "trend_item", "colSpan": 8, "child": {"id": "trend_node", "type": "block", "blockId": "trend"}},
                                    {"id": "records_item", "colSpan": 4, "child": {"id": "records_node", "type": "block", "blockId": "records"}}
                                ]
                            }}
                        ]
                    }
                }
            }
        },
        "blockShape": {
            "common": {
                "id": "Stable id, unique within the report. Layout block nodes reference this as blockId.",
                "type": "table | chart | metric | actions | markdown | card",
                "title": "Optional UI title.",
                "lazy": "Optional boolean. Lazy blocks fetch only when requested.",
                "showWhen": "Optional visibility condition such as {filter:'case_id', exists:true}. Use this for inline dependent content.",
                "dataset": "Preferred BI query shape: {id, dimensions, measures, orderBy?, limit?}. The id must match definition.datasets[].id.",
                "source": "Object Model data source and query plan. Required only when block.dataset is absent, except static markdown blocks can omit source data.",
                "filters": "Optional per-block filter presets.",
                "interactions": "Optional drill/cross-filter/navigation actions. Use point_click, row_click, or cell_click triggers with set_filter, clear_filter, clear_filters, and navigate_view actions."
            },
            "table": {
                "type": "table",
                "configKey": "table",
                "columnsPath": "table.columns",
                "columns": [{"field": "sku", "label": "SKU", "format": "optional formatter", "maxChars": "Optional positive integer display cutoff; omit to show the full formatted value."}, {"field": "stock_trend", "label": "Trend", "type": "chart", "chart": {"kind": "line", "x": "snapshot_date", "series": [{"field": "qty", "label": "Qty"}]}, "source": {"schema": "StockSnapshot", "mode": "aggregate", "groupBy": ["snapshot_date"], "aggregates": [{"alias": "qty", "op": "sum", "field": "qty"}], "join": [{"parentField": "sku", "field": "sku"}]}}],
                "defaultSort": [{"field": "sku", "direction": "asc"}],
                "pagination": {"defaultPageSize": 50, "allowedPageSizes": [25, 50, 100]},
                "selectable": "Optional boolean. Shows a per-row checkbox selection column. table.actions also enables selection automatically.",
                "actions": [{"id": "bulk_process", "label": "Process selected", "workflowAction": {"workflowId": "process_items", "label": "Process selected", "runningLabel": "Processing...", "successMessage": "Selected rows processed.", "reloadBlock": true, "context": {"mode": "selection", "inputKey": "items"}}}],
                "writeback": {
                    "editable": "Optional boolean. When true, the table renders an inline editor on the cell and writes the new value back to the underlying Object Model record via PUT /api/runtime/object-model/instances/{schemaId}/{instanceId}. Only honored when source.kind='object_model', source.mode='filter', and source.join is empty/absent (rows must carry a stable id+schemaId). Type='chart' columns and joined lookup columns are never editable.",
                    "displayField": "Optional row field to render while writes still target field. Use this with joined labels, e.g. field='category_id', displayField='category.name'. Lookup-editor columns without an explicit displayField automatically render editor.lookup.labelField for the current value.",
                    "displayTemplate": "Optional display-only safe-interpolation template rendered from the row while sort/filter/writeback still target field. Only variable paths and optional format pipes compile, e.g. {{first_name}} {{last_name}} or {{requested_loan.amount | number_compact}} AUD. Helpers, blocks, expressions, and partials are not supported.",
                    "editor": "Optional explicit editor config: {kind, lookup?, options?, min?, max?, step?, regex?, placeholder?}. kind is one of text | textarea | number | select | toggle | date | datetime | lookup. For lookup, set editor.lookup={schema, valueField, labelField, searchFields?, connectionId?, condition?, filterMappings?}. The editor searches the lookup schema, automatically using generated tsvector fields when present, displays labelField, and writes valueField into the edited row field. Add an explicit source.join/displayField only when the related label must also participate in table search/sort/filtering.",
                    "note": "Writeback is opt-in per column. Auth + type validation happens on the object-model endpoint, not in the report layer — viewers need write permission on the underlying schema. The 'editable' flag here is a UI hint; it does not relax server-side authorization."
                },
                "workflowAction": "Optional table column button: set type='workflow_button' and workflowAction={workflowId, version?, label?, runningLabel?, successMessage?, reloadBlock?, visibleWhen?, hiddenWhen?, disabledWhen?, context?}. context.mode is row | field | value. mode=row passes the whole row as workflow data; mode=field passes context.field or column.field; mode=value passes the cell value. context.inputKey wraps the context as {inputKey: context}. visibleWhen/hiddenWhen/disabledWhen are ConditionExpression objects evaluated against the rendered row, e.g. disabledWhen={type:'operation', op:'EQ', arguments:[{valueType:'reference', value:'status'}, {valueType:'immediate', value:'processed'}]}.",
                "interactionButtons": "Optional row navigation/action buttons: set type='interaction_buttons' and interactionButtons=[{id,label?,icon?,visibleWhen?,hiddenWhen?,disabledWhen?,actions:[...]}]. Button actions use the same set_filter, clear_filter, clear_filters, and navigate_view vocabulary as block.interactions. Use this for rows like SKU | Qty | Price | View 1 | View 2.",
                "note": "Tables support source.mode='filter' for row data and source.mode='aggregate' for grouped aggregate result sets. Configure visible/searchable/sortable fields in table.columns. A table column may use maxChars for display-only text cutoff, format='pill' + pillVariants for enum/status coloring, displayTemplate for display-only concatenation/formatting, type='chart' for inline aggregate charts, type='value' with source.select for scalar joined lookups, type='workflow_button' with workflowAction for a row-scoped workflow launcher, type='interaction_buttons' with interactionButtons for row-scoped report navigation/action buttons, or table.actions[] for table-wide selected-row workflow launchers. To enable inline writeback on a column, see writeback.editable."
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
            },
            "markdown": {
                "type": "markdown",
                "configKey": "markdown",
                "required": {
                    "markdown.content": "Markdown content. Use {{source.field}} or {{source[0].field}} to interpolate the block's own source rows."
                },
                "note": "Markdown blocks can be static or data-backed. Static markdown blocks omit source/dataset. Data-backed markdown blocks may use source or dataset and interpolate source fields only; no loops or template helpers are supported."
            },
            "actions": {
                "type": "actions",
                "required": {
                    "source.kind": "workflow_runtime",
                    "source.entity": "actions",
                    "source.workflowId": "Workflow id. Add source.instanceId to scope forms to one workflow instance."
                },
                "actions.submit": "Optional submit configuration. Use actions.submit.label to override the button label and actions.submit.implicitPayload for server-side viewer fields such as {{viewer.user_id}}.",
                "note": "Actions blocks render executable forms from workflow action inputSchema. Do not add table/chart/metric config to actions blocks."
            },
            "card": {
                "type": "card",
                "configKey": "card",
                "required": {
                    "source.kind": "object_model (only object_model sources are supported for cards)",
                    "source.mode": "filter (cards render the first matching row)",
                    "card.groups": "Array of {id, title?, description?, columns?, fields[]}. Cards stack groups vertically; each group lays its fields in an inner grid (1–4 columns)."
                },
                "fieldShape": {
                    "field": "Property name on the row to read.",
                    "label": "Optional override for the field label (defaults to humanized field name).",
                    "displayField": "Optional row field to render while writes still target field. Use this with joined labels, e.g. field='category_id', displayField='category.name'.",
                    "displayTemplate": "Optional display-only safe-interpolation row template such as {{first_name}} {{last_name}}. Only variable paths and optional format pipes compile; field remains the value/writeback target.",
                    "kind": "value (default) | json | markdown | subcard | subtable | workflow_button",
                    "format": "Format hint for kind=value: currency, currency_compact, decimal, percent, datetime, date, number, pill.",
                    "workflowAction": "Optional card field button: set kind='workflow_button' and workflowAction={workflowId, version?, label?, runningLabel?, successMessage?, reloadBlock?, visibleWhen?, hiddenWhen?, disabledWhen?, context?}. context.mode is row | field | value, and context.inputKey can wrap the selected context into an object. visibleWhen/hiddenWhen/disabledWhen are ConditionExpression objects (same shape used for workflow step conditions) evaluated against the rendered row.",
                    "pillVariants": "{value: variant} map for color-coding enum/status fields. variant is one of default, secondary, destructive, outline, muted, success, warning. Use this on enum columns like status/severity/decision.",
                    "collapsed": "Optional. For json/markdown/subcard/subtable: start collapsed behind a Show/Hide toggle.",
                    "colSpan": "Optional 1–4 grid column span within the parent group.",
                    "subcard": "Required when kind=subcard. Recursive card config {groups: […]} applied to the nested object value at row[field].",
                    "subtable": "Required when kind=subtable. {columns: [{field, label?, format?, pillVariants?, align?}], emptyLabel?} applied to the array value at row[field].",
                    "editable": "Optional boolean. Only honored on kind=value fields when the rendered card row carries id+schemaId (filter-mode object_model card). Renders an inline editor that writes back via PUT /api/runtime/object-model/instances/{schemaId}/{instanceId}.",
                    "editor": "Optional explicit editor config: {kind, lookup?, options?, min?, max?, step?, regex?, placeholder?}. kind is one of text | textarea | number | select | toggle | date | datetime | lookup. For lookup, set editor.lookup={schema, valueField, labelField, searchFields?, connectionId?, condition?, filterMappings?}. The editor searches the lookup schema, automatically using generated tsvector fields when present, displays labelField, and writes valueField into the edited row field."
                },
                "note": "Cards are the right primitive for single-row dossier-style presentation: case headers, AI/Human decision recaps, raw L1 source rows. Use kind=subtable for arrays-of-objects (timelines, line items) and kind=subcard for nested object summaries (applicant_summary, financial_summary). Pair format=pill + pillVariants on enum fields to color-code status, severity, decision, etc."
            }
        },
        "sourceShape": {
            "kind": "object_model | workflow_runtime | system. Omit for Object Model back compatibility. Nested table value-column sources may also set kind='object_model'.",
            "schema": "Object Model schema name. Use get_object_schema to inspect valid fields.",
            "entity": "workflow_runtime: instances | actions. system: runtime_execution_metric_buckets | runtime_system_snapshot | connection_rate_limit_status | connection_rate_limit_events | connection_rate_limit_timeline.",
            "workflowId": "Workflow runtime only: workflow id whose instances/actions should be shown.",
            "instanceId": "Workflow runtime actions only: optional workflow instance UUID to scope open actions.",
            "select": "Table value-column source only: scalar field to copy from the joined schema. May use JSON dot-paths from a JSON column, e.g. applicant_summary.full_name; dot-paths are select-only and are not valid join/order/condition fields.",
            "connectionId": "Optional connection id for connection-scoped schemas.",
            "mode": "filter | aggregate",
            "condition": "Optional condition DSL. Object Model sources can use schema fields and same-store subquery operands. workflow_runtime actions can filter virtual action fields including actionKey, correlation.<key>, and context.<key>. system sources can filter their exposed virtual fields, especially bucketTime/createdAt/connectionId/eventType/tag.",
            "filterMappings": "Optional mappings from global filter ids to source fields.",
            "groupBy": "Aggregate output grouping fields.",
            "aggregates": "Aggregate specs. Report aggregate specs use {alias, op, field?, distinct?, orderBy?, expression?}. Use op/field here, not fn/column.",
            "orderBy": "Sort array using {field, direction}. Field must be a row field, groupBy field, or aggregate alias depending on source mode.",
            "limit": "Optional row/group cap.",
            "granularity": "System only: runtime metrics support hourly/daily; rate-limit timeline supports minute/hourly/daily.",
            "interval": "System rate-limit status only: period stats interval such as 1h, 24h, 7d, or 30d.",
            "join": "Optional single-hop Object Model joins. Use [{schema, alias?, connectionId?, parentField, field, op?, kind?}] and qualify joined fields as <alias>.<field>."
        },
        "datasetShape": {
            "id": "Stable dataset id, unique within definition.datasets.",
            "label": "Human-readable dataset name.",
            "source": {"schema": "Object Model schema name", "connectionId": "Optional connection id or null"},
            "timeDimension": "Optional date/datetime dimension field used as the default time axis.",
            "dimensions": "Array of {field, label, type, format?}. The field is the stable dimension id and must exist on the source schema.",
            "measures": "Array of {id, label, op, field?, distinct?, orderBy?, expression?, percentile?, format}. The id is the aggregate alias exposed to blocks.",
            "blockDataset": {"id": "stock_snapshots", "dimensions": ["vendor"], "measures": ["snapshot_count", "qty_total"], "orderBy": [{"field": "qty_total", "direction": "desc"}]}
        },
        "aggregateOps": {
            "core": ["count", "sum", "avg", "min", "max", "first_value", "last_value", "percentile_cont", "percentile_disc", "stddev_samp", "var_samp", "expr"],
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
            "strictWhenReferenced": "Optional boolean. When true, any block whose source `condition` references this filter will short-circuit to an empty 'filter not set' result if the filter has no value. Use this on navigation-driven filters (set by row-click + navigate_view) so detail-view blocks never silently fall back to an unfiltered query when the filter is missing from the URL/state.",
            "example": {
                "id": "vendor",
                "label": "Vendor",
                "type": "select",
                "options": {"source": "static", "values": [{"label": "TD Synnex", "value": "TD Synnex"}]},
                "appliesTo": [{"blockId": "products", "field": "vendor", "op": "eq"}]
            },
            "dynamicOptions": {
                "options": {"source": "object_model", "schema": "StockSnapshot", "field": "vendor", "labelField": "vendor", "search": true, "dependsOn": ["date_range"]},
                "note": "Dynamic options are loaded from grouped Object Model values and can cascade through dependsOn plus filterMappings."
            }
        },
        "fieldRules": [
            "For table.columns, use Object Model row fields when source.mode='filter'.",
            "For table.columns[].displayTemplate and card fields[].displayTemplate, use presentation-only safe interpolation like {{first_name}} {{last_name}}. Supported tokens are {{field.path}} and {{field.path | format}} only; do not rely on displayTemplate for sort, search, filter, or writeback behavior.",
            "For aggregate table.columns, use source.groupBy fields and source.aggregates aliases, including expr aliases.",
            "For chart table columns, field is a synthetic cell key; configure column.chart and column.source.join.",
            "For scalar value table columns, use type='value' plus column.source.select and one column.source.join entry.",
            "For chart.x, use an aggregate output field, usually a source.groupBy field.",
            "For chart.series[].field and metric.valueField, use aggregate aliases from source.aggregates.",
            "For source.orderBy and table.defaultSort, use field, not column.",
            "For large category/product filters, prefer IN with a same-store subquery instead of materializing large value arrays.",
            "For dataset-backed blocks, table columns/chart fields/metric valueField use selected block.dataset dimensions and measures.",
            "For workflow_runtime entity='instances', table.columns and orderBy use instance fields such as instanceId, status, hasActions, actionCount, createdAt.",
            "For workflow_runtime entity='actions', table.columns and orderBy use action fields such as actionId, actionKey, label, status, instanceId, requestedAt. Conditions can additionally use nested metadata fields such as correlation.case_id or context.purpose.",
            "For type='actions', do not configure table columns; the block renders forms from each action.inputSchema and submits through the report-scoped workflow action endpoint.",
            "For type='card', use card.groups[].fields. Each field references a row property by name. Use kind='subtable' (with subtable.columns) for arrays-of-objects and kind='subcard' (with subcard.groups) for nested objects. Use format='pill' + pillVariants to color-code enum/status fields.",
            "For workflow launch buttons, use table.columns[].type='workflow_button' or card.groups[].fields[].kind='workflow_button' with workflowAction.workflowId. Set workflowAction.context.mode='row' to pass the whole row, 'field' to pass a row field, or 'value' to pass the cell/field value. Use workflowAction.visibleWhen or hiddenWhen for row-level visibility, and disabledWhen for visible-but-disabled buttons. These are ConditionExpression objects, e.g. disabledWhen={type:'operation', op:'EQ', arguments:[{valueType:'reference', value:'status'}, {valueType:'immediate', value:'processed'}]}. For table-wide bulk workflow buttons, use table.actions[] with workflowAction.context.mode='selection'; buttons remain visible but disabled until one or more current table rows are selected.",
            "For row-scoped report navigation buttons, use table.columns[].type='interaction_buttons' with interactionButtons. Each button can set row-derived filters and navigate to a named view, e.g. actions=[{type:'set_filter', filterId:'sku', valueFrom:'datum.sku'}, {type:'navigate_view', viewId:'inventory_detail'}].",
            "For editable lookup/reference fields, keep the stored id in field, optionally render a joined label with displayField, and set editor.kind='lookup' with editor.lookup={schema, valueField, labelField, searchFields?}. Lookup search automatically uses generated tsvector fields when the lookup schema has them."
        ],
        "commonMistakes": [
            "For BI reports, do not hand-author repeated raw aggregate block.source specs when the same semantic fields can live in definition.datasets.",
            "Do not put dataset dimensions/measures directly on a block. Use block.dataset.dimensions and block.dataset.measures.",
            "Do not put columns at block.columns, block.fields, or source.columns. Use block.table.columns.",
            "Do not put chartType, x, or y at block top-level. Use block.chart.kind, block.chart.x, and block.chart.series[].field.",
            "Do not use metric.valueAlias or top-level valueAlias. Use block.metric.valueField.",
            "Do not copy query_aggregate specs directly: report aggregates use op/field while query_aggregate uses fn/column.",
            "Do not use source.mode='aggregate' with table.columns pointing at ungrouped raw schema fields; use groupBy fields or aggregate aliases.",
            "Do not put layout structure inside markdown.content. Use definition.layout with block + grid layout nodes.",
            "Do not omit layout node ids. edit_report addresses layout nodes by id for add/replace/patch/move/remove ops.",
            "Do not hardcode large select option lists when the values live in Object Model data. Use filter.options.source='object_model'.",
            "Do not hardcode lookup editor option lists when the values live in another Object Model. Use editor.kind='lookup' and editor.lookup instead.",
            "Do not call workflow signals 'pendingInput' in report definitions. Use the generic actions abstraction: type='actions' and source.entity='actions'.",
            "Do not put schema, connectionId, joins, groupBy, or aggregates on workflow_runtime sources.",
            "Do not use type='actions' with Object Model sources; actions blocks currently require source.kind='workflow_runtime' and entity='actions'.",
            "Do not sort, filter, search, or write back against displayTemplate output. Use stored top-level columns for queryable computed values.",
            "Always run validate_report with mode='all' before saving or mutating report blocks.",
            "Do not use type='card' with source.mode='aggregate' or workflow_runtime sources. Cards only support object_model + filter mode and render the first matching row.",
            "Do not put card fields at block.fields or block.card.fields. Use block.card.groups[].fields.",
            "Do not stuff arrays into kind='subcard' or objects into kind='subtable'. Subcard expects an object value, subtable expects an array of objects."
        ],
        "examples": {
            "datasetBackedTable": {
                "definitionDatasets": [
                    {
                        "id": "stock_snapshots",
                        "label": "Stock snapshots",
                        "source": {"schema": "StockSnapshot", "connectionId": null},
                        "timeDimension": "snapshot_date",
                        "dimensions": [{"field": "vendor", "label": "Vendor", "type": "string"}],
                        "measures": [
                            {"id": "snapshot_count", "label": "Snapshots", "op": "count", "format": "number"},
                            {"id": "qty_total", "label": "Total quantity", "op": "sum", "field": "qty", "format": "number"}
                        ]
                    }
                ],
                "block": {
                    "id": "vendor_summary",
                    "type": "table",
                    "title": "Vendor summary",
                    "dataset": {"id": "stock_snapshots", "dimensions": ["vendor"], "measures": ["snapshot_count", "qty_total"], "orderBy": [{"field": "qty_total", "direction": "desc"}]},
                    "table": {"columns": [{"field": "vendor", "label": "Vendor"}, {"field": "snapshot_count", "label": "Snapshots", "format": "number"}, {"field": "qty_total", "label": "Total quantity", "format": "number"}]}
                }
            },
            "layout": {
                "id": "root",
                "columns": 1,
                "items": [
                    {"id": "root_i0", "child": {"id": "intro_node", "type": "block", "blockId": "intro"}},
                    {"id": "root_i1", "child": {"id": "summary", "type": "grid", "columns": 2, "items": [
                        {"id": "summary_a", "child": {"id": "summary_a_node", "type": "block", "blockId": "total_snaps"}},
                        {"id": "summary_b", "child": {"id": "summary_b_node", "type": "block", "blockId": "unique_skus"}}
                    ]}},
                    {"id": "root_i2", "child": {"id": "main_grid", "type": "grid", "columns": 12, "items": [
                        {"id": "main_a", "colSpan": 8, "child": {"id": "main_a_node", "type": "block", "blockId": "daily_qty"}},
                        {"id": "main_b", "colSpan": 4, "child": {"id": "main_b_node", "type": "block", "blockId": "top_vendors"}}
                    ]}}
                ]
            },
            "markdownBlock": {
                "id": "intro",
                "type": "markdown",
                "markdown": {"content": "# Demand summary\n\nLive Object Model data."}
            },
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
            "editableLookupTable": {
                "id": "products",
                "type": "table",
                "title": "Products",
                "source": {
                    "schema": "Product",
                    "mode": "filter",
                    "join": [{"schema": "Category", "alias": "category", "parentField": "category_id", "field": "id", "kind": "left"}]
                },
                "table": {
                    "columns": [
                        {"field": "name", "label": "Product"},
                        {
                            "field": "category_id",
                            "label": "Category",
                            "displayField": "category.name",
                            "editable": true,
                            "editor": {
                                "kind": "lookup",
                                "lookup": {
                                    "schema": "Category",
                                    "valueField": "id",
                                    "labelField": "name",
                                    "searchFields": ["name"]
                                }
                            }
                        }
                    ]
                }
            },
            "joinedTable": {
                "id": "stock_with_product",
                "type": "table",
                "title": "Stock with product details",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "filter",
                    "join": [{"schema": "TDProduct", "alias": "product", "parentField": "sku", "field": "sku", "kind": "left"}],
                    "condition": {"op": "EQ", "arguments": ["product.category_leaf_id", {"filter": "category", "path": "value"}]}
                },
                "table": {
                    "columns": [
                        {"field": "sku", "label": "SKU"},
                        {"field": "qty", "label": "Qty"},
                        {
                            "field": "part_number_lookup",
                            "label": "Part Number",
                            "type": "value",
                            "source": {
                                "schema": "TDProduct",
                                "mode": "filter",
                                "select": "part_number",
                                "join": [{"parentField": "sku", "field": "sku", "kind": "left"}],
                                "orderBy": [{"field": "createdAt", "direction": "asc"}]
                            }
                        },
                        {"field": "product.part_number", "label": "Part Number"}
                    ],
                    "defaultSort": [{"field": "product.part_number", "direction": "asc"}]
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
            "workflowInstances": {
                "id": "workflow_runs",
                "type": "table",
                "title": "Workflow runs",
                "source": {"kind": "workflow_runtime", "entity": "instances", "workflowId": "inventory_sync", "mode": "filter"},
                "table": {
                    "columns": [
                        {"field": "instanceId", "label": "Instance"},
                        {"field": "status", "label": "Status"},
                        {"field": "hasActions", "label": "Has actions", "format": "boolean"},
                        {"field": "actionCount", "label": "Actions", "format": "number"},
                        {"field": "createdAt", "label": "Created", "format": "datetime"}
                    ]
                }
            },
            "workflowActions": {
                "id": "workflow_actions",
                "type": "actions",
                "title": "Workflow actions",
                "source": {"kind": "workflow_runtime", "entity": "actions", "workflowId": "inventory_sync", "mode": "filter"}
            },
            "card": {
                "id": "case_header",
                "type": "card",
                "title": "Case header",
                "source": {"schema": "LoanCase", "mode": "filter", "condition": {"op": "EQ", "arguments": ["id", {"filter": "case_id", "path": "value"}]}},
                "card": {
                    "groups": [
                        {
                            "id": "identity",
                            "title": "Identity",
                            "columns": 3,
                            "fields": [
                                {"field": "case_id", "label": "Case", "colSpan": 2},
                                {"field": "loan_application_id", "label": "Application"},
                                {"field": "current_owner", "label": "Owner"}
                            ]
                        },
                        {
                            "id": "lifecycle",
                            "title": "Lifecycle",
                            "columns": 3,
                            "fields": [
                                {"field": "opened_at", "label": "Opened", "format": "datetime"},
                                {"field": "closed_at", "label": "Closed", "format": "datetime"},
                                {"field": "current_status", "label": "Status", "format": "pill", "pillVariants": {"decided": "success", "withdrawn": "muted"}},
                                {"field": "final_decision", "label": "Final decision", "format": "pill", "pillVariants": {"APPROVED": "success", "DECLINED": "destructive", "PENDING": "muted"}}
                            ]
                        },
                        {
                            "id": "events",
                            "title": "Decision events",
                            "columns": 1,
                            "fields": [
                                {
                                    "field": "decision_events",
                                    "label": "Events",
                                    "kind": "subtable",
                                    "subtable": {
                                        "columns": [
                                            {"field": "seq", "label": "#", "align": "right"},
                                            {"field": "timestamp", "label": "When", "format": "datetime"},
                                            {"field": "layer", "label": "Layer", "format": "pill", "pillVariants": {"L1": "default", "L2": "default", "L3": "warning", "L4": "success"}},
                                            {"field": "actor", "label": "Actor"},
                                            {"field": "summary", "label": "Summary"}
                                        ],
                                        "emptyLabel": "No events recorded yet."
                                    }
                                }
                            ]
                        },
                        {
                            "id": "applicant",
                            "title": "Applicant snapshot",
                            "columns": 1,
                            "fields": [
                                {
                                    "field": "applicant_summary",
                                    "label": "Applicant",
                                    "kind": "subcard",
                                    "subcard": {
                                        "groups": [
                                            {
                                                "id": "identity",
                                                "columns": 3,
                                                "fields": [
                                                    {"field": "full_name", "label": "Name"},
                                                    {"field": "dob", "label": "DOB", "format": "date"},
                                                    {"field": "residency_status", "label": "Residency", "format": "pill", "pillVariants": {"citizen": "success", "permanent_resident": "default", "temporary": "warning"}}
                                                ]
                                            }
                                        ]
                                    }
                                }
                            ]
                        }
                    ]
                }
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
    }"###)
    .expect("report authoring schema JSON must be valid");
    result["workflowRuntimeGuidance"] = workflow_runtime_authoring_schema();
    result["systemSourceGuidance"] = system_authoring_schema();
    result
}

fn workflow_runtime_authoring_schema() -> Value {
    json!({
        "currentContract": [
            "Reports can use virtual workflow runtime sources in addition to Object Model sources.",
            "Use source.kind='workflow_runtime' with source.entity='instances' to list workflow instances and status/action summaries.",
            "Use source.kind='workflow_runtime' with source.entity='actions' to list/render open workflow actions backed by WaitForSignal/human input requests.",
            "Use type='actions' only with source.kind='workflow_runtime' and source.entity='actions'. The UI renders action forms from each action.inputSchema using the same SchemaField/JSON Schema form renderer as WaitForSignal.",
            "WaitForSignal steps may expose generic action metadata. Use source.condition with actionKey, correlation.<key>, or context.<key> to bind a report to the relevant open action without project-owned Object Model index schemas.",
            "Actions blocks submit through the report runtime. The server re-fetches the filtered virtual action row before sending the signal response. For MCP submission, call submit_action_response with report_id + block_id + action_id.",
            "For actions scoped to one execution, set source.instanceId to the workflow instance UUID. Without instanceId, workflow-wide action listing is bounded by the paged instance list and intended for dashboard display.",
            "Workflow runtime sources require source.workflowId. Do not set source.schema, connectionId, join, groupBy, or aggregates on workflow_runtime sources."
        ],
        "instanceFields": [
            "instanceId",
            "workflowId",
            "workflowName",
            "status",
            "createdAt",
            "updatedAt",
            "usedVersion",
            "durationSeconds",
            "hasActions",
            "actionCount"
        ],
        "actionFields": [
            "actionId",
            "actionKind",
            "targetKind",
            "targetId",
            "workflowId",
            "instanceId",
            "signalId",
            "actionKey",
            "label",
            "message",
            "inputSchema",
            "schemaFormat",
            "status",
            "requestedAt",
            "correlation",
            "context",
            "runtime"
        ],
        "instancesTableExample": {
            "id": "workflow_runs",
            "type": "table",
            "title": "Workflow runs",
            "source": {"kind": "workflow_runtime", "entity": "instances", "workflowId": "inventory_sync", "mode": "filter", "orderBy": [{"field": "createdAt", "direction": "desc"}]},
            "table": {
                "columns": [
                    {"field": "instanceId", "label": "Instance"},
                    {"field": "status", "label": "Status"},
                    {"field": "hasActions", "label": "Has actions", "format": "boolean"},
                    {"field": "actionCount", "label": "Actions", "format": "number"},
                    {"field": "createdAt", "label": "Created", "format": "datetime"}
                ],
                "pagination": {"defaultPageSize": 25, "allowedPageSizes": [25, 50, 100]}
            }
        },
        "actionsBlockExample": {
            "id": "workflow_actions",
            "type": "actions",
            "title": "Workflow actions",
            "source": {"kind": "workflow_runtime", "entity": "actions", "workflowId": "inventory_sync", "instanceId": "00000000-0000-0000-0000-000000000000", "mode": "filter"},
            "actions": {"submit": {"label": "Submit", "implicitPayload": {"reviewer_id": "{{viewer.user_id}}"}}}
        },
        "correlatedActionsBlockExample": {
            "id": "case_action",
            "type": "actions",
            "title": "Case action",
            "source": {
                "kind": "workflow_runtime",
                "entity": "actions",
                "workflowId": "loan_review",
                "mode": "filter",
                "condition": {"op": "AND", "arguments": [
                    {"op": "EQ", "arguments": ["actionKey", "case_review_decision"]},
                    {"op": "EQ", "arguments": ["correlation.case_id", {"filter": "case_id", "path": "value"}]}
                ]}
            },
            "actions": {"submit": {"label": "Submit decision", "implicitPayload": {"reviewer_id": "{{viewer.user_id}}"}}}
        },
        "actionsTableExample": {
            "id": "workflow_actions_table",
            "type": "table",
            "title": "Open actions",
            "source": {"kind": "workflow_runtime", "entity": "actions", "workflowId": "inventory_sync", "mode": "filter", "orderBy": [{"field": "requestedAt", "direction": "desc"}]},
            "table": {
                "columns": [
                    {"field": "label", "label": "Action"},
                    {"field": "status", "label": "Status"},
                    {"field": "instanceId", "label": "Instance"},
                    {"field": "requestedAt", "label": "Requested", "format": "datetime"}
                ]
            }
        }
    })
}

fn system_authoring_schema() -> Value {
    json!({
        "currentContract": [
            "Reports can use virtual system sources for the existing Analytics pages without creating Object Model mirror tables.",
            "Use source.kind='system' and leave source.schema, workflowId, instanceId, and join unset.",
            "System sources support table blocks in filter mode and chart/metric/table blocks in aggregate mode.",
            "Date-range filters should target bucketTime for runtime_execution_metric_buckets and connection_rate_limit_timeline, and createdAt for connection_rate_limit_events.",
            "Rate-limit timeline requires a connectionId, supplied by source.connectionId or an EQ condition/filter mapping on connectionId."
        ],
        "entities": {
            "runtime_execution_metric_buckets": {
                "fields": ["tenantId", "bucketTime", "granularity", "invocationCount", "successCount", "failureCount", "cancelledCount", "avgDurationSeconds", "minDurationSeconds", "maxDurationSeconds", "avgMemoryBytes", "maxMemoryBytes", "successRatePercent"],
                "granularity": ["hourly", "daily"]
            },
            "runtime_system_snapshot": {
                "fields": ["capturedAt", "cpuArchitecture", "cpuPhysicalCores", "cpuLogicalCores", "memoryTotalBytes", "memoryAvailableBytes", "memoryAvailableForWorkflowsBytes", "memoryUsedBytes", "memoryUsedPercent", "diskPath", "diskTotalBytes", "diskAvailableBytes", "diskUsedBytes", "diskUsedPercent"]
            },
            "connection_rate_limit_status": {
                "fields": ["connectionId", "connectionTitle", "integrationId", "configRequestsPerSecond", "configBurstSize", "configRetryOnLimit", "configMaxRetries", "configMaxWaitMs", "stateAvailable", "stateCurrentTokens", "stateLastRefillMs", "stateLearnedLimit", "stateCallsInWindow", "stateTotalCalls", "stateWindowStartMs", "capacityPercent", "utilizationPercent", "isRateLimited", "retryAfterMs", "periodInterval", "periodTotalRequests", "periodRateLimitedCount", "periodRetryCount", "periodRateLimitedPercent"],
                "interval": ["1h", "24h", "7d", "30d"]
            },
            "connection_rate_limit_events": {
                "fields": ["id", "connectionId", "eventType", "createdAt", "metadata", "tag"]
            },
            "connection_rate_limit_timeline": {
                "fields": ["connectionId", "bucket", "bucketTime", "granularity", "requestCount", "rateLimitedCount", "retryCount"],
                "granularity": ["minute", "hourly", "daily"]
            }
        },
        "usageChartExample": {
            "id": "execution_trend",
            "type": "chart",
            "source": {
                "kind": "system",
                "entity": "runtime_execution_metric_buckets",
                "mode": "aggregate",
                "granularity": "hourly",
                "condition": {"op": "AND", "arguments": [
                    {"op": "GTE", "arguments": ["bucketTime", {"filter": "date_range", "path": "from"}]},
                    {"op": "LT", "arguments": ["bucketTime", {"filter": "date_range", "path": "to"}]}
                ]},
                "groupBy": ["bucketTime"],
                "aggregates": [
                    {"alias": "invocations", "op": "sum", "field": "invocationCount"},
                    {"alias": "failures", "op": "sum", "field": "failureCount"}
                ],
                "orderBy": [{"field": "bucketTime", "direction": "asc"}]
            },
            "chart": {"kind": "line", "x": "bucketTime", "series": [{"field": "invocations"}, {"field": "failures"}]}
        },
        "rateLimitMasterDetailExample": {
            "master": {
                "id": "rate_limit_connections",
                "type": "table",
                "source": {"kind": "system", "entity": "connection_rate_limit_status", "mode": "filter", "interval": "24h"},
                "table": {"columns": [{"field": "connectionTitle"}, {"field": "capacityPercent", "format": "percent"}, {"field": "periodRateLimitedCount", "format": "number"}]},
                "interactions": [{"id": "open_connection", "trigger": {"event": "row_click"}, "actions": [{"type": "set_filter", "filterId": "connection_id", "field": "connectionId"}, {"type": "navigate_view", "viewId": "rate_limit_detail"}]}]
            },
            "detailTimeline": {
                "id": "rate_limit_timeline",
                "type": "chart",
                "source": {
                    "kind": "system",
                    "entity": "connection_rate_limit_timeline",
                    "mode": "aggregate",
                    "granularity": "hourly",
                    "condition": {"op": "EQ", "arguments": ["connectionId", {"filter": "connection_id", "path": "value"}]},
                    "groupBy": ["bucketTime"],
                    "aggregates": [
                        {"alias": "requests", "op": "sum", "field": "requestCount"},
                        {"alias": "limited", "op": "sum", "field": "rateLimitedCount"},
                        {"alias": "retries", "op": "sum", "field": "retryCount"}
                    ],
                    "orderBy": [{"field": "bucketTime", "direction": "asc"}]
                },
                "chart": {"kind": "bar", "x": "bucketTime", "series": [{"field": "requests"}, {"field": "limited"}, {"field": "retries"}]}
            }
        }
    })
}
