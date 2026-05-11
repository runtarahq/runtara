export type ReportStatus = 'draft' | 'published' | 'archived';

export type ReportBlockType =
  | 'table'
  | 'chart'
  | 'metric'
  | 'actions'
  | 'markdown'
  | 'card';

export type ReportCardFieldKind =
  | 'value'
  | 'json'
  | 'markdown'
  | 'subcard'
  | 'subtable'
  | 'workflow_button';

export type ReportWorkflowActionContextMode =
  | 'row'
  | 'field'
  | 'value'
  | 'selection';

export interface ReportWorkflowActionContext {
  mode?: ReportWorkflowActionContextMode;
  field?: string;
  inputKey?: string;
}

export interface ReportRowCondition {
  op: string;
  arguments?: unknown[];
}

export interface ReportWorkflowActionConfig {
  workflowId: string;
  version?: number;
  label?: string;
  runningLabel?: string;
  successMessage?: string;
  reloadBlock?: boolean;
  visibleWhen?: ReportRowCondition;
  hiddenWhen?: ReportRowCondition;
  disabledWhen?: ReportRowCondition;
  context?: ReportWorkflowActionContext;
}

export interface ReportSubtableColumn {
  field: string;
  label?: string;
  format?: string;
  pillVariants?: Record<string, string>;
  align?: 'left' | 'right' | 'center';
}

export interface ReportSubtableConfig {
  columns: ReportSubtableColumn[];
  emptyLabel?: string;
}

export interface ReportCardField {
  field: string;
  label?: string;
  /** Row field rendered instead of `field` while writeback still targets `field`. */
  displayField?: string;
  /** Renderer for this field. Defaults to `value`. */
  kind?: ReportCardFieldKind;
  /** Format hint for `kind=value` (currency, datetime, pill, etc). */
  format?: string;
  /** Pill variant map for color-coding `format=pill` value cells. */
  pillVariants?: Record<string, string>;
  /** Start collapsed (json/markdown/subcard/subtable). */
  collapsed?: boolean;
  /** Inner-grid column span. Default 1. */
  colSpan?: number;
  /** Recursive card config used when `kind=subcard`. */
  subcard?: ReportCardConfig;
  /** Inline-table config used when `kind=subtable`. */
  subtable?: ReportSubtableConfig;
  /** Opt-in writeback. Honored only when the rendered row carries `id`+`schemaId`. */
  editable?: boolean;
  /** Explicit editor; overrides format-based inference. */
  editor?: ReportEditorConfig;
  /** Workflow launcher rendered as a button for this field. */
  workflowAction?: ReportWorkflowActionConfig;
}

export type ReportEditorKind =
  | 'text'
  | 'textarea'
  | 'number'
  | 'select'
  | 'toggle'
  | 'date'
  | 'datetime'
  | 'lookup';

export interface ReportEditorOption {
  label: string;
  value: unknown;
}

export interface ReportEditorConfig {
  kind: ReportEditorKind;
  lookup?: ReportLookupConfig;
  options?: ReportEditorOption[];
  min?: number;
  max?: number;
  step?: number;
  regex?: string;
  placeholder?: string;
}

export interface ReportLookupConfig {
  schema: string;
  connectionId?: string | null;
  valueField: string;
  labelField: string;
  searchFields?: string[];
  condition?: unknown;
  filterMappings?: Array<{
    filterId: string;
    field: string;
    op?: string;
  }>;
}

export interface ReportCardGroup {
  id: string;
  title?: string;
  description?: string;
  /** Number of columns in this group's inner grid (1–4). Default 2. */
  columns?: number;
  fields: ReportCardField[];
}

export interface ReportCardConfig {
  groups: ReportCardGroup[];
}

export type ReportFilterType =
  | 'select'
  | 'multi_select'
  | 'radio'
  | 'checkbox'
  | 'time_range'
  | 'number_range'
  | 'text'
  | 'search';

export type ReportChartKind = 'line' | 'bar' | 'area' | 'pie' | 'donut';

export type ReportAggregateFn =
  | 'count'
  | 'sum'
  | 'avg'
  | 'min'
  | 'max'
  | 'first_value'
  | 'last_value'
  | 'percentile_cont'
  | 'percentile_disc'
  | 'stddev_samp'
  | 'var_samp'
  | 'expr';

export interface ReportFilterOption {
  label: string;
  value: unknown;
  count?: number;
}

export interface ReportFilterOptionsConfig {
  source?: 'static' | 'object_model';
  values?: ReportFilterOption[];
  schema?: string;
  field?: string;
  valueField?: string;
  labelField?: string;
  connectionId?: string;
  search?: boolean;
  dependsOn?: string[];
  filterMappings?: Array<{
    filterId: string;
    field: string;
    op?: string;
  }>;
  condition?: unknown;
}

