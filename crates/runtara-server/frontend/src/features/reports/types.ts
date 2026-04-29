export type ReportStatus = 'draft' | 'published' | 'archived';

export type ReportBlockType = 'table' | 'chart' | 'metric' | 'markdown';

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

export interface ReportTableSearchRequest {
  query: string;
  fields?: string[];
}

export interface ReportSource {
  schema: string;
  connectionId?: string;
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
}

export interface ReportTableColumnSource extends ReportSource {
  join?: Array<{
    parentField: string;
    field: string;
    op?: string;
  }>;
}

export interface ReportTableColumn {
  field: string;
  label?: string;
  format?: string;
  type?: 'value' | 'chart';
  chart?: {
    kind: ReportChartKind;
    x: string;
    series?: Array<{
      field: string;
      label?: string;
    }>;
  };
  source?: ReportTableColumnSource;
}

export interface ReportBlockDefinition {
  id: string;
  type: ReportBlockType;
  title?: string;
  lazy?: boolean;
  source: ReportSource;
  table?: {
    columns?: ReportTableColumn[];
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
  filters?: ReportFilterDefinition[];
  interactions?: ReportInteractionDefinition[];
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
  valueFrom?: string;
  value?: unknown;
}

export type ReportLayoutNode =
  | {
      id: string;
      type: 'markdown';
      content: string;
    }
  | {
      id: string;
      type: 'block';
      blockId: string;
    }
  | {
      id: string;
      type: 'metric_row';
      title?: string;
      blocks: string[];
    }
  | {
      id: string;
      type: 'section';
      title?: string;
      description?: string;
      children?: ReportLayoutNode[];
    }
  | {
      id: string;
      type: 'columns';
      columns: Array<{
        id: string;
        width?: number;
        children?: ReportLayoutNode[];
      }>;
    }
  | {
      id: string;
      type: 'grid';
      columns?: number;
      items: Array<{
        id?: string;
        blockId: string;
        colSpan?: number;
        rowSpan?: number;
      }>;
    };

export interface ReportDefinition {
  definitionVersion: number;
  markdown: string;
  layout?: ReportLayoutNode[];
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
