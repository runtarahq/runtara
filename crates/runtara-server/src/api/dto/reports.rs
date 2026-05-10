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

fn default_report_source_kind() -> ReportSourceKind {
    ReportSourceKind::ObjectModel
}

fn is_default_report_source_kind(kind: &ReportSourceKind) -> bool {
    *kind == ReportSourceKind::ObjectModel
}

pub(crate) fn default_report_source() -> ReportSource {
    ReportSource {
        kind: default_report_source_kind(),
        schema: String::new(),
        connection_id: None,
        entity: None,
        workflow_id: None,
        instance_id: None,
        mode: default_source_mode(),
        condition: None,
        filter_mappings: vec![],
        group_by: vec![],
        aggregates: vec![],
        order_by: vec![],
        limit: None,
        granularity: None,
        interval: None,
        join: vec![],
    }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layout: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<ReportViewDefinition>,
    #[serde(default)]
    pub filters: Vec<ReportFilterDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub datasets: Vec<ReportDatasetDefinition>,
    #[serde(default)]
    pub blocks: Vec<ReportBlockDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportViewDefinition {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, rename = "titleFrom", skip_serializing_if = "Option::is_none")]
    pub title_from: Option<String>,
    /// Resolves the view title from a rendered block's row. The first row of
    /// the referenced block is used; the value comes from `field` if given,
    /// otherwise from the block column flagged `descriptive: true`.
    #[serde(
        default,
        rename = "titleFromBlock",
        skip_serializing_if = "Option::is_none"
    )]
    pub title_from_block: Option<ReportTitleFromBlock>,
    #[serde(
        default,
        rename = "parentViewId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_view_id: Option<String>,
    #[serde(
        default,
        rename = "clearFiltersOnBack",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub clear_filters_on_back: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breadcrumb: Vec<ReportViewBreadcrumb>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layout: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportTitleFromBlock {
    pub block: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportViewBreadcrumb {
    pub label: String,
    #[serde(default, rename = "viewId", skip_serializing_if = "Option::is_none")]
    pub view_id: Option<String>,
    #[serde(
        default,
        rename = "clearFilters",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub clear_filters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetDefinition {
    pub id: String,
    pub label: String,
    pub source: ReportDatasetSource,
    #[serde(
        default,
        rename = "timeDimension",
        skip_serializing_if = "Option::is_none"
    )]
    pub time_dimension: Option<String>,
    #[serde(default)]
    pub dimensions: Vec<ReportDatasetDimension>,
    #[serde(default)]
    pub measures: Vec<ReportDatasetMeasure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetSource {
    pub schema: String,
    #[serde(
        default,
        rename = "connectionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub connection_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetDimension {
    pub field: String,
    pub label: String,
    #[serde(rename = "type")]
    pub dimension_type: ReportDatasetFieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ReportDatasetValueFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetMeasure {
    pub id: String,
    pub label: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentile: Option<f64>,
    pub format: ReportDatasetValueFormat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportDatasetFieldType {
    String,
    Number,
    Decimal,
    Boolean,
    Date,
    Datetime,
    Json,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportDatasetValueFormat {
    String,
    Number,
    Decimal,
    Currency,
    Percent,
    Boolean,
    Date,
    Datetime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportSourceKind {
    #[default]
    ObjectModel,
    WorkflowRuntime,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportWorkflowRuntimeEntity {
    Instances,
    Actions,
    RuntimeExecutionMetricBuckets,
    RuntimeSystemSnapshot,
    ConnectionRateLimitStatus,
    ConnectionRateLimitEvents,
    ConnectionRateLimitTimeline,
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
    /// When true, any block whose source `condition` references this filter
    /// will short-circuit to an empty result if the filter has no value at
    /// render time. Use this for navigation-driven filters (e.g. populated by
    /// row-click + navigate_view) so the block never silently falls back to an
    /// unfiltered query when the filter is missing from the URL/state.
    #[serde(
        default,
        rename = "strictWhenReferenced",
        skip_serializing_if = "is_false_filter"
    )]
    pub strict_when_referenced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
    #[serde(default, rename = "appliesTo")]
    pub applies_to: Vec<ReportFilterTarget>,
}

fn is_false_filter(value: &bool) -> bool {
    !value
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<ReportBlockDatasetQuery>,
    #[serde(
        default = "default_report_source",
        skip_serializing_if = "ReportSource::is_empty"
    )]
    pub source: ReportSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<ReportTableConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<ReportChartConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<ReportMetricConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<ReportActionsConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<ReportCardConfig>,
    #[serde(default)]
    pub filters: Vec<ReportFilterDefinition>,
    #[serde(default)]
    pub interactions: Vec<ReportInteractionDefinition>,
    #[serde(default, rename = "showWhen", skip_serializing_if = "Option::is_none")]
    pub show_when: Option<Value>,
    /// When true, the renderer drops the entire block (title bar included) if
    /// its data is empty (e.g. zero table rows or zero open actions). Useful
    /// for action lists or "open issues" tables that should disappear once
    /// there's nothing to show, rather than rendering a stub "No items"
    /// state.
    #[serde(
        default,
        rename = "hideWhenEmpty",
        skip_serializing_if = "is_false_block"
    )]
    pub hide_when_empty: bool,
}

