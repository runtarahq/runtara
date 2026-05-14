import {
  ReportBlockDefinition,
  ReportDatasetDefinition,
  ReportDefinition,
  ReportFilterDefinition,
  ReportFilterOptionsConfig,
  ReportLayoutNode,
  ReportSource,
  ReportTableColumn,
} from '../../types';
import { reconcileDatasetBlock } from '../../datasetBlocks';
import {
  WIZARD_FILTER_TARGET_ALL,
  WIZARD_FILTER_TARGET_CUSTOM,
  WIZARD_FILTER_TARGET_NONE,
  WizardBlock,
  WizardBlockType,
  WizardColumnFormat,
  WizardFieldConfig,
  WizardFilter,
  WizardGrid,
  WizardPillVariant,
  WizardState,
  isActionFieldKey,
  makeActionFieldKey,
  makeGridId,
} from './wizardTypes';

const WIZARD_SUPPORTED_BLOCK_TYPES: ReadonlySet<WizardBlockType> = new Set([
  'markdown',
  'metric',
  'chart',
  'table',
  'card',
]);

export interface WizardCompatibility {
  fullyEditable: boolean;
  reasons: string[];
}

function safeFields(
  fields: string[] | undefined,
  fallback: string[]
): string[] {
  const filtered = (fields ?? []).filter(Boolean);
  if (filtered.length > 0) return filtered;
  return fallback;
}

function fieldConfig(
  block: WizardBlock,
  field: string
): WizardFieldConfig | undefined {
  return block.fieldConfigs?.[field];
}

function buildSourceBase(
  block: WizardBlock
): Pick<
  ReportSource,
  | 'kind'
  | 'schema'
  | 'entity'
  | 'workflowId'
  | 'instanceId'
  | 'interval'
  | 'granularity'
  | 'orderBy'
  | 'limit'
  | 'join'
  | 'condition'
> {
  const extras = {
    ...(block.sourceJoins && block.sourceJoins.length > 0
      ? { join: block.sourceJoins }
      : {}),
    ...(block.sourceCondition ? { condition: block.sourceCondition } : {}),
    ...(block.sourceOrderBy && block.sourceOrderBy.length > 0
      ? { orderBy: block.sourceOrderBy }
      : {}),
    ...(block.sourceLimit && block.sourceLimit > 0
      ? { limit: block.sourceLimit }
      : {}),
    ...(block.sourceInterval ? { interval: block.sourceInterval } : {}),
    ...(block.sourceGranularity
      ? { granularity: block.sourceGranularity }
      : {}),
  };
  if (block.sourceKind === 'workflow_runtime') {
    return {
      kind: 'workflow_runtime',
      schema: '',
      entity: block.sourceEntity ?? 'instances',
      workflowId: block.workflowId ?? '',
      ...(block.instanceId ? { instanceId: block.instanceId } : {}),
      ...(block.sourceCondition ? { condition: block.sourceCondition } : {}),
      ...(block.sourceOrderBy && block.sourceOrderBy.length > 0
        ? { orderBy: block.sourceOrderBy }
        : {}),
      ...(block.sourceLimit && block.sourceLimit > 0
        ? { limit: block.sourceLimit }
        : {}),
    };
  }
  if (block.sourceKind === 'system') {
    return {
      kind: 'system',
      schema: '',
      entity: block.sourceEntity ?? 'runtime_system_snapshot',
      ...(block.sourceInterval ? { interval: block.sourceInterval } : {}),
      ...(block.sourceGranularity
        ? { granularity: block.sourceGranularity }
        : {}),
      ...(block.sourceCondition ? { condition: block.sourceCondition } : {}),
      ...(block.sourceOrderBy && block.sourceOrderBy.length > 0
        ? { orderBy: block.sourceOrderBy }
        : {}),
      ...(block.sourceLimit && block.sourceLimit > 0
        ? { limit: block.sourceLimit }
        : {}),
    };
  }
  return { schema: block.schema ?? '', ...extras };
}

function aggregateNeedsField(op: string | undefined): boolean {
  return Boolean(op && op !== 'count' && op !== 'expr');
}

