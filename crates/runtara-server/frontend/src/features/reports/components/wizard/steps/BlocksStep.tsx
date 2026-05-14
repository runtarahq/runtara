import { useState } from 'react';
import {
  ArrowDown,
  ArrowUp,
  ChevronUp,
  GripVertical,
  Minus,
  Plus,
  Settings2,
  Trash2,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Textarea } from '@/shared/components/ui/textarea';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Checkbox } from '@/shared/components/ui/checkbox';
import {
  ReportBlockResult,
  ReportAggregateFn,
  ReportCondition,
  ReportDatasetDefinition,
  ReportEditorConfig,
  ReportEditorKind,
  ReportEditorOption,
  ReportFilterDefinition,
  ReportFilterOption,
  ReportFilterType,
  ReportOrderBy,
  ReportSource,
  ReportSourceJoin,
  ReportTableInteractionButtonConfig,
  ReportWorkflowActionConfig,
} from '../../../types';
import {
  createDefaultDatasetBlockQuery,
  datasetQueryOutputFields,
} from '../../../datasetBlocks';
import {
  WIZARD_BLOCK_TYPES,
  WIZARD_COLUMN_FORMATS,
  WIZARD_METRIC_FORMATS,
  WIZARD_PILL_VARIANTS,
  WizardBlock,
  WizardBlockType,
  WizardColumnFormat,
  WizardFieldConfig,
  WizardFilter,
  WizardGrid,
  WizardPillVariant,
  WizardTableColumnType,
  isActionFieldKey,
  makeActionFieldKey,
  makeGridId,
} from '../wizardTypes';
import { humanizeFieldName, slugify } from '../../../utils';
import { BlockPreview } from './BlockPreview';
import { BlockDatasetQueryEditor } from './BlockDatasetQueryEditor';
import {
  InteractionActionsList,
  InteractionButtonsEditor,
  TableBulkActionsEditor,
  WorkflowActionEditor,
  createDefaultInteractionButton,
  createDefaultWorkflowAction,
} from './tableActionEditors';

function fieldsOfSchema(
  schemas: Schema[],
  schemaName: string | undefined
): string[] {
  if (!schemaName) return [];
  return (
    schemas
      .find((schema) => schema.name === schemaName)
      ?.columns.map((column) => column.name) ?? []
  );
}

function fieldsForWizardSource(
  schemas: Schema[],
  block: WizardBlock
): string[] {
  if (block.sourceKind === 'workflow_runtime') {
    return (
      WORKFLOW_RUNTIME_FIELDS[block.sourceEntity ?? 'instances'] ??
      WORKFLOW_RUNTIME_FIELDS.instances
    );
  }
  if (block.sourceKind === 'system') {
    return (
      SYSTEM_FIELDS[block.sourceEntity ?? 'runtime_system_snapshot'] ??
      SYSTEM_FIELDS.runtime_system_snapshot
    );
  }
  const baseFields = fieldsOfSchema(schemas, block.schema);
  const joinFields = (block.sourceJoins ?? []).flatMap((join) => {
    const alias = joinAlias(join);
    return fieldsOfSchema(schemas, join.schema).map(
      (field) => `${alias}.${field}`
    );
  });
  return uniqueStrings([...baseFields, ...joinFields]);
}

function joinAlias(join: Pick<ReportSourceJoin, 'schema' | 'alias'>): string {
  return join.alias || join.schema;
}

interface BlocksStepProps {
  grids: WizardGrid[];
  blocks: WizardBlock[];
  schemas: Schema[];
  defaultSchema?: string;
  datasets: ReportDatasetDefinition[];
  filters: WizardFilter[];
  blockResults?: Record<string, ReportBlockResult>;
  /** When false, all editing affordances are hidden; layout still renders. */
  editing?: boolean;
  onGridsChange: (next: WizardGrid[]) => void;
  onBlocksChange: (next: WizardBlock[]) => void;
  /** Atomic update for ops that touch both grids and blocks at once. */
  onGridsAndBlocksChange: (grids: WizardGrid[], blocks: WizardBlock[]) => void;
}

const CHART_KINDS: Array<{
  value: 'line' | 'bar' | 'area' | 'pie' | 'donut';
  label: string;
}> = [
  { value: 'bar', label: 'Bar' },
  { value: 'line', label: 'Line' },
  { value: 'area', label: 'Area' },
  { value: 'pie', label: 'Pie' },
  { value: 'donut', label: 'Donut' },
];

const METRIC_AGGREGATES: Array<{ value: ReportAggregateFn; label: string }> = [
  { value: 'count', label: 'Count' },
  { value: 'sum', label: 'Sum' },
  { value: 'avg', label: 'Average' },
  { value: 'min', label: 'Min' },
  { value: 'max', label: 'Max' },
  { value: 'first_value', label: 'First value' },
  { value: 'last_value', label: 'Last value' },
  { value: 'percentile_cont', label: 'Percentile (continuous)' },
  { value: 'percentile_disc', label: 'Percentile (discrete)' },
  { value: 'stddev_samp', label: 'Std. deviation (sample)' },
  { value: 'var_samp', label: 'Variance (sample)' },
  { value: 'expr', label: 'Custom expression' },
];

const FILTER_TYPES: Array<{ value: ReportFilterType; label: string }> = [
  { value: 'select', label: 'Single select' },
  { value: 'multi_select', label: 'Multi select' },
  { value: 'radio', label: 'Radio' },
  { value: 'checkbox', label: 'Checkbox' },
  { value: 'time_range', label: 'Time range' },
  { value: 'number_range', label: 'Number range' },
  { value: 'text', label: 'Text' },
  { value: 'search', label: 'Search' },
];

const BLOCK_FILTER_OPERATORS = [
  { value: 'eq', label: 'Equals' },
  { value: 'in', label: 'In list' },
  { value: 'contains', label: 'Contains' },
  { value: 'between', label: 'Between' },
  { value: 'ne', label: 'Not equal' },
  { value: 'gt', label: 'Greater than' },
  { value: 'gte', label: 'Greater or equal' },
  { value: 'lt', label: 'Less than' },
  { value: 'lte', label: 'Less or equal' },
];

const EDITOR_AUTO = '__auto__';

