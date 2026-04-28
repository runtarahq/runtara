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
  | 'last_value';

export interface ReportFilterOption {
  label: string;
  value: string | number | boolean;
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
  };
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
  }>;
  orderBy?: ReportOrderBy[];
  limit?: number;
}

export interface ReportTableColumn {
  field: string;
  label?: string;
  format?: string;
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
}

export interface ReportDefinition {
  definitionVersion: number;
  markdown: string;
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
  blockFilters?: Record<string, unknown>;
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