function buildBlockSource(
  block: WizardBlock,
  primaryFields: string[]
): ReportSource {
  if (block.type === 'markdown') {
    return { schema: '', mode: 'filter' };
  }

  const sourceBase = buildSourceBase(block);

  if (block.type === 'metric') {
    const op = block.metricAggregate ?? 'count';
    const field = aggregateNeedsField(op)
      ? block.metricField || primaryFields[0]
      : undefined;
    return {
      ...sourceBase,
      mode: 'aggregate',
      aggregates: [
        {
          alias: 'value',
          op,
          ...(field ? { field } : {}),
          ...(op !== 'expr' && block.metricDistinct ? { distinct: true } : {}),
          ...(block.metricPercentile !== undefined
            ? { percentile: block.metricPercentile }
            : {}),
          ...(block.metricExpression !== undefined
            ? { expression: block.metricExpression }
            : {}),
        },
      ],
    };
  }

  if (block.type === 'chart') {
    const groupBy = block.chartGroupBy || primaryFields[0];
    const op = block.chartAggregate ?? 'count';
    const field = aggregateNeedsField(op)
      ? block.chartAggregateField || primaryFields[0]
      : undefined;
    return {
      ...sourceBase,
      mode: 'aggregate',
      groupBy: [groupBy],
      aggregates: [
        {
          alias: 'value',
          op,
          ...(field ? { field } : {}),
          ...(op !== 'expr' && block.chartAggregateDistinct
            ? { distinct: true }
            : {}),
          ...(block.chartAggregatePercentile !== undefined
            ? { percentile: block.chartAggregatePercentile }
            : {}),
          ...(block.chartAggregateExpression !== undefined
            ? { expression: block.chartAggregateExpression }
            : {}),
        },
      ],
    };
  }

  return { ...sourceBase, mode: 'filter' };
}

function normalizePageSize(value: number | undefined): number {
  return Number.isFinite(value) && value !== undefined && value > 0
    ? Math.floor(value)
    : 50;
}

function normalizeAllowedPageSizes(
  values: number[] | undefined,
  defaultPageSize: number
): number[] {
  const sizes = values && values.length > 0 ? values : [25, 50, 100];
  return Array.from(new Set([...sizes, defaultPageSize]))
    .filter((size) => Number.isFinite(size) && size > 0)
    .map((size) => Math.floor(size))
    .sort((left, right) => left - right);
}

