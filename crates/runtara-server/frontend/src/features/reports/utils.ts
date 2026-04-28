import {
  ReportBlockDefinition,
  ReportDefinition,
  ReportFilterDefinition,
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
  if (Array.isArray(value)) return value.join(',');
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

export function decodeFilterValue(
  filter: ReportFilterDefinition,
  value: string | null
): unknown {
  if (value === null) return getFilterDefaultValue(filter);

  if (filter.type === 'multi_select') {
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

export function getEagerBlocks(definition: ReportDefinition) {
  return definition.blocks.filter((block) => !block.lazy);
}

export function getBlockById(
  definition: ReportDefinition,
  blockId: string
): ReportBlockDefinition | undefined {
  return definition.blocks.find((block) => block.id === blockId);
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
