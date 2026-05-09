use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

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
        description = "Full report definition: {definitionVersion, markdown, layout?, filters, datasets?, blocks}. For BI reports, define datasets and let blocks reference them. Every block must include a stable id; every layout node must include a stable id."
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
        description = "Full replacement report definition: {definitionVersion, markdown, layout?, filters, datasets?, blocks}. Use datasets for BI reports and block/layout mutation tools for atomic edits."
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddReportLayoutNodeParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Full layout node. Must include stable id and type.")]
    pub node: Value,
    #[schemars(
        description = "Optional container layout node id. Omit to insert at the root layout array. Sections accept children; columns require column_id."
    )]
    pub parent_node_id: Option<String>,
    #[schemars(
        description = "Target column id when parent_node_id points at a columns layout node."
    )]
    pub column_id: Option<String>,
    #[schemars(
        description = "Insert at zero-based sibling index. Mutually exclusive with before_node_id and after_node_id."
    )]
    pub index: Option<usize>,
    #[schemars(
        description = "Insert before this sibling layout node id. Mutually exclusive with index and after_node_id."
    )]
    pub before_node_id: Option<String>,
    #[schemars(
        description = "Insert after this sibling layout node id. Mutually exclusive with index and before_node_id."
    )]
    pub after_node_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceReportLayoutNodeParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable layout node id to replace.")]
    pub node_id: String,
    #[schemars(description = "Full replacement layout node. Its id must match node_id.")]
    pub node: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchReportLayoutNodeParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable layout node id to update.")]
    pub node_id: String,
    #[schemars(
        description = "RFC 7386-style JSON merge patch applied to the layout node. The id field cannot be changed."
    )]
    pub patch: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveReportLayoutNodeParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable layout node id to move.")]
    pub node_id: String,
    #[schemars(
        description = "Optional destination container layout node id. Omit to move to the root layout array. Sections accept children; columns require column_id."
    )]
    pub parent_node_id: Option<String>,
    #[schemars(
        description = "Target column id when parent_node_id points at a columns layout node."
    )]
    pub column_id: Option<String>,
    #[schemars(
        description = "Move to zero-based sibling index. Mutually exclusive with before_node_id and after_node_id."
    )]
    pub index: Option<usize>,
    #[schemars(
        description = "Move before this sibling layout node id. Mutually exclusive with index and after_node_id."
    )]
    pub before_node_id: Option<String>,
    #[schemars(
        description = "Move after this sibling layout node id. Mutually exclusive with index and before_node_id."
    )]
    pub after_node_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveReportLayoutNodeParams {
    #[schemars(description = "Report id or slug")]
    pub report_id: String,
    #[schemars(description = "Stable layout node id to remove.")]
    pub node_id: String,
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

pub async fn add_report_layout_node(
    server: &SmoMcpServer,
    params: AddReportLayoutNodeParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    let mut report = get_report_value(server, &params.report_id).await?;
    let definition = report_definition_mut(&mut report)?;
    let layout = layout_array_mut(definition)?;

    insert_layout_node(
        layout,
        params.parent_node_id.as_deref(),
        params.column_id.as_deref(),
        params.node,
        LayoutPosition {
            index: params.index,
            before_node_id: params.before_node_id.as_deref(),
            after_node_id: params.after_node_id.as_deref(),
        },
    )?;

    save_report_value(server, &params.report_id, report).await
}

pub async fn replace_report_layout_node(
    server: &SmoMcpServer,
    params: ReplaceReportLayoutNodeParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("node_id", &params.node_id)?;
    if params.node.get("id").and_then(Value::as_str) != Some(params.node_id.as_str()) {
        return Err(rmcp::ErrorData::invalid_params(
            "Replacement layout node id must match node_id.",
            None,
        ));
    }

    let mut report = get_report_value(server, &params.report_id).await?;
    let definition = report_definition_mut(&mut report)?;
    let layout = layout_array_mut(definition)?;
    if !replace_layout_node(layout, &params.node_id, params.node) {
        return Err(layout_node_not_found(&params.node_id));
    }

    save_report_value(server, &params.report_id, report).await
}

pub async fn patch_report_layout_node(
    server: &SmoMcpServer,
    params: PatchReportLayoutNodeParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("node_id", &params.node_id)?;
    if !params.patch.is_object() {
        return Err(rmcp::ErrorData::invalid_params(
            "Report layout node patch must be a JSON object.",
            None,
        ));
    }
    if params.patch.get("id").is_some() {
        return Err(rmcp::ErrorData::invalid_params(
            "Report layout node id cannot be changed with patch_report_layout_node.",
            None,
        ));
    }

    let mut report = get_report_value(server, &params.report_id).await?;
    let definition = report_definition_mut(&mut report)?;
    let layout = layout_array_mut(definition)?;
    if !patch_layout_node(layout, &params.node_id, &params.patch) {
        return Err(layout_node_not_found(&params.node_id));
    }

    save_report_value(server, &params.report_id, report).await
}

pub async fn move_report_layout_node(
    server: &SmoMcpServer,
    params: MoveReportLayoutNodeParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("node_id", &params.node_id)?;

    let mut report = get_report_value(server, &params.report_id).await?;
    let definition = report_definition_mut(&mut report)?;
    let layout = layout_array_mut(definition)?;
    let Some(node) = remove_layout_node(layout, &params.node_id) else {
        return Err(layout_node_not_found(&params.node_id));
    };
    insert_layout_node(
        layout,
        params.parent_node_id.as_deref(),
        params.column_id.as_deref(),
        node,
        LayoutPosition {
            index: params.index,
            before_node_id: params.before_node_id.as_deref(),
            after_node_id: params.after_node_id.as_deref(),
        },
    )?;

    save_report_value(server, &params.report_id, report).await
}

pub async fn remove_report_layout_node(
    server: &SmoMcpServer,
    params: RemoveReportLayoutNodeParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    validate_path_param("report_id", &params.report_id)?;
    validate_path_param("node_id", &params.node_id)?;

    let mut report = get_report_value(server, &params.report_id).await?;
    let definition = report_definition_mut(&mut report)?;
    let layout = layout_array_mut(definition)?;
    if remove_layout_node(layout, &params.node_id).is_none() {
        return Err(layout_node_not_found(&params.node_id));
    }

    save_report_value(server, &params.report_id, report).await
}

#[derive(Clone, Copy)]
struct LayoutPosition<'a> {
    index: Option<usize>,
    before_node_id: Option<&'a str>,
    after_node_id: Option<&'a str>,
}

async fn get_report_value(
    server: &SmoMcpServer,
    report_id: &str,
) -> Result<Value, rmcp::ErrorData> {
    let result = api_get(server, &format!("/api/runtime/reports/{}", report_id)).await?;
    result
        .get("report")
        .cloned()
        .ok_or_else(|| rmcp::ErrorData::internal_error("Report API response missing report.", None))
}

async fn save_report_value(
    server: &SmoMcpServer,
    report_id: &str,
    report: Value,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let definition = report
        .get("definition")
        .cloned()
        .ok_or_else(|| rmcp::ErrorData::invalid_params("Report is missing definition.", None))?;
    let issues = collect_report_definition_authoring_issues(&definition);
    if authoring_errors(&issues).next().is_some() {
        return Err(authoring_invalid_params(issues));
    }

    let body = json!({
        "name": report.get("name").cloned().unwrap_or(Value::String("Report".to_string())),
        "slug": report.get("slug").cloned().unwrap_or(Value::String("report".to_string())),
        "description": report.get("description").cloned().unwrap_or(Value::Null),
        "tags": report.get("tags").cloned().unwrap_or_else(|| json!([])),
        "status": report.get("status").cloned().unwrap_or(Value::String("published".to_string())),
        "definition": definition,
    });

    let result = api_put(
        server,
        &format!("/api/runtime/reports/{}", report_id),
        Some(body),
    )
    .await?;
    json_result(result)
}

fn report_definition_mut(report: &mut Value) -> Result<&mut Value, rmcp::ErrorData> {
    report
        .get_mut("definition")
        .ok_or_else(|| rmcp::ErrorData::invalid_params("Report is missing definition.", None))
}

fn layout_array_mut(definition: &mut Value) -> Result<&mut Vec<Value>, rmcp::ErrorData> {
    if definition.get("layout").is_none() {
        definition["layout"] = json!([]);
    }
    definition
        .get_mut("layout")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            rmcp::ErrorData::invalid_params("Report definition.layout must be an array.", None)
        })
}

fn insert_layout_node(
    nodes: &mut Vec<Value>,
    parent_node_id: Option<&str>,
    column_id: Option<&str>,
    node: Value,
    position: LayoutPosition<'_>,
) -> Result<(), rmcp::ErrorData> {
    match parent_node_id {
        None => insert_layout_node_into(nodes, node, position),
        Some(parent_node_id) => {
            if insert_layout_node_in_container(nodes, parent_node_id, column_id, node, position)? {
                Ok(())
            } else {
                Err(layout_node_not_found(parent_node_id))
            }
        }
    }
}

