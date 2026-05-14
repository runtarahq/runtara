import {
  ReportBlockDatasetQuery,
  ReportBlockType,
  ReportChartKind,
  ReportCondition,
  ReportDatasetDefinition,
  ReportEditorConfig,
  ReportAggregateFn,
  ReportFilterDefinition,
  ReportFilterType,
  ReportInteractionDefinition,
  ReportOrderBy,
  ReportSource,
  ReportSourceJoin,
  ReportTableActionConfig,
  ReportTableInteractionButtonConfig,
  ReportViewDefinition,
  ReportVisibilityCondition,
  ReportWorkflowActionConfig,
} from '../../types';

export type WizardBlockType = Extract<
  ReportBlockType,
  'markdown' | 'metric' | 'chart' | 'table' | 'card'
>;

export const WIZARD_BLOCK_TYPES: Array<{
  value: WizardBlockType;
  label: string;
  description: string;
}> = [
  {
    value: 'markdown',
    label: 'Text',
    description: 'Narrative section rendered as markdown.',
  },
  {
    value: 'metric',
    label: 'Metric',
    description: 'Single number (count or aggregate).',
  },
  {
    value: 'chart',
    label: 'Chart',
    description: 'Distribution or trend grouped by a field.',
  },
  {
    value: 'table',
    label: 'Table',
    description: 'Scannable list of records.',
  },
  {
    value: 'card',
    label: 'Card',
    description: "Detail view of a single record's fields.",
  },
];

export type WizardColumnFormat =
  | 'number'
  | 'decimal'
  | 'currency'
  | 'percent'
  | 'date'
  | 'datetime'
  | 'pill'
  | 'bar_indicator'
  | 'boolean';

export const WIZARD_COLUMN_FORMATS: Array<{
  value: WizardColumnFormat | 'plain';
  label: string;
}> = [
  { value: 'plain', label: 'Plain text' },
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'currency', label: 'Currency' },
  { value: 'percent', label: 'Percent' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
  { value: 'pill', label: 'Pill' },
  { value: 'bar_indicator', label: 'Bar indicator' },
  { value: 'boolean', label: 'Boolean' },
];

export const WIZARD_METRIC_FORMATS: Array<{
  value: WizardColumnFormat;
  label: string;
}> = [
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'currency', label: 'Currency' },
  { value: 'percent', label: 'Percent' },
];

export type WizardPillVariant =
  | 'success'
  | 'warning'
  | 'muted'
  | 'secondary'
  | 'destructive'
  | 'outline'
  | 'default';

export const WIZARD_PILL_VARIANTS: WizardPillVariant[] = [
  'default',
  'success',
  'warning',
  'destructive',
  'secondary',
  'muted',
  'outline',
];

export type WizardTableColumnType =
  | 'value'
  | 'workflow_button'
  | 'interaction_buttons';

export interface WizardFieldConfig {
  format?: WizardColumnFormat;
  label?: string;
  pillVariants?: Record<string, WizardPillVariant>;
  /** Column variant — only meaningful for table blocks. Defaults to "value". */
  columnType?: WizardTableColumnType;
  /** Workflow-button config when `columnType === 'workflow_button'`. */
  workflowAction?: ReportWorkflowActionConfig;
  /** Interaction buttons when `columnType === 'interaction_buttons'`. */
  interactionButtons?: ReportTableInteractionButtonConfig[];
  /** Opts table cells/card fields into Object Model writeback. */
  editable?: boolean;
  /** Explicit writeback editor config; omitted to let the renderer infer. */
  editor?: ReportEditorConfig;
}

/** Synthetic field key prefix for action/interaction columns that don't bind
 *  to a row field. The editor renders these as table columns without listing
 *  them as schema-backed fields. */
export const WIZARD_ACTION_FIELD_PREFIX = '__action_';

export function makeActionFieldKey(): string {
  return `${WIZARD_ACTION_FIELD_PREFIX}${Math.random()
    .toString(36)
    .slice(2, 9)}`;
}

export function isActionFieldKey(field: string): boolean {
  return field.startsWith(WIZARD_ACTION_FIELD_PREFIX);
}

export interface WizardBlockPlacement {
  gridId: string;
  row: number;
  column: number;
}