export interface ReportFilterDefinition {
  /** When true, blocks whose source condition references this filter render an
   * empty "filter not set" state if the filter has no value, instead of
   * silently falling back to an unfiltered query. Use for navigation-driven
   * filters set by row-click + navigate_view. */
  strictWhenReferenced?: boolean;
  id: string;
  label: string;
  type: ReportFilterType;
  default?: unknown;
  required?: boolean;
  options?: {
    source?: 'static' | 'object_model';
    values?: ReportFilterOption[];
  } & ReportFilterOptionsConfig;
  appliesTo?: Array<{
    blockId?: string;
    field: string;
    op?: string;
  }>;
}

export interface ReportOrderBy {
  field: string;
  direction?: 'asc' | 'desc' | string;
}

export type ReportDatasetFieldType =
  | 'string'
  | 'number'
  | 'decimal'
  | 'boolean'
  | 'date'
  | 'datetime'
  | 'json';

export type ReportDatasetValueFormat =
  | 'string'
  | 'number'
  | 'decimal'
  | 'currency'
  | 'percent'
  | 'boolean'
  | 'date'
  | 'datetime';

export interface ReportDatasetDefinition {
  id: string;
  label: string;
  source: {
    schema: string;
    connectionId?: string | null;
  };
  timeDimension?: string;
  dimensions: Array<{
    field: string;
    label: string;
    type: ReportDatasetFieldType;
    format?: ReportDatasetValueFormat;
  }>;
  measures: Array<{
    id: string;
    label: string;
    op: ReportAggregateFn;
    field?: string;
    distinct?: boolean;
    orderBy?: ReportOrderBy[];
    expression?: unknown;
    percentile?: number;
    format: ReportDatasetValueFormat;
  }>;
}

export interface ReportBlockDatasetQuery {
  id: string;
  dimensions?: string[];
  measures?: string[];
  orderBy?: ReportOrderBy[];
  datasetFilters?: ReportDatasetFilterRequest[];
  limit?: number;
}

export interface ReportDatasetFilterRequest {
  field: string;
  op?: string;
  value: unknown;
}

export interface ReportDatasetQueryRequest {
  filters?: Record<string, unknown>;
  datasetFilters?: ReportDatasetFilterRequest[];
  dimensions: string[];
  measures: string[];
  orderBy?: ReportOrderBy[];
  sort?: ReportOrderBy[];
  limit?: number;
  search?: ReportTableSearchRequest;
  page?: {
    offset: number;
    size: number;
  };
  timezone?: string;
}

export interface ReportDatasetQueryColumn {
  key?: string;
  field?: string;
  label: string;
  type: string;
  format?: ReportDatasetValueFormat;
}

export interface ReportDatasetQueryResponse {
  success: boolean;
  dataset: {
    id: string;
  };
  columns: ReportDatasetQueryColumn[];
  rows: unknown[][];
  page: {
    offset: number;
    size: number;
    totalCount: number;
    hasNextPage: boolean;
  };
}

export interface ReportTableSearchRequest {
  query: string;
  fields?: string[];
}

export interface ReportSource {
  kind?: 'object_model' | 'workflow_runtime' | 'system';
  schema: string;
  connectionId?: string;
  entity?:
    | 'instances'
    | 'actions'
    | 'runtime_execution_metric_buckets'
    | 'runtime_system_snapshot'
    | 'connection_rate_limit_status'
    | 'connection_rate_limit_events'
    | 'connection_rate_limit_timeline';
  workflowId?: string;
  instanceId?: string;
  mode?: 'filter' | 'aggregate';
  condition?: unknown;
  filterMappings?: Array<{
    filterId: string;
    field: string;
    op?: string;
  }>;
  groupBy?: string[];
  aggregates?: Array<{
    alias: string;
    op: ReportAggregateFn;
    field?: string;
    distinct?: boolean;
    orderBy?: ReportOrderBy[];
    expression?: unknown;
    percentile?: number;
  }>;
  orderBy?: ReportOrderBy[];
  limit?: number;
  granularity?: string;
  interval?: string;
  join?: ReportSourceJoin[];
}

export interface ReportSourceJoin {
  schema: string;
  alias?: string;
  connectionId?: string;
  parentField: string;
  field: string;
  op?: string;
  kind?: 'inner' | 'left';
}

export interface ReportTableColumnSource extends Omit<ReportSource, 'join'> {
  select?: string;
  join?: Array<{
    parentField: string;
    field: string;
    op?: string;
    kind?: 'inner' | 'left';
  }>;
}