fn insert_layout_node_in_container(
    nodes: &mut [Value],
    parent_node_id: &str,
    column_id: Option<&str>,
    node: Value,
    position: LayoutPosition<'_>,
) -> Result<bool, rmcp::ErrorData> {
    for current in nodes {
        if layout_node_id(current) == Some(parent_node_id) {
            insert_layout_node_into_container(current, column_id, node, position)?;
            return Ok(true);
        }

        if let Some(children) = current.get_mut("children").and_then(Value::as_array_mut)
            && insert_layout_node_in_container(
                children,
                parent_node_id,
                column_id,
                node.clone(),
                position,
            )?
        {
            return Ok(true);
        }

        if let Some(columns) = current.get_mut("columns").and_then(Value::as_array_mut) {
            for column in columns {
                if let Some(children) = column.get_mut("children").and_then(Value::as_array_mut)
                    && insert_layout_node_in_container(
                        children,
                        parent_node_id,
                        column_id,
                        node.clone(),
                        position,
                    )?
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn insert_layout_node_into_container(
    container: &mut Value,
    column_id: Option<&str>,
    node: Value,
    position: LayoutPosition<'_>,
) -> Result<(), rmcp::ErrorData> {
    match container.get("type").and_then(Value::as_str) {
        Some("section") => {
            if column_id.is_some() {
                return Err(rmcp::ErrorData::invalid_params(
                    "column_id can only be used with columns layout nodes.",
                    None,
                ));
            }
            if container.get("children").is_none() {
                container["children"] = json!([]);
            }
            let children = container
                .get_mut("children")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| {
                    rmcp::ErrorData::invalid_params(
                        "Section layout node children must be an array.",
                        None,
                    )
                })?;
            insert_layout_node_into(children, node, position)
        }
        Some("columns") => {
            let column_id = column_id.ok_or_else(|| {
                rmcp::ErrorData::invalid_params(
                    "column_id is required when inserting into a columns layout node.",
                    None,
                )
            })?;
            let columns = container
                .get_mut("columns")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| {
                    rmcp::ErrorData::invalid_params(
                        "Columns layout node columns must be an array.",
                        None,
                    )
                })?;
            for column in columns {
                if column.get("id").and_then(Value::as_str) == Some(column_id) {
                    if column.get("children").is_none() {
                        column["children"] = json!([]);
                    }
                    let children = column
                        .get_mut("children")
                        .and_then(Value::as_array_mut)
                        .ok_or_else(|| {
                            rmcp::ErrorData::invalid_params(
                                "Column children must be an array.",
                                None,
                            )
                        })?;
                    return insert_layout_node_into(children, node, position);
                }
            }
            Err(rmcp::ErrorData::invalid_params(
                format!("Unknown report layout column '{}'.", column_id),
                None,
            ))
        }
        Some(other) => Err(rmcp::ErrorData::invalid_params(
            format!(
                "Layout node type '{}' cannot contain child layout nodes.",
                other
            ),
            None,
        )),
        None => Err(rmcp::ErrorData::invalid_params(
            "Layout container node must include type.",
            None,
        )),
    }
}

fn insert_layout_node_into(
    nodes: &mut Vec<Value>,
    node: Value,
    position: LayoutPosition<'_>,
) -> Result<(), rmcp::ErrorData> {
    let index = resolve_layout_position(nodes, position)?;
    nodes.insert(index, node);
    Ok(())
}

fn resolve_layout_position(
    nodes: &[Value],
    position: LayoutPosition<'_>,
) -> Result<usize, rmcp::ErrorData> {
    let selector_count = usize::from(position.index.is_some())
        + usize::from(position.before_node_id.is_some())
        + usize::from(position.after_node_id.is_some());
    if selector_count > 1 {
        return Err(rmcp::ErrorData::invalid_params(
            "Layout position must use only one of index, before_node_id, or after_node_id.",
            None,
        ));
    }
    if let Some(index) = position.index {
        return Ok(index.min(nodes.len()));
    }
    if let Some(before_node_id) = position.before_node_id {
        return nodes
            .iter()
            .position(|node| layout_node_id(node) == Some(before_node_id))
            .ok_or_else(|| layout_node_not_found(before_node_id));
    }
    if let Some(after_node_id) = position.after_node_id {
        return nodes
            .iter()
            .position(|node| layout_node_id(node) == Some(after_node_id))
            .map(|index| index + 1)
            .ok_or_else(|| layout_node_not_found(after_node_id));
    }
    Ok(nodes.len())
}

fn patch_layout_node(nodes: &mut [Value], node_id: &str, patch: &Value) -> bool {
    for node in nodes {
        if layout_node_id(node) == Some(node_id) {
            apply_json_merge_patch(node, patch);
            return true;
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut)
            && patch_layout_node(children, node_id, patch)
        {
            return true;
        }
        if let Some(columns) = node.get_mut("columns").and_then(Value::as_array_mut) {
            for column in columns {
                if let Some(children) = column.get_mut("children").and_then(Value::as_array_mut)
                    && patch_layout_node(children, node_id, patch)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn replace_layout_node(nodes: &mut [Value], node_id: &str, replacement: Value) -> bool {
    for node in nodes {
        if layout_node_id(node) == Some(node_id) {
            *node = replacement;
            return true;
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut)
            && replace_layout_node(children, node_id, replacement.clone())
        {
            return true;
        }
        if let Some(columns) = node.get_mut("columns").and_then(Value::as_array_mut) {
            for column in columns {
                if let Some(children) = column.get_mut("children").and_then(Value::as_array_mut)
                    && replace_layout_node(children, node_id, replacement.clone())
                {
                    return true;
                }
            }
        }
    }
    false
}

fn remove_layout_node(nodes: &mut Vec<Value>, node_id: &str) -> Option<Value> {
    if let Some(index) = nodes
        .iter()
        .position(|node| layout_node_id(node) == Some(node_id))
    {
        return Some(nodes.remove(index));
    }
    for node in nodes {
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut)
            && let Some(removed) = remove_layout_node(children, node_id)
        {
            return Some(removed);
        }
        if let Some(columns) = node.get_mut("columns").and_then(Value::as_array_mut) {
            for column in columns {
                if let Some(children) = column.get_mut("children").and_then(Value::as_array_mut)
                    && let Some(removed) = remove_layout_node(children, node_id)
                {
                    return Some(removed);
                }
            }
        }
    }
    None
}

fn layout_node_id(node: &Value) -> Option<&str> {
    node.get("id").and_then(Value::as_str)
}

fn layout_node_not_found(node_id: &str) -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params(format!("Unknown report layout node '{}'.", node_id), None)
}

fn apply_json_merge_patch(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, patch_value) in patch {
                if patch_value.is_null() {
                    target.remove(key);
                } else {
                    apply_json_merge_patch(
                        target.entry(key.clone()).or_insert(Value::Null),
                        patch_value,
                    );
                }
            }
        }
        (target, patch) => {
            *target = patch.clone();
        }
    }
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
    let mut result: Value = serde_json::from_str(r###"{
        "definitionVersion": 1,
        "purpose": "Canonical MCP contract for authoring Runtara reports. Call this before create_report, update_report, add_report_block, replace_report_block, patch_report_block, or report layout mutations.",
        "relatedTools": [
            "list_workflows",
            "list_executions",
            "get_execution",
            "list_object_schemas",
            "get_object_schema",
            "query_aggregate",
            "validate_report",
            "render_report",
            "get_report_block_data",
            "add_report_layout_node",
            "replace_report_layout_node",
            "patch_report_layout_node",
            "move_report_layout_node",
            "remove_report_layout_node"
        ],
        "definitionShape": {
            "definitionVersion": 1,
            "markdown": "Backward-compatible narrative Markdown. Render data blocks with standalone placeholders like {{ block.daily_qty }} only when definition.layout is absent. Do not put block placeholders inside Markdown tables for alignment/layout.",
            "layout": "Optional structured layout tree. Prefer this over Markdown placeholders for dashboards and report layout. Every layout node must include a stable id and type.",
            "views": "Optional named report views for master/detail navigation. Each view has an id, optional title/titleFrom, breadcrumb, and its own layout.",
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
                    {"id": "list", "title": "Review cases", "layout": [{"id": "cases_node", "type": "block", "blockId": "cases"}]},
                    {"id": "detail", "titleFrom": "filters.case_id", "breadcrumb": [{"label": "Review cases", "viewId": "list", "clearFilters": ["case_id"]}], "layout": [{"id": "case_summary_node", "type": "block", "blockId": "case_summary"}]}
                ],
                "interaction": {"id": "open_case", "trigger": {"event": "row_click"}, "actions": [{"type": "set_filter", "filterId": "case_id", "valueFrom": "datum.case_id"}, {"type": "navigate_view", "viewId": "detail"}]}
            }
        },
        "layoutGuidance": {
            "currentContract": [
                "Prefer definition.layout for all visual arrangement.",
                "Supported layout node types are markdown, block, metric_row, section, columns, and grid.",
                "Every layout node has a stable id so MCP can add, replace, patch, move, or remove one layout node at a time.",
                "Use Markdown only for narrative text. If using legacy markdown placeholders, put each {{ block.<id> }} placeholder on its own line or paragraph.",
                "Never wrap block placeholders in Markdown table rows such as | {{ block.a }} | {{ block.b }} |. Use metric_row, columns, or grid instead."
            ],
            "layoutNodes": {
                "markdown": {"id": "intro", "type": "markdown", "content": "# Report\n\nNarrative text."},
                "block": {"id": "records_node", "type": "block", "blockId": "records"},
                "metric_row": {"id": "summary_metrics", "type": "metric_row", "blocks": ["total_records", "open_records"]},
                "section": {"id": "summary_section", "type": "section", "title": "Summary", "description": "Optional context.", "children": [{"id": "summary_metrics", "type": "metric_row", "blocks": ["total_records"]}]},
                "columns": {"id": "comparison", "type": "columns", "columns": [{"id": "left", "width": 1, "children": [{"id": "left_chart_node", "type": "block", "blockId": "left_chart"}]}, {"id": "right", "width": 1, "children": [{"id": "right_table_node", "type": "block", "blockId": "right_table"}]}]},
                "grid": {"id": "dashboard_grid", "type": "grid", "columns": 12, "items": [{"blockId": "trend", "colSpan": 8}, {"blockId": "records", "colSpan": 4}]}
            }
        },
        "blockShape": {
            "common": {
                "id": "Stable id, unique within the report. Referenced as {{ block.<id> }} in markdown.",
                "type": "table | chart | metric | actions | markdown | card",
                "title": "Optional UI title.",
                "lazy": "Optional boolean. Lazy blocks fetch only when requested.",
                "showWhen": "Optional visibility condition such as {filter:'case_id', exists:true}. Use this for inline dependent content.",
                "dataset": "Preferred BI query shape: {id, dimensions, measures, orderBy?, limit?}. The id must match definition.datasets[].id.",
                "source": "Object Model data source and query plan. Required only when block.dataset is absent.",
                "filters": "Optional per-block filter presets.",
                "interactions": "Optional drill/cross-filter/navigation actions. Use point_click, row_click, or cell_click triggers with set_filter, clear_filter, clear_filters, and navigate_view actions."
            },
            "table": {
                "type": "table",
                "configKey": "table",
                "columnsPath": "table.columns",
                "columns": [{"field": "sku", "label": "SKU", "format": "optional formatter"}, {"field": "stock_trend", "label": "Trend", "type": "chart", "chart": {"kind": "line", "x": "snapshot_date", "series": [{"field": "qty", "label": "Qty"}]}, "source": {"schema": "StockSnapshot", "mode": "aggregate", "groupBy": ["snapshot_date"], "aggregates": [{"alias": "qty", "op": "sum", "field": "qty"}], "join": [{"parentField": "sku", "field": "sku"}]}}],
                "defaultSort": [{"field": "sku", "direction": "asc"}],
                "pagination": {"defaultPageSize": 50, "allowedPageSizes": [25, 50, 100]},
                "writeback": {
                    "editable": "Optional boolean. When true, the table renders an inline editor on the cell and writes the new value back to the underlying Object Model record via PUT /api/runtime/object-model/instances/{schemaId}/{instanceId}. Only honored when source.kind='object_model', source.mode='filter', and source.join is empty/absent (rows must carry a stable id+schemaId). Type='chart' columns and joined lookup columns are never editable.",
                    "displayField": "Optional row field to render while writes still target field. Use this with joined labels, e.g. field='category_id', displayField='category.name'.",
                    "editor": "Optional explicit editor config: {kind, lookup?, options?, min?, max?, step?, regex?, placeholder?}. kind is one of text | textarea | number | select | toggle | date | datetime | lookup. For lookup, set editor.lookup={schema, valueField, labelField, searchFields?, connectionId?, condition?, filterMappings?}. The editor searches the lookup schema, displays labelField, and writes valueField into the edited row field.",
                    "note": "Writeback is opt-in per column. Auth + type validation happens on the object-model endpoint, not in the report layer — viewers need write permission on the underlying schema. The 'editable' flag here is a UI hint; it does not relax server-side authorization."
                },
                "note": "Tables support source.mode='filter' for row data and source.mode='aggregate' for grouped aggregate result sets. Configure visible/searchable/sortable fields in table.columns. A table column may use type='chart' for inline aggregate charts or type='value' with source.select for scalar joined lookups. To enable inline writeback on a column, see writeback.editable."
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
                    "kind": "value (default) | json | markdown | subcard | subtable",
                    "format": "Format hint for kind=value: currency, currency_compact, decimal, percent, datetime, date, number, pill.",
                    "pillVariants": "{value: variant} map for color-coding enum/status fields. variant is one of default, secondary, destructive, outline, muted, success, warning. Use this on enum columns like status/severity/decision.",
                    "collapsed": "Optional. For json/markdown/subcard/subtable: start collapsed behind a Show/Hide toggle.",
                    "colSpan": "Optional 1–4 grid column span within the parent group.",
                    "subcard": "Required when kind=subcard. Recursive card config {groups: […]} applied to the nested object value at row[field].",
                    "subtable": "Required when kind=subtable. {columns: [{field, label?, format?, pillVariants?, align?}], emptyLabel?} applied to the array value at row[field].",
                    "editable": "Optional boolean. Only honored on kind=value fields when the rendered card row carries id+schemaId (filter-mode object_model card). Renders an inline editor that writes back via PUT /api/runtime/object-model/instances/{schemaId}/{instanceId}.",
                    "editor": "Optional explicit editor config: {kind, lookup?, options?, min?, max?, step?, regex?, placeholder?}. kind is one of text | textarea | number | select | toggle | date | datetime | lookup. For lookup, set editor.lookup={schema, valueField, labelField, searchFields?, connectionId?, condition?, filterMappings?}. The editor searches the lookup schema, displays labelField, and writes valueField into the edited row field."
                },
                "note": "Cards are the right primitive for single-row dossier-style presentation: case headers, AI/Human decision recaps, raw L1 source rows. Use kind=subtable for arrays-of-objects (timelines, line items) and kind=subcard for nested object summaries (applicant_summary, financial_summary). Pair format=pill + pillVariants on enum fields to color-code status, severity, decision, etc."
            }
        },
        "sourceShape": {
            "kind": "object_model | workflow_runtime. Omit for Object Model back compatibility.",
            "schema": "Object Model schema name. Use get_object_schema to inspect valid fields.",
            "entity": "Workflow runtime only: instances | actions.",
            "workflowId": "Workflow runtime only: workflow id whose instances/actions should be shown.",
            "instanceId": "Workflow runtime actions only: optional workflow instance UUID to scope open actions.",
            "select": "Table value-column source only: scalar field to copy from the joined schema.",
            "connectionId": "Optional connection id for connection-scoped schemas.",
            "mode": "filter | aggregate",
            "condition": "Optional condition DSL. Object Model sources can use schema fields and same-store subquery operands. workflow_runtime actions can filter virtual action fields including actionKey, correlation.<key>, and context.<key>.",
            "filterMappings": "Optional mappings from global filter ids to source fields.",
            "groupBy": "Aggregate output grouping fields.",
            "aggregates": "Aggregate specs. Report aggregate specs use {alias, op, field?, distinct?, orderBy?, expression?}. Use op/field here, not fn/column.",
            "orderBy": "Sort array using {field, direction}. Field must be a row field, groupBy field, or aggregate alias depending on source mode.",
            "limit": "Optional row/group cap.",
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
            "For editable lookup/reference fields, keep the stored id in field, optionally render a joined label with displayField, and set editor.kind='lookup' with editor.lookup={schema, valueField, labelField, searchFields?}."
        ],
        "commonMistakes": [
            "For BI reports, do not hand-author repeated raw aggregate block.source specs when the same semantic fields can live in definition.datasets.",
            "Do not put dataset dimensions/measures directly on a block. Use block.dataset.dimensions and block.dataset.measures.",
            "Do not put columns at block.columns, block.fields, or source.columns. Use block.table.columns.",
            "Do not put chartType, x, or y at block top-level. Use block.chart.kind, block.chart.x, and block.chart.series[].field.",
            "Do not use metric.valueAlias or top-level valueAlias. Use block.metric.valueField.",
            "Do not copy query_aggregate specs directly: report aggregates use op/field while query_aggregate uses fn/column.",
            "Do not use source.mode='aggregate' with table.columns pointing at ungrouped raw schema fields; use groupBy fields or aggregate aliases.",
            "Do not use Markdown tables to align report block placeholders. Use definition.layout with metric_row, columns, or grid.",
            "Do not omit layout node ids. MCP layout mutation tools address layout nodes by id.",
            "Do not hardcode large select option lists when the values live in Object Model data. Use filter.options.source='object_model'.",
            "Do not hardcode lookup editor option lists when the values live in another Object Model. Use editor.kind='lookup' and editor.lookup instead.",
            "Do not call workflow signals 'pendingInput' in report definitions. Use the generic actions abstraction: type='actions' and source.entity='actions'.",
            "Do not put schema, connectionId, joins, groupBy, or aggregates on workflow_runtime sources.",
            "Do not use type='actions' with Object Model sources; actions blocks currently require source.kind='workflow_runtime' and entity='actions'.",
            "Always run validate_report before saving or mutating report blocks.",
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
            "layout": [
                {"id": "intro", "type": "markdown", "content": "# Demand summary\n\nLive Object Model data."},
                {"id": "summary", "type": "metric_row", "blocks": ["total_snaps", "unique_skus"]},
                {"id": "main_grid", "type": "grid", "columns": 12, "items": [{"blockId": "daily_qty", "colSpan": 8}, {"blockId": "top_vendors", "colSpan": 4}]}
            ],
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

fn collect_report_definition_authoring_issues(definition: &Value) -> Vec<AuthoringIssue> {
    let mut issues = Vec::new();
    collect_unknown_keys(
        "$",
        definition,
        &[
            "definitionVersion",
            "markdown",
            "layout",
            "views",
            "filters",
            "datasets",
            "blocks",
        ],
        &mut issues,
    );
    collect_markdown_layout_issues(definition, &mut issues);
    collect_layout_authoring_issues(definition, &mut issues);
    collect_report_view_authoring_issues(definition, &mut issues);

    if let Some(datasets) = definition.get("datasets") {
        match datasets.as_array() {
            Some(datasets) => {
                for (index, dataset) in datasets.iter().enumerate() {
                    collect_dataset_authoring_issues(
                        &format!("$.datasets[{index}]"),
                        dataset,
                        &mut issues,
                    );
                }
            }
            None => issues.push(error(
                "$.datasets",
                "INVALID_DATASETS",
                "Report definition datasets must be an array.",
            )),
        }
    }

    if let Some(filters) = definition.get("filters") {
        match filters.as_array() {
            Some(filters) => {
                for (index, filter) in filters.iter().enumerate() {
                    collect_report_filter_authoring_issues(
                        &format!("$.filters[{index}]"),
                        filter,
                        &mut issues,
                    );
                }
            }
            None => issues.push(error(
                "$.filters",
                "INVALID_FILTERS",
                "Report definition filters must be an array.",
            )),
        }
    }

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

    collect_dynamic_condition_filter_ref_authoring_issues(definition, &mut issues);

    issues
}

fn collect_dataset_authoring_issues(path: &str, dataset: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys(
        path,
        dataset,
        &[
            "id",
            "label",
            "source",
            "timeDimension",
            "dimensions",
            "measures",
        ],
        issues,
    );
    if dataset
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        issues.push(error(
            format!("{path}.id"),
            "MISSING_DATASET_ID",
            "Dataset must include a stable non-empty id.",
        ));
    }
    if dataset
        .get("label")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        issues.push(error(
            format!("{path}.label"),
            "MISSING_DATASET_LABEL",
            "Dataset must include a label.",
        ));
    }
    match dataset.get("source") {
        Some(source) => {
            collect_unknown_keys(
                &format!("{path}.source"),
                source,
                &["schema", "connectionId"],
                issues,
            );
            if source
                .get("schema")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                issues.push(error(
                    format!("{path}.source.schema"),
                    "MISSING_DATASET_SOURCE_SCHEMA",
                    "Dataset source must include an Object Model schema name.",
                ));
            }
        }
        None => issues.push(error(
            format!("{path}.source"),
            "MISSING_DATASET_SOURCE",
            "Dataset must include source with at least {schema}.",
        )),
    }

    match dataset.get("dimensions").and_then(Value::as_array) {
        Some(dimensions) => {
            for (index, dimension) in dimensions.iter().enumerate() {
                let dimension_path = format!("{path}.dimensions[{index}]");
                collect_unknown_keys(
                    &dimension_path,
                    dimension,
                    &["field", "label", "type", "format"],
                    issues,
                );
                for key in ["field", "label", "type"] {
                    if dimension
                        .get(key)
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
                    {
                        issues.push(error(
                            format!("{dimension_path}.{key}"),
                            "MISSING_DATASET_DIMENSION_FIELD",
                            "Dataset dimensions must include field, label, and type.",
                        ));
                    }
                }
            }
        }
        None => issues.push(error(
            format!("{path}.dimensions"),
            "MISSING_DATASET_DIMENSIONS",
            "Dataset must include dimensions: [{field, label, type, format?}, ...].",
        )),
    }

    match dataset.get("measures").and_then(Value::as_array) {
        Some(measures) => {
            for (index, measure) in measures.iter().enumerate() {
                let measure_path = format!("{path}.measures[{index}]");
                collect_unknown_keys_with_messages(
                    &measure_path,
                    measure,
                    &[
                        "id",
                        "label",
                        "op",
                        "field",
                        "distinct",
                        "orderBy",
                        "expression",
                        "percentile",
                        "format",
                    ],
                    |key| match key {
                        "alias" => Some((
                            "MISNAMED_DATASET_MEASURE_ID",
                            "Dataset measures use id, not alias.",
                        )),
                        "column" => Some((
                            "MISNAMED_DATASET_MEASURE_FIELD",
                            "Dataset measures use field, not column.",
                        )),
                        _ => None,
                    },
                    issues,
                );
                for key in ["id", "label", "op", "format"] {
                    if measure
                        .get(key)
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
                    {
                        issues.push(error(
                            format!("{measure_path}.{key}"),
                            "MISSING_DATASET_MEASURE_FIELD",
                            "Dataset measures must include id, label, op, and format.",
                        ));
                    }
                }
                if let Some(order_by) = measure.get("orderBy").and_then(Value::as_array) {
                    for (order_index, order) in order_by.iter().enumerate() {
                        collect_order_by_issues(
                            &format!("{measure_path}.orderBy[{order_index}]"),
                            order,
                            issues,
                        );
                    }
                }
            }
        }
        None => issues.push(error(
            format!("{path}.measures"),
            "MISSING_DATASET_MEASURES",
            "Dataset must include measures: [{id, label, op, field?, format}, ...].",
        )),
    }
}

