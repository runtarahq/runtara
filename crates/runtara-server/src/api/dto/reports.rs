//! Report DTOs.
//!
//! Reports are described by markdown plus typed data blocks. The browser sends
//! viewer state to the backend; the backend validates and executes block data
//! queries through Object Model services.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use utoipa::ToSchema;

use crate::api::dto::object_model::Condition;

fn default_definition_version() -> i32 {
    1
}

fn default_report_status() -> ReportStatus {
    ReportStatus::Published
}

fn default_source_mode() -> ReportSourceMode {
    ReportSourceMode::Filter
}

fn default_block_status() -> ReportBlockStatus {
    ReportBlockStatus::Ready
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Draft,
    Published,
    Archived,
}

impl ReportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Archived => "archived",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "draft" => Self::Draft,
            "archived" => Self::Archived,
            _ => Self::Published,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportDefinition {
    #[serde(default = "default_definition_version", rename = "definitionVersion")]
    pub definition_version: i32,
    #[serde(default)]
    pub markdown: String,
    #[serde(default)]
    pub filters: Vec<ReportFilterDefinition>,
    #[serde(default)]
    pub blocks: Vec<ReportBlockDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportFilterDefinition {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub filter_type: ReportFilterType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
    #[serde(default, rename = "appliesTo")]
    pub applies_to: Vec<ReportFilterTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportFilterType {
    Select,
    MultiSelect,
    Radio,
    Checkbox,
    TimeRange,
    NumberRange,
    Text,
    Search,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportFilterTarget {
    #[serde(default, rename = "filterId", skip_serializing_if = "Option::is_none")]
    pub filter_id: Option<String>,
    #[serde(default, rename = "blockId", skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    pub field: String,
    #[serde(default = "default_filter_op")]
    pub op: String,
}

fn default_filter_op() -> String {
    "eq".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportBlockDefinition {
    pub id: String,
    #[serde(rename = "type")]
    pub block_type: ReportBlockType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub lazy: bool,
    pub source: ReportSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<ReportTableConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<ReportChartConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<ReportMetricConfig>,
    #[serde(default)]
    pub filters: Vec<ReportFilterDefinition>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportBlockType {
    Table,
    Chart,
    Metric,
    Markdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportSource {
    pub schema: String,
    #[serde(
        default,
        rename = "connectionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub connection_id: Option<String>,
    #[serde(default = "default_source_mode")]
    pub mode: ReportSourceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    #[serde(default, rename = "filterMappings")]
    pub filter_mappings: Vec<ReportFilterTarget>,
    #[serde(default, rename = "groupBy")]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub aggregates: Vec<ReportAggregateSpec>,
    #[serde(default, rename = "orderBy")]
    pub order_by: Vec<ReportOrderBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportSourceMode {
    Filter,
    Aggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportAggregateSpec {
    pub alias: String,
    #[serde(rename = "op")]
    pub op: ReportAggregateFn,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default)]
    pub distinct: bool,
    #[serde(default, rename = "orderBy")]
    pub order_by: Vec<ReportOrderBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<Value>,
    /// Fraction in `[0.0, 1.0]` for `percentile_cont` / `percentile_disc`
    /// aggregates. Required for those ops, rejected otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentile: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportAggregateFn {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    FirstValue,
    LastValue,
    PercentileCont,
    PercentileDisc,
    StddevSamp,
    VarSamp,
    Expr,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportOrderBy {
    pub field: String,
    #[serde(default = "default_sort_direction")]
    pub direction: String,
}

fn default_sort_direction() -> String {
    "asc".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportTableConfig {
    #[serde(default)]
    pub columns: Vec<ReportTableColumn>,
    #[serde(default, rename = "defaultSort")]
    pub default_sort: Vec<ReportOrderBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<ReportPaginationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportTableColumn {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportPaginationConfig {
    #[serde(default = "default_page_size", rename = "defaultPageSize")]
    pub default_page_size: i64,
    #[serde(default, rename = "allowedPageSizes")]
    pub allowed_page_sizes: Vec<i64>,
}

fn default_page_size() -> i64 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportChartConfig {
    pub kind: ReportChartKind,
    pub x: String,
    #[serde(default)]
    pub series: Vec<ReportChartSeries>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportChartKind {
    Line,
    Bar,
    Area,
    Pie,
    Donut,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportChartSeries {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportMetricConfig {
    #[serde(rename = "valueField")]
    pub value_field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportSummary {
    pub id: String,
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub status: ReportStatus,
    #[serde(rename = "definitionVersion")]
    pub definition_version: i32,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportDto {
    pub id: String,
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub status: ReportStatus,
    #[serde(rename = "definitionVersion")]
    pub definition_version: i32,
    pub definition: ReportDefinition,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

impl From<&ReportDto> for ReportSummary {
    fn from(report: &ReportDto) -> Self {
        Self {
            id: report.id.clone(),
            slug: report.slug.clone(),
            name: report.name.clone(),
            description: report.description.clone(),
            tags: report.tags.clone(),
            status: report.status,
            definition_version: report.definition_version,
            created_at: report.created_at,
            updated_at: report.updated_at,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ListReportsResponse {
    pub success: bool,
    pub reports: Vec<ReportSummary>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct GetReportResponse {
    pub success: bool,
    pub report: ReportDto,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateReportRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub definition: ReportDefinition,
    #[serde(default = "default_report_status")]
    pub status: ReportStatus,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateReportRequest {
    pub name: String,
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub definition: ReportDefinition,
    #[serde(default = "default_report_status")]
    pub status: ReportStatus,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ValidateReportRequest {
    pub definition: ReportDefinition,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ValidateReportResponse {
    pub valid: bool,
    #[serde(default)]
    pub errors: Vec<ReportValidationIssue>,
    #[serde(default)]
    pub warnings: Vec<ReportValidationIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportValidationIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportRenderRequest {
    #[serde(default)]
    pub filters: HashMap<String, Value>,
    #[serde(default)]
    pub blocks: Option<Vec<ReportBlockDataRequest>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportBlockOnlyDataRequest {
    #[serde(default)]
    pub filters: HashMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<ReportPageRequest>,
    #[serde(default)]
    pub sort: Vec<ReportOrderBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<ReportTableSearchRequest>,
    #[serde(default, rename = "blockFilters")]
    pub block_filters: HashMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportBlockDataRequest {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<ReportPageRequest>,
    #[serde(default)]
    pub sort: Vec<ReportOrderBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<ReportTableSearchRequest>,
    #[serde(default, rename = "blockFilters")]
    pub block_filters: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportTableSearchRequest {
    pub query: String,
    #[serde(default)]
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportPageRequest {
    #[serde(default)]
    pub offset: i64,
    #[serde(default = "default_page_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportRenderResponse {
    pub success: bool,
    pub report: ReportRenderMetadata,
    #[serde(rename = "resolvedFilters")]
    pub resolved_filters: HashMap<String, Value>,
    pub blocks: HashMap<String, ReportBlockRenderResult>,
    #[serde(default)]
    pub errors: Vec<ReportBlockError>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportRenderMetadata {
    pub id: String,
    #[serde(rename = "definitionVersion")]
    pub definition_version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportBlockRenderResult {
    #[serde(rename = "type")]
    pub block_type: ReportBlockType,
    #[serde(default = "default_block_status")]
    pub status: ReportBlockStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ReportBlockError>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportBlockStatus {
    Ready,
    Loading,
    Empty,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportBlockError {
    pub code: String,
    pub message: String,
    #[serde(default, rename = "blockId", skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DeleteReportResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct ReportBlockPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(
        default,
        rename = "beforeBlockId",
        skip_serializing_if = "Option::is_none"
    )]
    pub before_block_id: Option<String>,
    #[serde(
        default,
        rename = "afterBlockId",
        skip_serializing_if = "Option::is_none"
    )]
    pub after_block_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddReportBlockRequest {
    pub block: ReportBlockDefinition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<ReportBlockPosition>,
    #[serde(default = "default_true", rename = "insertMarkdownPlaceholder")]
    pub insert_markdown_placeholder: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplaceReportBlockRequest {
    pub block: ReportBlockDefinition,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PatchReportBlockRequest {
    /// RFC 7386-style JSON merge patch applied to the block definition.
    /// The block id cannot be changed through this operation.
    pub patch: Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MoveReportBlockRequest {
    pub position: ReportBlockPosition,
    #[serde(default = "default_true", rename = "moveMarkdownPlaceholder")]
    pub move_markdown_placeholder: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RemoveReportBlockRequest {
    #[serde(default = "default_true", rename = "removeMarkdownPlaceholder")]
    pub remove_markdown_placeholder: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportBlockMutationResponse {
    pub success: bool,
    pub report: ReportDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<ReportBlockDefinition>,
    pub message: String,
}

fn default_true() -> bool {
    true
}
