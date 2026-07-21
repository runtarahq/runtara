import {
  ReportBlockDefinition,
  ReportCondition,
  ReportDefinition,
  ReportFilterDefinition,
  ReportGridLayoutNode,
  ReportRowCondition,
  ReportViewBreadcrumb,
  ReportViewDefinition,
  ReportVisibilityCondition,
  ReportWorkflowActionConfig,
} from './types';
import {
  defaultRenderContext,
  reportDsl,
} from '@/wasm/runtara-report-dsl/index';
import { evaluateCanonicalCondition } from '@/shared/lib/rust-validation-wasm';

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
): ReportGridLayoutNode {
  const view = getActiveReportView(definition, viewId);
  return view?.layout ?? definition.layout;
}

export function getDefaultReportViewId(
  definition: ReportDefinition
): string | null {
  return getActiveReportView(definition)?.id ?? null;
}

/** Default URL/render target. Stage groups use their group id so the server
 * resolves the persisted current stage instead of treating the first stage
 * as an intentional historical deep link. */
export function getDefaultReportViewTarget(
  definition: ReportDefinition
): string | null {
  const viewId = getDefaultReportViewId(definition);
  if (!viewId) return null;
  const stageGroup = (definition.viewGroups ?? []).find(
    (group) =>
      group.mode === 'stages' &&
      (group.stages ?? []).some((stage) => stage.viewId === viewId)
  );
  return stageGroup?.id ?? viewId;
}

