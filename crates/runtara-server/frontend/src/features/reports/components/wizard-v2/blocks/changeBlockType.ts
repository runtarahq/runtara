// Phase 11 follow-up: switch a block's type in place. The block keeps
// its id, title, visibility, filters, interactions, and dataset
// binding; the previous type's config (markdown.content / table.columns
// / chart.* / metric.* / card.groups / actions.submit) is dropped and
// the new type's required defaults are seeded.
//
// Switching to/from `actions` swaps `source` too because actions blocks
// require source.kind='workflow_runtime' and other types default to
// object_model.

import {
  ReportBlockDefinition,
  ReportBlockType,
  ReportSource,
} from '../../../types';

const TYPE_SPECIFIC_FIELDS: Record<ReportBlockType, keyof ReportBlockDefinition> = {
  markdown: 'markdown',
  table: 'table',
  chart: 'chart',
  metric: 'metric',
  card: 'card',
  actions: 'actions',
};

/** Build a fresh block of `newType` while preserving the cross-cutting
 *  fields from `block`. Used by the in-place type picker. */
export function changeBlockType(
  block: ReportBlockDefinition,
  newType: ReportBlockType
): ReportBlockDefinition {
  if (block.type === newType) return block;

  // Cross-cutting fields kept across the switch.
  const kept: Partial<ReportBlockDefinition> = {
    id: block.id,
    title: block.title,
    showWhen: block.showWhen,
    hideWhenEmpty: block.hideWhenEmpty,
    lazy: block.lazy,
    filters: block.filters,
    interactions: block.interactions,
    dataset: block.dataset,
  };

  // Source: actions blocks have a different shape; transitions to/from
  // actions reset source to a sensible default.
  let source: ReportSource = block.source;
  if (newType === 'actions') {
    source = {
      kind: 'workflow_runtime',
      schema: '',
      entity: 'actions',
      workflowId: '',
      mode: 'filter',
    };
  } else if (block.type === 'actions') {
    source = { kind: 'object_model', schema: '', mode: 'filter' };
  }

  // Strip the previous type's config field. We rebuild the object
  // from `kept` + the new type's default rather than spreading `block`
  // so the old type-specific key never makes it through.
  const next: ReportBlockDefinition = {
    ...kept,
    id: block.id,
    type: newType,
    source,
    ...defaultConfigFor(newType),
  } as ReportBlockDefinition;

  return next;
}

function defaultConfigFor(
  type: ReportBlockType
): Partial<ReportBlockDefinition> {
  switch (type) {
    case 'markdown':
      return { markdown: { content: '' } };
    case 'table':
      return { table: { columns: [] } };
    case 'chart':
      return {
        chart: { kind: 'bar', x: '', series: [] },
      };
    case 'metric':
      return { metric: { valueField: '' } };
    case 'card':
      return { card: { groups: [] } };
    case 'actions':
      return { actions: { submit: {} } };
  }
}

/** Returns `true` when the block has user-authored type-specific config
 *  that would be discarded by switching types. Callers use this to
 *  decide whether to prompt for confirmation. */
export function hasMeaningfulTypeConfig(
  block: ReportBlockDefinition
): boolean {
  const field = TYPE_SPECIFIC_FIELDS[block.type as ReportBlockType];
  const config = (block as unknown as Record<string, unknown>)[field];
  if (config == null) return false;
  switch (block.type) {
    case 'markdown': {
      const content = (config as { content?: unknown }).content;
      return typeof content === 'string' && content.trim().length > 0;
    }
    case 'table': {
      const columns = (config as { columns?: unknown[] }).columns;
      return Array.isArray(columns) && columns.length > 0;
    }
    case 'chart': {
      const c = config as { x?: unknown; series?: unknown[] };
      return Boolean(c.x) || (Array.isArray(c.series) && c.series.length > 0);
    }
    case 'metric': {
      const valueField = (config as { valueField?: unknown }).valueField;
      return typeof valueField === 'string' && valueField.length > 0;
    }
    case 'card': {
      const groups = (config as { groups?: unknown[] }).groups;
      return Array.isArray(groups) && groups.length > 0;
    }
    case 'actions': {
      // Any non-empty workflowId on the source counts as meaningful.
      return Boolean(block.source?.workflowId);
    }
    default:
      return false;
  }
}

/** Human-readable label for the type. Used by the picker UI. */
export function blockTypeLabel(type: ReportBlockType): string {
  switch (type) {
    case 'markdown':
      return 'Markdown';
    case 'table':
      return 'Table';
    case 'chart':
      return 'Chart';
    case 'metric':
      return 'Metric';
    case 'card':
      return 'Card';
    case 'actions':
      return 'Actions';
  }
}

export const BLOCK_TYPES: ReportBlockType[] = [
  'markdown',
  'table',
  'chart',
  'metric',
  'card',
  'actions',
];