export interface WizardBlock {
  id: string;
  type: WizardBlockType;
  title: string;
  /** Per-block data source. Schema name from the Object Model.
   *  Markdown blocks don't need a schema. Mutually exclusive with `dataset`. */
  schema?: string;
  /** Source kind. Undefined/object_model uses the Object Model schema picker. */
  sourceKind?: ReportSource['kind'];
  /** Virtual workflow/system source entity. */
  sourceEntity?: ReportSource['entity'];
  /** Workflow runtime source workflow id. */
  workflowId?: string;
  /** Optional workflow runtime source instance id. */
  instanceId?: string;
  /** Optional system source interval. */
  sourceInterval?: string;
  /** Optional system source granularity. */
  sourceGranularity?: string;
  /** Source-level ordering applied before block rendering. */
  sourceOrderBy?: ReportOrderBy[];
  /** Source-level row limit. */
  sourceLimit?: number;
  /** Optional Object Model joins exposed as alias.field paths. */
  sourceJoins?: ReportSourceJoin[];
  /** Optional source condition DSL. */
  sourceCondition?: ReportCondition;
  fields: string[];
  fieldConfigs?: Record<string, WizardFieldConfig>;
  placement: WizardBlockPlacement;
  chartKind?: ReportChartKind;
  chartGroupBy?: string;
  metricAggregate?: ReportAggregateFn;
  metricField?: string;
  metricDistinct?: boolean;
  metricPercentile?: number;
  metricExpression?: unknown;
  metricFormat?: WizardColumnFormat;
  chartAggregate?: ReportAggregateFn;
  chartAggregateField?: string;
  chartAggregateDistinct?: boolean;
  chartAggregatePercentile?: number;
  chartAggregateExpression?: unknown;
  markdownContent?: string;
  /** Table-block: enables row checkboxes. Forced true while bulk actions exist. */
  selectable?: boolean;
  /** Table-block: bulk action buttons rendered above the table. */
  tableActions?: ReportTableActionConfig[];
  /** Table-block: default sort applied before viewers interact with columns. */
  defaultSort?: ReportOrderBy[];
  /** Table-block: initial page size. */
  defaultPageSize?: number;
  /** Table-block: page-size choices exposed to viewers. */
  allowedPageSizes?: number[];
  /** Defers block data loading until the viewer scrolls near it. */
  lazy?: boolean;
  /** Drops the whole block if render data is empty. */
  hideWhenEmpty?: boolean;
  /** Optional filter-driven visibility rule for the whole block. */
  showWhen?: ReportVisibilityCondition;
  /** Filters rendered inside this block and sent as blockFilters data requests. */
  filters?: ReportFilterDefinition[];
  /** Row/cell/point click interactions emitted by the block renderer. */
  interactions?: ReportInteractionDefinition[];
  /** Pre-aggregated dataset query — when set, the block resolves through
   *  `definition.datasets[dataset.id]` instead of `schema`/`fields`. */
  dataset?: ReportBlockDatasetQuery;
}

export const WIZARD_FILTER_TARGET_ALL = '__all__';
export const WIZARD_FILTER_TARGET_NONE = '__none__';

export type WizardFilterOptionsSource = 'static' | 'object_model';

export interface WizardFilter {
  id: string;
  label: string;
  field: string;
  type: ReportFilterType;
  target: string;
  optionsSource?: WizardFilterOptionsSource;
  staticOptions?: string;
  optionsField?: string;
  required?: boolean;
  defaultValue?: unknown;
  strictWhenReferenced?: boolean;
}

/** A single grid layout section. Stack multiple grids to compose the report. */
export interface WizardGrid {
  id: string;
  /** Optional section title; when set, the grid renders inside a titled section. */
  title?: string;
  /** Optional section description shown beneath the title. */
  description?: string;
  rows: number;
  columns: number;
}

export interface WizardState {
  /** Optional last-used schema; just a hint for new-block defaults. */
  defaultSchema?: string;
  title: string;
  grids: WizardGrid[];
  blocks: WizardBlock[];
  filters: WizardFilter[];
  /** Top-level semantic dataset definitions (Cube/LookML-style). Blocks reference
   *  one via `block.dataset.id` instead of querying a schema directly. */
  datasets: ReportDatasetDefinition[];
  /** Optional named report views with independent layouts and breadcrumbs. */
  views: ReportViewDefinition[];
}

/** Logical anchor that readiness checks point at; used to scroll the editor
 *  to the relevant panel rather than to a step page. */
export type WizardAnchor = 'details' | 'blocks' | 'filters' | 'datasets';

export function makeGridId(): string {
  return `grid_${Math.random().toString(36).slice(2, 9)}`;
}

export function makeDatasetId(): string {
  return `dataset_${Math.random().toString(36).slice(2, 9)}`;
}

export function makeMeasureId(): string {
  return `measure_${Math.random().toString(36).slice(2, 9)}`;
}
