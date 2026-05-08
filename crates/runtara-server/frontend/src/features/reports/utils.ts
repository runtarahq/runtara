import {
  ReportBlockDefinition,
  ReportDefinition,
  ReportFilterDefinition,
  ReportLayoutNode,
  ReportViewDefinition,
  ReportVisibilityCondition,
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

export function extractBlockPlaceholders(markdown: string): string[] {
  const ids: string[] = [];
  const re = /\{\{\s*block\.([a-zA-Z0-9_-]+)\s*\}\}/g;
  let match = re.exec(markdown);

  while (match) {
    ids.push(match[1]);
    match = re.exec(markdown);
  }

  return ids;
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