function buildBlockDefinition(
  block: WizardBlock,
  primaryFields: string[],
  datasetsById: Map<string, ReportDatasetDefinition>
): ReportBlockDefinition {
  // Dataset blocks short-circuit: the dataset query drives the table columns /
  // chart series / metric value field via reconcileDatasetBlock.
  if (block.dataset) {
    const dataset = datasetsById.get(block.dataset.id);
    const stub: ReportBlockDefinition = {
      id: block.id,
      type:
        block.type === 'markdown' || block.type === 'card'
          ? 'table'
          : block.type,
      title: block.title,
      source: { schema: '' },
      dataset: block.dataset,
      ...(block.lazy ? { lazy: true } : {}),
      ...(block.hideWhenEmpty ? { hideWhenEmpty: true } : {}),
      ...(block.showWhen ? { showWhen: block.showWhen } : {}),
      ...(block.filters && block.filters.length > 0
        ? { filters: block.filters }
        : {}),
      ...(block.interactions && block.interactions.length > 0
        ? { interactions: block.interactions }
        : {}),
    };
    return dataset ? reconcileDatasetBlock(stub, dataset, block.dataset) : stub;
  }

  const base: ReportBlockDefinition = {
    id: block.id,
    type: block.type,
    title: block.title,
    source: buildBlockSource(block, primaryFields),
    ...(block.lazy ? { lazy: true } : {}),
    ...(block.hideWhenEmpty ? { hideWhenEmpty: true } : {}),
    ...(block.showWhen ? { showWhen: block.showWhen } : {}),
    ...(block.filters && block.filters.length > 0
      ? { filters: block.filters }
      : {}),
    ...(block.interactions && block.interactions.length > 0
      ? { interactions: block.interactions }
      : {}),
  };

  if (block.type === 'markdown') {
    return {
      ...base,
      markdown: {
        content:
          block.markdownContent || (block.title ? `# ${block.title}` : ''),
      },
    };
  }

  if (block.type === 'metric') {
    return {
      ...base,
      metric: {
        valueField: 'value',
        label: block.title,
        format: block.metricFormat ?? 'number',
      },
    };
  }

  if (block.type === 'chart') {
    const groupBy = block.chartGroupBy || primaryFields[0] || 'id';
    const fields = safeFields(block.fields, primaryFields);
    return {
      ...base,
      chart: {
        kind: block.chartKind ?? 'bar',
        x: groupBy,
        series: fields.map((field) => {
          const cfg = fieldConfig(block, field);
          return {
            field: field || 'value',
            label: cfg?.label || humanize(field || 'value'),
          };
        }),
      },
    };
  }

  if (block.type === 'table') {
    const rawFields = safeFields(block.fields, primaryFields).slice(0, 12);
    const columns: ReportTableColumn[] = rawFields.map((field) => {
      const cfg = fieldConfig(block, field);
      const columnType = cfg?.columnType ?? 'value';
      const isAction = isActionFieldKey(field);
      // Action columns don't bind to a row field — keep field stable for
      // round-trip but drop format/pill metadata that doesn't apply.
      const column: ReportTableColumn = {
        field,
        label: cfg?.label || (isAction ? '' : humanize(field)),
      };
      if (columnType === 'workflow_button') {
        column.type = 'workflow_button';
        if (cfg?.workflowAction) column.workflowAction = cfg.workflowAction;
        return column;
      }
      if (columnType === 'interaction_buttons') {
        column.type = 'interaction_buttons';
        if (cfg?.interactionButtons && cfg.interactionButtons.length > 0) {
          column.interactionButtons = cfg.interactionButtons;
        }
        return column;
      }
      if (cfg?.editable) column.editable = true;
      if (cfg?.editor) column.editor = cfg.editor;
      if (cfg?.format) column.format = cfg.format;
      if (cfg?.format === 'pill' && cfg.pillVariants) {
        column.pillVariants = cfg.pillVariants;
      }
      if (cfg?.displayField) column.displayField = cfg.displayField;
      if (cfg?.displayTemplate) column.displayTemplate = cfg.displayTemplate;
      if (cfg?.secondaryField) column.secondaryField = cfg.secondaryField;
      if (cfg?.linkField) column.linkField = cfg.linkField;
      if (cfg?.tooltipField) column.tooltipField = cfg.tooltipField;
      if (cfg?.levels && cfg.levels.length > 0) column.levels = cfg.levels;
      if (cfg?.align) column.align = cfg.align;
      if (cfg?.maxChars !== undefined) column.maxChars = cfg.maxChars;
      if (cfg?.descriptive) column.descriptive = true;
      return column;
    });

    const tableActions = block.tableActions ?? [];
    const selectable = Boolean(block.selectable || tableActions.length > 0);
    const defaultPageSize = normalizePageSize(block.defaultPageSize);
    const allowedPageSizes = normalizeAllowedPageSizes(
      block.allowedPageSizes,
      defaultPageSize
    );

    return {
      ...base,
      table: {
        columns,
        ...(selectable ? { selectable: true } : {}),
        ...(tableActions.length > 0 ? { actions: tableActions } : {}),
        ...(block.defaultSort && block.defaultSort.length > 0
          ? { defaultSort: block.defaultSort }
          : {}),
        pagination: {
          defaultPageSize,
          allowedPageSizes,
        },
      },
    };
  }

  // card
  if (block.cardConfig) {
    return {
      ...base,
      card: block.cardConfig,
    };
  }

  const cardFields = safeFields(block.fields, primaryFields).slice(0, 12);
  return {
    ...base,
    card: {
      groups: [
        {
          id: 'main',
          fields: cardFields.map((field) => {
            const cfg = fieldConfig(block, field);
            return {
              field,
              label: cfg?.label || humanize(field),
              ...(cfg?.format ? { format: cfg.format } : {}),
              ...(cfg?.format === 'pill' && cfg.pillVariants
                ? { pillVariants: cfg.pillVariants }
                : {}),
              ...(cfg?.editable ? { editable: true } : {}),
              ...(cfg?.editor ? { editor: cfg.editor } : {}),
            };
          }),
        },
      ],
    },
  };
}