fn collect_markdown_layout_issues(definition: &Value, issues: &mut Vec<AuthoringIssue>) {
    let Some(markdown) = definition.get("markdown").and_then(Value::as_str) else {
        return;
    };

    for (index, line) in markdown.lines().enumerate() {
        if line.contains("{{ block.") && line.contains('|') {
            issues.push(warning(
                format!("$.markdown:{}", index + 1),
                "MARKDOWN_TABLE_BLOCK_LAYOUT",
                "Do not place report block placeholders inside Markdown table rows. Report blocks render as block-level components, so Markdown pipe characters can appear as visible vertical lines. Put each {{ block.<id> }} placeholder on its own line until layout primitives are available.",
            ));
        }
    }
}

fn collect_layout_authoring_issues(definition: &Value, issues: &mut Vec<AuthoringIssue>) {
    let Some(layout) = definition.get("layout") else {
        return;
    };
    let Some(layout) = layout.as_array() else {
        issues.push(error(
            "$.layout",
            "INVALID_REPORT_LAYOUT",
            "Report definition.layout must be an array of layout nodes.",
        ));
        return;
    };

    let block_types = definition
        .get("blocks")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    Some((
                        block.get("id")?.as_str()?.to_string(),
                        block.get("type")?.as_str()?.to_string(),
                    ))
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let block_ids = block_types.keys().cloned().collect::<HashSet<_>>();
    let mut layout_node_ids = HashSet::new();
    for (index, node) in layout.iter().enumerate() {
        collect_layout_node_authoring_issues(
            &format!("$.layout[{index}]"),
            node,
            &block_ids,
            &block_types,
            &mut layout_node_ids,
            issues,
        );
    }
}

fn collect_report_view_authoring_issues(definition: &Value, issues: &mut Vec<AuthoringIssue>) {
    let Some(views) = definition.get("views") else {
        return;
    };
    let Some(views) = views.as_array() else {
        issues.push(error(
            "$.views",
            "INVALID_REPORT_VIEWS",
            "Report definition.views must be an array of named report views.",
        ));
        return;
    };

    let block_types = definition
        .get("blocks")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    Some((
                        block.get("id")?.as_str()?.to_string(),
                        block.get("type")?.as_str()?.to_string(),
                    ))
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let block_ids = block_types.keys().cloned().collect::<HashSet<_>>();
    let mut view_ids = HashSet::new();

    for (view_index, view) in views.iter().enumerate() {
        let path = format!("$.views[{view_index}]");
        collect_unknown_keys(
            &path,
            view,
            &["id", "title", "titleFrom", "breadcrumb", "layout"],
            issues,
        );
        let Some(view_id) = view.get("id").and_then(Value::as_str) else {
            issues.push(error(
                format!("{path}.id"),
                "MISSING_REPORT_VIEW_ID",
                "Report views must include a stable id.",
            ));
            continue;
        };
        if view_id.trim().is_empty() {
            issues.push(error(
                format!("{path}.id"),
                "MISSING_REPORT_VIEW_ID",
                "Report view id cannot be empty.",
            ));
        } else if !view_ids.insert(view_id.to_string()) {
            issues.push(error(
                format!("{path}.id"),
                "DUPLICATE_REPORT_VIEW_ID",
                format!("Duplicate report view id '{view_id}'."),
            ));
        }

        if let Some(breadcrumbs) = view.get("breadcrumb") {
            match breadcrumbs.as_array() {
                Some(breadcrumbs) => {
                    for (breadcrumb_index, breadcrumb) in breadcrumbs.iter().enumerate() {
                        collect_unknown_keys(
                            &format!("{path}.breadcrumb[{breadcrumb_index}]"),
                            breadcrumb,
                            &["label", "viewId", "clearFilters"],
                            issues,
                        );
                    }
                }
                None => issues.push(error(
                    format!("{path}.breadcrumb"),
                    "INVALID_REPORT_VIEW_BREADCRUMB",
                    "Report view breadcrumb must be an array.",
                )),
            }
        }

        if let Some(layout) = view.get("layout") {
            let Some(layout) = layout.as_array() else {
                issues.push(error(
                    format!("{path}.layout"),
                    "INVALID_REPORT_VIEW_LAYOUT",
                    "Report view layout must be an array of layout nodes.",
                ));
                continue;
            };
            let mut layout_node_ids = HashSet::new();
            for (node_index, node) in layout.iter().enumerate() {
                collect_layout_node_authoring_issues(
                    &format!("{path}.layout[{node_index}]"),
                    node,
                    &block_ids,
                    &block_types,
                    &mut layout_node_ids,
                    issues,
                );
            }
        }
    }
}

