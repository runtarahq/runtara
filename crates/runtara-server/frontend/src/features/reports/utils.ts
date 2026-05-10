import {
  ReportBlockDefinition,
  ReportDefinition,
  ReportFilterDefinition,
  ReportLayoutNode,
  ReportRowCondition,
  ReportViewBreadcrumb,
  ReportViewDefinition,
  ReportVisibilityCondition,
  ReportWorkflowActionConfig,
} from './types';

export const TIME_RANGE_PRESETS = [
  { label: 'Today', value: 'today' },
  { label: 'Yesterday', value: 'yesterday' },
  { label: 'Last 7 days', value: 'last_7_days' },
  { label: 'Last 30 days', value: 'last_30_days' },
  { label: 'This month', value: 'this_month' },
];

export function getFilterDefaultValue(filter: ReportFilterDefinition): unknown {
  if (filter.default !== undefined) {
    if (
      filter.type === 'time_range' &&
      typeof filter.default === 'object' &&
      filter.default !== null &&
      'preset' in filter.default
    ) {
      return String((filter.default as { preset: unknown }).preset);
    }
    return filter.default;
  }

  if (filter.type === 'multi_select') return [];
  if (filter.type === 'checkbox') return false;
  if (filter.type === 'time_range') return 'last_30_days';
  return '';
}