fn is_false_block(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportActionsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<ReportActionSubmitConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportActionSubmitConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(
        default,
        rename = "implicitPayload",
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub implicit_payload: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportWorkflowActionConfig {
    #[serde(rename = "workflowId")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(
        default,
        rename = "runningLabel",
        skip_serializing_if = "Option::is_none"
    )]
    pub running_label: Option<String>,
    #[serde(
        default,
        rename = "successMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub success_message: Option<String>,
    #[serde(default, rename = "reloadBlock", skip_serializing_if = "is_false")]
    pub reload_block: bool,
    /// Optional row-level condition. When set, the frontend renders the button
    /// only for rows that match this condition.
    #[serde(
        default,
        rename = "visibleWhen",
        skip_serializing_if = "Option::is_none"
    )]
    pub visible_when: Option<Condition>,
    /// Optional row-level condition. When set, the frontend hides the button
    /// for rows that match this condition.
    #[serde(
        default,
        rename = "hiddenWhen",
        skip_serializing_if = "Option::is_none"
    )]
    pub hidden_when: Option<Condition>,
    /// Optional row-level condition. When set, the frontend renders the button
    /// disabled for rows that match this condition.
    #[serde(
        default,
        rename = "disabledWhen",
        skip_serializing_if = "Option::is_none"
    )]
    pub disabled_when: Option<Condition>,
    #[serde(default)]
    pub context: ReportWorkflowActionContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportWorkflowActionContext {
    #[serde(default)]
    pub mode: ReportWorkflowActionContextMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, rename = "inputKey", skip_serializing_if = "Option::is_none")]
    pub input_key: Option<String>,
}