fn collect_layout_node_authoring_issues(
    path: &str,
    node: &Value,
    block_ids: &HashSet<String>,
    block_types: &HashMap<String, String>,
    layout_node_ids: &mut HashSet<String>,
    issues: &mut Vec<AuthoringIssue>,
) {
    let Some(object) = node.as_object() else {
        issues.push(error(
            path,
            "INVALID_REPORT_LAYOUT_NODE",
            "Report layout node must be an object.",
        ));
        return;
    };
    let Some(node_type) = object.get("type").and_then(Value::as_str) else {
        issues.push(error(
            format!("{path}.type"),
            "MISSING_LAYOUT_NODE_TYPE",
            "Report layout node must include type: markdown, block, metric_row, section, columns, or grid.",
        ));
        return;
    };
    let Some(node_id) = object.get("id").and_then(Value::as_str) else {
        issues.push(error(
            format!("{path}.id"),
            "MISSING_LAYOUT_NODE_ID",
            "Report layout node must include a stable id for MCP mutations.",
        ));
        return;
    };
    if node_id.trim().is_empty() {
        issues.push(error(
            format!("{path}.id"),
            "MISSING_LAYOUT_NODE_ID",
            "Report layout node id cannot be empty.",
        ));
    } else if !layout_node_ids.insert(node_id.to_string()) {
        issues.push(error(
            format!("{path}.id"),
            "DUPLICATE_LAYOUT_NODE_ID",
            format!("Duplicate report layout node id '{node_id}'."),
        ));
    }

    match node_type {
        "markdown" => {
            collect_unknown_keys(path, node, &["id", "type", "content", "showWhen"], issues);
            if object.get("content").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.content"),
                    "MISSING_LAYOUT_MARKDOWN_CONTENT",
                    "Markdown layout nodes must include content.",
                ));
            }
        }
        "block" => {
            collect_unknown_keys(path, node, &["id", "type", "blockId", "showWhen"], issues);
            collect_layout_block_reference_issue(
                &format!("{path}.blockId"),
                object.get("blockId"),
                block_ids,
                issues,
            );
        }
        "metric_row" => {
            collect_unknown_keys(
                path,
                node,
                &["id", "type", "title", "blocks", "showWhen"],
                issues,
            );
            let Some(blocks) = object.get("blocks").and_then(Value::as_array) else {
                issues.push(error(
                    format!("{path}.blocks"),
                    "MISSING_LAYOUT_METRIC_ROW_BLOCKS",
                    "Metric row layout nodes must include blocks: [metricBlockId, ...].",
                ));
                return;
            };
            for (index, block) in blocks.iter().enumerate() {
                let block_path = format!("{path}.blocks[{index}]");
                let Some(block_id) = block.as_str() else {
                    issues.push(error(
                        block_path,
                        "INVALID_LAYOUT_BLOCK_REFERENCE",
                        "Metric row block references must be block id strings.",
                    ));
                    continue;
                };
                if !block_ids.contains(block_id) {
                    issues.push(error(
                        block_path,
                        "UNKNOWN_LAYOUT_BLOCK_REFERENCE",
                        format!("Layout references unknown report block '{block_id}'."),
                    ));
                } else if block_types.get(block_id).map(String::as_str) != Some("metric") {
                    issues.push(error(
                        block_path,
                        "INVALID_METRIC_ROW_BLOCK",
                        format!("Metric row references non-metric block '{block_id}'."),
                    ));
                }
            }
        }
        "section" => {
            collect_unknown_keys(
                path,
                node,
                &["id", "type", "title", "description", "children", "showWhen"],
                issues,
            );
            if let Some(children) = object.get("children") {
                collect_layout_children_authoring_issues(
                    &format!("{path}.children"),
                    children,
                    block_ids,
                    block_types,
                    layout_node_ids,
                    issues,
                );
            }
        }
        "columns" => {
            collect_unknown_keys(path, node, &["id", "type", "columns", "showWhen"], issues);
            let Some(columns) = object.get("columns").and_then(Value::as_array) else {
                issues.push(error(
                    format!("{path}.columns"),
                    "MISSING_LAYOUT_COLUMNS",
                    "Columns layout nodes must include columns.",
                ));
                return;
            };
            for (column_index, column) in columns.iter().enumerate() {
                let column_path = format!("{path}.columns[{column_index}]");
                collect_unknown_keys(&column_path, column, &["id", "width", "children"], issues);
                if column.get("id").and_then(Value::as_str).is_none() {
                    issues.push(error(
                        format!("{column_path}.id"),
                        "MISSING_LAYOUT_COLUMN_ID",
                        "Layout columns must include id so MCP can target them.",
                    ));
                }
                if let Some(children) = column.get("children") {
                    collect_layout_children_authoring_issues(
                        &format!("{column_path}.children"),
                        children,
                        block_ids,
                        block_types,
                        layout_node_ids,
                        issues,
                    );
                }
            }
        }
        "grid" => {
            collect_unknown_keys(
                path,
                node,
                &["id", "type", "columns", "items", "showWhen"],
                issues,
            );
            let Some(items) = object.get("items").and_then(Value::as_array) else {
                issues.push(error(
                    format!("{path}.items"),
                    "MISSING_LAYOUT_GRID_ITEMS",
                    "Grid layout nodes must include items.",
                ));
                return;
            };
            for (item_index, item) in items.iter().enumerate() {
                let item_path = format!("{path}.items[{item_index}]");
                collect_unknown_keys(
                    &item_path,
                    item,
                    &["id", "blockId", "colSpan", "rowSpan"],
                    issues,
                );
                collect_layout_block_reference_issue(
                    &format!("{item_path}.blockId"),
                    item.get("blockId"),
                    block_ids,
                    issues,
                );
            }
        }
        _ => issues.push(error(
            format!("{path}.type"),
            "UNKNOWN_LAYOUT_NODE_TYPE",
            format!("Unknown report layout node type '{node_type}'."),
        )),
    }
}

fn collect_layout_children_authoring_issues(
    path: &str,
    children: &Value,
    block_ids: &HashSet<String>,
    block_types: &HashMap<String, String>,
    layout_node_ids: &mut HashSet<String>,
    issues: &mut Vec<AuthoringIssue>,
) {
    let Some(children) = children.as_array() else {
        issues.push(error(
            path,
            "INVALID_LAYOUT_CHILDREN",
            "Report layout children must be an array.",
        ));
        return;
    };
    for (index, child) in children.iter().enumerate() {
        collect_layout_node_authoring_issues(
            &format!("{path}[{index}]"),
            child,
            block_ids,
            block_types,
            layout_node_ids,
            issues,
        );
    }
}

fn collect_layout_block_reference_issue(
    path: &str,
    block_value: Option<&Value>,
    block_ids: &HashSet<String>,
    issues: &mut Vec<AuthoringIssue>,
) {
    let Some(block_id) = block_value.and_then(Value::as_str) else {
        issues.push(error(
            path,
            "MISSING_LAYOUT_BLOCK_REFERENCE",
            "Layout block reference must be a block id string.",
        ));
        return;
    };
    if !block_ids.contains(block_id) {
        issues.push(error(
            path,
            "UNKNOWN_LAYOUT_BLOCK_REFERENCE",
            format!("Layout references unknown report block '{block_id}'."),
        ));
    }
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

    let allowed_block_keys = [
        "id",
        "type",
        "title",
        "lazy",
        "dataset",
        "source",
        "table",
        "chart",
        "metric",
        "actions",
        "card",
        "filters",
        "interactions",
        "showWhen",
    ];
    for key in block_object.keys() {
        if allowed_block_keys.contains(&key.as_str()) {
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
                format!(
                    "Unknown report block field '{key}'.{} The report API ignores unknown block fields; use get_report_authoring_schema for the canonical shape.",
                    similar_key_hint(key, &allowed_block_keys)
                        .map(|known| format!(" Did you mean '{known}'?"))
                        .unwrap_or_default()
                ),
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
                "Report block must include type: table, chart, metric, actions, markdown, or card.",
            ));
        }
        let has_dataset = block.get("dataset").is_some();
        match block.get("source") {
            Some(source) if source.is_object() => {
                if !has_dataset {
                    match source_kind(source) {
                        "workflow_runtime" => {
                            if source
                                .get("workflowId")
                                .and_then(Value::as_str)
                                .is_none_or(str::is_empty)
                            {
                                issues.push(error(
                                    format!("{path}.source.workflowId"),
                                    "MISSING_WORKFLOW_RUNTIME_WORKFLOW_ID",
                                    "Workflow runtime report source must include workflowId.",
                                ));
                            }
                            if source.get("entity").and_then(Value::as_str).is_none() {
                                issues.push(error(
                                    format!("{path}.source.entity"),
                                    "MISSING_WORKFLOW_RUNTIME_ENTITY",
                                    "Workflow runtime report source must include entity: instances or actions.",
                                ));
                            }
                        }
                        _ => {
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
                    }
                }
            }
            _ if !has_dataset => issues.push(error(
                format!("{path}.source"),
                "MISSING_BLOCK_SOURCE",
                "Report block must include either dataset or source. Object Model sources need schema; workflow_runtime sources need kind, entity, and workflowId.",
            )),
            _ => {}
        }
    }

    if let Some(dataset) = block.get("dataset") {
        collect_block_dataset_issues(&format!("{path}.dataset"), dataset, issues);
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
    if let Some(actions) = block.get("actions") {
        collect_block_actions_issues(&format!("{path}.actions"), actions, issues);
    }
    if let Some(card) = block.get("card") {
        collect_card_issues(&format!("{path}.card"), card, issues);
    }
    if let Some(filters) = block.get("filters") {
        match filters.as_array() {
            Some(filters) => {
                for (index, filter) in filters.iter().enumerate() {
                    collect_report_filter_authoring_issues(
                        &format!("{path}.filters[{index}]"),
                        filter,
                        issues,
                    );
                }
            }
            None => issues.push(error(
                format!("{path}.filters"),
                "INVALID_BLOCK_FILTERS",
                "Report block filters must be an array.",
            )),
        }
    }
    if let Some(interactions) = block.get("interactions") {
        match interactions.as_array() {
            Some(interactions) => {
                for (index, interaction) in interactions.iter().enumerate() {
                    collect_interaction_issues(
                        &format!("{path}.interactions[{index}]"),
                        interaction,
                        issues,
                    );
                }
            }
            None => issues.push(error(
                format!("{path}.interactions"),
                "INVALID_BLOCK_INTERACTIONS",
                "Report block interactions must be an array.",
            )),
        }
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
                && block.get("dataset").is_none()
            {
                issues.push(error(
                    format!("{path}.source.aggregates"),
                    "MISSING_CHART_QUERY",
                    "Chart blocks need either dataset.measures or source.aggregates so the renderer has value series to plot.",
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
                && block.get("dataset").is_none()
            {
                issues.push(error(
                    format!("{path}.source.aggregates"),
                    "MISSING_METRIC_QUERY",
                    "Metric blocks need either dataset.measures or source.aggregates so metric.valueField has a value.",
                ));
            }
        }
        Some("actions") => {
            let Some(source) = block.get("source").filter(|source| source.is_object()) else {
                issues.push(error(
                    format!("{path}.source"),
                    "MISSING_ACTIONS_SOURCE",
                    "Actions blocks must include source.kind='workflow_runtime' and entity='actions'.",
                ));
                return;
            };
            if source_kind(source) != "workflow_runtime" {
                issues.push(error(
                    format!("{path}.source.kind"),
                    "INVALID_ACTIONS_SOURCE_KIND",
                    "Actions blocks require source.kind='workflow_runtime'.",
                ));
            }
            if source.get("entity").and_then(Value::as_str) != Some("actions") {
                issues.push(error(
                    format!("{path}.source.entity"),
                    "INVALID_ACTIONS_SOURCE_ENTITY",
                    "Actions blocks require source.entity='actions'.",
                ));
            }
        }
        _ => {}
    }
}

fn collect_block_actions_issues(path: &str, actions: &Value, issues: &mut Vec<AuthoringIssue>) {
    let Some(actions_object) = actions.as_object() else {
        issues.push(error(
            path,
            "INVALID_ACTIONS_CONFIG",
            "Report block actions config must be an object.",
        ));
        return;
    };

    collect_unknown_keys(path, actions, &["submit"], issues);
    if let Some(submit) = actions_object.get("submit") {
        let Some(submit_object) = submit.as_object() else {
            issues.push(error(
                format!("{path}.submit"),
                "INVALID_ACTIONS_SUBMIT_CONFIG",
                "Report block actions.submit config must be an object.",
            ));
            return;
        };

        collect_unknown_keys(
            &format!("{path}.submit"),
            submit,
            &["label", "implicitPayload"],
            issues,
        );
        if submit_object
            .get("implicitPayload")
            .is_some_and(|implicit_payload| !implicit_payload.is_object())
        {
            issues.push(error(
                format!("{path}.submit.implicitPayload"),
                "INVALID_ACTIONS_IMPLICIT_PAYLOAD",
                "Report block actions.submit.implicitPayload must be an object keyed by payload field name.",
            ));
        }
    }
}

fn collect_block_dataset_issues(path: &str, dataset: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys(
        path,
        dataset,
        &[
            "id",
            "dimensions",
            "measures",
            "orderBy",
            "datasetFilters",
            "limit",
        ],
        issues,
    );
    if dataset
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        issues.push(error(
            format!("{path}.id"),
            "MISSING_BLOCK_DATASET_ID",
            "Block dataset reference must include id matching definition.datasets[].id.",
        ));
    }
    match dataset.get("dimensions") {
        Some(value) if value.as_array().is_some() => {}
        Some(_) => issues.push(error(
            format!("{path}.dimensions"),
            "INVALID_BLOCK_DATASET_DIMENSIONS",
            "Block dataset dimensions must be an array of dataset dimension ids.",
        )),
        None => {}
    }
    match dataset.get("measures") {
        Some(value) if value.as_array().is_some() => {}
        Some(_) => issues.push(error(
            format!("{path}.measures"),
            "INVALID_BLOCK_DATASET_MEASURES",
            "Block dataset measures must be an array of dataset measure ids.",
        )),
        None => issues.push(error(
            format!("{path}.measures"),
            "MISSING_BLOCK_DATASET_MEASURES",
            "Block dataset reference must select at least one measure.",
        )),
    }
    if let Some(order_by) = dataset.get("orderBy").and_then(Value::as_array) {
        for (index, order) in order_by.iter().enumerate() {
            collect_order_by_issues(&format!("{path}.orderBy[{index}]"), order, issues);
        }
    }
    if let Some(dataset_filters) = dataset.get("datasetFilters").and_then(Value::as_array) {
        for (index, filter) in dataset_filters.iter().enumerate() {
            collect_unknown_keys(
                &format!("{path}.datasetFilters[{index}]"),
                filter,
                &["field", "op", "value"],
                issues,
            );
        }
    }
}

fn collect_source_issues(path: &str, source: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys_with_messages(
        path,
        source,
        &[
            "kind",
            "schema",
            "connectionId",
            "entity",
            "workflowId",
            "instanceId",
            "mode",
            "condition",
            "filterMappings",
            "groupBy",
            "aggregates",
            "orderBy",
            "limit",
            "join",
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

    let kind = source_kind(source);
    if !matches!(kind, "object_model" | "workflow_runtime") {
        issues.push(error(
            format!("{path}.kind"),
            "INVALID_SOURCE_KIND",
            "Report source kind must be object_model or workflow_runtime.",
        ));
    }

    if kind == "workflow_runtime" {
        collect_workflow_runtime_source_issues(path, source, issues);
        return;
    }

    if let Some(condition) = source.get("condition") {
        collect_condition_issues(&format!("{path}.condition"), condition, issues);
    }

    if let Some(filter_mappings) = source.get("filterMappings").and_then(Value::as_array) {
        for (index, mapping) in filter_mappings.iter().enumerate() {
            collect_filter_target_issues(
                &format!("{path}.filterMappings[{index}]"),
                mapping,
                issues,
            );
        }
    }

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

    if let Some(join) = source.get("join").and_then(Value::as_array) {
        for (index, join_entry) in join.iter().enumerate() {
            collect_unknown_keys(
                &format!("{path}.join[{index}]"),
                join_entry,
                &[
                    "schema",
                    "alias",
                    "connectionId",
                    "parentField",
                    "field",
                    "op",
                    "kind",
                ],
                issues,
            );
            if join_entry.get("schema").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.join[{index}].schema"),
                    "MISSING_JOIN_SCHEMA",
                    "Block-level join entries must include schema (the dimension to join in).",
                ));
            }
            if join_entry
                .get("parentField")
                .and_then(Value::as_str)
                .is_none()
            {
                issues.push(error(
                    format!("{path}.join[{index}].parentField"),
                    "MISSING_JOIN_PARENT_FIELD",
                    "Block-level join entries must include parentField (the column on the \
                     primary schema).",
                ));
            }
            if join_entry.get("field").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.join[{index}].field"),
                    "MISSING_JOIN_FIELD",
                    "Block-level join entries must include field (the column on the joined \
                     dimension).",
                ));
            }
        }
    }
}