export type ReportPillVariant =
  | 'success'
  | 'info'
  | 'warning'
  | 'danger'
  | 'muted'
  | 'default';

export interface ReportTableColumn {
  field: string;
  label?: string;
  /** Row field rendered instead of `field` while sort/writeback still target `field`. */
  displayField?: string;
  format?: string;
  type?: 'value' | 'chart' | 'workflow_button' | 'interaction_buttons';
  chart?: {
    kind: ReportChartKind;
    x: string;
    series?: Array<{
      field: string;
      label?: string;
    }>;
  };
  source?: ReportTableColumnSource;
  /** Row field rendered as a subdued line beneath the primary value. */
  secondaryField?: string;
  /** Row field whose value is treated as a URL and rendered as an external-link icon. */
  linkField?: string;
  /** Row field whose value is shown in a tooltip (e.g. full email behind an avatar). */
  tooltipField?: string;
  /** Mapping from cell value to pill variant for `format: "pill"` columns. */
  pillVariants?: Record<string, ReportPillVariant | string>;
  /** Ordered levels for `format: "bar_indicator"` columns. */
  levels?: string[];
  /** Cell alignment hint. */
  align?: 'left' | 'right' | 'center';
  /** Marks this column as the row's human-readable label for entity-title lookups. */
  descriptive?: boolean;
  /** Opt-in writeback. Honored only when the rendered row carries `id`+`schemaId`. */
  editable?: boolean;
  /** Explicit editor; overrides format-based inference. */
  editor?: ReportEditorConfig;
  /** Workflow launcher rendered as a button in this column. */
  workflowAction?: ReportWorkflowActionConfig;
  /** Row-scoped report interaction buttons rendered in this column. */
  interactionButtons?: ReportTableInteractionButtonConfig[];
}

export interface ReportTableInteractionButtonConfig {
  id: string;
  label?: string;
  icon?: 'eye' | 'file_text' | 'activity' | 'arrow_right';
  visibleWhen?: ReportRowCondition;
  hiddenWhen?: ReportRowCondition;
  disabledWhen?: ReportRowCondition;
  actions: ReportInteractionAction[];
}

export interface ReportTableActionConfig {
  id: string;
  label?: string;
  workflowAction: ReportWorkflowActionConfig;
}

export interface ReportBlockDefinition {
  id: string;
  type: ReportBlockType;
  title?: string;
  lazy?: boolean;
  dataset?: ReportBlockDatasetQuery;
  source: ReportSource;
  table?: {
    columns?: ReportTableColumn[];
    selectable?: boolean;
    actions?: ReportTableActionConfig[];
    defaultSort?: ReportOrderBy[];
    pagination?: {
      defaultPageSize?: number;
      allowedPageSizes?: number[];
    };
  };
  chart?: {
    kind: ReportChartKind;
    x: string;
    series: Array<{
      field: string;
      label?: string;
    }>;
  };
  metric?: {
    valueField: string;
    label?: string;
    format?: string;
  };
  actions?: {
    submit?: {
      label?: string;
      implicitPayload?: Record<string, unknown>;
    };
  };
  markdown?: {
    content: string;
  };
  card?: ReportCardConfig;
  filters?: ReportFilterDefinition[];
  interactions?: ReportInteractionDefinition[];
  showWhen?: ReportVisibilityCondition;
  /** When true, the renderer drops the block entirely (title bar included)
   *  if its data is empty. Used for action / pending-work blocks that should
   *  disappear once there's nothing to do. */
  hideWhenEmpty?: boolean;
}

export interface ReportInteractionDefinition {
  id: string;
  trigger: {
    event: 'point_click' | 'row_click' | 'cell_click' | string;
    field?: string;
  };
  actions: ReportInteractionAction[];
}

export interface ReportInteractionAction {
  type: 'set_filter' | string;
  filterId?: string;
  filterIds?: string[];
  viewId?: string;
  valueFrom?: string;
  value?: unknown;
}

export interface ReportInteractionOptions {
  replace?: boolean;
  viewId?: string | null;
  clearFilters?: string[];
}

export interface ReportVisibilityCondition {
  filter: string;
  exists?: boolean;
  equals?: unknown;
  notEquals?: unknown;
}

type ReportLayoutNodeBase = {
  id: string;
  showWhen?: ReportVisibilityCondition;
};