export function encodeFilterValue(value: unknown): string {
  if (Array.isArray(value)) return JSON.stringify(value);
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

export function decodeFilterValue(
  filter: ReportFilterDefinition,
  value: string | null
): unknown {
  if (value === null) return getFilterDefaultValue(filter);

  if (filter.type === 'multi_select') {
    if (value.startsWith('[')) {
      try {
        const parsed = JSON.parse(value);
        return Array.isArray(parsed) ? parsed : [];
      } catch {
        // Fall through to legacy comma-separated URLs.
      }
    }
    return value
      .split(',')
      .map((part) => part.trim())
      .filter(Boolean);
  }

  if (filter.type === 'checkbox') {
    return value === 'true';
  }

  if (value.startsWith('{') || value.startsWith('[')) {
    try {
      return JSON.parse(value);
    } catch {
      return value;
    }
  }

  return value;
}

export function extractLayoutBlockReferences(layout: ReportLayoutNode[] = []) {
  const ids: string[] = [];
  for (const node of layout) {
    collectLayoutBlockReferences(node, ids);
  }
  return ids;
}

function collectLayoutBlockReferences(node: ReportLayoutNode, ids: string[]) {
  if (node.type === 'block') {
    ids.push(node.blockId);
    return;
  }
  if (node.type === 'metric_row') {
    ids.push(...node.blocks);
    return;
  }
  if (node.type === 'section') {
    for (const child of node.children ?? []) {
      collectLayoutBlockReferences(child, ids);
    }
    return;
  }
  if (node.type === 'columns') {
    for (const column of node.columns) {
      for (const child of column.children ?? []) {
        collectLayoutBlockReferences(child, ids);
      }
    }
    return;
  }
  if (node.type === 'grid') {
    ids.push(...node.items.map((item) => item.blockId));
  }
}

export function getActiveReportView(
  definition: ReportDefinition,
  viewId?: string | null
): ReportViewDefinition | undefined {
  const views = definition.views ?? [];
  if (views.length === 0) return undefined;
  if (viewId) {
    const selected = views.find((view) => view.id === viewId);
    if (selected) return selected;
  }
  return views.find((view) => view.id === 'list') ?? views[0];
}

export function getActiveReportLayout(
  definition: ReportDefinition,
  viewId?: string | null
): ReportLayoutNode[] {
  return (
    getActiveReportView(definition, viewId)?.layout ?? definition.layout ?? []
  );
}

export function getDefaultReportViewId(
  definition: ReportDefinition
): string | null {
  return getActiveReportView(definition)?.id ?? null;
}

export function getReportViewBreadcrumbs(
  definition: ReportDefinition,
  view: ReportViewDefinition,
  resolveLabel: (view: ReportViewDefinition) => string | null | undefined
): ReportViewBreadcrumb[] {
  if ((view.breadcrumb?.length ?? 0) > 0) {
    return view.breadcrumb ?? [];
  }

  const viewById = new Map(
    (definition.views ?? []).map((candidate) => [candidate.id, candidate])
  );
  const ancestors: ReportViewDefinition[] = [];
  const seen = new Set([view.id]);
  let current = view;

  while (current.parentViewId) {
    const parent = viewById.get(current.parentViewId);
    if (!parent || seen.has(parent.id)) break;
    ancestors.unshift(parent);
    seen.add(parent.id);
    current = parent;
  }

  return ancestors.map((ancestor, index) => {
    const clearFilters = new Set<string>();
    for (const descendant of [...ancestors.slice(index + 1), view]) {
      for (const filterId of descendant.clearFiltersOnBack ?? []) {
        clearFilters.add(filterId);
      }
    }

    return {
      label: resolveLabel(ancestor) ?? humanizeFieldName(ancestor.id),
      viewId: ancestor.id,
      clearFilters: Array.from(clearFilters),
    };
  });
}

export function getEagerBlocks(
  definition: ReportDefinition,
  filters: Record<string, unknown> = {},
  viewId?: string | null
) {
  const layout = getActiveReportLayout(definition, viewId);
  const visibleBlockIds = new Set(
    layout.length > 0
      ? extractVisibleLayoutBlockReferences(layout, filters)
      : definition.blocks
          .filter((block) => isVisibleByShowWhen(block.showWhen, filters))
          .map((block) => block.id)
  );

  return definition.blocks.filter((block) => {
    if (block.lazy || !isVisibleByShowWhen(block.showWhen, filters)) {
      return false;
    }
    return layout.length === 0 || visibleBlockIds.has(block.id);
  });
}

export function getBlockById(
  definition: ReportDefinition,
  blockId: string
): ReportBlockDefinition | undefined {
  return definition.blocks.find((block) => block.id === blockId);
}

export function isVisibleByShowWhen(
  showWhen: ReportVisibilityCondition | undefined,
  filters: Record<string, unknown>
): boolean {
  if (!showWhen) return true;

  const value = filters[showWhen.filter];
  const hasValue = !isEmptyVisibilityValue(value);

  if (showWhen.exists !== undefined && hasValue !== showWhen.exists) {
    return false;
  }
  if (
    showWhen.equals !== undefined &&
    JSON.stringify(value) !== JSON.stringify(showWhen.equals)
  ) {
    return false;
  }
  if (
    showWhen.notEquals !== undefined &&
    JSON.stringify(value) === JSON.stringify(showWhen.notEquals)
  ) {
    return false;
  }
  return true;
}

export function isWorkflowActionVisible(
  action: ReportWorkflowActionConfig,
  row: Record<string, unknown>
): boolean {
  if (
    action.visibleWhen &&
    !matchesReportRowCondition(action.visibleWhen, row)
  ) {
    return false;
  }
  if (action.hiddenWhen && matchesReportRowCondition(action.hiddenWhen, row)) {
    return false;
  }
  return true;
}

export function isWorkflowActionDisabled(
  action: ReportWorkflowActionConfig,
  row: Record<string, unknown>
): boolean {
  return action.disabledWhen
    ? matchesReportRowCondition(action.disabledWhen, row)
    : false;
}

export function matchesReportRowCondition(
  condition: ReportRowCondition,
  row: Record<string, unknown>
): boolean {
  const op = condition.op.toUpperCase();
  const args = condition.arguments ?? [];

  switch (op) {
    case 'AND':
      return args.every((argument) =>
        isReportRowCondition(argument)
          ? matchesReportRowCondition(argument, row)
          : false
      );
    case 'OR':
      return args.some((argument) =>
        isReportRowCondition(argument)
          ? matchesReportRowCondition(argument, row)
          : false
      );
    case 'NOT':
      return isReportRowCondition(args[0])
        ? !matchesReportRowCondition(args[0], row)
        : false;
    case 'EQ':
      return compareConditionValues(
        rowConditionOperand(args[0], row, true),
        rowConditionOperand(args[1], row, false)
      ).equal;
    case 'NE':
      return !compareConditionValues(
        rowConditionOperand(args[0], row, true),
        rowConditionOperand(args[1], row, false)
      ).equal;
    case 'GT':
      return (
        compareConditionValues(
          rowConditionOperand(args[0], row, true),
          rowConditionOperand(args[1], row, false)
        ).ordering === 1
      );
    case 'GTE': {
      const comparison = compareConditionValues(
        rowConditionOperand(args[0], row, true),
        rowConditionOperand(args[1], row, false)
      );
      return comparison.equal || comparison.ordering === 1;
    }
    case 'LT':
      return (
        compareConditionValues(
          rowConditionOperand(args[0], row, true),
          rowConditionOperand(args[1], row, false)
        ).ordering === -1
      );
    case 'LTE': {
      const comparison = compareConditionValues(
        rowConditionOperand(args[0], row, true),
        rowConditionOperand(args[1], row, false)
      );
      return comparison.equal || comparison.ordering === -1;
    }
    case 'IN': {
      const value = rowConditionOperand(args[0], row, true);
      return Array.isArray(args[1])
        ? args[1].some((candidate) => conditionValuesEqual(value, candidate))
        : false;
    }
    case 'NOT_IN': {
      const value = rowConditionOperand(args[0], row, true);
      return Array.isArray(args[1])
        ? !args[1].some((candidate) => conditionValuesEqual(value, candidate))
        : false;
    }
    case 'CONTAINS': {
      const value = rowConditionOperand(args[0], row, true);
      return typeof value === 'string' && typeof args[1] === 'string'
        ? value.includes(args[1])
        : false;
    }
    case 'IS_DEFINED':
      return rowConditionOperand(args[0], row, true) !== null;
    case 'IS_EMPTY':
      return isEmptyConditionValue(rowConditionOperand(args[0], row, true));
    case 'IS_NOT_EMPTY':
      return !isEmptyConditionValue(rowConditionOperand(args[0], row, true));
    default:
      return false;
  }
}

function isReportRowCondition(value: unknown): value is ReportRowCondition {
  return (
    typeof value === 'object' &&
    value !== null &&
    'op' in value &&
    typeof (value as { op?: unknown }).op === 'string'
  );
}

function rowConditionOperand(
  argument: unknown,
  row: Record<string, unknown>,
  fieldRef: boolean
) {
  if (fieldRef && typeof argument === 'string') {
    return rowValue(row, argument) ?? null;
  }
  return argument ?? null;
}

function rowValue(row: Record<string, unknown>, field: string): unknown {
  if (Object.prototype.hasOwnProperty.call(row, field)) {
    return row[field];
  }

  let current: unknown = row;
  for (const part of field.split('.')) {
    if (current === null || current === undefined) return undefined;
    if (Array.isArray(current)) {
      const index = Number(part);
      if (!Number.isInteger(index)) return undefined;
      current = current[index];
      continue;
    }
    if (typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}

function compareConditionValues(left: unknown, right: unknown) {
  return {
    equal: conditionValuesEqual(left, right),
    ordering: conditionValueOrdering(left, right),
  };
}

function conditionValuesEqual(left: unknown, right: unknown): boolean {
  if (typeof left === 'number' && typeof right === 'number') {
    return left === right;
  }
  return JSON.stringify(left) === JSON.stringify(right);
}

function conditionValueOrdering(
  left: unknown,
  right: unknown
): -1 | 0 | 1 | null {
  if (left === null && right === null) return 0;
  if (left === null) return 1;
  if (right === null) return -1;
  if (typeof left === 'number' && typeof right === 'number') {
    if (left === right) return 0;
    return left > right ? 1 : -1;
  }
  if (typeof left === 'string' && typeof right === 'string') {
    return compareStrings(left, right);
  }
  if (typeof left === 'boolean' && typeof right === 'boolean') {
    if (left === right) return 0;
    return left ? 1 : -1;
  }
  return compareStrings(
    conditionValueSortKey(left),
    conditionValueSortKey(right)
  );
}

function compareStrings(left: string, right: string): -1 | 0 | 1 {
  const result = left.localeCompare(right);
  if (result === 0) return 0;
  return result > 0 ? 1 : -1;
}

function conditionValueSortKey(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') {
    return String(value);
  }
  return JSON.stringify(value);
}

function isEmptyConditionValue(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'string') return value.trim().length === 0;
  if (Array.isArray(value)) return value.length === 0;
  if (typeof value === 'object') return Object.keys(value).length === 0;
  return false;
}

function extractVisibleLayoutBlockReferences(
  layout: ReportLayoutNode[],
  filters: Record<string, unknown>
) {
  const ids: string[] = [];
  for (const node of layout) {
    collectVisibleLayoutBlockReferences(node, filters, ids);
  }
  return ids;
}

function collectVisibleLayoutBlockReferences(
  node: ReportLayoutNode,
  filters: Record<string, unknown>,
  ids: string[]
) {
  if (!isVisibleByShowWhen(node.showWhen, filters)) return;
  if (node.type === 'block') {
    ids.push(node.blockId);
    return;
  }
  if (node.type === 'metric_row') {
    ids.push(...node.blocks);
    return;
  }
  if (node.type === 'section') {
    for (const child of node.children ?? []) {
      collectVisibleLayoutBlockReferences(child, filters, ids);
    }
    return;
  }
  if (node.type === 'columns') {
    for (const column of node.columns) {
      for (const child of column.children ?? []) {
        collectVisibleLayoutBlockReferences(child, filters, ids);
      }
    }
    return;
  }
  if (node.type === 'grid') {
    ids.push(...node.items.map((item) => item.blockId));
  }
}

function isEmptyVisibilityValue(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'string') return value.trim().length === 0;
  if (Array.isArray(value)) return value.length === 0;
  return false;
}