fn source_kind(source: &Value) -> &str {
    source
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("object_model")
}

fn collect_workflow_runtime_source_issues(
    path: &str,
    source: &Value,
    issues: &mut Vec<AuthoringIssue>,
) {
    if source
        .get("workflowId")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        issues.push(error(
            format!("{path}.workflowId"),
            "MISSING_WORKFLOW_RUNTIME_WORKFLOW_ID",
            "Workflow runtime report source must include workflowId.",
        ));
    }

    match source.get("entity").and_then(Value::as_str) {
        Some("instances" | "actions") => {}
        Some(_) => issues.push(error(
            format!("{path}.entity"),
            "INVALID_WORKFLOW_RUNTIME_ENTITY",
            "Workflow runtime source entity must be instances or actions.",
        )),
        None => issues.push(error(
            format!("{path}.entity"),
            "MISSING_WORKFLOW_RUNTIME_ENTITY",
            "Workflow runtime source must include entity: instances or actions.",
        )),
    }

    if source
        .get("schema")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        issues.push(error(
            format!("{path}.schema"),
            "WORKFLOW_RUNTIME_SCHEMA_NOT_ALLOWED",
            "Workflow runtime sources must not set schema.",
        ));
    }
    if source.get("connectionId").is_some() {
        issues.push(error(
            format!("{path}.connectionId"),
            "WORKFLOW_RUNTIME_CONNECTION_NOT_ALLOWED",
            "Workflow runtime sources must not set connectionId.",
        ));
    }
    if source
        .get("mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode != "filter")
    {
        issues.push(error(
            format!("{path}.mode"),
            "INVALID_WORKFLOW_RUNTIME_MODE",
            "Workflow runtime sources only support mode='filter'.",
        ));
    }

    for key in ["groupBy", "aggregates", "join"] {
        if source.get(key).is_some() {
            issues.push(error(
                format!("{path}.{key}"),
                "WORKFLOW_RUNTIME_QUERY_FIELD_NOT_ALLOWED",
                "Workflow runtime sources do not support groupBy, aggregates, or join.",
            ));
        }
    }

    if let Some(condition) = source.get("condition") {
        collect_condition_issues(&format!("{path}.condition"), condition, issues);
    }
    if let Some(filter_mappings) = source.get("filterMappings").and_then(Value::as_array) {
        for (index, mapping) in filter_mappings.iter().enumerate() {
            collect_filter_target_issues(
                &format!("{path}.filterMappings[{index}]"),
                mapping,
                issues,
            );
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
                &[
                    "field",
                    "label",
                    "displayField",
                    "format",
                    "type",
                    "chart",
                    "source",
                    "editable",
                    "editor",
                ],
                issues,
            );
            if column.get("field").and_then(Value::as_str).is_none() {
                issues.push(error(
                    format!("{path}.columns[{index}].field"),
                    "MISSING_TABLE_COLUMN_FIELD",
                    "Each table column must include field.",
                ));
            }
            let column_type = column.get("type").and_then(Value::as_str);
            if column_type == Some("chart") {
                if let Some(chart) = column.get("chart") {
                    collect_chart_issues(&format!("{path}.columns[{index}].chart"), chart, issues);
                } else {
                    issues.push(error(
                        format!("{path}.columns[{index}].chart"),
                        "MISSING_TABLE_COLUMN_CHART",
                        "Chart table columns must include chart.kind, chart.x, and chart.series.",
                    ));
                }
                if let Some(source) = column.get("source") {
                    collect_table_column_source_issues(
                        &format!("{path}.columns[{index}].source"),
                        source,
                        "chart",
                        issues,
                    );
                } else {
                    issues.push(error(
                        format!("{path}.columns[{index}].source"),
                        "MISSING_TABLE_COLUMN_SOURCE",
                        "Chart table columns must include an aggregate source joined to the parent row.",
                    ));
                }
            } else if column_type == Some("value")
                && let Some(source) = column.get("source")
            {
                collect_table_column_source_issues(
                    &format!("{path}.columns[{index}].source"),
                    source,
                    "value",
                    issues,
                );
            }
            if let Some(editor) = column.get("editor") {
                collect_editor_issues(&format!("{path}.columns[{index}].editor"), editor, issues);
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

fn collect_table_column_source_issues(
    path: &str,
    source: &Value,
    column_type: &str,
    issues: &mut Vec<AuthoringIssue>,
) {
    collect_unknown_keys_with_messages(
        path,
        source,
        &[
            "schema",
            "select",
            "connectionId",
            "mode",
            "condition",
            "filterMappings",
            "groupBy",
            "aggregates",
            "orderBy",
            "limit",
            "join",
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

    if let Some(condition) = source.get("condition") {
        collect_condition_issues(&format!("{path}.condition"), condition, issues);
    }

    if let Some(filter_mappings) = source.get("filterMappings").and_then(Value::as_array) {
        for (index, mapping) in filter_mappings.iter().enumerate() {
            collect_filter_target_issues(
                &format!("{path}.filterMappings[{index}]"),
                mapping,
                issues,
            );
        }
    }

    if column_type == "chart" && source.get("mode").and_then(Value::as_str) != Some("aggregate") {
        issues.push(error(
            format!("{path}.mode"),
            "INVALID_TABLE_COLUMN_SOURCE_MODE",
            "Chart table column sources must use mode='aggregate'.",
        ));
    }
    if column_type == "value" {
        if source
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("filter")
            != "filter"
        {
            issues.push(error(
                format!("{path}.mode"),
                "INVALID_TABLE_COLUMN_SOURCE_MODE",
                "Value table column sources must use mode='filter'.",
            ));
        }
        if source
            .get("select")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            issues.push(error(
                format!("{path}.select"),
                "MISSING_TABLE_VALUE_SELECT",
                "Value table column sources must include select.",
            ));
        }
    }
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
    if let Some(join) = source.get("join").and_then(Value::as_array) {
        for (index, join_entry) in join.iter().enumerate() {
            collect_unknown_keys(
                &format!("{path}.join[{index}]"),
                join_entry,
                &["parentField", "field", "op", "kind"],
                issues,
            );
        }
    }
}

fn collect_card_issues(path: &str, card: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys(path, card, &["groups"], issues);
    let Some(groups) = card.get("groups").and_then(Value::as_array) else {
        issues.push(error(
            format!("{path}.groups"),
            "MISSING_CARD_GROUPS",
            "Card blocks must include card.groups.",
        ));
        return;
    };

    for (group_index, group) in groups.iter().enumerate() {
        let group_path = format!("{path}.groups[{group_index}]");
        collect_unknown_keys(
            &group_path,
            group,
            &["id", "title", "description", "columns", "fields"],
            issues,
        );
        if group
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            issues.push(error(
                format!("{group_path}.id"),
                "MISSING_CARD_GROUP_ID",
                "Card groups must include a stable id.",
            ));
        }
        let Some(fields) = group.get("fields").and_then(Value::as_array) else {
            issues.push(error(
                format!("{group_path}.fields"),
                "MISSING_CARD_GROUP_FIELDS",
                "Card groups must include fields.",
            ));
            continue;
        };
        for (field_index, field) in fields.iter().enumerate() {
            let field_path = format!("{group_path}.fields[{field_index}]");
            collect_card_field_issues(&field_path, field, issues);
        }
    }
}

fn collect_card_field_issues(path: &str, field: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys(
        path,
        field,
        &[
            "field",
            "label",
            "displayField",
            "kind",
            "format",
            "pillVariants",
            "collapsed",
            "colSpan",
            "subcard",
            "subtable",
            "editable",
            "editor",
        ],
        issues,
    );
    if field.get("field").and_then(Value::as_str).is_none() {
        issues.push(error(
            format!("{path}.field"),
            "MISSING_CARD_FIELD",
            "Card fields must include field.",
        ));
    }
    if let Some(editor) = field.get("editor") {
        collect_editor_issues(&format!("{path}.editor"), editor, issues);
    }
    if let Some(subtable) = field.get("subtable") {
        collect_unknown_keys(
            &format!("{path}.subtable"),
            subtable,
            &["columns", "emptyLabel"],
            issues,
        );
    }
    if let Some(subcard) = field.get("subcard") {
        collect_card_issues(&format!("{path}.subcard"), subcard, issues);
    }
}

fn collect_editor_issues(path: &str, editor: &Value, issues: &mut Vec<AuthoringIssue>) {
    let Some(object) = editor.as_object() else {
        issues.push(error(
            path,
            "INVALID_EDITOR_CONFIG",
            "Report editor config must be an object.",
        ));
        return;
    };
    collect_unknown_keys(
        path,
        editor,
        &[
            "kind",
            "lookup",
            "options",
            "min",
            "max",
            "step",
            "regex",
            "placeholder",
        ],
        issues,
    );

    let kind = object.get("kind").and_then(Value::as_str);
    match kind {
        Some(
            "text" | "textarea" | "number" | "select" | "toggle" | "date" | "datetime"
            | "lookup",
        ) => {}
        Some(_) => issues.push(error(
            format!("{path}.kind"),
            "INVALID_EDITOR_KIND",
            "Editor kind must be text, textarea, number, select, toggle, date, datetime, or lookup.",
        )),
        None => issues.push(error(
            format!("{path}.kind"),
            "MISSING_EDITOR_KIND",
            "Editor config must include kind.",
        )),
    }

    if kind == Some("lookup") {
        match object.get("lookup") {
            Some(lookup) => collect_lookup_issues(&format!("{path}.lookup"), lookup, issues),
            None => issues.push(error(
                format!("{path}.lookup"),
                "MISSING_LOOKUP_CONFIG",
                "Lookup editors must include lookup: {schema, valueField, labelField, searchFields?}.",
            )),
        }
    } else if object.get("lookup").is_some() {
        issues.push(error(
            format!("{path}.lookup"),
            "LOOKUP_CONFIG_WITH_NON_LOOKUP_EDITOR",
            "editor.lookup is only valid when editor.kind is lookup.",
        ));
    }

    if let Some(options) = object.get("options")
        && !options.is_array()
    {
        issues.push(error(
            format!("{path}.options"),
            "INVALID_EDITOR_OPTIONS",
            "Editor options must be an array of {label, value}.",
        ));
    }
}

fn collect_lookup_issues(path: &str, lookup: &Value, issues: &mut Vec<AuthoringIssue>) {
    let Some(object) = lookup.as_object() else {
        issues.push(error(
            path,
            "INVALID_LOOKUP_CONFIG",
            "Lookup config must be an object.",
        ));
        return;
    };
    collect_unknown_keys(
        path,
        lookup,
        &[
            "schema",
            "connectionId",
            "field",
            "valueField",
            "labelField",
            "searchFields",
            "condition",
            "filterMappings",
        ],
        issues,
    );
    if object
        .get("schema")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        issues.push(error(
            format!("{path}.schema"),
            "MISSING_LOOKUP_SCHEMA",
            "Lookup config must include schema.",
        ));
    }
    if object
        .get("valueField")
        .or_else(|| object.get("field"))
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        issues.push(error(
            format!("{path}.valueField"),
            "MISSING_LOOKUP_VALUE_FIELD",
            "Lookup config must include valueField (field is accepted as an alias).",
        ));
    }
    if object
        .get("labelField")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        issues.push(error(
            format!("{path}.labelField"),
            "MISSING_LOOKUP_LABEL_FIELD",
            "Lookup config must include labelField.",
        ));
    }
    if let Some(search_fields) = object.get("searchFields")
        && !search_fields.is_array()
    {
        issues.push(error(
            format!("{path}.searchFields"),
            "INVALID_LOOKUP_SEARCH_FIELDS",
            "Lookup searchFields must be an array of field names.",
        ));
    }
    if let Some(condition) = object.get("condition") {
        collect_condition_issues(&format!("{path}.condition"), condition, issues);
    }
    if let Some(filter_mappings) = object.get("filterMappings") {
        match filter_mappings.as_array() {
            Some(mappings) => {
                for (index, mapping) in mappings.iter().enumerate() {
                    collect_filter_target_issues(
                        &format!("{path}.filterMappings[{index}]"),
                        mapping,
                        issues,
                    );
                }
            }
            None => issues.push(error(
                format!("{path}.filterMappings"),
                "INVALID_LOOKUP_FILTER_MAPPINGS",
                "Lookup filterMappings must be an array.",
            )),
        }
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

fn collect_report_filter_authoring_issues(
    path: &str,
    filter: &Value,
    issues: &mut Vec<AuthoringIssue>,
) {
    collect_unknown_keys(
        path,
        filter,
        &[
            "id",
            "label",
            "type",
            "default",
            "required",
            "options",
            "appliesTo",
        ],
        issues,
    );
    for key in ["id", "label", "type"] {
        if filter
            .get(key)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            issues.push(error(
                format!("{path}.{key}"),
                "MISSING_REPORT_FILTER_FIELD",
                "Report filters must include id, label, and type.",
            ));
        }
    }
    if let Some(options) = filter.get("options") {
        collect_filter_options_issues(&format!("{path}.options"), options, issues);
    }
    if let Some(applies_to) = filter.get("appliesTo") {
        match applies_to.as_array() {
            Some(targets) => {
                for (index, target) in targets.iter().enumerate() {
                    collect_filter_target_issues(
                        &format!("{path}.appliesTo[{index}]"),
                        target,
                        issues,
                    );
                }
            }
            None => issues.push(error(
                format!("{path}.appliesTo"),
                "INVALID_FILTER_TARGETS",
                "Report filter appliesTo must be an array.",
            )),
        }
    }
}

fn collect_filter_options_issues(path: &str, options: &Value, issues: &mut Vec<AuthoringIssue>) {
    let Some(object) = options.as_object() else {
        issues.push(error(
            path,
            "INVALID_FILTER_OPTIONS",
            "Report filter options must be an object.",
        ));
        return;
    };

    let source = object.get("source").and_then(Value::as_str);
    let allowed = match source {
        Some("static") => &["source", "values"][..],
        Some("object_model") => &[
            "source",
            "schema",
            "connectionId",
            "field",
            "valueField",
            "labelField",
            "search",
            "dependsOn",
            "filterMappings",
            "condition",
        ][..],
        _ => &[
            "source",
            "values",
            "schema",
            "connectionId",
            "field",
            "valueField",
            "labelField",
            "search",
            "dependsOn",
            "filterMappings",
            "condition",
        ][..],
    };
    collect_unknown_keys(path, options, allowed, issues);

    if let Some(values) = options.get("values")
        && !values.is_array()
    {
        issues.push(error(
            format!("{path}.values"),
            "INVALID_STATIC_FILTER_OPTIONS",
            "Static filter options values must be an array.",
        ));
    }
    if let Some(filter_mappings) = options.get("filterMappings").and_then(Value::as_array) {
        for (index, mapping) in filter_mappings.iter().enumerate() {
            collect_filter_target_issues(
                &format!("{path}.filterMappings[{index}]"),
                mapping,
                issues,
            );
        }
    }
    if let Some(condition) = options.get("condition") {
        collect_condition_issues(&format!("{path}.condition"), condition, issues);
    }
}

fn collect_filter_target_issues(path: &str, target: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys(
        path,
        target,
        &["filterId", "blockId", "field", "op"],
        issues,
    );
    if target.get("field").and_then(Value::as_str).is_none() {
        issues.push(error(
            format!("{path}.field"),
            "MISSING_FILTER_TARGET_FIELD",
            "Report filter targets must include field.",
        ));
    }
}

fn collect_interaction_issues(path: &str, interaction: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_unknown_keys(path, interaction, &["id", "trigger", "actions"], issues);
    if interaction
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        issues.push(error(
            format!("{path}.id"),
            "MISSING_INTERACTION_ID",
            "Report interactions must include a stable id.",
        ));
    }
    if let Some(trigger) = interaction.get("trigger") {
        collect_unknown_keys(
            &format!("{path}.trigger"),
            trigger,
            &["event", "field"],
            issues,
        );
    }
    if let Some(actions) = interaction.get("actions") {
        match actions.as_array() {
            Some(actions) => {
                for (index, action) in actions.iter().enumerate() {
                    collect_unknown_keys(
                        &format!("{path}.actions[{index}]"),
                        action,
                        &[
                            "type",
                            "filterId",
                            "filterIds",
                            "viewId",
                            "valueFrom",
                            "value",
                        ],
                        issues,
                    );
                }
            }
            None => issues.push(error(
                format!("{path}.actions"),
                "INVALID_INTERACTION_ACTIONS",
                "Report interaction actions must be an array.",
            )),
        }
    }
}

