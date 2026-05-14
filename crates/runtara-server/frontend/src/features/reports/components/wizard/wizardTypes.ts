import {
  ReportBlockType,
  ReportChartKind,
  ReportFilterType,
  ReportTableActionConfig,
  ReportTableInteractionButtonConfig,
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
    description: 'Detail view of a single record\'s fields.',
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
   *  Markdown blocks don't need a schema. */
  schema?: string;
  fields: string[];
  fieldConfigs?: Record<string, WizardFieldConfig>;
  placement: WizardBlockPlacement;
  chartKind?: ReportChartKind;
  chartGroupBy?: string;
  metricAggregate?: 'count' | 'sum' | 'avg' | 'min' | 'max';
  metricField?: string;
  metricFormat?: WizardColumnFormat;
  markdownContent?: string;
  /** Table-block: enables row checkboxes. Forced true while bulk actions exist. */
  selectable?: boolean;
  /** Table-block: bulk action buttons rendered above the table. */
  tableActions?: ReportTableActionConfig[];
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
}

/** Logical anchor that readiness checks point at; used to scroll the editor
 *  to the relevant panel rather than to a step page. */
export type WizardAnchor = 'details' | 'blocks' | 'filters';

export function makeGridId(): string {
  return `grid_${Math.random().toString(36).slice(2, 9)}`;
}