impl Default for ReportWorkflowActionContext {
    fn default() -> Self {
        Self {
            mode: ReportWorkflowActionContextMode::Row,
            field: None,
            input_key: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportWorkflowActionContextMode {
    #[default]
    Row,
    Field,
    Value,
    Selection,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportBlockDatasetQuery {
    pub id: String,
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub measures: Vec<String>,
    #[serde(default, rename = "orderBy")]
    pub order_by: Vec<ReportOrderBy>,
    #[serde(
        default,
        rename = "datasetFilters",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub dataset_filters: Vec<ReportDatasetFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReportBlockType {
    Table,
    Chart,
    Metric,
    Actions,
    Markdown,
    Card,
}

/// Card block configuration. A card renders a single record (the first row of
/// a filter-mode source) as a vertical key→value layout, optionally split into
/// titled groups with multi-column inner grids and per-field formatting.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportCardConfig {
    #[serde(default)]
    pub groups: Vec<ReportCardGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportCardGroup {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Number of columns to lay fields out in within this group (1–4).
    #[serde(default = "default_card_group_columns")]
    pub columns: u8,
    pub fields: Vec<ReportCardField>,
}

fn default_card_group_columns() -> u8 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportCardField {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Optional row property to display instead of `field` while preserving
    /// `field` as the writeback target. Useful for lookup/reference fields
    /// where the row stores an id but a joined label should be shown.
    #[serde(
        default,
        rename = "displayField",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_field: Option<String>,
    /// How to render this field. `value` runs through the standard cell
    /// formatter (date/currency/pill/etc); `json` shows a collapsible JSON
    /// tree; `markdown` renders the value as markdown; `subcard` renders a
    /// nested object as a card with its own `groups`; `subtable` renders an
    /// array of objects as a small inline table with its own `columns`.
    #[serde(default = "default_card_field_kind")]
    pub kind: ReportCardFieldKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Pill variants for `kind=value` + `format=pill`. Maps the cell value to
    /// a badge variant (`success`, `warning`, `destructive`, `default`, …) so
    /// enum/status fields can be color-coded.
    #[serde(
        default,
        rename = "pillVariants",
        skip_serializing_if = "Option::is_none"
    )]
    pub pill_variants: Option<std::collections::BTreeMap<String, String>>,
    /// Whether the field starts collapsed. Only meaningful for `kind=json` /
    /// `kind=markdown` / `kind=subcard` / `kind=subtable`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    /// Span in inner-grid columns. Default 1; use 2+ for fields that should
    /// occupy a wider slot than their siblings (e.g. long descriptions).
    #[serde(default = "default_card_field_col_span", rename = "colSpan")]
    pub col_span: u8,
    /// Recursive card config used when `kind=subcard`. The value at `field`
    /// must be a JSON object; the inner groups read keys off that object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subcard: Option<Box<ReportCardConfig>>,
    /// Inline-table config used when `kind=subtable`. The value at `field`
    /// must be a JSON array of objects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtable: Option<ReportSubtableConfig>,
    /// Opt-in writeback for this field. Only honored when the rendered row
    /// carries `id` and `schemaId` (filter-mode object-model sources). The
    /// renderer doesn't enforce this on the server — the FE shows an editor
    /// that calls the object-model PUT endpoint directly, which performs its
    /// own auth + type validation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub editable: bool,
    /// Optional explicit editor configuration. When set, takes precedence
    /// over the default control inferred from `format` / `pillVariants`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<ReportEditorConfig>,
    /// Optional workflow launcher rendered as a button for this card field.
    /// The frontend executes the referenced workflow with either the whole row,
    /// this field value, or a configured row field as the workflow input context.
    #[serde(
        default,
        rename = "workflowAction",
        skip_serializing_if = "Option::is_none"
    )]
    pub workflow_action: Option<ReportWorkflowActionConfig>,
}

fn default_card_field_kind() -> ReportCardFieldKind {
    ReportCardFieldKind::Value
}

fn default_card_field_col_span() -> u8 {
    1
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportCardFieldKind {
    Value,
    Json,
    Markdown,
    Subcard,
    Subtable,
    WorkflowButton,
}

/// Inline-table rendering for an array-of-objects card field.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportSubtableConfig {
    #[serde(default)]
    pub columns: Vec<ReportSubtableColumn>,
    /// Optional message shown when the array is empty (defaults to "No items").
    #[serde(
        default,
        rename = "emptyLabel",
        skip_serializing_if = "Option::is_none"
    )]
    pub empty_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportSubtableColumn {
    /// Property name on each array element to read for this cell.
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Cell format hint. Same vocabulary as table columns
    /// (`currency`, `datetime`, `pill`, `decimal`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Pill variant map for `format=pill`.
    #[serde(
        default,
        rename = "pillVariants",
        skip_serializing_if = "Option::is_none"
    )]
    pub pill_variants: Option<std::collections::BTreeMap<String, String>>,
    /// Cell alignment hint: `left`, `right`, `center`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportSource {
    #[serde(
        default = "default_report_source_kind",
        skip_serializing_if = "is_default_report_source_kind"
    )]
    pub kind: ReportSourceKind,
    #[serde(default)]
    pub schema: String,
    #[serde(
        default,
        rename = "connectionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<ReportWorkflowRuntimeEntity>,
    #[serde(
        default,
        rename = "workflowId",
        skip_serializing_if = "Option::is_none"
    )]
    pub workflow_id: Option<String>,
    #[serde(
        default,
        rename = "instanceId",
        skip_serializing_if = "Option::is_none"
    )]
    pub instance_id: Option<String>,
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
    /// Optional virtual-source granularity. Used by system sources such as
    /// execution metric buckets (`hourly`/`daily`) and rate-limit timelines
    /// (`minute`/`hourly`/`daily`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<String>,
    /// Optional virtual-source period. Used by rate-limit status period stats
    /// (`1h`/`24h`/`7d`/`30d`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// Cross-schema joins. When non-empty, fields prefixed with `<alias>.`
    /// resolve against the joined dimension schema. Currently supported on
    /// aggregate-mode blocks; v1 implementation uses broadcast-hash join
    /// (dim resolved client-side, primary query pushed down with the resolved
    /// keys, rows enriched after).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub join: Vec<ReportSourceJoin>,
}