export type ReportLayoutNode =
  | ({
      id: string;
      type: 'block';
      blockId: string;
    } & ReportLayoutNodeBase)
  | ({
      id: string;
      type: 'metric_row';
      title?: string;
      blocks: string[];
    } & ReportLayoutNodeBase)
  | ({
      id: string;
      type: 'section';
      title?: string;
      description?: string;
      children?: ReportLayoutNode[];
    } & ReportLayoutNodeBase)
  | ({
      id: string;
      type: 'columns';
      columns: Array<{
        id: string;
        width?: number;
        children?: ReportLayoutNode[];
      }>;
    } & ReportLayoutNodeBase)
  | ({
      id: string;
      type: 'grid';
      columns?: number;
      items: Array<{
        id?: string;
        blockId: string;
        colSpan?: number;
        rowSpan?: number;
      }>;
    } & ReportLayoutNodeBase);

export interface ReportViewBreadcrumb {
  label: string;
  viewId?: string;
  clearFilters?: string[];
}

export interface ReportTitleFromBlock {
  block: string;
  field?: string;
}

export interface ReportViewDefinition {
  id: string;
  title?: string;
  titleFrom?: string;
  titleFromBlock?: ReportTitleFromBlock;
  parentViewId?: string;
  clearFiltersOnBack?: string[];
  breadcrumb?: ReportViewBreadcrumb[];
  layout?: ReportLayoutNode[];
}

export interface ReportDefinition {
  definitionVersion: number;
  layout?: ReportLayoutNode[];
  views?: ReportViewDefinition[];
  datasets?: ReportDatasetDefinition[];
  filters: ReportFilterDefinition[];
  blocks: ReportBlockDefinition[];
}

export interface ReportSummary {
  id: string;
  slug: string;
  name: string;
  description?: string | null;
  tags: string[];
  status: ReportStatus;
  definitionVersion: number;
  createdAt: string;
  updatedAt: string;
}

export interface ReportDto extends ReportSummary {
  definition: ReportDefinition;
}

export interface ReportBlockResult {
  type: ReportBlockType;
  status: 'ready' | 'loading' | 'empty' | 'error';
  title?: string;
  data?: unknown;
  error?: {
    code: string;
    message: string;
    blockId?: string;
  };
}

export interface ReportWorkflowAction {
  id: string;
  actionId: string;
  actionKind: string;
  targetKind: string;
  targetId: string;
  workflowId: string;
  instanceId: string;
  signalId: string;
  actionKey?: string | null;
  label: string;
  message: string;
  inputSchema?: Record<string, unknown> | null;
  schemaFormat: string;
  status: string;
  requestedAt?: string | null;
  correlation?: Record<string, unknown>;
  context?: Record<string, unknown>;
  runtime?: Record<string, unknown>;
}

export interface ReportRenderResponse {
  success: boolean;
  report: {
    id: string;
    definitionVersion: number;
  };
  resolvedFilters: Record<string, unknown>;
  blocks: Record<string, ReportBlockResult>;
  errors: Array<{
    code: string;
    message: string;
    blockId?: string;
  }>;
}

export interface RunReportWorkflowRequest {
  workflowId: string;
  version?: number;
  context: unknown;
}

export interface RunReportWorkflowResponse {
  instanceId: string;
  status: string;
}

export interface ReportWorkflowInstanceStatus {
  id: string;
  status: string;
}

export interface ReportBlockDataRequest {
  id: string;
  page?: {
    offset: number;
    size: number;
  };
  sort?: ReportOrderBy[];
  search?: ReportTableSearchRequest;
  blockFilters?: Record<string, unknown>;
}

export interface ReportFilterOptionsRequest {
  filters: Record<string, unknown>;
  query?: string;
  offset?: number;
  limit?: number;
  timezone?: string;
}

export interface ReportFilterOptionsResponse {
  success: boolean;
  filter: {
    id: string;
  };
  options: ReportFilterOption[];
  page: {
    offset: number;
    size: number;
    totalCount: number;
    hasNextPage: boolean;
  };
}

export interface ReportLookupOptionsRequest {
  filters: Record<string, unknown>;
  blockFilters?: Record<string, unknown>;
  query?: string;
  offset?: number;
  limit?: number;
  timezone?: string;
}

export interface ReportLookupOptionsResponse {
  success: boolean;
  block: {
    id: string;
  };
  field: string;
  options: ReportFilterOption[];
  page: {
    offset: number;
    size: number;
    totalCount: number;
    hasNextPage: boolean;
  };
}

export interface ReportRenderRequest {
  filters: Record<string, unknown>;
  blocks?: ReportBlockDataRequest[];
  timezone?: string;
}

export interface CreateReportRequest {
  name: string;
  slug?: string;
  description?: string | null;
  tags: string[];
  status: ReportStatus;
  definition: ReportDefinition;
}

export interface UpdateReportRequest {
  name: string;
  slug: string;
  description?: string | null;
  tags: string[];
  status: ReportStatus;
  definition: ReportDefinition;
}