export function formatCellValue(value: unknown, format?: string): string {
  if (value === null || value === undefined) return '';

  if (format === 'currency' && typeof value === 'number') {
    return new Intl.NumberFormat(undefined, {
      style: 'currency',
      currency: 'USD',
      maximumFractionDigits: 2,
    }).format(value);
  }

  if (format === 'currency_compact' && typeof value === 'number') {
    return new Intl.NumberFormat(undefined, {
      style: 'currency',
      currency: 'USD',
      notation: 'compact',
      maximumFractionDigits: value < 1_000_000 ? 1 : 0,
    }).format(value);
  }

  if (format === 'number' && typeof value === 'number') {
    return new Intl.NumberFormat(undefined, {
      maximumFractionDigits: 0,
    }).format(value);
  }

  if (format === 'decimal' && typeof value === 'number') {
    return new Intl.NumberFormat(undefined, {
      maximumFractionDigits: 2,
    }).format(value);
  }

  if (format === 'percent' && typeof value === 'number') {
    return new Intl.NumberFormat(undefined, {
      style: 'percent',
      maximumFractionDigits: 2,
    }).format(value);
  }

  if (format === 'datetime' && typeof value === 'string') {
    const date = new Date(value);
    if (!Number.isNaN(date.getTime())) return date.toLocaleString();
  }

  if (format === 'date' && typeof value === 'string') {
    const date = new Date(value);
    if (!Number.isNaN(date.getTime())) return date.toLocaleDateString();
  }

  if (format === 'string') {
    return String(value);
  }

  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

export function humanizeFieldName(field: string): string {
  return field
    .split(/[_-]/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

export function slugify(value: string): string {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}