impl ReportSource {
    pub fn is_empty(&self) -> bool {
        self.kind == ReportSourceKind::ObjectModel
            && self.schema.trim().is_empty()
            && self.connection_id.is_none()
            && self.entity.is_none()
            && self.workflow_id.is_none()
            && self.instance_id.is_none()
            && self.mode == default_source_mode()
            && self.condition.is_none()
            && self.filter_mappings.is_empty()
            && self.group_by.is_empty()
            && self.aggregates.is_empty()
            && self.order_by.is_empty()
            && self.limit.is_none()
            && self.granularity.is_none()
            && self.interval.is_none()
            && self.join.is_empty()
    }
}

/// Cross-schema join declared on a block-level source. Mirrors the per-cell
/// `ReportTableColumnJoin` but adds `schema`, `alias`, and `kind` since the
/// primary schema is the block's source rather than the column's.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportSourceJoin {
    /// Joined (dimension) schema name.
    pub schema: String,
    /// Optional alias for qualified field references in `groupBy`,
    /// `condition`, `aggregates[].field`, and `orderBy`. Defaults to `schema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Optional connection ID for the dimension schema.
    #[serde(
        default,
        rename = "connectionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub connection_id: Option<String>,
    /// Field on the joined (dimension) schema.
    pub field: String,
    /// Field on the parent (block-source) schema.
    #[serde(rename = "parentField")]
    pub parent_field: String,
    /// Comparison op — eq | ne | gt | gte | lt | lte | in | contains | search.
    /// Default: eq. Mirrors `ReportTableColumnJoin.op`.
    #[serde(default = "default_filter_op")]
    pub op: String,
    /// Inner or left join. Default: inner. Inner drops fact rows with no
    /// matching dim row; left keeps them with null dim columns.
    #[serde(default)]
    pub kind: ReportJoinKind,
}