function humanize(value: string): string {
  return value
    .split(/[_-]/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

/** Build the report's layout as an ordered list of grid (or section-wrapped grid) nodes. */
function buildLayout(state: WizardState): ReportLayoutNode[] {
  const blocksByGrid = new Map<string, WizardBlock[]>();
  for (const block of state.blocks) {
    const list = blocksByGrid.get(block.placement.gridId) ?? [];
    list.push(block);
    blocksByGrid.set(block.placement.gridId, list);
  }

  return state.grids.map((grid) => {
    const blocks = (blocksByGrid.get(grid.id) ?? []).sort(
      (a, b) =>
        a.placement.row - b.placement.row ||
        a.placement.column - b.placement.column
    );
    const gridNode: Extract<ReportLayoutNode, { type: 'grid' }> = {
      id: `${grid.id}_grid`,
      type: 'grid',
      columns: grid.columns,
      items: blocks.map((block) => ({
        id: `node_${block.id}`,
        blockId: block.id,
        colSpan: 1,
        rowSpan: 1,
      })),
    };

    if (grid.title || grid.description) {
      return {
        id: grid.id,
        type: 'section',
        title: grid.title,
        description: grid.description,
        children: [gridNode],
      };
    }
    return { ...gridNode, id: grid.id };
  });
}

function buildFilterOptions(
  filter: WizardFilter,
  blocks: WizardBlock[]
): ReportFilterOptionsConfig | undefined {
  if (!filterUsesOptions(filter.type)) return undefined;
  if (filter.optionsSource === 'object_model') {
    const schema =
      filter.optionsSchema ||
      blocks.find((block) => block.id === filter.target)?.schema ||
      blocks.find((block) => block.schema)?.schema;
    const valueField =
      filter.optionsValueField || filter.optionsField || filter.field;
    const labelField =
      filter.optionsLabelField || filter.optionsField || filter.field;
    return {
      source: 'object_model',
      ...(schema ? { schema } : {}),
      field: valueField,
      valueField,
      labelField,
      search: true,
      ...(filter.dependsOn && filter.dependsOn.length > 0
        ? { dependsOn: filter.dependsOn }
        : {}),
      ...(filter.filterMappings && filter.filterMappings.length > 0
        ? { filterMappings: filter.filterMappings }
        : {}),
      ...(filter.optionsCondition
        ? { condition: filter.optionsCondition }
        : {}),
    };
  }
  const values = parseStaticOptions(filter.staticOptions ?? '');
  return { source: 'static', values };
}

function filterUsesOptions(type: WizardFilter['type']): boolean {
  return type === 'select' || type === 'multi_select' || type === 'radio';
}

function parseStaticOptions(raw: string) {
  return raw
    .split(/[\n,]+/)
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((entry) => {
      const [value, label] = entry.split('=').map((part) => part.trim());
      return { value, label: label || humanize(value) };
    });
}

function buildFilterDefinition(
  filter: WizardFilter,
  blocks: WizardBlock[]
): ReportFilterDefinition {
  const appliesTo =
    filter.target === WIZARD_FILTER_TARGET_CUSTOM
      ? (filter.targetMappings ?? [])
      : filter.target === WIZARD_FILTER_TARGET_NONE
        ? []
        : filter.target === WIZARD_FILTER_TARGET_ALL
          ? [{ field: filter.field, op: defaultOperatorFor(filter.type) }]
          : (() => {
              const targetBlock = blocks.find(
                (block) => block.id === filter.target
              );
              return targetBlock
                ? [
                    {
                      blockId: targetBlock.id,
                      field: filter.field,
                      op: defaultOperatorFor(filter.type),
                    },
                  ]
                : [];
            })();

  const options = buildFilterOptions(filter, blocks);

  return {
    id: filter.id,
    label: filter.label,
    type: filter.type,
    appliesTo,
    ...(options ? { options } : {}),
    ...(filter.required ? { required: true } : {}),
    ...(filter.defaultValue !== undefined && filter.defaultValue !== ''
      ? { default: filter.defaultValue }
      : {}),
    ...(filter.strictWhenReferenced ? { strictWhenReferenced: true } : {}),
  };
}

function defaultOperatorFor(type: WizardFilter['type']): string {
  switch (type) {
    case 'multi_select':
      return 'in';
    case 'time_range':
      return 'between';
    case 'number_range':
      return 'between';
    case 'search':
    case 'text':
      return 'contains';
    case 'checkbox':
      return 'eq';
    case 'radio':
    case 'select':
    default:
      return 'eq';
  }
}

export function wizardStateToDefinition(
  state: WizardState,
  schemaFieldsByName: Record<string, string[]>,
  existing?: ReportDefinition
): ReportDefinition {
  const datasetsById = new Map(
    state.datasets.map((dataset) => [dataset.id, dataset])
  );
  const blocks = state.blocks.map((block) =>
    buildBlockDefinition(
      block,
      schemaFieldsByName[block.schema ?? ''] ?? [],
      datasetsById
    )
  );
  const layout = buildLayout(state);
  const filters = state.filters.map((filter) =>
    buildFilterDefinition(filter, state.blocks)
  );

  return {
    ...(existing ?? {
      definitionVersion: 1,
      layout: [],
      filters: [],
      blocks: [],
    }),
    definitionVersion: existing?.definitionVersion ?? 1,
    layout,
    filters,
    blocks,
    datasets: state.datasets.length > 0 ? state.datasets : undefined,
    views: state.views.length > 0 ? state.views : undefined,
  };
}

function isWizardFormat(
  value: string | undefined
): value is WizardColumnFormat {
  return (
    value === 'number' ||
    value === 'decimal' ||
    value === 'currency' ||
    value === 'percent' ||
    value === 'date' ||
    value === 'datetime' ||
    value === 'pill' ||
    value === 'bar_indicator' ||
    value === 'boolean'
  );
}

function isAdvancedCardConfig(
  card: ReportBlockDefinition['card'] | undefined
): boolean {
  if (!card) return false;
  if ((card.groups ?? []).length > 1) return true;
  return (card.groups ?? []).some((group) =>
    group.fields.some((field) => {
      const kind = field.kind ?? 'value';
      return (
        kind !== 'value' ||
        Boolean(field.subcard) ||
        Boolean(field.subtable) ||
        Boolean(field.workflowAction) ||
        Boolean(field.collapsed) ||
        field.colSpan !== undefined ||
        field.displayField !== undefined ||
        field.displayTemplate !== undefined
      );
    })
  );
}

interface LayoutFlattenResult {
  grids: WizardGrid[];
  placements: Record<string, { gridId: string; row: number; column: number }>;
  unsupported: string[];
}

/** Walk the saved layout and coalesce nodes into a list of wizard grids. */
function flattenLayoutBlocks(
  layout: ReportLayoutNode[] | undefined
): LayoutFlattenResult {
  const grids: WizardGrid[] = [];
  const placements: Record<
    string,
    { gridId: string; row: number; column: number }
  > = {};
  const unsupported: string[] = [];

  if (!layout || layout.length === 0) {
    return { grids, placements, unsupported };
  }

  function placeIntoGrid(
    gridId: string,
    columns: number,
    items: Array<{ blockId: string }>
  ) {
    items.forEach((item, index) => {
      const row = Math.floor(index / Math.max(columns, 1)) + 1;
      const col = (index % Math.max(columns, 1)) + 1;
      placements[item.blockId] = { gridId, row, column: col };
    });
  }

  function gridFromBlock(blockId: string): WizardGrid {
    const id = makeGridId();
    grids.push({ id, rows: 1, columns: 1 });
    placements[blockId] = { gridId: id, row: 1, column: 1 };
    return grids[grids.length - 1];
  }

  function appendGrid(grid: WizardGrid, items: Array<{ blockId: string }>) {
    grids.push(grid);
    placeIntoGrid(grid.id, grid.columns, items);
  }

  for (const node of layout) {
    if (node.type === 'block') {
      gridFromBlock(node.blockId);
      continue;
    }

    if (node.type === 'grid') {
      const id = node.id || makeGridId();
      const columns = node.columns ?? 2;
      const rows = Math.max(
        1,
        Math.ceil(node.items.length / Math.max(columns, 1))
      );
      appendGrid({ id, rows, columns }, node.items);
      continue;
    }

    if (node.type === 'section') {
      const innerGrid = (node.children ?? []).find(
        (child) => child.type === 'grid'
      );
      if (innerGrid && innerGrid.type === 'grid') {
        const id = node.id || makeGridId();
        const columns = innerGrid.columns ?? 2;
        const rows = Math.max(
          1,
          Math.ceil(innerGrid.items.length / Math.max(columns, 1))
        );
        appendGrid(
          {
            id,
            title: node.title,
            description: node.description,
            rows,
            columns,
          },
          innerGrid.items
        );
      } else if ((node.children ?? []).every((c) => c.type === 'block')) {
        const id = node.id || makeGridId();
        const blockChildren = (node.children ?? []) as Array<{
          type: 'block';
          blockId: string;
        }>;
        appendGrid(
          {
            id,
            title: node.title,
            description: node.description,
            rows: blockChildren.length || 1,
            columns: 1,
          },
          blockChildren.map((child) => ({ blockId: child.blockId }))
        );
      } else {
        unsupported.push('Nested sections or non-grid section children');
      }
      continue;
    }

    if (node.type === 'metric_row') {
      const id = node.id || makeGridId();
      appendGrid(
        { id, rows: 1, columns: Math.max(1, node.blocks.length) },
        node.blocks.map((blockId) => ({ blockId }))
      );
      continue;
    }

    if (node.type === 'columns') {
      const id = node.id || makeGridId();
      const columnCount = node.columns.length;
      // Flatten column children into a single grid where each column is a column.
      const flat: Array<{ blockId: string }> = [];
      const maxRowsInColumn = Math.max(
        1,
        ...node.columns.map((col) => (col.children ?? []).length)
      );
      for (let row = 0; row < maxRowsInColumn; row += 1) {
        for (let col = 0; col < columnCount; col += 1) {
          const child = node.columns[col].children?.[row];
          if (child && child.type === 'block') {
            flat.push({ blockId: child.blockId });
          } else if (child) {
            unsupported.push(`Nested ${child.type} inside columns`);
            flat.push({ blockId: '' }); // placeholder; will be ignored
          }
        }
      }
      grids.push({ id, rows: maxRowsInColumn, columns: columnCount });
      flat.forEach((item, index) => {
        if (!item.blockId) return;
        const row = Math.floor(index / columnCount) + 1;
        const c = (index % columnCount) + 1;
        placements[item.blockId] = { gridId: id, row, column: c };
      });
      continue;
    }

    unsupported.push((node as { type: string }).type);
  }

  return { grids, placements, unsupported };
}

function blockDefinitionToWizard(
  block: ReportBlockDefinition,
  fallbackPlacement: { gridId: string; row: number; column: number }
): { block: WizardBlock; unsupported: string[] } {
  const unsupported: string[] = [];
  const source = getOptionalBlockSource(block);

  if (!WIZARD_SUPPORTED_BLOCK_TYPES.has(block.type as WizardBlockType)) {
    unsupported.push(`Block type "${block.type}"`);
  }

  if (!source && block.type !== 'markdown') {
    unsupported.push('Missing data source');
  }

  const fields: string[] = [];
  const fieldConfigs: Record<string, WizardFieldConfig> = {};

  if (block.type === 'table') {
    for (const column of block.table?.columns ?? []) {
      const inferredColumnType =
        column.type === 'workflow_button' || column.workflowAction
          ? 'workflow_button'
          : column.type === 'interaction_buttons' ||
              (column.interactionButtons &&
                column.interactionButtons.length > 0)
            ? 'interaction_buttons'
            : column.type;
      // Action columns may arrive with an empty `field` — synthesize a stable
      // key so the editor can list them as rows.
      const fieldKey =
        column.field && column.field.length > 0
          ? column.field
          : makeActionFieldKey();
      fields.push(fieldKey);
      const cfg: WizardFieldConfig = {};
      if (column.label && column.label !== humanize(fieldKey)) {
        cfg.label = column.label;
      }
      if (inferredColumnType === 'workflow_button') {
        cfg.columnType = 'workflow_button';
        if (column.workflowAction) cfg.workflowAction = column.workflowAction;
      } else if (inferredColumnType === 'interaction_buttons') {
        cfg.columnType = 'interaction_buttons';
        if (column.interactionButtons) {
          cfg.interactionButtons = column.interactionButtons;
        }
      } else {
        if (inferredColumnType && inferredColumnType !== 'value') {
          unsupported.push(`Column type "${inferredColumnType}"`);
        }
        if (column.format && isWizardFormat(column.format)) {
          cfg.format = column.format as WizardColumnFormat;
        } else if (column.format) {
          unsupported.push(`Column format "${column.format}"`);
        }
        if (column.pillVariants) {
          cfg.pillVariants = column.pillVariants as Record<
            string,
            WizardPillVariant
          >;
        }
        if (column.displayField) cfg.displayField = column.displayField;
        if (column.displayTemplate) {
          cfg.displayTemplate = column.displayTemplate;
        }
        if (column.secondaryField) cfg.secondaryField = column.secondaryField;
        if (column.linkField) cfg.linkField = column.linkField;
        if (column.tooltipField) cfg.tooltipField = column.tooltipField;
        if (column.levels && column.levels.length > 0) {
          cfg.levels = column.levels;
        }
        if (column.align) cfg.align = column.align;
        if (column.maxChars !== undefined) cfg.maxChars = column.maxChars;
        if (column.descriptive) cfg.descriptive = true;
      }
      if (column.editable) cfg.editable = true;
      if (column.editor) cfg.editor = column.editor;
      if (Object.keys(cfg).length > 0) {
        fieldConfigs[fieldKey] = cfg;
      }
    }
  } else if (block.type === 'card') {
    for (const group of block.card?.groups ?? []) {
      for (const field of group.fields) {
        fields.push(field.field);
        const cfg: WizardFieldConfig = {};
        if (field.label && field.label !== humanize(field.field)) {
          cfg.label = field.label;
        }
        if (field.format && isWizardFormat(field.format)) {
          cfg.format = field.format as WizardColumnFormat;
        } else if (field.format) {
          unsupported.push(`Field format "${field.format}"`);
        }
        if (field.pillVariants) {
          cfg.pillVariants = field.pillVariants as Record<
            string,
            WizardPillVariant
          >;
        }
        if (field.editable) cfg.editable = true;
        if (field.editor) cfg.editor = field.editor;
        if (Object.keys(cfg).length > 0) {
          fieldConfigs[field.field] = cfg;
        }
      }
    }
  } else if (block.type === 'chart') {
    for (const series of block.chart?.series ?? []) {
      fields.push(series.field);
      if (series.label && series.label !== humanize(series.field)) {
        fieldConfigs[series.field] = { label: series.label };
      }
    }
  } else if (block.type === 'metric') {
    const aggField = (source?.aggregates ?? [])[0]?.field;
    if (aggField) fields.push(aggField);
  }

  const metricFormat = block.metric?.format;
  const primaryAggregate = source?.aggregates?.[0];

  const wizardBlock: WizardBlock = {
    id: block.id,
    type: WIZARD_SUPPORTED_BLOCK_TYPES.has(block.type as WizardBlockType)
      ? (block.type as WizardBlockType)
      : 'table',
    title: block.title || humanize(block.id),
    schema:
      source?.kind && source.kind !== 'object_model'
        ? undefined
        : source?.schema || undefined,
    ...(source?.kind ? { sourceKind: source.kind } : {}),
    ...(source?.entity ? { sourceEntity: source.entity } : {}),
    ...(source?.workflowId ? { workflowId: source.workflowId } : {}),
    ...(source?.instanceId ? { instanceId: source.instanceId } : {}),
    ...(source?.interval ? { sourceInterval: source.interval } : {}),
    ...(source?.granularity ? { sourceGranularity: source.granularity } : {}),
    ...(source?.orderBy && source.orderBy.length > 0
      ? { sourceOrderBy: source.orderBy }
      : {}),
    ...(source?.limit && source.limit > 0 ? { sourceLimit: source.limit } : {}),
    ...(source?.join && source.join.length > 0
      ? { sourceJoins: source.join }
      : {}),
    ...(source?.condition ? { sourceCondition: source.condition } : {}),
    fields,
    placement: fallbackPlacement,
    chartKind: block.chart?.kind,
    chartGroupBy: block.chart?.x,
    chartAggregate: primaryAggregate?.op,
    chartAggregateField: primaryAggregate?.field,
    chartAggregateDistinct: primaryAggregate?.distinct,
    chartAggregatePercentile: primaryAggregate?.percentile,
    chartAggregateExpression: primaryAggregate?.expression,
    metricAggregate: primaryAggregate?.op,
    metricField: primaryAggregate?.field,
    metricDistinct: primaryAggregate?.distinct,
    metricPercentile: primaryAggregate?.percentile,
    metricExpression: primaryAggregate?.expression,
    metricFormat: isWizardFormat(metricFormat)
      ? (metricFormat as WizardColumnFormat)
      : undefined,
    ...(block.type === 'card' && isAdvancedCardConfig(block.card)
      ? { cardConfig: block.card }
      : {}),
    markdownContent: block.markdown?.content,
    ...(block.lazy ? { lazy: true } : {}),
    ...(block.hideWhenEmpty ? { hideWhenEmpty: true } : {}),
    ...(block.showWhen ? { showWhen: block.showWhen } : {}),
    ...(block.filters && block.filters.length > 0
      ? { filters: block.filters }
      : {}),
    ...(block.interactions && block.interactions.length > 0
      ? { interactions: block.interactions }
      : {}),
    ...(Object.keys(fieldConfigs).length > 0 ? { fieldConfigs } : {}),
    ...(block.type === 'table' && block.table?.selectable
      ? { selectable: true }
      : {}),
    ...(block.type === 'table' &&
    block.table?.actions &&
    block.table.actions.length > 0
      ? { tableActions: block.table.actions }
      : {}),
    ...(block.type === 'table' &&
    block.table?.defaultSort &&
    block.table.defaultSort.length > 0
      ? { defaultSort: block.table.defaultSort }
      : {}),
    ...(block.type === 'table' && block.table?.pagination?.defaultPageSize
      ? { defaultPageSize: block.table.pagination.defaultPageSize }
      : {}),
    ...(block.type === 'table' &&
    block.table?.pagination?.allowedPageSizes &&
    block.table.pagination.allowedPageSizes.length > 0
      ? { allowedPageSizes: block.table.pagination.allowedPageSizes }
      : {}),
    ...(block.dataset ? { dataset: block.dataset } : {}),
  };

  return { block: wizardBlock, unsupported };
}

function getOptionalBlockSource(
  block: ReportBlockDefinition
): ReportSource | undefined {
  return (block as { source?: ReportSource }).source;
}

export function definitionToWizardState(
  definition: ReportDefinition | null | undefined,
  fallbackSchema: string
): { state: WizardState; compatibility: WizardCompatibility } {
  const unsupportedReasons: string[] = [];

  if (!definition || definition.blocks.length === 0) {
    const seedGridId = makeGridId();
    return {
      state: {
        defaultSchema: fallbackSchema || undefined,
        title: 'New report',
        grids: [{ id: seedGridId, rows: 2, columns: 2 }],
        blocks: [],
        filters: [],
        datasets: [],
        views: [],
      },
      compatibility: { fullyEditable: true, reasons: [] },
    };
  }

  const primarySchema =
    definition.blocks
      .map((block) => getOptionalBlockSource(block)?.schema)
      .find((schema): schema is string => Boolean(schema)) || fallbackSchema;

  const layoutInfo = flattenLayoutBlocks(definition.layout);
  unsupportedReasons.push(...layoutInfo.unsupported);

  // Ensure at least one grid exists, so unplaced blocks have a home.
  let grids = layoutInfo.grids;
  if (grids.length === 0) {
    grids = [{ id: makeGridId(), rows: 2, columns: 2 }];
  }

  const blocks: WizardBlock[] = [];
  const fallbackGridId = grids[0].id;
  const fillCursor: Record<string, { row: number; column: number }> = {};
  for (const block of definition.blocks) {
    let placement = layoutInfo.placements[block.id];
    if (!placement) {
      const cursor = fillCursor[fallbackGridId] ?? { row: 1, column: 1 };
      placement = {
        gridId: fallbackGridId,
        row: cursor.row,
        column: cursor.column,
      };
      const grid = grids.find((g) => g.id === fallbackGridId)!;
      const nextColumn = cursor.column + 1;
      fillCursor[fallbackGridId] =
        nextColumn > grid.columns
          ? { row: cursor.row + 1, column: 1 }
          : { row: cursor.row, column: nextColumn };
    }
    const converted = blockDefinitionToWizard(block, placement);
    unsupportedReasons.push(...converted.unsupported);
    blocks.push(converted.block);
  }

  const filters: WizardFilter[] = (definition.filters ?? []).map((filter) => {
    const mappings = filter.appliesTo ?? [];
    let target = WIZARD_FILTER_TARGET_NONE;
    if (mappings.length === 0) {
      target = WIZARD_FILTER_TARGET_NONE;
    } else if (mappings.length === 1) {
      target = mappings[0].blockId ?? WIZARD_FILTER_TARGET_ALL;
    } else {
      target = WIZARD_FILTER_TARGET_CUSTOM;
    }
    const field =
      mappings[0]?.field ||
      filter.options?.field ||
      filter.options?.valueField ||
      '';
    const opts = filter.options;
    const optionsSource: WizardFilter['optionsSource'] =
      opts?.source === 'object_model' ? 'object_model' : 'static';
    const staticOptions =
      opts?.source !== 'object_model' && Array.isArray(opts?.values)
        ? opts!.values
            .map((entry) =>
              entry.label && entry.label !== humanize(String(entry.value))
                ? `${entry.value}=${entry.label}`
                : String(entry.value)
            )
            .join('\n')
        : undefined;
    return {
      id: filter.id,
      label: filter.label,
      type: filter.type,
      field,
      target,
      targetMappings: mappings.length > 1 ? mappings : undefined,
      optionsSource,
      staticOptions,
      optionsField: opts?.field || opts?.valueField,
      optionsSchema: opts?.schema,
      optionsValueField: opts?.valueField || opts?.field,
      optionsLabelField: opts?.labelField || opts?.valueField || opts?.field,
      dependsOn: opts?.dependsOn,
      filterMappings: opts?.filterMappings,
      optionsCondition: opts?.condition,
      required: filter.required,
      defaultValue: filter.default,
      strictWhenReferenced: filter.strictWhenReferenced,
    };
  });

  // Grow each grid's rows to accommodate placed blocks.
  for (const grid of grids) {
    const gridBlocks = blocks.filter((b) => b.placement.gridId === grid.id);
    if (gridBlocks.length === 0) continue;
    const maxRow = Math.max(...gridBlocks.map((b) => b.placement.row));
    grid.rows = Math.max(grid.rows, maxRow);
  }

  return {
    state: {
      defaultSchema: primarySchema || undefined,
      title: 'Report',
      grids,
      blocks,
      filters,
      datasets: definition.datasets ?? [],
      views: definition.views ?? [],
    },
    compatibility: {
      fullyEditable: unsupportedReasons.length === 0,
      reasons: Array.from(new Set(unsupportedReasons)),
    },
  };
}
