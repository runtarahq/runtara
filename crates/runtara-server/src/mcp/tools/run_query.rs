// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs queried as records (`docs/workflow-records.md`), as MCP tools.
//! They ask the API's own routes, so they are held to what those allow.

use rmcp::model::{CallToolResult, ContentBlock};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::super::server::SmoMcpServer;
use super::internal_api::{api_get, api_post};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRunsParams {
    /// The workflow, by id or slug (describe_run_state lists them).
    pub workflow: String,
    /// One row per run label: its latest run. Use it for "the current
    /// state of each <thing>"; leave it off to read every run (history).
    pub latest_per_label: Option<bool>,
    /// Which rows to keep: {"op": OP, "arguments": [...]}. AND/OR take
    /// conditions, NOT one condition. EQ, NE, GT, GTE, LT, LTE, CONTAINS,
    /// STARTS_WITH, ENDS_WITH take [path, value]; IN, NOT_IN [path,
    /// [values]]; IS_DEFINED, IS_EMPTY, IS_NOT_EMPTY [path].
    #[schemars(schema_with = "crate::mcp::tools::internal_api::optional_json_object_schema")]
    pub condition: Option<Value>,
    /// A path to an array in state (e.g. "state.items"): each element
    /// becomes a row, read as "item" and "item.<field>".
    pub unnest: Option<String>,
    /// Paths each row returns; by default runId, label, status, version,
    /// createdAt, updatedAt and state.
    pub select: Option<Vec<String>>,
    /// Return one row per distinct value of these paths (with aggregates).
    pub group_by: Option<Vec<String>>,
    /// Computed per group (or over all rows kept): [{"fn": "count" |
    /// "count_distinct" | "min" | "max" | "sum" | "avg", "path": PATH
    /// (none for a count of rows), "as": NAME}].
    #[schemars(schema_with = "crate::mcp::tools::internal_api::optional_json_array_schema")]
    pub aggregates: Option<Value>,
    /// [{"path": PATH or aggregate name, "desc": true}].
    #[schemars(schema_with = "crate::mcp::tools::internal_api::optional_json_array_schema")]
    pub order_by: Option<Value>,
    /// Rows to return, 1 to 1000; 100 by default.
    pub limit: Option<u32>,
    /// Rows to skip, for the next page.
    pub offset: Option<u32>,
    /// Also return the total number of rows (or groups) kept.
    pub count: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeRunStateParams {}

pub async fn query_runs(
    server: &SmoMcpServer,
    params: QueryRunsParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let mut query = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            query.insert(key.to_owned(), value);
        }
    };
    put("latestPerLabel", params.latest_per_label.map(Value::Bool));
    put("condition", params.condition);
    put("unnest", params.unnest.map(Value::String));
    put("select", params.select.map(|v| json!(v)));
    put("groupBy", params.group_by.map(|v| json!(v)));
    put("aggregates", params.aggregates);
    put("orderBy", params.order_by);
    put("limit", params.limit.map(|v| json!(v)));
    put("offset", params.offset.map(|v| json!(v)));
    put("count", params.count.map(Value::Bool));
    let result = api_post(
        server,
        "/api/runtime/runs/query",
        Some(json!({ "workflow": params.workflow, "query": query })),
    )
    .await?;
    // Compact: rows can be many.
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string(result.get("data").unwrap_or(&result)).unwrap_or_default(),
    )]))
}

pub async fn describe_run_state(
    server: &SmoMcpServer,
    _params: DescribeRunStateParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let result = api_get(server, "/api/runtime/runs/state").await?;
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string(result.get("data").unwrap_or(&result)).unwrap_or_default(),
    )]))
}