export function getReportViewGroupViewIds(
  definition: ReportDefinition,
  groupId: string
): string[] {
  const group = (definition.viewGroups ?? []).find(
    (candidate) => candidate.id === groupId
  );
  if (!group) return [];
  return group.mode === 'stages'
    ? (group.stages ?? []).map((stage) => stage.viewId)
    : (group.viewIds ?? []);
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

export function getBlockById(
  definition: ReportDefinition,
  blockId: string
): ReportBlockDefinition | undefined {
  return definition.blocks.find((block) => block.id === blockId);
}

export function isVisibleByShowWhen(
  showWhen: ReportVisibilityCondition | null | undefined,
  filters: Record<string, unknown>
): boolean {
  if (!showWhen) return true;
  const condition = reportVisibilityToCanonicalCondition(showWhen);
  return condition ? matchesReportRowCondition(condition, filters) : true;
}

/** Normalize the persisted legacy report visibility shape into the canonical
 * condition vocabulary used by the shared condition editor/evaluator. */
export function reportVisibilityToCanonicalCondition(
  visibility: ReportVisibilityCondition | null | undefined
): ReportRowCondition | undefined {
  if (!visibility?.filter) return undefined;
  const conditions: ReportCondition[] = [];
  if (visibility.equals !== undefined) {
    conditions.push({
      op: 'EQ',
      arguments: [visibility.filter, visibility.equals],
    });
  }
  if (visibility.notEquals !== undefined) {
    conditions.push({
      op: 'NE',
      arguments: [visibility.filter, visibility.notEquals],
    });
  }
  if (visibility.exists !== undefined) {
    const defined: ReportCondition = {
      op: 'IS_DEFINED',
      arguments: [visibility.filter],
    };
    conditions.push(
      visibility.exists ? defined : { op: 'NOT', arguments: [defined] }
    );
  }
  if (conditions.length === 0) return undefined;
  return legacyToCanonicalCondition(
    conditions.length === 1
      ? conditions[0]
      : { op: 'AND', arguments: conditions }
  );
}

/** Convert editor output back to the persisted legacy report visibility
 * shape while that wire format remains supported. */
export function canonicalConditionToReportVisibility(
  condition: ReportRowCondition | null | undefined
): ReportVisibilityCondition | undefined {
  const legacy = canonicalToLegacyCondition(condition);
  if (!legacy) return undefined;
  const clauses = legacy.op === 'AND' ? (legacy.arguments ?? []) : [legacy];
  const visibility: ReportVisibilityCondition = { filter: '' };
  for (const clause of clauses) {
    if (!clause || typeof clause !== 'object' || !('op' in clause))
      return undefined;
    const item = clause as ReportCondition;
    const op = item.op.toUpperCase();
    if (
      op === 'NOT' &&
      typeof item.arguments?.[0] === 'object' &&
      (item.arguments[0] as ReportCondition).op === 'IS_DEFINED'
    ) {
      const inner = item.arguments[0] as ReportCondition;
      if (typeof inner.arguments?.[0] !== 'string') return undefined;
      if (visibility.filter && visibility.filter !== inner.arguments[0])
        return undefined;
      visibility.filter = inner.arguments[0];
      visibility.exists = false;
      continue;
    }
    const field = item.arguments?.[0];
    if (typeof field !== 'string') return undefined;
    if (visibility.filter && visibility.filter !== field) return undefined;
    visibility.filter = field;
    if (op === 'EQ') visibility.equals = item.arguments?.[1];
    else if (op === 'NE') visibility.notEquals = item.arguments?.[1];
    else if (op === 'IS_DEFINED') visibility.exists = true;
    else return undefined;
  }
  return visibility.filter ? visibility : undefined;
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
 * Evaluate a canonical `ConditionExpression` row visibility/disability
 * condition against a row. Delegates to the domain-neutral validation WASM
 * — the same evaluator the server uses. Returns false on any error
 * (e.g. before the bundle has loaded).
 */
export function matchesReportRowCondition(
  condition: ReportRowCondition,
  row: Record<string, unknown>
): boolean {
  if (!condition) return false;
  try {
    return evaluateCanonicalCondition(condition, row);
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Legacy ⇄ canonical row-condition bridges. The FE row-condition editor UI
// is built around the flat legacy shape `{op, arguments: [field, value]}`;
// the wire format and the WASM evaluator both use the canonical
// `ConditionExpression` shape. These helpers convert at the editor
// boundary so the UI doesn't have to know about the canonical form.
//
// Both forms are loose at this layer: legacy is the existing
// `ReportCondition`; canonical is treated as opaque structured data so we
// don't drag the full schemars-emitted union types through every helper.
// ---------------------------------------------------------------------------

type AnyJson = Record<string, unknown>;

function asObject(value: unknown): AnyJson | null {
  return typeof value === 'object' && value !== null
    ? (value as AnyJson)
    : null;
}

/**
 * Convert a canonical `ConditionExpression` into the legacy
 * `{op, arguments: [field, value]}` shape used by the rules editor.
 * Returns undefined for canonical shapes that don't fit the rules UI
 * (e.g. nested expressions in non-AND/OR positions).
 */
export function canonicalToLegacyCondition(
  expr: ReportRowCondition | null | undefined
): ReportCondition | undefined {
  const operation = asObject(expr);
  if (!operation || operation.type !== 'operation') return undefined;
  const op = typeof operation.op === 'string' ? operation.op.toUpperCase() : '';
  if (!op) return undefined;
  const args = Array.isArray(operation.arguments) ? operation.arguments : [];

  if (op === 'AND' || op === 'OR') {
    const legacyArgs = args
      .map((arg) => canonicalToLegacyCondition(arg as ReportRowCondition))
      .filter((arg): arg is ReportCondition => Boolean(arg));
    return { op, arguments: legacyArgs };
  }
  if (op === 'NOT') {
    const inner = args[0];
    const legacyInner = canonicalToLegacyCondition(inner as ReportRowCondition);
    return legacyInner ? { op, arguments: [legacyInner] } : undefined;
  }
  if (op === 'IN' || op === 'NOT_IN') {
    const field = readReferencePath(args[0]);
    if (!field) return undefined;
    return { op, arguments: [field, readImmediateValue(args[1])] };
  }
  if (op === 'IS_DEFINED' || op === 'IS_EMPTY' || op === 'IS_NOT_EMPTY') {
    const field = readReferencePath(args[0]);
    return field ? { op, arguments: [field] } : undefined;
  }
  // Binary comparison: EQ, NE, GT, GTE, LT, LTE, CONTAINS, STARTS_WITH, ...
  const field = readReferencePath(args[0]);
  if (!field) return undefined;
  return { op, arguments: [field, readImmediateValue(args[1])] };
}

/**
 * Convert a legacy `{op, arguments: [field, value]}` condition into
 * canonical `ConditionExpression`. Returns undefined when the condition
 * is empty or malformed.
 */
export function legacyToCanonicalCondition(
  condition: ReportCondition | null | undefined
): ReportRowCondition | undefined {
  if (!condition || typeof condition !== 'object' || !condition.op) {
    return undefined;
  }
  const op = condition.op.toUpperCase();
  const args = condition.arguments ?? [];

  if (op === 'AND' || op === 'OR') {
    const canonicalArgs = args
      .filter(
        (arg): arg is ReportCondition =>
          typeof arg === 'object' &&
          arg !== null &&
          'op' in (arg as object) &&
          typeof (arg as { op?: unknown }).op === 'string'
      )
      .map((arg) => legacyToCanonicalCondition(arg))
      .filter((arg): arg is ReportRowCondition => Boolean(arg));
    return makeOperation(op, canonicalArgs);
  }
  if (op === 'NOT') {
    const inner = args[0];
    if (!inner || typeof inner !== 'object' || !('op' in (inner as object))) {
      return undefined;
    }
    const canonicalInner = legacyToCanonicalCondition(inner as ReportCondition);
    if (!canonicalInner) return undefined;
    return makeOperation(op, [canonicalInner]);
  }
  if (op === 'IN' || op === 'NOT_IN') {
    const field = readEditorFieldPath(args[0]);
    if (field === undefined) return undefined;
    return makeOperation(op, [
      makeReferenceArg(field),
      makeImmediateArg(unwrapEditorImmediate(args[1])),
    ]);
  }
  if (op === 'IS_DEFINED' || op === 'IS_EMPTY' || op === 'IS_NOT_EMPTY') {
    const field = readEditorFieldPath(args[0]);
    if (field === undefined) return undefined;
    return makeOperation(op, [makeReferenceArg(field)]);
  }
  // Binary comparison: first arg = field, second = literal.
  const field = readEditorFieldPath(args[0]);
  if (field === undefined) return undefined;
  return makeOperation(op, [
    makeReferenceArg(field),
    makeImmediateArg(unwrapEditorImmediate(args[1])),
  ]);
}

/**
 * Extract a field path from a condition editor's first argument.
 *
 * The shared `ConditionEditor` emits the field as a
 * `{valueType:'reference', value}` object once the user picks it via the
 * variable picker (or as `{valueType:'immediate', value}` for a field it
 * loaded as a plain string and the user re-touched); `RowConditionRow` emits
 * a plain string. Reading the string, or the object's string `value`, covers
 * every editor shape — a nested `Condition` (`op` present) or a non-string
 * value still yields `undefined` so genuinely malformed args reject.
 *
 * Before this, the binary/IN/IS_DEFINED branches required `typeof args[0] ===
 * 'string'`, so any field set through the picker converted to `undefined` and
 * the caller dropped the block's existing `showWhen`.
 */
function readEditorFieldPath(arg: unknown): string | undefined {
  if (typeof arg === 'string') return arg;
  const o = asObject(arg);
  if (o && !('op' in o) && typeof o.value === 'string') return o.value;
  return undefined;
}

/**
 * Unwrap a condition editor's second (value) argument to its literal.
 *
 * The `ConditionEditor` emits an edited immediate as a
 * `{valueType:'immediate', value}` object. Passing that straight into
 * `makeImmediateArg` double-wraps it — the canonical arg's `.value` becomes
 * the whole `{valueType,value}` object, which then round-trips back into a
 * corrupt `equals`/`notEquals` via `canonicalConditionToReportVisibility`.
 * Raw literals (from `RowConditionRow`) pass through untouched.
 */
function unwrapEditorImmediate(arg: unknown): unknown {
  const o = asObject(arg);
  if (o && !('op' in o) && 'valueType' in o && 'value' in o) {
    return o.value;
  }
  return arg;
}

function readReferencePath(arg: unknown): string | undefined {
  const o = asObject(arg);
  if (!o) return undefined;
  if (o.type !== 'value' && o.valueType === undefined) return undefined;
  if (o.valueType !== 'reference') return undefined;
  return typeof o.value === 'string' ? o.value : undefined;
}

function readImmediateValue(arg: unknown): unknown {
  const o = asObject(arg);
  if (!o) return '';
  if (o.valueType !== 'immediate') return '';
  return o.value ?? '';
}

function makeOperation(op: string, args: unknown[]): ReportRowCondition {
  return {
    type: 'operation',
    op,
    arguments: args,
  } as unknown as ReportRowCondition;
}

function makeReferenceArg(path: string): unknown {
  return { type: 'value', valueType: 'reference', value: path };
}

function makeImmediateArg(value: unknown): unknown {
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
    return reportDsl().formatValue(value, format ?? '', defaultRenderContext());
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