const EDITOR_KINDS: Array<{ value: ReportEditorKind; label: string }> = [
  { value: 'text', label: 'Text' },
  { value: 'textarea', label: 'Textarea' },
  { value: 'number', label: 'Number' },
  { value: 'select', label: 'Select' },
  { value: 'toggle', label: 'Toggle' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
  { value: 'lookup', label: 'Lookup' },
];

type WizardSourceMode = 'schema' | 'dataset' | 'workflow_runtime' | 'system';

const WORKFLOW_RUNTIME_ENTITIES: Array<{
  value: NonNullable<ReportSource['entity']>;
  label: string;
}> = [
  { value: 'instances', label: 'Instances' },
  { value: 'actions', label: 'Actions' },
];

const SYSTEM_ENTITIES: Array<{
  value: NonNullable<ReportSource['entity']>;
  label: string;
}> = [
  {
    value: 'runtime_execution_metric_buckets',
    label: 'Runtime execution metrics',
  },
  { value: 'runtime_system_snapshot', label: 'Runtime system snapshot' },
  { value: 'connection_rate_limit_status', label: 'Rate limit status' },
  { value: 'connection_rate_limit_events', label: 'Rate limit events' },
  { value: 'connection_rate_limit_timeline', label: 'Rate limit timeline' },
];

const WORKFLOW_RUNTIME_FIELDS: Record<string, string[]> = {
  instances: [
    'id',
    'instanceId',
    'workflowId',
    'workflowName',
    'status',
    'createdAt',
    'updatedAt',
    'usedVersion',
    'durationSeconds',
    'hasActions',
    'actionCount',
  ],
  actions: [
    'id',
    'actionId',
    'actionKind',
    'targetKind',
    'targetId',
    'workflowId',
    'instanceId',
    'signalId',
    'actionKey',
    'label',
    'message',
    'inputSchema',
    'schemaFormat',
    'status',
    'requestedAt',
    'correlation',
    'context',
    'runtime',
  ],
};

const SYSTEM_FIELDS: Record<string, string[]> = {
  runtime_execution_metric_buckets: [
    'tenantId',
    'bucketTime',
    'granularity',
    'invocationCount',
    'successCount',
    'failureCount',
    'cancelledCount',
    'avgDurationSeconds',
    'minDurationSeconds',
    'maxDurationSeconds',
    'avgMemoryBytes',
    'maxMemoryBytes',
    'successRatePercent',
  ],
  runtime_system_snapshot: [
    'capturedAt',
    'cpuArchitecture',
    'cpuPhysicalCores',
    'cpuLogicalCores',
    'memoryTotalBytes',
    'memoryAvailableBytes',
    'memoryAvailableForWorkflowsBytes',
    'memoryUsedBytes',
    'memoryUsedPercent',
    'diskPath',
    'diskTotalBytes',
    'diskAvailableBytes',
    'diskUsedBytes',
    'diskUsedPercent',
  ],
  connection_rate_limit_status: [
    'connectionId',
    'connectionTitle',
    'integrationId',
    'configRequestsPerSecond',
    'configBurstSize',
    'configRetryOnLimit',
    'configMaxRetries',
    'configMaxWaitMs',
    'stateAvailable',
    'stateCurrentTokens',
    'stateLastRefillMs',
    'stateLearnedLimit',
    'stateCallsInWindow',
    'stateTotalCalls',
    'stateWindowStartMs',
    'capacityPercent',
    'utilizationPercent',
    'isRateLimited',
    'retryAfterMs',
    'periodInterval',
    'periodTotalRequests',
    'periodRateLimitedCount',
    'periodRetryCount',
    'periodRateLimitedPercent',
  ],
  connection_rate_limit_events: [
    'id',
    'connectionId',
    'eventType',
    'createdAt',
    'metadata',
    'tag',
  ],
  connection_rate_limit_timeline: [
    'connectionId',
    'bucket',
    'bucketTime',
    'granularity',
    'requestCount',
    'rateLimitedCount',
    'retryCount',
  ],
};

const NO_SORT_FIELD = '__none__';
const ALWAYS_VISIBLE = '__always__';

export function BlocksStep({
  grids,
  blocks,
  schemas,
  defaultSchema,
  datasets,
  filters,
  blockResults,
  editing = true,
  onGridsChange,
  onBlocksChange,
  onGridsAndBlocksChange,
}: BlocksStepProps) {
  const [openBlockId, setOpenBlockId] = useState<string | null>(null);
  const [draggedId, setDraggedId] = useState<string | null>(null);
  const [hoverCell, setHoverCell] = useState<{
    gridId: string;
    row: number;
    column: number;
  } | null>(null);

  function updateGrid(id: string, patch: Partial<WizardGrid>) {
    onGridsChange(
      grids.map((grid) => (grid.id === id ? { ...grid, ...patch } : grid))
    );
  }

  function appendGrid() {
    const newGrid: WizardGrid = {
      id: makeGridId(),
      rows: 2,
      columns: 2,
    };
    onGridsChange([...grids, newGrid]);
  }

  function removeGrid(id: string) {
    if (grids.length <= 1) return;
    const fallbackId = grids.find((g) => g.id !== id)!.id;
    const nextGrids = grids.filter((grid) => grid.id !== id);
    const nextBlocks = blocks.map((block) =>
      block.placement.gridId === id
        ? { ...block, placement: { gridId: fallbackId, row: 1, column: 1 } }
        : block
    );
    onGridsAndBlocksChange(nextGrids, nextBlocks);
  }

  function moveGrid(id: string, delta: number) {
    const index = grids.findIndex((grid) => grid.id === id);
    if (index < 0) return;
    const next = Math.max(0, Math.min(grids.length - 1, index + delta));
    if (next === index) return;
    const cloned = [...grids];
    const [removed] = cloned.splice(index, 1);
    cloned.splice(next, 0, removed);
    onGridsChange(cloned);
  }

  function resizeGrid(id: string, deltaRows: number, deltaColumns: number) {
    const grid = grids.find((g) => g.id === id);
    if (!grid) return;
    const nextRows = Math.max(1, grid.rows + deltaRows);
    const nextColumns = Math.max(1, grid.columns + deltaColumns);
    const nextGrids = grids.map((g) =>
      g.id === id ? { ...g, rows: nextRows, columns: nextColumns } : g
    );
    const nextBlocks = blocks.map((block) => {
      if (block.placement.gridId !== id) return block;
      return {
        ...block,
        placement: {
          gridId: id,
          row: Math.min(block.placement.row, nextRows),
          column: Math.min(block.placement.column, nextColumns),
        },
      };
    });
    onGridsAndBlocksChange(nextGrids, nextBlocks);
  }

  function updateBlock(id: string, patch: Partial<WizardBlock>) {
    onBlocksChange(
      blocks.map((block) => (block.id === id ? { ...block, ...patch } : block))
    );
  }

  function removeBlock(id: string) {
    onBlocksChange(blocks.filter((block) => block.id !== id));
    if (openBlockId === id) setOpenBlockId(null);
  }

  function addBlockAtCell(gridId: string, row: number, column: number) {
    const id = `block_${Date.now().toString(36)}`;
    const seedSchema = defaultSchema || schemas[0]?.name;
    const seedFields = fieldsOfSchema(schemas, seedSchema).slice(0, 4);
    const seed: WizardBlock = {
      id,
      type: 'table',
      title: '',
      schema: seedSchema,
      fields: seedFields,
      placement: { gridId, row, column },
    };
    onBlocksChange([...blocks, seed]);
    setOpenBlockId(id);
  }

  function moveBlock(
    blockId: string,
    target: { gridId: string; row: number; column: number }
  ) {
    const source = blocks.find((block) => block.id === blockId);
    if (!source) return;
    const occupant = blocks.find(
      (block) =>
        block.id !== blockId &&
        block.placement.gridId === target.gridId &&
        block.placement.row === target.row &&
        block.placement.column === target.column
    );

    if (occupant) {
      // Swap places with the existing occupant.
      onBlocksChange(
        blocks.map((block) => {
          if (block.id === blockId) {
            return { ...block, placement: target };
          }
          if (block.id === occupant.id) {
            return { ...block, placement: source.placement };
          }
          return block;
        })
      );
      return;
    }

    updateBlock(blockId, { placement: target });
  }

  function onDragStart(blockId: string) {
    setDraggedId(blockId);
  }

  function onDragEnd() {
    setDraggedId(null);
    setHoverCell(null);
  }

  function onDropOnCell(target: {
    gridId: string;
    row: number;
    column: number;
  }) {
    if (draggedId) moveBlock(draggedId, target);
    setHoverCell(null);
    setDraggedId(null);
  }

  // Viewer-side empty state: avoid showing an unstyled blank canvas when a
  // saved report contains no blocks. Editors still see the grid placeholders
  // so they can drop blocks in.
  if (!editing && blocks.length === 0) {
    return (
      <div className="grid place-items-center gap-2 rounded-xl border border-dashed bg-muted/10 px-6 py-12 text-center">
        <p className="text-sm font-medium text-foreground">
          This report has no content yet
        </p>
        <p className="max-w-prose text-xs text-muted-foreground">
          Switch to edit mode to add a markdown section, metric, chart, table,
          or card.
        </p>
      </div>
    );
  }

  return (
    <div className="grid gap-4">
      {grids.map((grid, gridIndex) => {
        const gridBlocks = blocks.filter((b) => b.placement.gridId === grid.id);
        const hasBlocksInLastRow = gridBlocks.some(
          (b) => b.placement.row === grid.rows
        );
        const hasBlocksInLastColumn = gridBlocks.some(
          (b) => b.placement.column === grid.columns
        );
        return (
          <GridSection
            key={grid.id}
            grid={grid}
            index={gridIndex}
            gridCount={grids.length}
            blocks={gridBlocks}
            schemas={schemas}
            datasets={datasets}
            filters={filters}
            blockResults={blockResults}
            editing={editing}
            draggedId={draggedId}
            hoverCell={hoverCell}
            openBlockId={openBlockId}
            canDecreaseRows={!hasBlocksInLastRow && grid.rows > 1}
            canDecreaseColumns={!hasBlocksInLastColumn && grid.columns > 1}
            onTitleChange={(title) => updateGrid(grid.id, { title })}
            onDescriptionChange={(description) =>
              updateGrid(grid.id, { description })
            }
            onResize={(deltaRows, deltaColumns) =>
              resizeGrid(grid.id, deltaRows, deltaColumns)
            }
            onAddBlockAtCell={(row, column) =>
              addBlockAtCell(grid.id, row, column)
            }
            onRemove={() => removeGrid(grid.id)}
            onMoveUp={() => moveGrid(grid.id, -1)}
            onMoveDown={() => moveGrid(grid.id, 1)}
            onSetHoverCell={setHoverCell}
            onDropCell={onDropOnCell}
            onBlockUpdate={updateBlock}
            onBlockRemove={removeBlock}
            onBlockToggleOpen={(id) =>
              setOpenBlockId(openBlockId === id ? null : id)
            }
            onBlockDragStart={onDragStart}
            onBlockDragEnd={onDragEnd}
          />
        );
      })}

      {editing ? (
        <button
          type="button"
          onClick={appendGrid}
          className="flex w-full items-center justify-center gap-1.5 rounded-md border border-dashed bg-muted/10 py-4 text-xs text-muted-foreground transition-colors hover:bg-muted/20 hover:text-foreground"
        >
          <Plus className="h-4 w-4" />
          <span>Add section</span>
        </button>
      ) : null}
    </div>
  );
}

function GridSection({
  grid,
  index,
  gridCount,
  blocks,
  schemas,
  datasets,
  filters,
  blockResults,
  editing,
  draggedId,
  hoverCell,
  openBlockId,
  canDecreaseRows,
  canDecreaseColumns,
  onTitleChange,
  onDescriptionChange,
  onResize,
  onAddBlockAtCell,
  onRemove,
  onMoveUp,
  onMoveDown,
  onSetHoverCell,
  onDropCell,
  onBlockUpdate,
  onBlockRemove,
  onBlockToggleOpen,
  onBlockDragStart,
  onBlockDragEnd,
}: {
  grid: WizardGrid;
  index: number;
  gridCount: number;
  blocks: WizardBlock[];
  schemas: Schema[];
  datasets: ReportDatasetDefinition[];
  filters: WizardFilter[];
  blockResults?: Record<string, ReportBlockResult>;
  editing: boolean;
  draggedId: string | null;
  hoverCell: { gridId: string; row: number; column: number } | null;
  openBlockId: string | null;
  canDecreaseRows: boolean;
  canDecreaseColumns: boolean;
  onTitleChange: (title: string) => void;
  onDescriptionChange: (description: string) => void;
  onResize: (deltaRows: number, deltaColumns: number) => void;
  onAddBlockAtCell: (row: number, column: number) => void;
  onRemove: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  onSetHoverCell: (
    cell: { gridId: string; row: number; column: number } | null
  ) => void;
  onDropCell: (target: { gridId: string; row: number; column: number }) => void;
  onBlockUpdate: (id: string, patch: Partial<WizardBlock>) => void;
  onBlockRemove: (id: string) => void;
  onBlockToggleOpen: (id: string) => void;
  onBlockDragStart: (id: string) => void;
  onBlockDragEnd: () => void;
}) {
  return (
    <section className="grid gap-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid min-w-0 flex-1 gap-1">
          {editing ? (
            <>
              <input
                value={grid.title ?? ''}
                placeholder={`Section ${index + 1} title (optional)`}
                onChange={(event) => onTitleChange(event.target.value)}
                className="w-full bg-transparent text-base font-semibold placeholder:text-muted-foreground focus:outline-none"
                style={{
                  border: 'none',
                  outline: 'none',
                  boxShadow: 'none',
                  padding: 0,
                }}
              />
              <input
                value={grid.description ?? ''}
                placeholder="Optional description shown beneath the title"
                onChange={(event) => onDescriptionChange(event.target.value)}
                className="w-full bg-transparent text-xs text-muted-foreground placeholder:text-muted-foreground focus:outline-none"
                style={{
                  border: 'none',
                  outline: 'none',
                  boxShadow: 'none',
                  padding: 0,
                }}
              />
            </>
          ) : grid.title || grid.description ? (
            <div className="grid gap-0.5">
              {grid.title ? (
                <h2 className="text-base font-semibold">{grid.title}</h2>
              ) : null}
              {grid.description ? (
                <p className="text-xs text-muted-foreground">
                  {grid.description}
                </p>
              ) : null}
            </div>
          ) : null}
        </div>
        {editing ? (
          <div className="flex items-center gap-1.5">
            <div className="flex items-center gap-1 rounded-md border bg-background px-2 py-1 text-xs">
              <span className="text-muted-foreground">Rows</span>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-6 w-6"
                disabled={!canDecreaseRows}
                title={
                  !canDecreaseRows && grid.rows > 1
                    ? 'Last row still has a block — move or remove it first'
                    : undefined
                }
                onClick={() => onResize(-1, 0)}
                aria-label="Remove row"
              >
                <Minus className="h-3 w-3" />
              </Button>
              <span className="min-w-4 text-center text-sm font-semibold">
                {grid.rows}
              </span>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-6 w-6"
                onClick={() => onResize(1, 0)}
                aria-label="Add row"
              >
                <Plus className="h-3 w-3" />
              </Button>
            </div>
            <div className="flex items-center gap-1 rounded-md border bg-background px-2 py-1 text-xs">
              <span className="text-muted-foreground">Cols</span>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-6 w-6"
                disabled={!canDecreaseColumns}
                title={
                  !canDecreaseColumns && grid.columns > 1
                    ? 'Last column still has a block — move or remove it first'
                    : undefined
                }
                onClick={() => onResize(0, -1)}
                aria-label="Remove column"
              >
                <Minus className="h-3 w-3" />
              </Button>
              <span className="min-w-4 text-center text-sm font-semibold">
                {grid.columns}
              </span>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-6 w-6"
                onClick={() => onResize(0, 1)}
                aria-label="Add column"
              >
                <Plus className="h-3 w-3" />
              </Button>
            </div>
            <div className="flex items-center gap-0.5">
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-7 w-7"
                disabled={index === 0}
                onClick={onMoveUp}
                aria-label="Move section up"
              >
                <ArrowUp className="h-3.5 w-3.5" />
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-7 w-7"
                disabled={index === gridCount - 1}
                onClick={onMoveDown}
                aria-label="Move section down"
              >
                <ArrowDown className="h-3.5 w-3.5" />
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-7 w-7"
                disabled={gridCount <= 1}
                onClick={onRemove}
                aria-label="Remove section"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>
        ) : null}
      </div>

      <div
        className="grid gap-3"
        style={{
          gridTemplateColumns: `repeat(${grid.columns}, minmax(0, 1fr))`,
        }}
      >
        {Array.from({ length: grid.rows * grid.columns }, (_, cellIndex) => {
          const row = Math.floor(cellIndex / grid.columns) + 1;
          const column = (cellIndex % grid.columns) + 1;
          const cellBlocks = blocks.filter(
            (block) =>
              block.placement.row === row && block.placement.column === column
          );
          const isHover =
            hoverCell?.gridId === grid.id &&
            hoverCell.row === row &&
            hoverCell.column === column;
          return (
            <div
              key={`${row}-${column}`}
              onDragOver={(event) => {
                event.preventDefault();
                if (
                  hoverCell?.gridId !== grid.id ||
                  hoverCell.row !== row ||
                  hoverCell.column !== column
                ) {
                  onSetHoverCell({ gridId: grid.id, row, column });
                }
              }}
              onDragLeave={() => {
                if (
                  hoverCell?.gridId === grid.id &&
                  hoverCell.row === row &&
                  hoverCell.column === column
                ) {
                  onSetHoverCell(null);
                }
              }}
              onDrop={(event) => {
                event.preventDefault();
                onDropCell({ gridId: grid.id, row, column });
              }}
              className={cn(
                'grid min-h-[120px] min-w-0 gap-2 overflow-hidden transition-colors',
                // Empty cells in edit mode get a dashed outline so users see
                // where they can drop a block. Occupied cells let the block's
                // own border define the boundary — no nested borders.
                editing && cellBlocks.length === 0
                  ? 'place-content-center rounded-md border border-dashed bg-muted/10 p-3'
                  : '',
                !editing && cellBlocks.length === 0 && 'pointer-events-none',
                editing && cellBlocks.length > 0 && 'content-start',
                isHover && 'rounded-md ring-2 ring-primary/30'
              )}
            >
              {cellBlocks.length === 0 && editing ? (
                <button
                  type="button"
                  onClick={() => onAddBlockAtCell(row, column)}
                  className="flex flex-col items-center justify-center gap-1 rounded text-xs text-muted-foreground transition-colors hover:text-foreground"
                >
                  <Plus className="h-5 w-5" />
                  <span>Configure block</span>
                </button>
              ) : (
                cellBlocks.map((block) => (
                  <BlockCard
                    key={block.id}
                    block={block}
                    schemas={schemas}
                    datasets={datasets}
                    filters={filters}
                    result={blockResults?.[block.id]}
                    editing={editing}
                    open={editing && openBlockId === block.id}
                    isDragging={draggedId === block.id}
                    onToggle={() => onBlockToggleOpen(block.id)}
                    onRemove={() => onBlockRemove(block.id)}
                    onChange={(patch) => onBlockUpdate(block.id, patch)}
                    onDragStart={() => onBlockDragStart(block.id)}
                    onDragEnd={onBlockDragEnd}
                  />
                ))
              )}
            </div>
          );
        })}
      </div>
    </section>
  );
}

function BlockCard({
  block,
  schemas,
  datasets,
  filters,
  result,
  editing,
  open,
  isDragging,
  onToggle,
  onRemove,
  onChange,
  onDragStart,
  onDragEnd,
}: {
  block: WizardBlock;
  schemas: Schema[];
  datasets: ReportDatasetDefinition[];
  filters: WizardFilter[];
  result?: ReportBlockResult;
  editing: boolean;
  open: boolean;
  isDragging: boolean;
  onToggle: () => void;
  onRemove: () => void;
  onChange: (patch: Partial<WizardBlock>) => void;
  onDragStart: () => void;
  onDragEnd: () => void;
}) {
  const schemaFields = fieldsForWizardSource(schemas, block);
  const usingDataset = Boolean(block.dataset);
  const sourceMode: WizardSourceMode = usingDataset
    ? 'dataset'
    : block.sourceKind === 'workflow_runtime'
      ? 'workflow_runtime'
      : block.sourceKind === 'system'
        ? 'system'
        : 'schema';
  const dataset = block.dataset
    ? datasets.find((candidate) => candidate.id === block.dataset?.id)
    : undefined;
  const tableSortFields = block.dataset
    ? datasetQueryOutputFields(block.dataset)
    : schemaFields;
  const interactionFields = block.dataset
    ? tableSortFields
    : block.type === 'chart'
      ? uniqueStrings([block.chartGroupBy, ...block.fields])
      : block.fields.filter((field) => !isActionFieldKey(field));
  const supportsBlockFilters = !usingDataset && block.type !== 'markdown';
  // Card and markdown blocks don't make sense over pre-aggregated datasets —
  // hide the dataset toggle for them.
  const supportsDataset =
    block.type === 'table' || block.type === 'chart' || block.type === 'metric';
  const supportsVirtualSources = supportsDataset;
  const supportsFields =
    !usingDataset &&
    (block.type === 'table' || block.type === 'card' || block.type === 'chart');
  const needsSchema =
    !usingDataset && !block.sourceKind && block.type !== 'markdown';
  const supportsObjectModelSourceTools = needsSchema && Boolean(block.schema);

  function changeSchema(nextSchema: string) {
    if (nextSchema === block.schema) return;
    // Reset field-related config when the schema changes — the old fields
    // probably don't exist on the new schema.
    onChange({
      sourceKind: undefined,
      sourceEntity: undefined,
      workflowId: undefined,
      instanceId: undefined,
      sourceInterval: undefined,
      sourceGranularity: undefined,
      sourceOrderBy: undefined,
      sourceLimit: undefined,
      sourceJoins: undefined,
      sourceCondition: undefined,
      schema: nextSchema,
      fields: [],
      fieldConfigs: undefined,
      chartGroupBy: undefined,
      metricField: undefined,
    });
  }

  function switchToDatasetMode() {
    const seed = datasets[0];
    if (!seed) return;
    const query = createDefaultDatasetBlockQuery(seed);
    onChange({
      dataset: query,
      sourceKind: undefined,
      sourceEntity: undefined,
      workflowId: undefined,
      instanceId: undefined,
      sourceInterval: undefined,
      sourceGranularity: undefined,
      sourceOrderBy: undefined,
      sourceLimit: undefined,
      sourceJoins: undefined,
      sourceCondition: undefined,
      schema: undefined,
      fields: [],
      fieldConfigs: undefined,
      chartGroupBy: undefined,
      metricField: undefined,
      metricAggregate: undefined,
    });
  }

  function switchToSchemaMode() {
    onChange({
      dataset: undefined,
      sourceKind: undefined,
      sourceEntity: undefined,
      workflowId: undefined,
      instanceId: undefined,
      sourceInterval: undefined,
      sourceGranularity: undefined,
      sourceOrderBy: undefined,
      sourceLimit: undefined,
      sourceJoins: undefined,
      sourceCondition: undefined,
      schema: schemas[0]?.name,
      fields: [],
      fieldConfigs: undefined,
      chartGroupBy: undefined,
      metricField: undefined,
    });
  }

  function switchToWorkflowRuntimeMode() {
    onChange({
      dataset: undefined,
      sourceKind: 'workflow_runtime',
      sourceEntity: 'instances',
      workflowId: block.workflowId ?? '',
      instanceId: undefined,
      sourceInterval: undefined,
      sourceGranularity: undefined,
      sourceOrderBy: undefined,
      sourceLimit: undefined,
      sourceJoins: undefined,
      sourceCondition: undefined,
      schema: undefined,
      fields: WORKFLOW_RUNTIME_FIELDS.instances.slice(0, 5),
      fieldConfigs: undefined,
      chartGroupBy: undefined,
      metricField: undefined,
    });
  }

  function switchToSystemMode() {
    onChange({
      dataset: undefined,
      sourceKind: 'system',
      sourceEntity: 'runtime_system_snapshot',
      workflowId: undefined,
      instanceId: undefined,
      sourceInterval: undefined,
      sourceGranularity: undefined,
      sourceOrderBy: undefined,
      sourceLimit: undefined,
      sourceJoins: undefined,
      sourceCondition: undefined,
      schema: undefined,
      fields: SYSTEM_FIELDS.runtime_system_snapshot.slice(0, 5),
      fieldConfigs: undefined,
      chartGroupBy: undefined,
      metricField: undefined,
    });
  }

  function changeWorkflowEntity(entity: NonNullable<ReportSource['entity']>) {
    const fields =
      WORKFLOW_RUNTIME_FIELDS[entity] ?? WORKFLOW_RUNTIME_FIELDS.instances;
    onChange({
      sourceEntity: entity,
      fields: fields.slice(0, 5),
      fieldConfigs: undefined,
      chartGroupBy: undefined,
      metricField: undefined,
    });
  }

  function changeSystemEntity(entity: NonNullable<ReportSource['entity']>) {
    const fields =
      SYSTEM_FIELDS[entity] ?? SYSTEM_FIELDS.runtime_system_snapshot;
    onChange({
      sourceEntity: entity,
      fields: fields.slice(0, 5),
      fieldConfigs: undefined,
      chartGroupBy: undefined,
      metricField: undefined,
    });
  }

  function toggleField(field: string) {
    if (block.fields.includes(field)) {
      const nextConfigs = { ...(block.fieldConfigs ?? {}) };
      delete nextConfigs[field];
      onChange({
        fields: block.fields.filter((f) => f !== field),
        fieldConfigs:
          Object.keys(nextConfigs).length > 0 ? nextConfigs : undefined,
      });
    } else {
      onChange({ fields: [...block.fields, field] });
    }
  }

  function addActionColumn(columnType: WizardTableColumnType) {
    const field = makeActionFieldKey();
    const cfg: WizardFieldConfig =
      columnType === 'workflow_button'
        ? {
            columnType,
            label: 'Run workflow',
            workflowAction: createDefaultWorkflowAction('row'),
          }
        : {
            columnType,
            label: 'Actions',
            interactionButtons: [createDefaultInteractionButton()],
          };
    onChange({
      fields: [...block.fields, field],
      fieldConfigs: {
        ...(block.fieldConfigs ?? {}),
        [field]: cfg,
      },
    });
  }

  return (
    <article
      draggable={editing}
      onDragStart={editing ? onDragStart : undefined}
      onDragEnd={editing ? onDragEnd : undefined}
      className={cn(
        'w-full min-w-0 overflow-hidden rounded-md border bg-background shadow-sm transition-shadow',
        editing && 'cursor-grab active:cursor-grabbing',
        isDragging && 'opacity-50'
      )}
    >
      {editing ? (
        <div className="flex items-start justify-between gap-2 border-b bg-muted/20 px-3 py-2">
          <div className="flex min-w-0 flex-1 items-center gap-2">
            <GripVertical className="h-4 w-4 shrink-0 cursor-grab text-muted-foreground" />
            <div className="min-w-0 flex-1">
              <input
                value={block.title}
                placeholder="Untitled block"
                onChange={(event) => onChange({ title: event.target.value })}
                onMouseDown={(event) => event.stopPropagation()}
                onDragStart={(event) => event.preventDefault()}
                draggable={false}
                className="w-full bg-transparent text-sm font-semibold placeholder:text-muted-foreground focus:outline-none"
                style={{
                  border: 'none',
                  outline: 'none',
                  boxShadow: 'none',
                  paddingLeft: 4,
                  paddingRight: 4,
                  paddingTop: 2,
                  paddingBottom: 2,
                }}
              />
              <Select
                value={block.type}
                onValueChange={(value) =>
                  onChange({ type: value as WizardBlockType })
                }
              >
                <SelectTrigger
                  onMouseDown={(event) => event.stopPropagation()}
                  className="ml-1 h-auto w-fit gap-1 border-0 bg-transparent p-0 text-[11px] uppercase tracking-wider text-muted-foreground shadow-none focus:ring-0 focus:ring-offset-0 focus-visible:ring-0 focus-visible:ring-offset-0 [&>svg]:h-3 [&>svg]:w-3 [&>svg]:opacity-60"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {WIZARD_BLOCK_TYPES.map((option) => (
                    <SelectItem key={option.value} value={option.value}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="h-7 w-7"
              onClick={onToggle}
              aria-label={open ? 'Collapse block' : 'Configure block'}
            >
              {open ? (
                <ChevronUp className="h-3.5 w-3.5" />
              ) : (
                <Settings2 className="h-3.5 w-3.5" />
              )}
            </Button>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="h-7 w-7"
              onClick={onRemove}
              aria-label="Remove block"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      ) : block.title ? (
        <div className="border-b px-3 py-2 text-sm font-semibold">
          {block.title}
        </div>
      ) : null}

      {open ? (
        <div className="grid gap-3 px-3 py-3">
          {supportsDataset ? (
            <div className="grid gap-1.5">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Data source
              </Label>
              <div className="grid grid-cols-2 gap-1 rounded-md border bg-muted/10 p-0.5 sm:grid-cols-4">
                <button
                  type="button"
                  onClick={
                    sourceMode === 'schema' ? undefined : switchToSchemaMode
                  }
                  className={cn(
                    'flex-1 rounded px-2 py-1 text-xs font-medium transition-colors',
                    sourceMode === 'schema'
                      ? 'bg-background shadow-sm'
                      : 'text-muted-foreground hover:text-foreground'
                  )}
                >
                  Schema
                </button>
                <button
                  type="button"
                  onClick={
                    sourceMode === 'dataset' || datasets.length === 0
                      ? undefined
                      : switchToDatasetMode
                  }
                  disabled={sourceMode !== 'dataset' && datasets.length === 0}
                  className={cn(
                    'flex-1 rounded px-2 py-1 text-xs font-medium transition-colors',
                    sourceMode === 'dataset'
                      ? 'bg-background shadow-sm'
                      : 'text-muted-foreground hover:text-foreground',
                    sourceMode !== 'dataset' &&
                      datasets.length === 0 &&
                      'cursor-not-allowed opacity-50'
                  )}
                  title={
                    datasets.length === 0 && sourceMode !== 'dataset'
                      ? 'Add a dataset in the Datasets section first'
                      : undefined
                  }
                >
                  Dataset
                </button>
                <button
                  type="button"
                  onClick={
                    sourceMode === 'workflow_runtime'
                      ? undefined
                      : switchToWorkflowRuntimeMode
                  }
                  disabled={!supportsVirtualSources}
                  className={cn(
                    'flex-1 rounded px-2 py-1 text-xs font-medium transition-colors',
                    sourceMode === 'workflow_runtime'
                      ? 'bg-background shadow-sm'
                      : 'text-muted-foreground hover:text-foreground',
                    !supportsVirtualSources && 'cursor-not-allowed opacity-50'
                  )}
                >
                  Workflow
                </button>
                <button
                  type="button"
                  onClick={
                    sourceMode === 'system' ? undefined : switchToSystemMode
                  }
                  disabled={!supportsVirtualSources}
                  className={cn(
                    'flex-1 rounded px-2 py-1 text-xs font-medium transition-colors',
                    sourceMode === 'system'
                      ? 'bg-background shadow-sm'
                      : 'text-muted-foreground hover:text-foreground',
                    !supportsVirtualSources && 'cursor-not-allowed opacity-50'
                  )}
                >
                  System
                </button>
              </div>
            </div>
          ) : null}

          {usingDataset ? (
            <BlockDatasetQueryEditor
              block={block}
              datasets={datasets}
              onChange={onChange}
            />
          ) : null}

          {block.sourceKind === 'workflow_runtime' ? (
            <WorkflowRuntimeSourceSettings
              block={block}
              onChange={onChange}
              onEntityChange={changeWorkflowEntity}
            />
          ) : null}

          {block.sourceKind === 'system' ? (
            <SystemSourceSettings
              block={block}
              onEntityChange={changeSystemEntity}
            />
          ) : null}

          {!usingDataset && block.type !== 'markdown' ? (
            <SourceQuerySettings
              block={block}
              fields={schemaFields}
              showBucketing={block.sourceKind !== 'workflow_runtime'}
              onChange={onChange}
            />
          ) : null}

          {block.type === 'table' ? (
            <TableBehaviorSettings
              block={block}
              fields={tableSortFields}
              disabled={usingDataset && !dataset}
              onChange={onChange}
            />
          ) : null}

          <BlockVisibilitySettings
            block={block}
            filters={filters}
            onChange={onChange}
          />

          {supportsBlockFilters ? (
            <BlockFiltersSettings
              block={block}
              fields={schemaFields}
              onChange={onChange}
            />
          ) : null}

          {block.type === 'table' || block.type === 'chart' ? (
            <BlockInteractionsSettings
              block={block}
              fields={interactionFields}
              filters={filters}
              onChange={onChange}
            />
          ) : null}

          {needsSchema ? (
            <div className="grid gap-1.5">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Schema
              </Label>
              <Select value={block.schema ?? ''} onValueChange={changeSchema}>
                <SelectTrigger>
                  <SelectValue placeholder="Select schema" />
                </SelectTrigger>
                <SelectContent>
                  {schemas.map((schema) => (
                    <SelectItem key={schema.id} value={schema.name}>
                      {schema.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          ) : null}

          {supportsObjectModelSourceTools ? (
            <SourceJoinsSettings
              block={block}
              schemas={schemas}
              baseFields={fieldsOfSchema(schemas, block.schema)}
              onChange={onChange}
            />
          ) : null}

          {supportsObjectModelSourceTools ? (
            <SourceConditionSettings block={block} onChange={onChange} />
          ) : null}

          {block.type === 'markdown' ? (
            <div className="grid gap-1.5">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Markdown
              </Label>
              <Textarea
                rows={3}
                value={
                  block.markdownContent ??
                  (block.title ? `# ${block.title}` : '')
                }
                onChange={(event) =>
                  onChange({ markdownContent: event.target.value })
                }
              />
            </div>
          ) : null}

          {!usingDataset && block.type === 'metric' ? (
            <div className="grid gap-2">
              <div className="grid gap-2 sm:grid-cols-3">
                <div className="grid gap-1.5">
                  <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Aggregate
                  </Label>
                  <Select
                    value={block.metricAggregate ?? 'count'}
                    onValueChange={(value) => {
                      const op = value as ReportAggregateFn;
                      onChange({
                        metricAggregate: op,
                        metricField: aggregateOpNeedsField(op)
                          ? block.metricField
                          : undefined,
                        metricDistinct:
                          op === 'expr' ? undefined : block.metricDistinct,
                        metricPercentile: aggregateOpIsPercentile(op)
                          ? block.metricPercentile
                          : undefined,
                        metricExpression:
                          op === 'expr' ? block.metricExpression : undefined,
                      });
                    }}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {METRIC_AGGREGATES.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                {aggregateOpNeedsField(block.metricAggregate ?? 'count') ? (
                  <div className="grid gap-1.5">
                    <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Field
                    </Label>
                    <Select
                      value={block.metricField ?? schemaFields[0] ?? ''}
                      onValueChange={(value) =>
                        onChange({ metricField: value })
                      }
                    >
                      <SelectTrigger>
                        <SelectValue placeholder="Select field" />
                      </SelectTrigger>
                      <SelectContent>
                        {schemaFields.map((field) => (
                          <SelectItem key={field} value={field}>
                            {field}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                ) : null}
                <div className="grid gap-1.5">
                  <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Format
                  </Label>
                  <Select
                    value={block.metricFormat ?? 'number'}
                    onValueChange={(value) =>
                      onChange({ metricFormat: value as WizardColumnFormat })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {WIZARD_METRIC_FORMATS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              </div>
              <AggregateOptionsSettings
                op={block.metricAggregate ?? 'count'}
                distinct={block.metricDistinct}
                percentile={block.metricPercentile}
                expression={block.metricExpression}
                onChange={(patch) =>
                  onChange({
                    metricDistinct: patch.distinct,
                    metricPercentile: patch.percentile,
                    metricExpression: patch.expression,
                  })
                }
              />
            </div>
          ) : null}

          {!usingDataset && block.type === 'chart' ? (
            <div className="grid gap-2">
              <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
                <div className="grid gap-1.5">
                  <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Chart style
                  </Label>
                  <Select
                    value={block.chartKind ?? 'bar'}
                    onValueChange={(value) =>
                      onChange({
                        chartKind: value as WizardBlock['chartKind'],
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {CHART_KINDS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-1.5">
                  <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Group by
                  </Label>
                  <Select
                    value={block.chartGroupBy ?? schemaFields[0] ?? ''}
                    onValueChange={(value) => onChange({ chartGroupBy: value })}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Select field" />
                    </SelectTrigger>
                    <SelectContent>
                      {schemaFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-1.5">
                  <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Aggregate
                  </Label>
                  <Select
                    value={block.chartAggregate ?? 'count'}
                    onValueChange={(value) => {
                      const op = value as ReportAggregateFn;
                      onChange({
                        chartAggregate: op,
                        chartAggregateField: aggregateOpNeedsField(op)
                          ? block.chartAggregateField
                          : undefined,
                        chartAggregateDistinct:
                          op === 'expr'
                            ? undefined
                            : block.chartAggregateDistinct,
                        chartAggregatePercentile: aggregateOpIsPercentile(op)
                          ? block.chartAggregatePercentile
                          : undefined,
                        chartAggregateExpression:
                          op === 'expr'
                            ? block.chartAggregateExpression
                            : undefined,
                      });
                    }}
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {METRIC_AGGREGATES.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                {aggregateOpNeedsField(block.chartAggregate ?? 'count') ? (
                  <div className="grid gap-1.5">
                    <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Aggregate field
                    </Label>
                    <Select
                      value={block.chartAggregateField ?? schemaFields[0] ?? ''}
                      onValueChange={(value) =>
                        onChange({ chartAggregateField: value })
                      }
                    >
                      <SelectTrigger>
                        <SelectValue placeholder="Select field" />
                      </SelectTrigger>
                      <SelectContent>
                        {schemaFields.map((field) => (
                          <SelectItem key={field} value={field}>
                            {field}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                ) : null}
              </div>
              <AggregateOptionsSettings
                op={block.chartAggregate ?? 'count'}
                distinct={block.chartAggregateDistinct}
                percentile={block.chartAggregatePercentile}
                expression={block.chartAggregateExpression}
                onChange={(patch) =>
                  onChange({
                    chartAggregateDistinct: patch.distinct,
                    chartAggregatePercentile: patch.percentile,
                    chartAggregateExpression: patch.expression,
                  })
                }
              />
            </div>
          ) : null}

          {supportsFields ? (
            <FieldPicker
              block={block}
              schemaFields={schemaFields}
              schemas={schemas}
              onToggleField={toggleField}
              onAddActionColumn={
                block.type === 'table' ? addActionColumn : undefined
              }
              onUpdateFieldConfig={(field, patch) => {
                const merged: WizardFieldConfig = {
                  ...(block.fieldConfigs?.[field] ?? {}),
                  ...patch,
                };
                const next = {
                  ...(block.fieldConfigs ?? {}),
                  [field]: merged,
                };
                if (
                  !merged.format &&
                  !merged.label &&
                  !merged.pillVariants &&
                  !merged.columnType &&
                  !merged.workflowAction &&
                  (!merged.interactionButtons ||
                    merged.interactionButtons.length === 0) &&
                  !merged.editable &&
                  !merged.editor
                ) {
                  delete next[field];
                }
                onChange({
                  fieldConfigs: Object.keys(next).length > 0 ? next : undefined,
                });
              }}
            />
          ) : null}
          {!usingDataset && block.type === 'table' ? (
            <TableSelectionAndBulkActions
              block={block}
              schemaFields={schemaFields.filter(
                (field) => !isActionFieldKey(field)
              )}
              onChange={onChange}
            />
          ) : null}
        </div>
      ) : editing ? (
        <button
          type="button"
          onClick={onToggle}
          aria-label={`Reconfigure ${block.title || 'block'}`}
          className="group w-full cursor-pointer text-left transition-colors hover:bg-muted/20"
        >
          <BlockPreview block={block} result={result} datasets={datasets} />
        </button>
      ) : (
        <BlockPreview block={block} result={result} datasets={datasets} />
      )}
    </article>
  );
}

function FieldPicker({
  block,
  schemaFields,
  schemas,
  onToggleField,
  onAddActionColumn,
  onUpdateFieldConfig,
}: {
  block: WizardBlock;
  schemaFields: string[];
  schemas: Schema[];
  onToggleField: (field: string) => void;
  onAddActionColumn?: (columnType: WizardTableColumnType) => void;
  onUpdateFieldConfig: (
    field: string,
    patch: Partial<WizardFieldConfig>
  ) => void;
}) {
  const formatChoices = block.type === 'chart' ? null : WIZARD_COLUMN_FORMATS;
  const isTable = block.type === 'table';
  const availableFields = schemaFields.filter(
    (field) => !block.fields.includes(field)
  );
  // The schema field-list for `valueFrom` selectors etc.
  const schemaOnlyFields = block.fields.filter(
    (field) => !isActionFieldKey(field)
  );

  return (
    <div className="grid gap-2">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {block.type === 'chart' ? 'Series' : isTable ? 'Columns' : 'Fields'}
      </Label>
      {block.fields.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No fields yet. Add one below.
        </p>
      ) : isTable ? (
        <table className="w-full text-sm">
          <thead>
            <tr className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
              <th className="py-1 pr-2 text-left font-semibold">Field</th>
              <th className="py-1 pr-2 text-left font-semibold">Label</th>
              <th className="py-1 pr-2 text-left font-semibold">Type</th>
              <th className="py-1 pr-2 text-left font-semibold">Format</th>
              <th className="w-8 py-1" />
            </tr>
          </thead>
          <tbody>
            {block.fields.map((field) => {
              const cfg = block.fieldConfigs?.[field] ?? {};
              return (
                <TableColumnRow
                  key={field}
                  field={field}
                  cfg={cfg}
                  schemaFields={schemaOnlyFields}
                  schemas={schemas}
                  formatChoices={formatChoices}
                  onLabelChange={(label) =>
                    onUpdateFieldConfig(field, { label: label || undefined })
                  }
                  onFormatChange={(value) =>
                    onUpdateFieldConfig(field, {
                      format:
                        value === 'plain'
                          ? undefined
                          : (value as WizardColumnFormat),
                      pillVariants:
                        value === 'pill' ? cfg.pillVariants : undefined,
                    })
                  }
                  onPillVariantsChange={(variants) =>
                    onUpdateFieldConfig(field, { pillVariants: variants })
                  }
                  onColumnTypeChange={(columnType) =>
                    onUpdateFieldConfig(field, {
                      columnType,
                      // Seed default config when switching to action columns.
                      workflowAction:
                        columnType === 'workflow_button'
                          ? (cfg.workflowAction ??
                            createDefaultWorkflowAction('row'))
                          : undefined,
                      interactionButtons:
                        columnType === 'interaction_buttons'
                          ? cfg.interactionButtons &&
                            cfg.interactionButtons.length > 0
                            ? cfg.interactionButtons
                            : [createDefaultInteractionButton()]
                          : undefined,
                      // Drop value-only config when switching away from value.
                      format: columnType === 'value' ? cfg.format : undefined,
                      editable:
                        columnType === 'value' ? cfg.editable : undefined,
                      editor: columnType === 'value' ? cfg.editor : undefined,
                    })
                  }
                  onWritebackChange={(patch) =>
                    onUpdateFieldConfig(field, patch)
                  }
                  onWorkflowActionChange={(workflowAction) =>
                    onUpdateFieldConfig(field, { workflowAction })
                  }
                  onInteractionButtonsChange={(interactionButtons) =>
                    onUpdateFieldConfig(field, { interactionButtons })
                  }
                  onRemove={() => onToggleField(field)}
                />
              );
            })}
          </tbody>
        </table>
      ) : (
        <table className="w-full text-sm">
          <thead>
            <tr className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
              <th className="py-1 pr-3 text-left font-semibold">Field</th>
              <th className="py-1 pr-3 text-left font-semibold">Label</th>
              {formatChoices ? (
                <th className="py-1 pr-3 text-left font-semibold">Format</th>
              ) : null}
              <th className="w-8 py-1" />
            </tr>
          </thead>
          <tbody>
            {block.fields.map((field) => {
              const cfg = block.fieldConfigs?.[field] ?? {};
              return (
                <FieldRow
                  key={field}
                  field={field}
                  cfg={cfg}
                  schemaFields={schemaFields}
                  schemas={schemas}
                  formatChoices={formatChoices}
                  onLabelChange={(label) =>
                    onUpdateFieldConfig(field, { label: label || undefined })
                  }
                  onFormatChange={(value) =>
                    onUpdateFieldConfig(field, {
                      format:
                        value === 'plain'
                          ? undefined
                          : (value as WizardColumnFormat),
                      pillVariants:
                        value === 'pill' ? cfg.pillVariants : undefined,
                    })
                  }
                  onPillVariantsChange={(variants) =>
                    onUpdateFieldConfig(field, { pillVariants: variants })
                  }
                  onWritebackChange={(patch) =>
                    onUpdateFieldConfig(field, patch)
                  }
                  onRemove={() => onToggleField(field)}
                />
              );
            })}
          </tbody>
        </table>
      )}
      <div className="flex flex-wrap items-center gap-2">
        {availableFields.length > 0 ? (
          <Select
            value=""
            onValueChange={(value) => {
              if (value) onToggleField(value);
            }}
          >
            <SelectTrigger className="h-8 w-auto min-w-[160px]">
              <SelectValue placeholder="+ Add field" />
            </SelectTrigger>
            <SelectContent>
              {availableFields.map((field) => (
                <SelectItem key={field} value={field}>
                  {humanizeFieldName(field)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : null}
        {isTable && onAddActionColumn ? (
          <>
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="h-8"
              onClick={() => onAddActionColumn('workflow_button')}
            >
              <Plus className="mr-1 h-3 w-3" />
              Workflow column
            </Button>
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="h-8"
              onClick={() => onAddActionColumn('interaction_buttons')}
            >
              <Plus className="mr-1 h-3 w-3" />
              Interaction column
            </Button>
          </>
        ) : null}
      </div>
    </div>
  );
}

const COLUMN_TYPE_LABELS: Record<WizardTableColumnType, string> = {
  value: 'Value',
  workflow_button: 'Workflow button',
  interaction_buttons: 'Interaction buttons',
};

function TableColumnRow({
  field,
  cfg,
  schemaFields,
  schemas,
  formatChoices,
  onLabelChange,
  onFormatChange,
  onPillVariantsChange,
  onColumnTypeChange,
  onWritebackChange,
  onWorkflowActionChange,
  onInteractionButtonsChange,
  onRemove,
}: {
  field: string;
  cfg: WizardFieldConfig;
  schemaFields: string[];
  schemas: Schema[];
  formatChoices: typeof WIZARD_COLUMN_FORMATS | null;
  onLabelChange: (label: string) => void;
  onFormatChange: (value: string) => void;
  onPillVariantsChange: (variants: Record<string, WizardPillVariant>) => void;
  onColumnTypeChange: (columnType: WizardTableColumnType) => void;
  onWritebackChange: (patch: Partial<WizardFieldConfig>) => void;
  onWorkflowActionChange: (action: ReportWorkflowActionConfig) => void;
  onInteractionButtonsChange: (
    buttons: ReportTableInteractionButtonConfig[]
  ) => void;
  onRemove: () => void;
}) {
  const columnType = cfg.columnType ?? 'value';
  const isAction = isActionFieldKey(field);
  const showPillVariants = columnType === 'value' && cfg.format === 'pill';
  const showEditorSettings = columnType === 'value' && !isAction;
  const showWorkflowEditor = columnType === 'workflow_button';
  const showInteractionEditor = columnType === 'interaction_buttons';
  const expansionRow =
    showPillVariants ||
    showEditorSettings ||
    showWorkflowEditor ||
    showInteractionEditor;
  // For action columns the field cell shows the column-type label instead of
  // a row-field name; format isn't applicable so we render an em-dash.
  const fieldLabel = isAction ? COLUMN_TYPE_LABELS[columnType] : field;

  return (
    <>
      <tr className="border-t">
        <td className="py-1.5 pr-2 align-middle">
          <span className="font-mono text-xs">{fieldLabel}</span>
        </td>
        <td className="py-1.5 pr-2 align-middle">
          <Input
            placeholder={isAction ? 'Actions' : humanizeFieldName(field)}
            value={cfg.label ?? ''}
            onChange={(event) => onLabelChange(event.target.value)}
            className="h-7"
          />
        </td>
        <td className="py-1.5 pr-2 align-middle">
          <Select value={columnType} onValueChange={onColumnTypeChange}>
            <SelectTrigger className="h-7">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="value" disabled={isAction}>
                Value
              </SelectItem>
              <SelectItem value="workflow_button">Workflow button</SelectItem>
              <SelectItem value="interaction_buttons">
                Interaction buttons
              </SelectItem>
            </SelectContent>
          </Select>
        </td>
        <td className="py-1.5 pr-2 align-middle">
          {columnType === 'value' && formatChoices ? (
            <Select
              value={cfg.format ?? 'plain'}
              onValueChange={onFormatChange}
            >
              <SelectTrigger className="h-7">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {formatChoices.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : (
            <span className="text-xs text-muted-foreground">—</span>
          )}
        </td>
        <td className="py-1.5 text-right align-middle">
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="h-7 w-7"
            onClick={onRemove}
            aria-label={`Remove ${field}`}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </td>
      </tr>
      {expansionRow ? (
        <tr>
          <td colSpan={5} className="pb-2">
            {showPillVariants ? (
              <PillVariantsEditor
                variants={cfg.pillVariants ?? {}}
                onChange={onPillVariantsChange}
              />
            ) : null}
            {showEditorSettings ? (
              <WritebackEditorSettings
                cfg={cfg}
                field={field}
                schemaFields={schemaFields}
                schemas={schemas}
                onChange={onWritebackChange}
              />
            ) : null}
            {showWorkflowEditor ? (
              <WorkflowActionEditor
                action={
                  cfg.workflowAction ?? createDefaultWorkflowAction('row')
                }
                fields={schemaFields}
                onChange={onWorkflowActionChange}
              />
            ) : null}
            {showInteractionEditor ? (
              <InteractionButtonsEditor
                buttons={cfg.interactionButtons ?? []}
                fields={schemaFields}
                onChange={onInteractionButtonsChange}
              />
            ) : null}
          </td>
        </tr>
      ) : null}
    </>
  );
}

function BlockVisibilitySettings({
  block,
  filters,
  onChange,
}: {
  block: WizardBlock;
  filters: WizardFilter[];
  onChange: (patch: Partial<WizardBlock>) => void;
}) {
  const conditionMode = visibilityConditionMode(block.showWhen);
  const compareValue =
    conditionMode === 'equals'
      ? block.showWhen?.equals
      : conditionMode === 'not_equals'
        ? block.showWhen?.notEquals
        : undefined;

  function updateFilter(filterId: string) {
    if (filterId === ALWAYS_VISIBLE) {
      onChange({ showWhen: undefined });
      return;
    }
    onChange({ showWhen: { filter: filterId, exists: true } });
  }

  function updateConditionMode(mode: string) {
    if (!block.showWhen?.filter) return;
    if (mode === 'exists') {
      onChange({ showWhen: { filter: block.showWhen.filter, exists: true } });
      return;
    }
    if (mode === 'missing') {
      onChange({ showWhen: { filter: block.showWhen.filter, exists: false } });
      return;
    }
    if (mode === 'equals') {
      onChange({
        showWhen: {
          filter: block.showWhen.filter,
          equals: stringVisibilityValue(compareValue),
        },
      });
      return;
    }
    if (mode === 'not_equals') {
      onChange({
        showWhen: {
          filter: block.showWhen.filter,
          notEquals: stringVisibilityValue(compareValue),
        },
      });
    }
  }

  function updateCompareValue(value: string) {
    if (!block.showWhen?.filter) return;
    if (conditionMode === 'equals') {
      onChange({ showWhen: { filter: block.showWhen.filter, equals: value } });
    } else if (conditionMode === 'not_equals') {
      onChange({
        showWhen: { filter: block.showWhen.filter, notEquals: value },
      });
    }
  }

  return (
    <div className="grid gap-3 rounded-md border bg-muted/10 p-3">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        Block behavior
      </Label>
      <div className="grid gap-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)]">
        <label className="flex min-h-10 items-center gap-2 rounded-md border bg-background px-3 text-sm">
          <Checkbox
            checked={Boolean(block.lazy)}
            onCheckedChange={(checked) => onChange({ lazy: Boolean(checked) })}
          />
          Lazy load
        </label>
        <label className="flex min-h-10 items-center gap-2 rounded-md border bg-background px-3 text-sm">
          <Checkbox
            checked={Boolean(block.hideWhenEmpty)}
            onCheckedChange={(checked) =>
              onChange({ hideWhenEmpty: Boolean(checked) })
            }
          />
          Hide when empty
        </label>
        <div className="grid gap-1.5">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Visibility filter
          </Label>
          <Select
            value={block.showWhen?.filter ?? ALWAYS_VISIBLE}
            onValueChange={updateFilter}
          >
            <SelectTrigger className="h-8">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALWAYS_VISIBLE}>Always visible</SelectItem>
              {filters.map((filter) => (
                <SelectItem key={filter.id} value={filter.id}>
                  {filter.label || filter.id}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>
      {block.showWhen?.filter ? (
        <div className="grid gap-3 md:grid-cols-2">
          <div className="grid gap-1.5">
            <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
              Show when
            </Label>
            <Select value={conditionMode} onValueChange={updateConditionMode}>
              <SelectTrigger className="h-8">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="exists">Filter has a value</SelectItem>
                <SelectItem value="missing">Filter is empty</SelectItem>
                <SelectItem value="equals">Filter equals</SelectItem>
                <SelectItem value="not_equals">
                  Filter does not equal
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          {conditionMode === 'equals' || conditionMode === 'not_equals' ? (
            <div className="grid gap-1.5">
              <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                Compare value
              </Label>
              <Input
                value={stringVisibilityValue(compareValue)}
                onChange={(event) => updateCompareValue(event.target.value)}
                className="h-8"
              />
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function visibilityConditionMode(
  showWhen: WizardBlock['showWhen']
): 'exists' | 'missing' | 'equals' | 'not_equals' {
  if (!showWhen) return 'exists';
  if (showWhen.equals !== undefined) return 'equals';
  if (showWhen.notEquals !== undefined) return 'not_equals';
  if (showWhen.exists === false) return 'missing';
  return 'exists';
}

function stringVisibilityValue(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

function WorkflowRuntimeSourceSettings({
  block,
  onChange,
  onEntityChange,
}: {
  block: WizardBlock;
  onChange: (patch: Partial<WizardBlock>) => void;
  onEntityChange: (entity: NonNullable<ReportSource['entity']>) => void;
}) {
  return (
    <div className="grid gap-3 rounded-md border bg-muted/10 p-3 md:grid-cols-3">
      <div className="grid gap-1.5">
        <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          Workflow ID
        </Label>
        <Input
          value={block.workflowId ?? ''}
          onChange={(event) => onChange({ workflowId: event.target.value })}
          className="h-8"
          placeholder="workflow_id"
        />
      </div>
      <div className="grid gap-1.5">
        <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          Entity
        </Label>
        <Select
          value={block.sourceEntity ?? 'instances'}
          onValueChange={(entity) =>
            onEntityChange(entity as NonNullable<ReportSource['entity']>)
          }
        >
          <SelectTrigger className="h-8">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {WORKFLOW_RUNTIME_ENTITIES.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="grid gap-1.5">
        <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          Instance ID
        </Label>
        <Input
          value={block.instanceId ?? ''}
          onChange={(event) =>
            onChange({ instanceId: event.target.value || undefined })
          }
          className="h-8"
          placeholder="optional"
        />
      </div>
    </div>
  );
}

function SystemSourceSettings({
  block,
  onEntityChange,
}: {
  block: WizardBlock;
  onEntityChange: (entity: NonNullable<ReportSource['entity']>) => void;
}) {
  return (
    <div className="grid gap-3 rounded-md border bg-muted/10 p-3 md:grid-cols-3">
      <div className="grid gap-1.5">
        <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          System entity
        </Label>
        <Select
          value={block.sourceEntity ?? 'runtime_system_snapshot'}
          onValueChange={(entity) =>
            onEntityChange(entity as NonNullable<ReportSource['entity']>)
          }
        >
          <SelectTrigger className="h-8">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {SYSTEM_ENTITIES.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </div>
  );
}

function SourceQuerySettings({
  block,
  fields,
  showBucketing,
  onChange,
}: {
  block: WizardBlock;
  fields: string[];
  showBucketing: boolean;
  onChange: (patch: Partial<WizardBlock>) => void;
}) {
  const primaryOrder = block.sourceOrderBy?.[0];
  const trailingOrders = block.sourceOrderBy?.slice(1) ?? [];
  const orderFields =
    primaryOrder?.field && !fields.includes(primaryOrder.field)
      ? [primaryOrder.field, ...fields]
      : fields;

  function updateOrderField(field: string) {
    if (field === NO_SORT_FIELD) {
      onChange({ sourceOrderBy: undefined });
      return;
    }
    onChange({
      sourceOrderBy: [
        {
          field,
          direction: primaryOrder?.direction ?? 'asc',
        },
        ...trailingOrders,
      ],
    });
  }

  function updateOrderDirection(direction: ReportOrderBy['direction']) {
    if (!primaryOrder?.field) return;
    onChange({
      sourceOrderBy: [{ ...primaryOrder, direction }, ...trailingOrders],
    });
  }

  return (
    <div className="grid gap-3 rounded-md border bg-muted/10 p-3">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        Source query
      </Label>
      <div className="grid gap-3 md:grid-cols-4">
        <div className="grid gap-1.5">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Order by
          </Label>
          <Select
            value={primaryOrder?.field ?? NO_SORT_FIELD}
            disabled={orderFields.length === 0}
            onValueChange={updateOrderField}
          >
            <SelectTrigger className="h-8">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NO_SORT_FIELD}>None</SelectItem>
              {orderFields.map((field) => (
                <SelectItem key={field} value={field}>
                  {field}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Direction
          </Label>
          <Select
            value={primaryOrder?.direction ?? 'asc'}
            disabled={!primaryOrder?.field}
            onValueChange={(direction) =>
              updateOrderDirection(direction as ReportOrderBy['direction'])
            }
          >
            <SelectTrigger className="h-8">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="asc">Ascending</SelectItem>
              <SelectItem value="desc">Descending</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Limit
          </Label>
          <Input
            type="number"
            min={1}
            value={block.sourceLimit ?? ''}
            onChange={(event) =>
              onChange({
                sourceLimit: optionalPositiveInteger(event.target.value),
              })
            }
            className="h-8"
            placeholder="No limit"
          />
        </div>
        {showBucketing ? (
          <>
            <div className="grid gap-1.5">
              <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                Interval
              </Label>
              <Input
                value={block.sourceInterval ?? ''}
                onChange={(event) =>
                  onChange({
                    sourceInterval: event.target.value || undefined,
                  })
                }
                className="h-8"
                placeholder="24h"
              />
            </div>
            <div className="grid gap-1.5">
              <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                Granularity
              </Label>
              <Input
                value={block.sourceGranularity ?? ''}
                onChange={(event) =>
                  onChange({
                    sourceGranularity: event.target.value || undefined,
                  })
                }
                className="h-8"
                placeholder="hourly"
              />
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}

function SourceJoinsSettings({
  block,
  schemas,
  baseFields,
  onChange,
}: {
  block: WizardBlock;
  schemas: Schema[];
  baseFields: string[];
  onChange: (patch: Partial<WizardBlock>) => void;
}) {
  const joins = block.sourceJoins ?? [];

  function updateJoin(index: number, patch: Partial<ReportSourceJoin>) {
    onChange({
      sourceJoins: joins.map((join, currentIndex) =>
        currentIndex === index ? { ...join, ...patch } : join
      ),
    });
  }

  function addJoin() {
    const schema =
      schemas.find((candidate) => candidate.name !== block.schema)?.name ??
      schemas[0]?.name ??
      '';
    const joinFields = fieldsOfSchema(schemas, schema);
    const next: ReportSourceJoin = {
      schema,
      parentField: baseFields[0] ?? 'id',
      field: joinFields[0] ?? 'id',
      op: 'eq',
      kind: 'left',
    };
    onChange({ sourceJoins: [...joins, next] });
  }

  function removeJoin(index: number) {
    const next = joins.filter((_, currentIndex) => currentIndex !== index);
    onChange({ sourceJoins: next.length > 0 ? next : undefined });
  }

  return (
    <div className="grid gap-3 rounded-md border bg-muted/10 p-3">
      <div className="flex items-center justify-between gap-2">
        <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Source joins
        </Label>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7"
          onClick={addJoin}
          disabled={schemas.length === 0 || baseFields.length === 0}
        >
          <Plus className="mr-1 h-3 w-3" />
          Add join
        </Button>
      </div>
      {joins.length === 0 ? (
        <p className="text-xs text-muted-foreground">No schema joins.</p>
      ) : (
        <div className="grid gap-2">
          {joins.map((join, index) => {
            const joinFields = fieldsOfSchema(schemas, join.schema);
            return (
              <div
                key={`${join.schema}-${index}`}
                className="grid gap-2 rounded-md border bg-background p-2 lg:grid-cols-[minmax(0,1fr)_minmax(0,0.8fr)_minmax(0,1fr)_minmax(0,1fr)_80px_80px_auto]"
              >
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Schema
                  </Label>
                  <Select
                    value={join.schema}
                    onValueChange={(schema) => {
                      const fields = fieldsOfSchema(schemas, schema);
                      updateJoin(index, {
                        schema,
                        field: fields[0] ?? join.field,
                      });
                    }}
                  >
                    <SelectTrigger className="h-8">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {schemas.map((schema) => (
                        <SelectItem key={schema.id} value={schema.name}>
                          {schema.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Alias
                  </Label>
                  <Input
                    value={join.alias ?? ''}
                    onChange={(event) =>
                      updateJoin(index, {
                        alias: event.target.value || undefined,
                      })
                    }
                    className="h-8"
                    placeholder={join.schema}
                  />
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Parent field
                  </Label>
                  <Select
                    value={join.parentField}
                    onValueChange={(parentField) =>
                      updateJoin(index, { parentField })
                    }
                  >
                    <SelectTrigger className="h-8">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {baseFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Join field
                  </Label>
                  <Select
                    value={join.field}
                    onValueChange={(field) => updateJoin(index, { field })}
                  >
                    <SelectTrigger className="h-8">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {joinFields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Op
                  </Label>
                  <Input
                    value={join.op ?? 'eq'}
                    onChange={(event) =>
                      updateJoin(index, { op: event.target.value || 'eq' })
                    }
                    className="h-8"
                  />
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Kind
                  </Label>
                  <Select
                    value={join.kind ?? 'left'}
                    onValueChange={(kind) =>
                      updateJoin(index, {
                        kind: kind as NonNullable<ReportSourceJoin['kind']>,
                      })
                    }
                  >
                    <SelectTrigger className="h-8">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="left">Left</SelectItem>
                      <SelectItem value="inner">Inner</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  className="mt-5 h-8 w-8"
                  onClick={() => removeJoin(index)}
                  aria-label="Remove join"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function SourceConditionSettings({
  block,
  onChange,
}: {
  block: WizardBlock;
  onChange: (patch: Partial<WizardBlock>) => void;
}) {
  const [text, setText] = useState(() =>
    formatSourceCondition(block.sourceCondition)
  );
  const [error, setError] = useState<string | null>(null);

  function updateText(value: string) {
    setText(value);
    if (!value.trim()) {
      setError(null);
      onChange({ sourceCondition: undefined });
      return;
    }
    try {
      const parsed = JSON.parse(value) as unknown;
      if (!isReportCondition(parsed)) {
        setError('Condition JSON must be an object with an op string.');
        return;
      }
      setError(null);
      onChange({ sourceCondition: parsed });
    } catch {
      setError('Condition JSON is not valid yet.');
    }
  }

  return (
    <div className="grid gap-2 rounded-md border bg-muted/10 p-3">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        Source condition
      </Label>
      <Textarea
        rows={4}
        value={text}
        onChange={(event) => updateText(event.target.value)}
        placeholder='{"op":"EQ","arguments":["status","open"]}'
        className="font-mono text-xs"
      />
      {error ? <p className="text-xs text-destructive">{error}</p> : null}
    </div>
  );
}

function AggregateOptionsSettings({
  op,
  distinct,
  percentile,
  expression,
  onChange,
}: {
  op: ReportAggregateFn;
  distinct?: boolean;
  percentile?: number;
  expression?: unknown;
  onChange: (patch: {
    distinct?: boolean;
    percentile?: number;
    expression?: unknown;
  }) => void;
}) {
  const showDistinct = op !== 'expr';
  const showPercentile = op === 'percentile_cont' || op === 'percentile_disc';
  const showExpression = op === 'expr';

  if (!showDistinct && !showPercentile && !showExpression) return null;

  return (
    <div className="grid gap-2 rounded-md border bg-muted/10 p-2">
      {showDistinct ? (
        <label className="flex min-h-8 items-center gap-2 text-sm">
          <Checkbox
            checked={Boolean(distinct)}
            onCheckedChange={(checked) =>
              onChange({ distinct: Boolean(checked), percentile, expression })
            }
          />
          Distinct values only
        </label>
      ) : null}
      {showPercentile ? (
        <div className="grid gap-1 sm:max-w-xs">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Percentile (0-1)
          </Label>
          <Input
            type="number"
            min={0}
            max={1}
            step={0.05}
            value={percentile !== undefined ? String(percentile) : '0.5'}
            onChange={(event) =>
              onChange({
                distinct,
                percentile: optionalNumber(event.target.value),
                expression,
              })
            }
            className="h-8"
          />
        </div>
      ) : null}
      {showExpression ? (
        <div className="grid gap-1">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Expression (JSON)
          </Label>
          <Input
            value={formatAggregateExpression(expression)}
            onChange={(event) =>
              onChange({
                distinct,
                percentile,
                expression: parseAggregateExpression(event.target.value),
              })
            }
            className="h-8 font-mono text-xs"
            placeholder='{"op":"divide","args":[...]}'
          />
        </div>
      ) : null}
    </div>
  );
}

function BlockFiltersSettings({
  block,
  fields,
  onChange,
}: {
  block: WizardBlock;
  fields: string[];
  onChange: (patch: Partial<WizardBlock>) => void;
}) {
  const filters = block.filters ?? [];
  const fieldOptions = blockFilterFieldOptions(filters, fields);
  const canAddFilter = fields.length > 0;

  function updateFilter(index: number, filter: ReportFilterDefinition) {
    const nextFilters = filters.map((current, currentIndex) =>
      currentIndex === index ? filter : current
    );
    onChange({ filters: nextFilters.length > 0 ? nextFilters : undefined });
  }

  function addFilter() {
    const field = fieldOptions[0] ?? fields[0] ?? 'id';
    const filter: ReportFilterDefinition = {
      id: uniqueBlockFilterId(filters, field),
      label: humanizeFieldName(field),
      type: 'select',
      appliesTo: [
        {
          blockId: block.id,
          field,
          op: 'eq',
        },
      ],
      options: {
        source: 'static',
        values: [],
      },
    };
    onChange({ filters: [...filters, filter] });
  }

  function removeFilter(index: number) {
    const nextFilters = filters.filter(
      (_, currentIndex) => currentIndex !== index
    );
    onChange({ filters: nextFilters.length > 0 ? nextFilters : undefined });
  }

  return (
    <div className="grid gap-3 rounded-md border bg-muted/10 p-3">
      <div className="flex items-center justify-between gap-2">
        <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Block filters
        </Label>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7"
          onClick={addFilter}
          disabled={!canAddFilter}
        >
          <Plus className="mr-1 h-3 w-3" />
          Add filter
        </Button>
      </div>
      {filters.length === 0 ? (
        <p className="text-xs text-muted-foreground">No block-local filters.</p>
      ) : (
        <div className="grid gap-2">
          {filters.map((filter, index) => {
            const target = firstBlockFilterTarget(
              filter,
              block.id,
              fieldOptions
            );
            const operatorOptions = blockFilterOperatorOptions(target.op);
            return (
              <div
                key={`${filter.id}-${index}`}
                className="grid gap-2 rounded-md border bg-background p-2"
              >
                <div className="grid gap-2 lg:grid-cols-[minmax(0,1fr)_minmax(0,0.9fr)_minmax(0,0.9fr)_minmax(0,0.9fr)_auto]">
                  <div className="grid gap-1">
                    <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Label
                    </Label>
                    <Input
                      value={filter.label}
                      className="h-8"
                      onChange={(event) =>
                        updateFilter(index, {
                          ...filter,
                          label: event.target.value,
                        })
                      }
                    />
                  </div>
                  <div className="grid gap-1">
                    <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Type
                    </Label>
                    <Select
                      value={filter.type}
                      onValueChange={(value) => {
                        const type = value as ReportFilterType;
                        updateFilter(index, {
                          ...filter,
                          type,
                          default: undefined,
                          appliesTo: updateBlockFilterTarget(
                            filter,
                            block.id,
                            fieldOptions,
                            {
                              op: defaultOperatorForBlockFilter(type),
                            }
                          ),
                          options: blockFilterUsesOptions(type)
                            ? {
                                source: 'static',
                                values: filter.options?.values ?? [],
                              }
                            : undefined,
                        });
                      }}
                    >
                      <SelectTrigger className="h-8">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {FILTER_TYPES.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="grid gap-1">
                    <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Field
                    </Label>
                    <Select
                      value={target.field}
                      disabled={fieldOptions.length === 0}
                      onValueChange={(field) =>
                        updateFilter(index, {
                          ...filter,
                          appliesTo: updateBlockFilterTarget(
                            filter,
                            block.id,
                            fieldOptions,
                            { field }
                          ),
                        })
                      }
                    >
                      <SelectTrigger className="h-8">
                        <SelectValue placeholder="Select field" />
                      </SelectTrigger>
                      <SelectContent>
                        {fieldOptions.map((field) => (
                          <SelectItem key={field} value={field}>
                            {field}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="grid gap-1">
                    <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Match
                    </Label>
                    <Select
                      value={
                        target.op ?? defaultOperatorForBlockFilter(filter.type)
                      }
                      onValueChange={(op) =>
                        updateFilter(index, {
                          ...filter,
                          appliesTo: updateBlockFilterTarget(
                            filter,
                            block.id,
                            fieldOptions,
                            { op }
                          ),
                        })
                      }
                    >
                      <SelectTrigger className="h-8">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {operatorOptions.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    className="mt-5 h-8 w-8"
                    onClick={() => removeFilter(index)}
                    aria-label={`Remove ${filter.label || filter.id}`}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
                {blockFilterUsesOptions(filter.type) ? (
                  <div className="grid gap-1">
                    <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Static options
                    </Label>
                    <Textarea
                      rows={2}
                      value={formatBlockFilterOptions(filter)}
                      placeholder={'open=Open\nclosed=Closed'}
                      onChange={(event) =>
                        updateFilter(index, {
                          ...filter,
                          options: {
                            source: 'static',
                            values: parseBlockFilterOptions(event.target.value),
                          },
                        })
                      }
                    />
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function BlockInteractionsSettings({
  block,
  fields,
  filters,
  onChange,
}: {
  block: WizardBlock;
  fields: string[];
  filters: WizardFilter[];
  onChange: (patch: Partial<WizardBlock>) => void;
}) {
  const interactions = block.interactions ?? [];
  const triggerOptions =
    block.type === 'chart'
      ? [{ value: 'point_click', label: 'Point click' }]
      : [
          { value: 'row_click', label: 'Row click' },
          { value: 'cell_click', label: 'Cell click' },
        ];

  function updateInteraction(
    index: number,
    patch: Partial<NonNullable<WizardBlock['interactions']>[number]>
  ) {
    onChange({
      interactions: interactions.map((interaction, currentIndex) =>
        currentIndex === index ? { ...interaction, ...patch } : interaction
      ),
    });
  }

  function addInteraction() {
    onChange({
      interactions: [
        ...interactions,
        createDefaultBlockInteraction(
          triggerOptions[0]?.value ?? 'row_click',
          filters,
          fields
        ),
      ],
    });
  }

  return (
    <div className="grid gap-3 rounded-md border bg-muted/10 p-3">
      <div className="flex items-center justify-between gap-2">
        <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Block interactions
        </Label>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7"
          onClick={addInteraction}
        >
          <Plus className="mr-1 h-3 w-3" />
          Add interaction
        </Button>
      </div>
      {interactions.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No row, cell, or chart click interactions.
        </p>
      ) : (
        <div className="grid gap-2">
          {interactions.map((interaction, index) => (
            <div
              key={`${interaction.id}-${index}`}
              className="grid gap-2 rounded-md border bg-background p-2"
            >
              <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_auto]">
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    ID
                  </Label>
                  <Input
                    value={interaction.id}
                    className="h-8"
                    onChange={(event) =>
                      updateInteraction(index, {
                        id: slugify(event.target.value).replace(/-/g, '_'),
                      })
                    }
                  />
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Trigger
                  </Label>
                  <Select
                    value={interaction.trigger.event}
                    onValueChange={(event) =>
                      updateInteraction(index, {
                        trigger: {
                          ...interaction.trigger,
                          event,
                        },
                      })
                    }
                  >
                    <SelectTrigger className="h-8">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {triggerOptions.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Field
                  </Label>
                  <Select
                    value={interaction.trigger.field ?? '__any__'}
                    disabled={fields.length === 0}
                    onValueChange={(field) =>
                      updateInteraction(index, {
                        trigger: {
                          ...interaction.trigger,
                          field: field === '__any__' ? undefined : field,
                        },
                      })
                    }
                  >
                    <SelectTrigger className="h-8">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__any__">Any field</SelectItem>
                      {fields.map((field) => (
                        <SelectItem key={field} value={field}>
                          {field}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  className="mt-5 h-8 w-8"
                  onClick={() =>
                    onChange({
                      interactions: interactions.filter(
                        (_, currentIndex) => currentIndex !== index
                      ),
                    })
                  }
                  aria-label={`Remove ${interaction.id}`}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
              <InteractionActionsList
                actions={interaction.actions}
                fields={fields}
                onChange={(actions) => updateInteraction(index, { actions })}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function createDefaultBlockInteraction(
  event: string,
  filters: WizardFilter[],
  fields: string[]
): NonNullable<WizardBlock['interactions']>[number] {
  const field = fields[0] ?? 'id';
  return {
    id: `interaction_${Math.random().toString(36).slice(2, 7)}`,
    trigger: { event },
    actions: [
      {
        type: 'set_filter',
        filterId: filters[0]?.id,
        valueFrom: `datum.${field}`,
      },
    ],
  };
}

function TableBehaviorSettings({
  block,
  fields,
  disabled,
  onChange,
}: {
  block: WizardBlock;
  fields: string[];
  disabled?: boolean;
  onChange: (patch: Partial<WizardBlock>) => void;
}) {
  const defaultSort = block.defaultSort?.[0];
  const trailingSorts = block.defaultSort?.slice(1) ?? [];
  const sortFieldOptions =
    defaultSort?.field && !fields.includes(defaultSort.field)
      ? [defaultSort.field, ...fields]
      : fields;

  function updateDefaultSort(field: string) {
    if (field === NO_SORT_FIELD) {
      onChange({ defaultSort: undefined });
      return;
    }
    onChange({
      defaultSort: [
        {
          field,
          direction: defaultSort?.direction ?? 'asc',
        },
        ...trailingSorts,
      ],
    });
  }

  function updateDefaultSortDirection(direction: ReportOrderBy['direction']) {
    if (!defaultSort?.field) return;
    onChange({
      defaultSort: [{ ...defaultSort, direction }, ...trailingSorts],
    });
  }

  return (
    <div className="grid gap-3 rounded-md border bg-muted/10 p-3">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        Table behavior
      </Label>
      <div className="grid gap-3 md:grid-cols-4">
        <div className="grid gap-1.5">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Default page size
          </Label>
          <Input
            type="number"
            min={1}
            value={block.defaultPageSize ?? 50}
            onChange={(event) =>
              onChange({
                defaultPageSize: positiveIntegerOrDefault(
                  event.target.value,
                  50
                ),
              })
            }
            className="h-8"
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Allowed page sizes
          </Label>
          <Input
            value={(block.allowedPageSizes ?? [25, 50, 100]).join(', ')}
            onChange={(event) =>
              onChange({
                allowedPageSizes: parsePageSizes(event.target.value),
              })
            }
            placeholder="25, 50, 100"
            className="h-8"
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Default sort
          </Label>
          <Select
            value={defaultSort?.field ?? NO_SORT_FIELD}
            disabled={disabled || sortFieldOptions.length === 0}
            onValueChange={updateDefaultSort}
          >
            <SelectTrigger className="h-8">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NO_SORT_FIELD}>None</SelectItem>
              {sortFieldOptions.map((field) => (
                <SelectItem key={field} value={field}>
                  {field}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Sort direction
          </Label>
          <Select
            value={defaultSort?.direction ?? 'asc'}
            disabled={disabled || !defaultSort?.field}
            onValueChange={(direction) =>
              updateDefaultSortDirection(
                direction as ReportOrderBy['direction']
              )
            }
          >
            <SelectTrigger className="h-8">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="asc">Ascending</SelectItem>
              <SelectItem value="desc">Descending</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
    </div>
  );
}

function positiveIntegerOrDefault(value: string, fallback: number): number {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function optionalPositiveInteger(value: string): number | undefined {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}

function aggregateOpNeedsField(op: ReportAggregateFn | undefined): boolean {
  return Boolean(op && op !== 'count' && op !== 'expr');
}

function aggregateOpIsPercentile(op: ReportAggregateFn | undefined): boolean {
  return op === 'percentile_cont' || op === 'percentile_disc';
}

function formatAggregateExpression(expression: unknown): string {
  if (expression === undefined || expression === null) return '';
  if (typeof expression === 'string') return expression;
  return JSON.stringify(expression);
}

function parseAggregateExpression(value: string): unknown {
  if (!value.trim()) return undefined;
  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
}

function parsePageSizes(value: string): number[] {
  return Array.from(
    new Set(
      value
        .split(',')
        .map((part) => Number.parseInt(part.trim(), 10))
        .filter((size) => Number.isFinite(size) && size > 0)
    )
  );
}

function uniqueStrings(values: Array<string | undefined>): string[] {
  return Array.from(
    new Set(values.filter((value): value is string => Boolean(value)))
  );
}

function blockFilterFieldOptions(
  filters: ReportFilterDefinition[],
  fields: string[]
): string[] {
  return uniqueStrings([
    ...fields,
    ...filters.flatMap(
      (filter) => filter.appliesTo?.map((target) => target.field) ?? []
    ),
  ]);
}

function firstBlockFilterTarget(
  filter: ReportFilterDefinition,
  blockId: string,
  fields: string[]
): NonNullable<ReportFilterDefinition['appliesTo']>[number] {
  const target = filter.appliesTo?.[0];
  return {
    blockId: target?.blockId ?? blockId,
    field: target?.field ?? fields[0] ?? 'id',
    op: target?.op ?? defaultOperatorForBlockFilter(filter.type),
  };
}

function updateBlockFilterTarget(
  filter: ReportFilterDefinition,
  blockId: string,
  fields: string[],
  patch: Partial<NonNullable<ReportFilterDefinition['appliesTo']>[number]>
): NonNullable<ReportFilterDefinition['appliesTo']> {
  const current = firstBlockFilterTarget(filter, blockId, fields);
  return [
    {
      ...current,
      ...patch,
      blockId,
    },
    ...(filter.appliesTo?.slice(1) ?? []),
  ];
}

function uniqueBlockFilterId(
  filters: ReportFilterDefinition[],
  seed: string
): string {
  const existingIds = new Set(filters.map((filter) => filter.id));
  const base = slugify(seed || 'filter').replace(/-/g, '_') || 'filter';
  let candidate = `${base}_filter`;
  let suffix = 1;
  while (existingIds.has(candidate)) {
    suffix += 1;
    candidate = `${base}_filter_${suffix}`;
  }
  return candidate;
}

function blockFilterUsesOptions(type: ReportFilterType): boolean {
  return type === 'select' || type === 'multi_select' || type === 'radio';
}

function defaultOperatorForBlockFilter(type: ReportFilterType): string {
  switch (type) {
    case 'multi_select':
      return 'in';
    case 'time_range':
    case 'number_range':
      return 'between';
    case 'search':
    case 'text':
      return 'contains';
    case 'checkbox':
    case 'radio':
    case 'select':
    default:
      return 'eq';
  }
}

function blockFilterOperatorOptions(op: string | undefined) {
  if (!op || BLOCK_FILTER_OPERATORS.some((option) => option.value === op)) {
    return BLOCK_FILTER_OPERATORS;
  }
  return [{ value: op, label: op }, ...BLOCK_FILTER_OPERATORS];
}

function formatBlockFilterOptions(filter: ReportFilterDefinition): string {
  return (
    filter.options?.values
      ?.map((option) => {
        const value = String(option.value);
        return option.label && option.label !== humanizeFieldName(value)
          ? `${value}=${option.label}`
          : value;
      })
      .join('\n') ?? ''
  );
}

function parseBlockFilterOptions(value: string): ReportFilterOption[] {
  return value
    .split(/[\n,]+/)
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => {
      const separator = part.indexOf('=');
      const rawValue = separator >= 0 ? part.slice(0, separator).trim() : part;
      const label =
        separator >= 0 ? part.slice(separator + 1).trim() : undefined;
      return {
        value: rawValue,
        label: label || humanizeFieldName(rawValue),
      };
    });
}

function formatSourceCondition(condition: ReportCondition | undefined): string {
  return condition ? JSON.stringify(condition, null, 2) : '';
}

function isReportCondition(value: unknown): value is ReportCondition {
  return (
    typeof value === 'object' &&
    value !== null &&
    !Array.isArray(value) &&
    typeof (value as { op?: unknown }).op === 'string'
  );
}

function TableSelectionAndBulkActions({
  block,
  schemaFields,
  onChange,
}: {
  block: WizardBlock;
  schemaFields: string[];
  onChange: (patch: Partial<WizardBlock>) => void;
}) {
  const tableActions = block.tableActions ?? [];
  const selectable = Boolean(block.selectable || tableActions.length > 0);
  return (
    <div className="grid gap-2 rounded-md border bg-muted/10 p-3">
      <label className="flex items-start gap-2 text-sm">
        <Checkbox
          checked={selectable}
          // Bulk actions require selectable; only allow disabling when none.
          disabled={tableActions.length > 0}
          onCheckedChange={(checked) =>
            onChange({ selectable: Boolean(checked) })
          }
        />
        <div className="grid gap-0.5">
          <span className="font-medium">Allow selection</span>
          <span className="text-xs text-muted-foreground">
            Show row checkboxes so viewers can pick rows for bulk actions.
            {tableActions.length > 0
              ? ' Bulk actions require selection — remove them first to turn this off.'
              : ''}
          </span>
        </div>
      </label>
      {selectable ? (
        <TableBulkActionsEditor
          actions={tableActions}
          fields={schemaFields}
          onChange={(next) =>
            onChange({
              tableActions: next,
              // Keep selectable on while bulk actions exist.
              selectable: next.length > 0 ? true : block.selectable,
            })
          }
        />
      ) : null}
    </div>
  );
}

function WritebackEditorSettings({
  cfg,
  field,
  schemaFields,
  schemas,
  onChange,
}: {
  cfg: WizardFieldConfig;
  field: string;
  schemaFields: string[];
  schemas: Schema[];
  onChange: (patch: Partial<WizardFieldConfig>) => void;
}) {
  const editor = cfg.editor;
  const editorKind = editor?.kind ?? EDITOR_AUTO;
  const isEditable = Boolean(cfg.editable);

  function setEditable(editable: boolean) {
    onChange({
      editable: editable ? true : undefined,
      editor: editable ? cfg.editor : undefined,
    });
  }

  function setEditorKind(value: string) {
    if (value === EDITOR_AUTO) {
      onChange({ editor: undefined });
      return;
    }
    onChange({
      editable: true,
      editor: defaultEditorConfig(
        value as ReportEditorKind,
        editor,
        field,
        schemaFields,
        schemas
      ),
    });
  }

  function patchEditor(patch: Partial<ReportEditorConfig>) {
    const next =
      editor ??
      defaultEditorConfig('text', undefined, field, schemaFields, schemas);
    onChange({ editable: true, editor: { ...next, ...patch } });
  }

  function patchLookup(
    patch: Partial<NonNullable<ReportEditorConfig['lookup']>>
  ) {
    const next =
      editor ??
      defaultEditorConfig('lookup', undefined, field, schemaFields, schemas);
    const lookup =
      next.lookup ?? defaultLookupConfig(schemas, schemaFields, field);
    onChange({
      editable: true,
      editor: {
        ...next,
        kind: 'lookup',
        lookup: {
          ...lookup,
          ...patch,
        },
      },
    });
  }

  function setLookupSchema(schema: string) {
    const lookupFields = fieldsOfSchema(schemas, schema);
    patchLookup({
      schema,
      valueField: preferredField(lookupFields, ['id']) ?? lookupFields[0] ?? '',
      labelField:
        preferredField(lookupFields, ['name', 'title', 'label', 'id']) ??
        lookupFields[0] ??
        '',
      searchFields: defaultLookupSearchFields(lookupFields),
    });
  }

  const lookup =
    editor?.kind === 'lookup'
      ? (editor.lookup ?? defaultLookupConfig(schemas, schemaFields, field))
      : undefined;
  const lookupFields = lookup ? fieldsOfSchema(schemas, lookup.schema) : [];

  return (
    <div className="mt-2 grid gap-2 rounded-md border bg-muted/10 p-2">
      <label className="flex min-h-8 items-center gap-2 text-sm">
        <Checkbox
          checked={isEditable}
          onCheckedChange={(checked) => setEditable(Boolean(checked))}
        />
        Editable
      </label>
      {isEditable ? (
        <div className="grid gap-2 md:grid-cols-2 xl:grid-cols-4">
          <div className="grid gap-1">
            <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
              Editor
            </Label>
            <Select value={editorKind} onValueChange={setEditorKind}>
              <SelectTrigger className="h-8">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={EDITOR_AUTO}>Infer from value</SelectItem>
                {EDITOR_KINDS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {editor ? (
            <div className="grid gap-1">
              <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                Placeholder
              </Label>
              <Input
                value={editor.placeholder ?? ''}
                className="h-8"
                onChange={(event) =>
                  patchEditor({
                    placeholder: event.target.value || undefined,
                  })
                }
              />
            </div>
          ) : null}
          {editor?.kind === 'number' ? (
            <>
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Min
                </Label>
                <Input
                  type="number"
                  value={numberInputValue(editor.min)}
                  className="h-8"
                  onChange={(event) =>
                    patchEditor({
                      min: optionalNumber(event.target.value),
                    })
                  }
                />
              </div>
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Max
                </Label>
                <Input
                  type="number"
                  value={numberInputValue(editor.max)}
                  className="h-8"
                  onChange={(event) =>
                    patchEditor({
                      max: optionalNumber(event.target.value),
                    })
                  }
                />
              </div>
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Step
                </Label>
                <Input
                  type="number"
                  value={numberInputValue(editor.step)}
                  className="h-8"
                  onChange={(event) =>
                    patchEditor({
                      step: optionalNumber(event.target.value),
                    })
                  }
                />
              </div>
            </>
          ) : null}
          {editor?.kind === 'select' ? (
            <div className="grid gap-1 md:col-span-2 xl:col-span-4">
              <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                Options
              </Label>
              <Textarea
                rows={2}
                value={formatEditorOptions(editor.options)}
                placeholder={'open=Open\nclosed=Closed'}
                onChange={(event) =>
                  patchEditor({
                    options: parseEditorOptions(event.target.value),
                  })
                }
              />
            </div>
          ) : null}
          {editor?.kind === 'lookup' && lookup ? (
            <div className="grid gap-2 md:col-span-2 xl:col-span-4 md:grid-cols-2 xl:grid-cols-4">
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Lookup schema
                </Label>
                <Select
                  value={lookup.schema}
                  onValueChange={setLookupSchema}
                  disabled={schemas.length === 0}
                >
                  <SelectTrigger className="h-8">
                    <SelectValue placeholder="Select schema" />
                  </SelectTrigger>
                  <SelectContent>
                    {schemas.map((schema) => (
                      <SelectItem key={schema.id} value={schema.name}>
                        {schema.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Value field
                </Label>
                <Select
                  value={lookup.valueField}
                  onValueChange={(valueField) => patchLookup({ valueField })}
                  disabled={lookupFields.length === 0}
                >
                  <SelectTrigger className="h-8">
                    <SelectValue placeholder="Select field" />
                  </SelectTrigger>
                  <SelectContent>
                    {lookupFields.map((candidate) => (
                      <SelectItem key={candidate} value={candidate}>
                        {candidate}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Label field
                </Label>
                <Select
                  value={lookup.labelField}
                  onValueChange={(labelField) => patchLookup({ labelField })}
                  disabled={lookupFields.length === 0}
                >
                  <SelectTrigger className="h-8">
                    <SelectValue placeholder="Select field" />
                  </SelectTrigger>
                  <SelectContent>
                    {lookupFields.map((candidate) => (
                      <SelectItem key={candidate} value={candidate}>
                        {candidate}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Search fields
                </Label>
                <Input
                  value={(lookup.searchFields ?? []).join(', ')}
                  className="h-8"
                  onChange={(event) =>
                    patchLookup({
                      searchFields: event.target.value
                        .split(',')
                        .map((value) => value.trim())
                        .filter(Boolean),
                    })
                  }
                  placeholder="name, email"
                />
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function defaultEditorConfig(
  kind: ReportEditorKind,
  current: ReportEditorConfig | undefined,
  field: string,
  schemaFields: string[],
  schemas: Schema[]
): ReportEditorConfig {
  if (current?.kind === kind) return current;
  if (kind === 'select') {
    return { kind, options: current?.options ?? [] };
  }
  if (kind === 'lookup') {
    return {
      kind,
      lookup:
        current?.lookup ?? defaultLookupConfig(schemas, schemaFields, field),
    };
  }
  if (kind === 'number') {
    return {
      kind,
      min: current?.min,
      max: current?.max,
      step: current?.step,
      placeholder: current?.placeholder,
    };
  }
  return {
    kind,
    placeholder: current?.placeholder,
  };
}

function defaultLookupConfig(
  schemas: Schema[],
  schemaFields: string[],
  editedField: string
): NonNullable<ReportEditorConfig['lookup']> {
  const inferredSchemaName =
    schemas.find((schema) => schema.name === editedField.replace(/_id$/, ''))
      ?.name ??
    schemas[0]?.name ??
    '';
  const lookupFields = fieldsOfSchema(schemas, inferredSchemaName);
  const fields = lookupFields.length > 0 ? lookupFields : schemaFields;
  const valueField = preferredField(fields, ['id']) ?? fields[0] ?? '';
  const labelField =
    preferredField(fields, ['name', 'title', 'label', 'email', 'id']) ??
    valueField;
  return {
    schema: inferredSchemaName,
    valueField,
    labelField,
    searchFields: defaultLookupSearchFields(fields),
  };
}

function preferredField(
  fields: string[],
  candidates: string[]
): string | undefined {
  return candidates.find((candidate) => fields.includes(candidate));
}

function defaultLookupSearchFields(fields: string[]): string[] {
  return fields.filter((field) =>
    ['name', 'title', 'label', 'email'].includes(field)
  );
}

function numberInputValue(value: number | undefined): string {
  return Number.isFinite(value) ? String(value) : '';
}

function optionalNumber(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function formatEditorOptions(
  options: ReportEditorOption[] | undefined
): string {
  return (
    options
      ?.map((option) => {
        const value = String(option.value);
        return option.label && option.label !== humanizeFieldName(value)
          ? `${value}=${option.label}`
          : value;
      })
      .join('\n') ?? ''
  );
}

function parseEditorOptions(value: string): ReportEditorOption[] {
  return value
    .split(/[\n,]+/)
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => {
      const separator = part.indexOf('=');
      const rawValue = separator >= 0 ? part.slice(0, separator).trim() : part;
      const label =
        separator >= 0 ? part.slice(separator + 1).trim() : undefined;
      return {
        value: rawValue,
        label: label || humanizeFieldName(rawValue),
      };
    });
}

function FieldRow({
  field,
  cfg,
  schemaFields,
  schemas,
  formatChoices,
  onLabelChange,
  onFormatChange,
  onPillVariantsChange,
  onWritebackChange,
  onRemove,
}: {
  field: string;
  cfg: WizardFieldConfig;
  schemaFields: string[];
  schemas: Schema[];
  formatChoices: typeof WIZARD_COLUMN_FORMATS | null;
  onLabelChange: (label: string) => void;
  onFormatChange: (value: string) => void;
  onPillVariantsChange: (variants: Record<string, WizardPillVariant>) => void;
  onWritebackChange: (patch: Partial<WizardFieldConfig>) => void;
  onRemove: () => void;
}) {
  const showPillVariants = cfg.format === 'pill';
  const showEditorSettings = formatChoices !== null;
  return (
    <>
      <tr className="border-t">
        <td className="py-1.5 pr-3 align-middle">
          <span className="font-mono text-xs">{field}</span>
        </td>
        <td className="py-1.5 pr-3 align-middle">
          <Input
            placeholder={humanizeFieldName(field)}
            value={cfg.label ?? ''}
            onChange={(event) => onLabelChange(event.target.value)}
            className="h-7"
          />
        </td>
        {formatChoices ? (
          <td className="py-1.5 pr-3 align-middle">
            <Select
              value={cfg.format ?? 'plain'}
              onValueChange={onFormatChange}
            >
              <SelectTrigger className="h-7">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {formatChoices.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </td>
        ) : null}
        <td className="py-1.5 text-right align-middle">
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="h-7 w-7"
            onClick={onRemove}
            aria-label={`Remove ${field}`}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </td>
      </tr>
      {showPillVariants || showEditorSettings ? (
        <tr>
          <td colSpan={formatChoices ? 4 : 3} className="pb-2 pl-3">
            {showPillVariants ? (
              <PillVariantsEditor
                variants={cfg.pillVariants ?? {}}
                onChange={onPillVariantsChange}
              />
            ) : null}
            {showEditorSettings ? (
              <WritebackEditorSettings
                cfg={cfg}
                field={field}
                schemaFields={schemaFields}
                schemas={schemas}
                onChange={onWritebackChange}
              />
            ) : null}
          </td>
        </tr>
      ) : null}
    </>
  );
}

function PillVariantsEditor({
  variants,
  onChange,
}: {
  variants: Record<string, WizardPillVariant>;
  onChange: (variants: Record<string, WizardPillVariant>) => void;
}) {
  const entries = Object.entries(variants);
  return (
    <div className="grid gap-1">
      <span className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
        Pill variants (value → variant)
      </span>
      <div className="grid gap-1">
        {entries.map(([value, variant], index) => (
          <div
            key={`${value}-${index}`}
            className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] gap-1"
          >
            <Input
              value={value}
              onChange={(event) => {
                const next = { ...variants };
                delete next[value];
                if (event.target.value) {
                  next[event.target.value] = variant;
                }
                onChange(next);
              }}
              placeholder="value"
            />
            <Select
              value={variant}
              onValueChange={(v) =>
                onChange({ ...variants, [value]: v as WizardPillVariant })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {WIZARD_PILL_VARIANTS.map((option) => (
                  <SelectItem key={option} value={option}>
                    {option}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="h-7 w-7"
              aria-label={`Remove ${value} variant`}
              onClick={() => {
                const next = { ...variants };
                delete next[value];
                onChange(next);
              }}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        ))}
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7"
          onClick={() => {
            const placeholder = `value_${entries.length + 1}`;
            onChange({ ...variants, [placeholder]: 'default' });
          }}
        >
          <Plus className="mr-1 h-3 w-3" />
          Add variant
        </Button>
      </div>
    </div>
  );
}