impl ReportSourceJoin {
    /// Resolve the alias used for qualified field refs.
    pub fn effective_alias(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.schema)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportJoinKind {
    #[default]
    Inner,
    Left,
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
    /// Enables row selection controls even when no table-wide actions are configured.
    #[serde(default, skip_serializing_if = "is_false")]
    pub selectable: bool,
    /// Optional table-wide workflow actions executed with selected rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ReportTableActionConfig>,
    #[serde(default, rename = "defaultSort")]
    pub default_sort: Vec<ReportOrderBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<ReportPaginationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportTableActionConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(rename = "workflowAction")]
    pub workflow_action: ReportWorkflowActionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportTableColumn {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Optional row property to display instead of `field` while preserving
    /// `field` as the sort/writeback key. For example, a Product row can store
    /// `category_id` while displaying a joined `category.name`.
    #[serde(
        default,
        rename = "displayField",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub column_type: Option<ReportTableColumnType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<ReportChartConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ReportTableColumnSource>,
    /// Optional row field rendered as a subdued line below the primary value.
    #[serde(
        default,
        rename = "secondaryField",
        skip_serializing_if = "Option::is_none"
    )]
    pub secondary_field: Option<String>,
    /// Optional row field whose value is treated as a URL and rendered as an external-link icon.
    #[serde(default, rename = "linkField", skip_serializing_if = "Option::is_none")]
    pub link_field: Option<String>,
    /// Optional row field whose value is shown in a tooltip on hover (e.g. full email behind an avatar).
    #[serde(
        default,
        rename = "tooltipField",
        skip_serializing_if = "Option::is_none"
    )]
    pub tooltip_field: Option<String>,
    /// Mapping from cell value to pill variant for `format: "pill"` columns
    /// (e.g. `{ "active_customer": "success", "churned": "muted" }`).
    #[serde(
        default,
        rename = "pillVariants",
        skip_serializing_if = "Option::is_none"
    )]
    pub pill_variants: Option<std::collections::BTreeMap<String, String>>,
    /// Ordered level list for `format: "bar_indicator"` columns; the value's
    /// position determines how many bars are filled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<Vec<String>>,
    /// Optional cell alignment hint: "left", "right", or "center".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    /// Marks this column as the human-readable label for the row's entity
    /// within this report. Consumed by view `titleFrom: { block }` resolution
    /// and similar entity-label lookups. At most one descriptive column per
    /// table is meaningful; the first encountered wins if multiple are flagged.
    #[serde(default, skip_serializing_if = "is_false")]
    pub descriptive: bool,
    /// Opt-in writeback for this column. Only honored when the rendered row
    /// carries `id` and `schemaId` (filter-mode object-model sources without
    /// joins or aggregates). The renderer doesn't enforce this on the server —
    /// the FE shows an editor that calls the object-model PUT endpoint
    /// directly, which performs its own auth + type validation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub editable: bool,
    /// Optional explicit editor configuration. When set, takes precedence
    /// over the default control inferred from `format` / `pillVariants`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<ReportEditorConfig>,
    /// Optional workflow launcher rendered as a button in this table column.
    /// The frontend executes the referenced workflow with either the whole row,
    /// this cell value, or a configured row field as the workflow input context.
    #[serde(
        default,
        rename = "workflowAction",
        skip_serializing_if = "Option::is_none"
    )]
    pub workflow_action: Option<ReportWorkflowActionConfig>,
}