fn collect_dynamic_condition_filter_ref_authoring_issues(
    definition: &Value,
    issues: &mut Vec<AuthoringIssue>,
) {
    let report_filters = collect_condition_filter_metadata(
        definition
            .get("filters")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
    );

    if let Some(filters) = definition.get("filters").and_then(Value::as_array) {
        for (index, filter) in filters.iter().enumerate() {
            if let Some(condition) = filter
                .get("options")
                .and_then(Value::as_object)
                .and_then(|options| options.get("condition"))
            {
                collect_condition_filter_ref_issues(
                    &format!("$.filters[{index}].options.condition"),
                    condition,
                    &report_filters,
                    issues,
                );
            }
        }
    }

    let Some(blocks) = definition.get("blocks").and_then(Value::as_array) else {
        return;
    };
    for (block_index, block) in blocks.iter().enumerate() {
        let mut block_filters = report_filters.clone();
        block_filters.extend(collect_condition_filter_metadata(
            block
                .get("filters")
                .and_then(Value::as_array)
                .map(Vec::as_slice),
        ));

        if let Some(condition) = block
            .get("source")
            .and_then(Value::as_object)
            .and_then(|source| source.get("condition"))
        {
            collect_condition_filter_ref_issues(
                &format!("$.blocks[{block_index}].source.condition"),
                condition,
                &block_filters,
                issues,
            );
        }

        if let Some(columns) = block
            .get("table")
            .and_then(Value::as_object)
            .and_then(|table| table.get("columns"))
            .and_then(Value::as_array)
        {
            for (column_index, column) in columns.iter().enumerate() {
                if let Some(condition) = column
                    .get("source")
                    .and_then(Value::as_object)
                    .and_then(|source| source.get("condition"))
                {
                    collect_condition_filter_ref_issues(
                        &format!(
                            "$.blocks[{block_index}].table.columns[{column_index}].source.condition"
                        ),
                        condition,
                        &block_filters,
                        issues,
                    );
                }
                if let Some(condition) = column
                    .get("editor")
                    .and_then(Value::as_object)
                    .and_then(|editor| editor.get("lookup"))
                    .and_then(Value::as_object)
                    .and_then(|lookup| lookup.get("condition"))
                {
                    collect_condition_filter_ref_issues(
                        &format!(
                            "$.blocks[{block_index}].table.columns[{column_index}].editor.lookup.condition"
                        ),
                        condition,
                        &block_filters,
                        issues,
                    );
                }
            }
        }

        if let Some(groups) = block
            .get("card")
            .and_then(Value::as_object)
            .and_then(|card| card.get("groups"))
            .and_then(Value::as_array)
        {
            for (group_index, group) in groups.iter().enumerate() {
                let Some(fields) = group.get("fields").and_then(Value::as_array) else {
                    continue;
                };
                for (field_index, field) in fields.iter().enumerate() {
                    if let Some(condition) = field
                        .get("editor")
                        .and_then(Value::as_object)
                        .and_then(|editor| editor.get("lookup"))
                        .and_then(Value::as_object)
                        .and_then(|lookup| lookup.get("condition"))
                    {
                        collect_condition_filter_ref_issues(
                            &format!(
                                "$.blocks[{block_index}].card.groups[{group_index}].fields[{field_index}].editor.lookup.condition"
                            ),
                            condition,
                            &block_filters,
                            issues,
                        );
                    }
                }
            }
        }
    }
}

fn collect_condition_filter_metadata(filters: Option<&[Value]>) -> HashMap<String, Option<String>> {
    filters
        .into_iter()
        .flatten()
        .filter_map(|filter| {
            let id = filter.get("id").and_then(Value::as_str)?.trim();
            if id.is_empty() {
                return None;
            }
            Some((
                id.to_string(),
                filter
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ))
        })
        .collect()
}

fn collect_condition_filter_ref_issues(
    path: &str,
    condition: &Value,
    filter_metadata: &HashMap<String, Option<String>>,
    issues: &mut Vec<AuthoringIssue>,
) {
    let Some(arguments) = condition.get("arguments").and_then(Value::as_array) else {
        return;
    };

    for (index, argument) in arguments.iter().enumerate() {
        let argument_path = format!("{path}.arguments[{index}]");
        if let Some(reference) = parse_condition_filter_ref(argument, &argument_path, issues) {
            match filter_metadata.get(&reference.filter_id) {
                Some(filter_type) => {
                    collect_condition_filter_ref_path_issues(
                        &argument_path,
                        &reference,
                        filter_type.as_deref(),
                        issues,
                    );
                }
                None => issues.push(error(
                    format!("{argument_path}.filter"),
                    "UNKNOWN_CONDITION_FILTER_REF",
                    format!(
                        "Report source condition references unknown filter '{}'.",
                        reference.filter_id
                    ),
                )),
            }
        }
        if let Some(condition) = condition_subquery_condition(argument) {
            collect_condition_filter_ref_issues(
                &format!("{argument_path}.subquery.condition"),
                condition,
                filter_metadata,
                issues,
            );
            continue;
        }
        if is_condition_object(argument) {
            collect_condition_filter_ref_issues(&argument_path, argument, filter_metadata, issues);
        }
    }
}

struct ConditionFilterRef {
    filter_id: String,
    path: String,
}

