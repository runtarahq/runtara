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
import {
  defaultRenderContext,
  reportDsl,
} from '@/wasm/runtara-report-dsl';

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

/**
 * Evaluate a row visibility/disability condition against a row.
 *
 * Delegates to the WASM `evaluateRowCondition` (single source of truth
 * shared with the server). The condition shape stored in report
 * definitions today is legacy — `{op, arguments: [bare_field, value]}`
 * — so this wrapper converts to canonical `ConditionExpression`
 * before invoking the WASM evaluator. The wire-format migration to
 * canonical lives in a follow-up.
 *
 * Returns false on any conversion or WASM error (e.g. before the
 * bundle has loaded).
 */
export function matchesReportRowCondition(
  condition: ReportRowCondition,
  row: Record<string, unknown>
): boolean {
  if (!condition) return false;
  try {
    const canonical = toCanonicalCondition(condition);
    if (!canonical) return false;
    return reportDsl().evaluateRowCondition(canonical, row);
  } catch {
    return false;
  }
}

type LegacyRowCondition = {
  op: string;
  arguments?: unknown[] | null;
};

type CanonicalExpression =
  | {
      type: 'operation';
      op: string;
      arguments: CanonicalArgument[];
    }
  | CanonicalValueArgument;

type CanonicalArgument = CanonicalExpression | CanonicalValueArgument;

type CanonicalValueArgument =
  | { type: 'value'; valueType: 'reference'; value: string }
  | { type: 'value'; valueType: 'immediate'; value: unknown };

/**
 * Bridge legacy `{op, arguments: [bare_field, value]}` conditions to the
 * canonical `ConditionExpression` shape the WASM evaluator consumes.
 * The first argument is treated as a field reference when it's a string;
 * subsequent arguments are immediate values unless they're nested
 * legacy operations (e.g. AND/OR/NOT).
 */
function toCanonicalCondition(
  condition: LegacyRowCondition | null | undefined
): CanonicalExpression | null {
  if (!condition || typeof condition !== 'object' || !condition.op) {
    return null;
  }
  const op = condition.op.toUpperCase();
  const args = condition.arguments ?? [];

  if (op === 'AND' || op === 'OR') {
    return {
      type: 'operation',
      op,
      arguments: args
        .map(toCanonicalArgument)
        .filter((arg): arg is CanonicalArgument => arg !== null),
    };
  }
  if (op === 'NOT') {
    const inner = toCanonicalArgument(args[0]);
    return inner
      ? { type: 'operation', op: 'NOT', arguments: [inner] }
      : null;
  }
  if (op === 'IN' || op === 'NOT_IN') {
    const fieldArg = legacyFieldArg(args[0]);
    if (!fieldArg) return null;
    return {
      type: 'operation',
      op,
      arguments: [
        fieldArg,
        immediateArg(args[1]),
      ],
    };
  }
  if (op === 'IS_DEFINED' || op === 'IS_EMPTY' || op === 'IS_NOT_EMPTY') {
    const fieldArg = legacyFieldArg(args[0]);
    return fieldArg
      ? { type: 'operation', op, arguments: [fieldArg] }
      : null;
  }
  // Binary comparison operators (EQ, NE, GT, GTE, LT, LTE, CONTAINS, etc.):
  // first arg is a field reference, second is an immediate value.
  const fieldArg = legacyFieldArg(args[0]);
  if (!fieldArg) return null;
  return {
    type: 'operation',
    op,
    arguments: [fieldArg, immediateArg(args[1])],
  };
}

function toCanonicalArgument(arg: unknown): CanonicalArgument | null {
  if (isLegacyOperation(arg)) {
    return toCanonicalCondition(arg);
  }
  if (typeof arg === 'string') return referenceArg(arg);
  return immediateArg(arg);
}

function isLegacyOperation(value: unknown): value is LegacyRowCondition {
  return (
    typeof value === 'object' &&
    value !== null &&
    'op' in value &&
    typeof (value as { op?: unknown }).op === 'string'
  );
}

function legacyFieldArg(arg: unknown): CanonicalValueArgument | null {
  if (typeof arg !== 'string') return null;
  return referenceArg(arg);
}

function referenceArg(path: string): CanonicalValueArgument {
  return { type: 'value', valueType: 'reference', value: path };
}

function immediateArg(value: unknown): CanonicalValueArgument {
  return { type: 'value', valueType: 'immediate', value };
}

export function getReportRowValue(
  row: Record<string, unknown>,
  field: string
): unknown {
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

/**
 * Render a `{{ field | format }}` template string against a row using
 * the WASM-backed minijinja engine. Returns an empty string if the
 * WASM bundle hasn't finished loading yet (the app preloads it at
 * shell mount; this fallback only fires before the first render).
 */
export function renderDisplayTemplate(
  row: Record<string, unknown>,
  template: string
): string {
  try {
    return reportDsl().renderTemplate(template, row, defaultRenderContext());
  } catch {
    return '';
  }
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

/**
 * Format a raw cell value using the WASM-backed formatter (delegates to
 * the FE-side `Intl` callback for locale-aware output). The WASM bundle
 * is preloaded at app shell mount; this passthrough fallback only fires
 * if the bundle hasn't finished loading yet.
 */
export function formatCellValue(
  value: unknown,
  format?: string | null
): string {
  if (value === null || value === undefined) return '';
  try {
    return reportDsl().formatValue(
      value,
      format ?? '',
      defaultRenderContext()
    );
  } catch {
    if (typeof value === 'object') {
      try {
        return JSON.stringify(value);
      } catch {
        return String(value);
      }
    }
    return String(value);
  }
}

export function truncateCellText(
  text: string,
  maxChars?: number | null
): { text: string; title?: string } {
  if (
    typeof maxChars !== 'number' ||
    !Number.isFinite(maxChars) ||
    maxChars <= 0
  ) {
    return { text };
  }

  const limit = Math.trunc(maxChars);
  const chars = Array.from(text);
  if (chars.length <= limit) return { text };

  return {
    text: `${chars.slice(0, limit).join('').trimEnd()}...`,
    title: text,
  };
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
