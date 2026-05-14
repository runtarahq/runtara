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
  ReportDatasetDefinition,
  ReportOrderBy,
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

const METRIC_AGGREGATES: Array<{
  value: 'count' | 'sum' | 'avg' | 'min' | 'max';
  label: string;
}> = [
  { value: 'count', label: 'Count' },
  { value: 'sum', label: 'Sum' },
  { value: 'avg', label: 'Average' },
  { value: 'min', label: 'Min' },
  { value: 'max', label: 'Max' },
];

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
      title: 'New block',
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
  const schemaFields = fieldsOfSchema(schemas, block.schema);
  const usingDataset = Boolean(block.dataset);
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
  // Card and markdown blocks don't make sense over pre-aggregated datasets —
  // hide the dataset toggle for them.
  const supportsDataset =
    block.type === 'table' || block.type === 'chart' || block.type === 'metric';
  const supportsFields =
    !usingDataset &&
    (block.type === 'table' || block.type === 'card' || block.type === 'chart');
  const needsSchema = !usingDataset && block.type !== 'markdown';

  function changeSchema(nextSchema: string) {
    if (nextSchema === block.schema) return;
    // Reset field-related config when the schema changes — the old fields
    // probably don't exist on the new schema.
    onChange({
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
      schema: schemas[0]?.name,
      fields: [],
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
              <div className="flex gap-1 rounded-md border bg-muted/10 p-0.5">
                <button
                  type="button"
                  onClick={!usingDataset ? undefined : switchToSchemaMode}
                  className={cn(
                    'flex-1 rounded px-2 py-1 text-xs font-medium transition-colors',
                    !usingDataset
                      ? 'bg-background shadow-sm'
                      : 'text-muted-foreground hover:text-foreground'
                  )}
                >
                  Schema
                </button>
                <button
                  type="button"
                  onClick={
                    usingDataset || datasets.length === 0
                      ? undefined
                      : switchToDatasetMode
                  }
                  disabled={!usingDataset && datasets.length === 0}
                  className={cn(
                    'flex-1 rounded px-2 py-1 text-xs font-medium transition-colors',
                    usingDataset
                      ? 'bg-background shadow-sm'
                      : 'text-muted-foreground hover:text-foreground',
                    !usingDataset &&
                      datasets.length === 0 &&
                      'cursor-not-allowed opacity-50'
                  )}
                  title={
                    datasets.length === 0 && !usingDataset
                      ? 'Add a dataset in the Datasets section first'
                      : undefined
                  }
                >
                  Use dataset
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

          {block.type === 'markdown' ? (
            <div className="grid gap-1.5">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Markdown
              </Label>
              <Textarea
                rows={3}
                value={block.markdownContent ?? `# ${block.title}`}
                onChange={(event) =>
                  onChange({ markdownContent: event.target.value })
                }
              />
            </div>
          ) : null}

          {!usingDataset && block.type === 'metric' ? (
            <div className="grid gap-2 sm:grid-cols-3">
              <div className="grid gap-1.5">
                <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Aggregate
                </Label>
                <Select
                  value={block.metricAggregate ?? 'count'}
                  onValueChange={(value) =>
                    onChange({
                      metricAggregate: value as WizardBlock['metricAggregate'],
                    })
                  }
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
              {(block.metricAggregate ?? 'count') !== 'count' ? (
                <div className="grid gap-1.5">
                  <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Field
                  </Label>
                  <Select
                    value={block.metricField ?? schemaFields[0] ?? ''}
                    onValueChange={(value) => onChange({ metricField: value })}
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
          ) : null}

          {!usingDataset && block.type === 'chart' ? (
            <div className="grid gap-2 sm:grid-cols-2">
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
            </div>
          ) : null}

          {supportsFields ? (
            <FieldPicker
              block={block}
              schemaFields={schemaFields}
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
                    merged.interactionButtons.length === 0)
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
  onToggleField,
  onAddActionColumn,
  onUpdateFieldConfig,
}: {
  block: WizardBlock;
  schemaFields: string[];
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
                    })
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
  formatChoices,
  onLabelChange,
  onFormatChange,
  onPillVariantsChange,
  onColumnTypeChange,
  onWorkflowActionChange,
  onInteractionButtonsChange,
  onRemove,
}: {
  field: string;
  cfg: WizardFieldConfig;
  schemaFields: string[];
  formatChoices: typeof WIZARD_COLUMN_FORMATS | null;
  onLabelChange: (label: string) => void;
  onFormatChange: (value: string) => void;
  onPillVariantsChange: (variants: Record<string, WizardPillVariant>) => void;
  onColumnTypeChange: (columnType: WizardTableColumnType) => void;
  onWorkflowActionChange: (action: ReportWorkflowActionConfig) => void;
  onInteractionButtonsChange: (
    buttons: ReportTableInteractionButtonConfig[]
  ) => void;
  onRemove: () => void;
}) {
  const columnType = cfg.columnType ?? 'value';
  const isAction = isActionFieldKey(field);
  const showPillVariants = columnType === 'value' && cfg.format === 'pill';
  const showWorkflowEditor = columnType === 'workflow_button';
  const showInteractionEditor = columnType === 'interaction_buttons';
  const expansionRow =
    showPillVariants || showWorkflowEditor || showInteractionEditor;
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

function FieldRow({
  field,
  cfg,
  formatChoices,
  onLabelChange,
  onFormatChange,
  onPillVariantsChange,
  onRemove,
}: {
  field: string;
  cfg: WizardFieldConfig;
  formatChoices: typeof WIZARD_COLUMN_FORMATS | null;
  onLabelChange: (label: string) => void;
  onFormatChange: (value: string) => void;
  onPillVariantsChange: (variants: Record<string, WizardPillVariant>) => void;
  onRemove: () => void;
}) {
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
      {cfg.format === 'pill' ? (
        <tr>
          <td colSpan={formatChoices ? 4 : 3} className="pb-2 pl-3">
            <PillVariantsEditor
              variants={cfg.pillVariants ?? {}}
              onChange={onPillVariantsChange}
            />
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