fn parse_condition_filter_ref(
    argument: &Value,
    path: &str,
    issues: &mut Vec<AuthoringIssue>,
) -> Option<ConditionFilterRef> {
    let object = argument.as_object()?;
    if !object.contains_key("filter") {
        return None;
    }
    let Some(filter_id) = object.get("filter").and_then(Value::as_str).map(str::trim) else {
        issues.push(error(
            format!("{path}.filter"),
            "INVALID_CONDITION_FILTER_REF",
            "Report source condition filter refs must include a string filter.",
        ));
        return None;
    };
    if filter_id.is_empty() {
        issues.push(error(
            format!("{path}.filter"),
            "INVALID_CONDITION_FILTER_REF",
            "Report source condition filter refs must include a non-empty filter.",
        ));
        return None;
    }
    let Some(path_value) = object.get("path").and_then(Value::as_str).map(str::trim) else {
        issues.push(error(
            format!("{path}.path"),
            "INVALID_CONDITION_FILTER_REF_PATH",
            "Report source condition filter refs must include path.",
        ));
        return None;
    };
    Some(ConditionFilterRef {
        filter_id: filter_id.to_string(),
        path: path_value.to_string(),
    })
}

fn collect_condition_filter_ref_path_issues(
    path: &str,
    reference: &ConditionFilterRef,
    filter_type: Option<&str>,
    issues: &mut Vec<AuthoringIssue>,
) {
    if !is_known_condition_filter_ref_path(&reference.path) {
        issues.push(error(
            format!("{path}.path"),
            "INVALID_CONDITION_FILTER_REF_PATH",
            format!(
                "Report source condition filter ref path '{}' is not supported. Use one of: value, values, from, to, min, max.",
                reference.path
            ),
        ));
        return;
    }

    let Some(filter_type) = filter_type else {
        return;
    };
    let allowed_paths = condition_filter_ref_paths_for_type(filter_type);
    if allowed_paths.contains(&reference.path.as_str()) {
        return;
    }
    issues.push(error(
        format!("{path}.path"),
        "INVALID_CONDITION_FILTER_REF_PATH",
        format!(
            "Filter '{}' has type '{}' and supports condition paths: {}.",
            reference.filter_id,
            filter_type,
            allowed_paths.join(", ")
        ),
    ));
}

fn condition_filter_ref_paths_for_type(filter_type: &str) -> &'static [&'static str] {
    match filter_type {
        "multi_select" => &["values"],
        "time_range" => &["from", "to"],
        "number_range" => &["min", "max"],
        "select" | "radio" | "checkbox" | "text" | "search" => &["value"],
        _ => &["value", "values", "from", "to", "min", "max"],
    }
}

fn is_known_condition_filter_ref_path(path: &str) -> bool {
    matches!(path, "value" | "values" | "from" | "to" | "min" | "max")
}

fn collect_condition_issues(path: &str, condition: &Value, issues: &mut Vec<AuthoringIssue>) {
    collect_condition_issues_at(path, condition, issues, false);
}

fn collect_condition_issues_at(
    path: &str,
    condition: &Value,
    issues: &mut Vec<AuthoringIssue>,
    inside_subquery: bool,
) {
    collect_unknown_keys(path, condition, &["op", "arguments"], issues);
    if condition.get("op").and_then(Value::as_str).is_none() {
        issues.push(error(
            format!("{path}.op"),
            "MISSING_CONDITION_OP",
            "Report source conditions must include op.",
        ));
    }

    let Some(arguments) = condition.get("arguments") else {
        return;
    };
    let Some(arguments) = arguments.as_array() else {
        issues.push(error(
            format!("{path}.arguments"),
            "INVALID_CONDITION_ARGUMENTS",
            "Report source condition arguments must be an array.",
        ));
        return;
    };
    let op = condition
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_uppercase();
    for (index, argument) in arguments.iter().enumerate() {
        let argument_path = format!("{path}.arguments[{index}]");
        if is_mapping_value_object(argument) {
            issues.push(error(
                argument_path,
                "UNSUPPORTED_CONDITION_MAPPING_VALUE",
                "Report source conditions do not evaluate workflow MappingValue reference/template objects. Use filterMappings, appliesTo, {\"filter\":\"...\",\"path\":\"...\"}, or literal Object Model condition arguments.",
            ));
        } else if collect_condition_subquery_issues(
            &argument_path,
            argument,
            issues,
            inside_subquery,
            &op,
            index,
        ) {
            // handled by collect_condition_subquery_issues
        } else if is_condition_object(argument) {
            collect_condition_issues_at(&argument_path, argument, issues, inside_subquery);
        }
    }
}

fn collect_condition_subquery_issues(
    path: &str,
    argument: &Value,
    issues: &mut Vec<AuthoringIssue>,
    inside_subquery: bool,
    op: &str,
    argument_index: usize,
) -> bool {
    let Some(object) = argument.as_object() else {
        return false;
    };
    let Some(subquery_value) = object.get("subquery") else {
        return false;
    };
    if inside_subquery {
        issues.push(error(
            format!("{path}.subquery"),
            "NESTED_CONDITION_SUBQUERY",
            "Report source condition subqueries cannot contain nested subqueries.",
        ));
    }
    if !matches!(op, "IN" | "NOT_IN") || argument_index != 1 {
        issues.push(error(
            format!("{path}.subquery"),
            "INVALID_CONDITION_SUBQUERY",
            "Report source condition subqueries are only supported as the second argument of IN or NOT_IN.",
        ));
    }
    if object.len() != 1 {
        issues.push(error(
            path.to_string(),
            "INVALID_CONDITION_SUBQUERY",
            "Report source condition subquery operands must contain only the subquery key.",
        ));
    }
    let Some(subquery) = subquery_value.as_object() else {
        issues.push(error(
            format!("{path}.subquery"),
            "INVALID_CONDITION_SUBQUERY",
            "Report source condition subquery must be an object.",
        ));
        return true;
    };
    collect_unknown_keys(
        &format!("{path}.subquery"),
        subquery_value,
        &["schema", "select", "condition", "connectionId"],
        issues,
    );
    if subquery
        .get("schema")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        issues.push(error(
            format!("{path}.subquery.schema"),
            "INVALID_CONDITION_SUBQUERY",
            "Report source condition subqueries must include schema.",
        ));
    }
    if subquery
        .get("select")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        issues.push(error(
            format!("{path}.subquery.select"),
            "INVALID_CONDITION_SUBQUERY",
            "Report source condition subqueries must include select.",
        ));
    }
    if let Some(condition) = subquery.get("condition") {
        if is_condition_object(condition) {
            collect_condition_issues_at(
                &format!("{path}.subquery.condition"),
                condition,
                issues,
                true,
            );
        } else {
            issues.push(error(
                format!("{path}.subquery.condition"),
                "INVALID_CONDITION_SUBQUERY",
                "Report source condition subquery.condition must be a condition object.",
            ));
        }
    }
    true
}

fn condition_subquery_condition(argument: &Value) -> Option<&Value> {
    argument
        .as_object()?
        .get("subquery")?
        .as_object()?
        .get("condition")
}

fn is_condition_object(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key("op") || object.contains_key("arguments"))
}

fn is_mapping_value_object(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|object| object.get("valueType"))
        .and_then(Value::as_str)
        .is_some_and(|value_type| {
            matches!(
                value_type,
                "reference" | "immediate" | "template" | "composite"
            )
        })
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
            let suggestion = similar_key_hint(key, allowed)
                .map(|known| format!(" Did you mean '{known}'?"))
                .unwrap_or_default();
            issues.push(error(
                &key_path,
                "UNKNOWN_REPORT_FIELD",
                format!("Unknown report field '{key}'.{suggestion} Use get_report_authoring_schema for the canonical shape."),
            ));
        }
    }
}

fn similar_key_hint<'a>(key: &str, allowed: &'a [&str]) -> Option<&'a str> {
    let key_lower = key.to_ascii_lowercase();
    allowed
        .iter()
        .copied()
        .filter_map(|allowed_key| {
            let allowed_lower = allowed_key.to_ascii_lowercase();
            let distance = levenshtein(&key_lower, &allowed_lower);
            let threshold = if allowed_lower.len() <= 4 { 1 } else { 3 };
            (distance <= threshold).then_some((distance, allowed_key))
        })
        .min_by_key(|(distance, allowed_key)| (*distance, allowed_key.len()))
        .map(|(_, allowed_key)| allowed_key)
}

