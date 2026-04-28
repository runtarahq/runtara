use rmcp::model::{CallToolResult, Content};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::server::SmoMcpServer;
use super::internal_api::{
    api_delete, api_delete_with_body, api_get, api_patch, api_post, api_put, validate_path_param,
};

fn json_result(value: Value) -> Result<CallToolResult, rmcp::ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}

// ===== Parameter Structs =====

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
    let result = api_post(
        server,
        "/api/runtime/reports/validate",
        Some(json!({ "definition": params.definition })),
    )
    .await?;
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