/// Explicit editor configuration for an editable column or card field.
///
/// When omitted, the FE infers a control from the column's `format` /
/// `pillVariants` (number for currency/decimal/percent, date for date,
/// select for pill with variants, toggle for booleans, text otherwise).
/// When set, the explicit `kind` wins.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportEditorConfig {
    pub kind: ReportEditorKind,
    /// Dynamic object-model lookup configuration for `kind=lookup`.
    /// The editor displays labels from the lookup schema but commits the
    /// selected value back to the edited row field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookup: Option<ReportLookupConfig>,
    /// Static option list for `kind=select`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ReportEditorOption>,
    /// Min value for `kind=number`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Max value for `kind=number`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Step / precision for `kind=number`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// Validation regex for `kind=text` / `kind=textarea`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// Placeholder shown in empty inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportEditorOption {
    pub label: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportLookupConfig {
    /// Object Model schema to search for options.
    pub schema: String,
    /// Optional connection ID for connection-scoped lookup schemas.
    #[serde(
        default,
        rename = "connectionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub connection_id: Option<String>,
    /// Field whose value is written to the edited row. `field` is accepted as
    /// a compatibility alias.
    #[serde(rename = "valueField", alias = "field")]
    pub value_field: String,
    /// Field shown to users in the searchable option list.
    #[serde(rename = "labelField")]
    pub label_field: String,
    /// Fields searched with CONTAINS when the user types. Defaults to
    /// `labelField` when omitted.
    #[serde(default, rename = "searchFields")]
    pub search_fields: Vec<String>,
    /// Optional Object Model condition applied to the lookup option query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    /// Optional mappings from report/block filters into lookup schema fields.
    #[serde(default, rename = "filterMappings")]
    pub filter_mappings: Vec<ReportFilterTarget>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportEditorKind {
    Text,
    Textarea,
    Number,
    Select,
    Toggle,
    Date,
    Datetime,
    Lookup,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportTableColumnType {
    Value,
    Chart,
    WorkflowButton,
}

impl ReportTableColumn {
    pub fn is_chart(&self) -> bool {
        matches!(self.column_type, Some(ReportTableColumnType::Chart))
    }

    pub fn is_value_lookup(&self) -> bool {
        matches!(self.column_type, Some(ReportTableColumnType::Value)) && self.source.is_some()
    }

    pub fn is_workflow_button(&self) -> bool {
        matches!(
            self.column_type,
            Some(ReportTableColumnType::WorkflowButton)
        ) || self.workflow_action.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportTableColumnSource {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<String>,
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
    #[serde(default)]
    pub join: Vec<ReportTableColumnJoin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportTableColumnJoin {
    #[serde(rename = "parentField")]
    pub parent_field: String,
    pub field: String,
    #[serde(default = "default_filter_op")]
    pub op: String,
    #[serde(default = "default_column_join_kind")]
    pub kind: ReportJoinKind,
}

fn default_column_join_kind() -> ReportJoinKind {
    ReportJoinKind::Left
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
pub struct ReportInteractionDefinition {
    pub id: String,
    pub trigger: ReportInteractionTrigger,
    #[serde(default)]
    pub actions: Vec<ReportInteractionAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportInteractionTrigger {
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportInteractionAction {
    #[serde(rename = "type")]
    pub action_type: String,
    #[serde(default, rename = "filterId", skip_serializing_if = "Option::is_none")]
    pub filter_id: Option<String>,
    #[serde(default, rename = "filterIds", skip_serializing_if = "Vec::is_empty")]
    pub filter_ids: Vec<String>,
    #[serde(default, rename = "viewId", skip_serializing_if = "Option::is_none")]
    pub view_id: Option<String>,
    #[serde(default, rename = "valueFrom", skip_serializing_if = "Option::is_none")]
    pub value_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
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
pub struct ReportFilterOptionsRequest {
    #[serde(default)]
    pub filters: HashMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default)]
    pub offset: i64,
    #[serde(default = "default_filter_options_limit")]
    pub limit: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportLookupOptionsRequest {
    #[serde(default)]
    pub filters: HashMap<String, Value>,
    #[serde(default, rename = "blockFilters")]
    pub block_filters: HashMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default)]
    pub offset: i64,
    #[serde(default = "default_filter_options_limit")]
    pub limit: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportFilterOptionsResponse {
    pub success: bool,
    pub filter: ReportFilterOptionsMetadata,
    pub options: Vec<ReportFilterOption>,
    pub page: ReportFilterOptionsPage,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportLookupOptionsResponse {
    pub success: bool,
    pub block: ReportLookupBlockMetadata,
    pub field: String,
    pub options: Vec<ReportFilterOption>,
    pub page: ReportFilterOptionsPage,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportLookupBlockMetadata {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetQueryRequest {
    #[serde(default)]
    pub filters: HashMap<String, Value>,
    #[serde(default, rename = "datasetFilters")]
    pub dataset_filters: Vec<ReportDatasetFilter>,
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub measures: Vec<String>,
    #[serde(default, rename = "orderBy", alias = "sort")]
    pub order_by: Vec<ReportOrderBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<ReportTableSearchRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<ReportPageRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetFilter {
    pub field: String,
    #[serde(default = "default_filter_op")]
    pub op: String,
    pub value: Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetQueryResponse {
    pub success: bool,
    pub dataset: ReportDatasetQueryMetadata,
    pub columns: Vec<ReportDatasetQueryColumn>,
    pub rows: Vec<Vec<Value>>,
    pub page: ReportDatasetQueryPage,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetQueryMetadata {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetQueryColumn {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub column_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ReportDatasetValueFormat>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportDatasetQueryPage {
    pub offset: i64,
    pub size: i64,
    #[serde(rename = "totalCount")]
    pub total_count: i64,
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportFilterOptionsMetadata {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReportFilterOption {
    pub label: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReportFilterOptionsPage {
    pub offset: i64,
    pub size: i64,
    #[serde(rename = "totalCount")]
    pub total_count: i64,
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
}

fn default_filter_options_limit() -> i64 {
    100
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

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SubmitReportWorkflowActionRequest {
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub filters: HashMap<String, Value>,
    #[serde(default, rename = "blockFilters")]
    pub block_filters: HashMap<String, Value>,
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