fn levenshtein(left: &str, right: &str) -> usize {
    let mut costs = (0..=right.chars().count()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut previous = costs[0];
        costs[0] = left_index + 1;
        for (right_index, right_char) in right.chars().enumerate() {
            let current = costs[right_index + 1];
            costs[right_index + 1] = if left_char == right_char {
                previous
            } else {
                1 + previous.min(current).min(costs[right_index])
            };
            previous = current;
        }
    }
    costs[right.chars().count()]
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

    #[test]
    fn report_authoring_accepts_dataset_backed_block_shape() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.vendor_summary }}",
            "datasets": [{
                "id": "stock_snapshots",
                "label": "Stock snapshots",
                "source": {"schema": "StockSnapshot", "connectionId": null},
                "timeDimension": "snapshot_date",
                "dimensions": [{"field": "vendor", "label": "Vendor", "type": "string"}],
                "measures": [
                    {"id": "snapshot_count", "label": "Snapshots", "op": "count", "format": "number"},
                    {"id": "qty_total", "label": "Total quantity", "op": "sum", "field": "qty", "format": "number"}
                ]
            }],
            "blocks": [{
                "id": "vendor_summary",
                "type": "table",
                "dataset": {
                    "id": "stock_snapshots",
                    "dimensions": ["vendor"],
                    "measures": ["snapshot_count", "qty_total"],
                    "orderBy": [{"field": "qty_total", "direction": "desc"}]
                },
                "table": {
                    "columns": [
                        {"field": "vendor", "label": "Vendor"},
                        {"field": "qty_total", "label": "Total quantity", "format": "number"}
                    ]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);

        assert!(authoring_errors(&issues).next().is_none());
    }

    #[test]
    fn report_authoring_warns_for_markdown_table_block_layout() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "| | |\n|---|---|\n| {{ block.a }} | {{ block.b }} |",
            "blocks": [
                {
                    "id": "a",
                    "type": "metric",
                    "source": {
                        "schema": "StockSnapshot",
                        "mode": "aggregate",
                        "aggregates": [{"alias": "n", "op": "count"}]
                    },
                    "metric": {"valueField": "n"}
                },
                {
                    "id": "b",
                    "type": "metric",
                    "source": {
                        "schema": "StockSnapshot",
                        "mode": "aggregate",
                        "aggregates": [{"alias": "n", "op": "count"}]
                    },
                    "metric": {"valueField": "n"}
                }
            ]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let response = authoring_validation_response(issues);

        assert_eq!(response["valid"], json!(true));
        assert_eq!(
            response["warnings"][0]["code"],
            json!("MARKDOWN_TABLE_BLOCK_LAYOUT")
        );
    }

    #[test]
    fn report_authoring_accepts_structured_layout() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "# Report",
            "layout": [
                {"id": "intro", "type": "markdown", "content": "# Report"},
                {"id": "summary", "type": "metric_row", "blocks": ["total"]}
            ],
            "blocks": [{
                "id": "total",
                "type": "metric",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "aggregate",
                    "aggregates": [{"alias": "n", "op": "count"}]
                },
                "metric": {"valueField": "n"}
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);

        assert!(authoring_errors(&issues).next().is_none());
    }

    #[test]
    fn report_authoring_rejects_unknown_layout_block_reference() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "# Report",
            "layout": [{"id": "missing", "type": "block", "blockId": "missing_block"}],
            "blocks": []
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"UNKNOWN_LAYOUT_BLOCK_REFERENCE"));
    }

    #[test]
    fn report_authoring_rejects_unknown_keys_with_similar_key_hint() {
        let definition = json!({
            "definitionVerison": 1,
            "markdown": "# Report",
            "blocks": [{
                "id": "products",
                "type": "table",
                "titel": "Products",
                "source": {"schema": "TDProduct", "mode": "filter"},
                "table": {"columns": [{"field": "sku"}]}
            }]
        });

        let response =
            authoring_validation_response(collect_report_definition_authoring_issues(&definition));
        let errors = response["errors"].as_array().unwrap();

        assert!(errors.iter().any(|error| {
            error["path"] == json!("$.definitionVerison")
                && error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("Did you mean 'definitionVersion'?"))
        }));
        assert!(errors.iter().any(|error| {
            error["path"] == json!("$.blocks[0].titel")
                && error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("Did you mean 'title'?"))
        }));
    }

    #[test]
    fn report_authoring_rejects_unknown_filter_option_keys() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "# Report",
            "filters": [{
                "id": "vendor",
                "label": "Vendor",
                "type": "select",
                "options": {
                    "source": "object_model",
                    "schema": "StockSnapshot",
                    "filed": "vendor"
                },
                "appliesTo": [{"field": "vendor"}]
            }],
            "blocks": []
        });

        let response =
            authoring_validation_response(collect_report_definition_authoring_issues(&definition));
        let errors = response["errors"].as_array().unwrap();

        assert!(errors.iter().any(|error| {
            error["path"] == json!("$.filters[0].options.filed")
                && error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("Did you mean 'field'?"))
        }));
    }

    #[test]
    fn report_authoring_rejects_mapping_value_in_source_condition() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.products }}",
            "blocks": [{
                "id": "products",
                "type": "table",
                "source": {
                    "schema": "TDProduct",
                    "mode": "filter",
                    "condition": {
                        "op": "EQ",
                        "arguments": [
                            "sku",
                            {"valueType": "template", "value": "{{ filters.sku }}"}
                        ]
                    }
                },
                "table": {"columns": [{"field": "sku"}]}
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"UNSUPPORTED_CONDITION_MAPPING_VALUE"));
    }

    #[test]
    fn report_authoring_accepts_condition_filter_ref() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.products }}",
            "filters": [{
                "id": "vendor_filter",
                "label": "Vendor",
                "type": "select"
            }],
            "blocks": [{
                "id": "products",
                "type": "table",
                "source": {
                    "schema": "TDProduct",
                    "mode": "filter",
                    "condition": {
                        "op": "EQ",
                        "arguments": [
                            "vendor",
                            {"filter": "vendor_filter", "path": "value"}
                        ]
                    }
                },
                "table": {"columns": [{"field": "sku"}]}
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(!codes.contains(&"UNKNOWN_CONDITION_FILTER_REF"));
        assert!(!codes.contains(&"INVALID_CONDITION_FILTER_REF_PATH"));
    }

    #[test]
    fn report_authoring_accepts_condition_filter_ref_inside_subquery() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.stock }}",
            "filters": [{
                "id": "category",
                "label": "Category",
                "type": "multi_select"
            }],
            "blocks": [{
                "id": "stock",
                "type": "table",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "filter",
                    "condition": {
                        "op": "IN",
                        "arguments": [
                            "sku",
                            {
                                "subquery": {
                                    "schema": "TDProduct",
                                    "select": "sku",
                                    "condition": {
                                        "op": "IN",
                                        "arguments": [
                                            "category_leaf_id",
                                            {"filter": "category", "path": "values"}
                                        ]
                                    }
                                }
                            }
                        ]
                    }
                },
                "table": {"columns": [{"field": "sku"}]}
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(!codes.contains(&"UNKNOWN_CONDITION_FILTER_REF"));
        assert!(!codes.contains(&"INVALID_CONDITION_FILTER_REF_PATH"));
        assert!(!codes.contains(&"INVALID_CONDITION_SUBQUERY"));
    }

    #[test]
    fn report_authoring_rejects_nested_condition_subquery() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.stock }}",
            "blocks": [{
                "id": "stock",
                "type": "table",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "filter",
                    "condition": {
                        "op": "IN",
                        "arguments": [
                            "sku",
                            {
                                "subquery": {
                                    "schema": "TDProduct",
                                    "select": "sku",
                                    "condition": {
                                        "op": "IN",
                                        "arguments": [
                                            "category_leaf_id",
                                            {"subquery": {"schema": "Category", "select": "id"}}
                                        ]
                                    }
                                }
                            }
                        ]
                    }
                },
                "table": {"columns": [{"field": "sku"}]}
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"NESTED_CONDITION_SUBQUERY"));
    }

    #[test]
    fn report_authoring_rejects_unknown_condition_filter_ref() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.products }}",
            "filters": [{
                "id": "vendor_filter",
                "label": "Vendor",
                "type": "select"
            }],
            "blocks": [{
                "id": "products",
                "type": "table",
                "source": {
                    "schema": "TDProduct",
                    "mode": "filter",
                    "condition": {
                        "op": "EQ",
                        "arguments": [
                            "vendor",
                            {"filter": "missing_filter", "path": "value"}
                        ]
                    }
                },
                "table": {"columns": [{"field": "sku"}]}
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"UNKNOWN_CONDITION_FILTER_REF"));
    }

    #[test]
    fn report_authoring_accepts_block_source_join_shape() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.stock }}",
            "blocks": [{
                "id": "stock",
                "type": "table",
                "source": {
                    "schema": "StockSnapshot",
                    "mode": "filter",
                    "join": [{
                        "schema": "TDProduct",
                        "alias": "product",
                        "parentField": "sku",
                        "field": "sku",
                        "kind": "left"
                    }],
                    "condition": {
                        "op": "EQ",
                        "arguments": ["product.category_leaf_id", "leaf-1"]
                    }
                },
                "table": {
                    "columns": [
                        {"field": "sku"},
                        {"field": "product.part_number"}
                    ]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(!codes.contains(&"MISSING_JOIN_SCHEMA"));
        assert!(!codes.contains(&"UNKNOWN_KEY"));
    }

    #[test]
    fn report_authoring_accepts_value_column_source_select() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.stock }}",
            "blocks": [{
                "id": "stock",
                "type": "table",
                "source": {"schema": "StockSnapshot", "mode": "filter"},
                "table": {
                    "columns": [{
                        "field": "part_number",
                        "type": "value",
                        "source": {
                            "schema": "TDProduct",
                            "mode": "filter",
                            "select": "part_number",
                            "join": [{"parentField": "sku", "field": "sku", "kind": "left"}]
                        }
                    }]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(!codes.contains(&"MISSING_TABLE_VALUE_SELECT"));
        assert!(!codes.contains(&"UNKNOWN_KEY"));
    }

    #[test]
    fn report_authoring_accepts_lookup_editor_column_shape() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.products }}",
            "filters": [{
                "id": "vendor",
                "label": "Vendor",
                "type": "select"
            }],
            "blocks": [{
                "id": "products",
                "type": "table",
                "source": {
                    "schema": "Product",
                    "mode": "filter",
                    "join": [{
                        "schema": "Category",
                        "alias": "category",
                        "parentField": "category_id",
                        "field": "id",
                        "kind": "left"
                    }]
                },
                "table": {
                    "columns": [{
                        "field": "category_id",
                        "displayField": "category.name",
                        "editable": true,
                        "editor": {
                            "kind": "lookup",
                            "lookup": {
                                "schema": "Category",
                                "valueField": "id",
                                "labelField": "name",
                                "searchFields": ["name"],
                                "condition": {
                                    "op": "EQ",
                                    "arguments": ["vendor", {"filter": "vendor", "path": "value"}]
                                }
                            }
                        }
                    }]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(!codes.contains(&"UNKNOWN_KEY"));
        assert!(!codes.contains(&"MISSING_LOOKUP_CONFIG"));
        assert!(!codes.contains(&"UNKNOWN_CONDITION_FILTER_REF"));
        assert!(authoring_errors(&issues).next().is_none());
    }

    #[test]
    fn report_authoring_rejects_lookup_editor_without_lookup_config() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.products }}",
            "blocks": [{
                "id": "products",
                "type": "table",
                "source": {"schema": "Product", "mode": "filter"},
                "table": {
                    "columns": [{
                        "field": "category_id",
                        "editable": true,
                        "editor": {"kind": "lookup"}
                    }]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"MISSING_LOOKUP_CONFIG"));
    }

    #[test]
    fn report_authoring_accepts_workflow_runtime_instances_table() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.workflow_runs }}",
            "blocks": [{
                "id": "workflow_runs",
                "type": "table",
                "source": {
                    "kind": "workflow_runtime",
                    "entity": "instances",
                    "workflowId": "inventory_sync",
                    "mode": "filter",
                    "orderBy": [{"field": "createdAt", "direction": "desc"}]
                },
                "table": {
                    "columns": [
                        {"field": "instanceId"},
                        {"field": "status"},
                        {"field": "hasActions"}
                    ]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(!codes.contains(&"MISSING_SOURCE_SCHEMA"));
        assert!(!codes.contains(&"UNKNOWN_REPORT_FIELD"));
        assert!(!codes.contains(&"INVALID_SOURCE_KIND"));
    }

    #[test]
    fn report_authoring_accepts_workflow_runtime_actions_block() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.workflow_actions }}",
            "blocks": [{
                "id": "workflow_actions",
                "type": "actions",
                "source": {
                    "kind": "workflow_runtime",
                    "entity": "actions",
                    "workflowId": "inventory_sync",
                    "instanceId": "00000000-0000-0000-0000-000000000000"
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(!codes.contains(&"MISSING_SOURCE_SCHEMA"));
        assert!(!codes.contains(&"INVALID_ACTIONS_SOURCE_KIND"));
        assert!(!codes.contains(&"INVALID_ACTIONS_SOURCE_ENTITY"));
    }

    #[test]
    fn report_authoring_accepts_workflow_runtime_actions_correlation_binding() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.case_action }}",
            "filters": [{
                "id": "case_id",
                "label": "Case",
                "type": "text",
                "required": true
            }],
            "blocks": [{
                "id": "case_action",
                "type": "actions",
                "source": {
                    "kind": "workflow_runtime",
                    "entity": "actions",
                    "workflowId": "loan_review",
                    "condition": {"op": "AND", "arguments": [
                        {"op": "EQ", "arguments": ["actionKey", "case_review_decision"]},
                        {"op": "EQ", "arguments": ["correlation.case_id", {"filter": "case_id", "path": "value"}]}
                    ]}
                },
                "actions": {
                    "submit": {
                        "label": "Submit decision",
                        "implicitPayload": {"reviewer_id": "{{viewer.user_id}}"}
                    }
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(!codes.contains(&"UNKNOWN_REPORT_BLOCK_FIELD"));
        assert!(!codes.contains(&"UNKNOWN_REPORT_FIELD"));
        assert!(!codes.contains(&"INVALID_ACTIONS_SOURCE_KIND"));
        assert!(!codes.contains(&"INVALID_ACTIONS_SOURCE_ENTITY"));
        assert!(!codes.contains(&"UNKNOWN_CONDITION_FILTER_REF"));
    }

    #[test]
    fn report_authoring_rejects_actions_block_without_workflow_runtime_source() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.workflow_actions }}",
            "blocks": [{
                "id": "workflow_actions",
                "type": "actions",
                "source": {"schema": "StockSnapshot"}
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"INVALID_ACTIONS_SOURCE_KIND"));
    }

    #[test]
    fn report_authoring_schema_includes_workflow_runtime_contract() {
        let schema = report_authoring_schema();

        assert_eq!(schema["definitionVersion"], json!(1));
        assert_eq!(
            schema["workflowRuntimeGuidance"]["actionsBlockExample"]["type"],
            json!("actions")
        );
        assert_eq!(
            schema["workflowRuntimeGuidance"]["correlatedActionsBlockExample"]["source"]["condition"]
                ["arguments"][1]["arguments"][0],
            json!("correlation.case_id")
        );
        assert_eq!(
            schema["blockShape"]["actions"]["required"]["source.kind"],
            json!("workflow_runtime")
        );
    }

    #[test]
    fn report_authoring_schema_documents_strict_when_referenced() {
        let schema = report_authoring_schema();
        let docs = schema["filterShape"]["strictWhenReferenced"]
            .as_str()
            .expect("filterShape.strictWhenReferenced is documented");
        assert!(docs.contains("filter not set"));
        assert_eq!(
            schema["biGuidance"]["masterDetailNavigationExample"]["filters"][0]["strictWhenReferenced"],
            json!(true)
        );
    }

    #[test]
    fn report_authoring_schema_documents_card_block() {
        let schema = report_authoring_schema();

        assert_eq!(schema["blockShape"]["card"]["type"], json!("card"));
        assert_eq!(
            schema["blockShape"]["card"]["required"]["source.mode"],
            json!("filter (cards render the first matching row)")
        );
        let kinds = schema["blockShape"]["card"]["fieldShape"]["kind"]
            .as_str()
            .expect("card.fieldShape.kind is a string");
        assert!(kinds.contains("subcard") && kinds.contains("subtable"));
        assert_eq!(schema["examples"]["card"]["type"], json!("card"));
    }

    #[test]
    fn report_authoring_rejects_value_column_source_without_select() {
        let definition = json!({
            "definitionVersion": 1,
            "markdown": "{{ block.stock }}",
            "blocks": [{
                "id": "stock",
                "type": "table",
                "source": {"schema": "StockSnapshot", "mode": "filter"},
                "table": {
                    "columns": [{
                        "field": "part_number",
                        "type": "value",
                        "source": {
                            "schema": "TDProduct",
                            "mode": "filter",
                            "join": [{"parentField": "sku", "field": "sku"}]
                        }
                    }]
                }
            }]
        });

        let issues = collect_report_definition_authoring_issues(&definition);
        let codes = issue_codes(&issues);

        assert!(codes.contains(&"MISSING_TABLE_VALUE_SELECT"));
    }

    fn issue_codes(issues: &[AuthoringIssue]) -> Vec<&'static str> {
        issues.iter().map(|issue| issue.code).collect()
    }
}
