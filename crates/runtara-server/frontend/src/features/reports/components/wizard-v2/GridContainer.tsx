// Phase 9/10: single layout-editor primitive. Walks
// `definition.layout` (the mandatory root grid) recursively, rendering
// each `grid` as a CSS grid with the configured `columns` /
// `columnWidths` / `rows`. Each item slot hosts either a block editor
// (`BlockHostInEdit`) or a nested grid. Drag-and-drop between slots is
// powered by `@dnd-kit/core` + `@dnd-kit/sortable` with cross-grid
// moves dispatched through `moveLayoutNode`.
//
// Phase 10 model: the report layout is always a single root grid;
// blocks are added into the root grid's slots (or any nested grid's
// slots) — not as floating siblings. The root grid cannot be removed
// or moved.

import {
  DndContext,
  DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useDroppable,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import { ChevronDown, GripVertical, Minus, Plus, Settings2, Trash2 } from 'lucide-react';
import { CSSProperties, useEffect, useState } from 'react';
import {
  ReportBlockResult,
  ReportDefinition,
  ReportGridLayoutNode,
  ReportLayoutNode,
} from '../../types';
import { BlockHostInEdit } from './BlockHostInEdit';
import { GridSettingsPanel } from './GridSettingsPanel';
import { InlineBlockEditor } from './InlineBlockEditor';
import { resolveDrop } from './dndResolve';
import {
  LayoutTarget,
  addLayoutNode,
  listEmptyCells,
  makeBlockId,
  moveLayoutNode,
  newGrid,
  pathToLayoutNode,
  removeLayoutNode,
  updateBlock,
  updateGrid,
} from './layoutOps';

interface GridContainerProps {
  definition: ReportDefinition;
  schemas: Schema[];
  blockResults?: Partial<Record<string, ReportBlockResult>>;
  reportId?: string;
  filters: Record<string, unknown>;
  onChange: (definition: ReportDefinition) => void;
}

const NESTED_GRID_PRESETS = [
  { label: 'Section (1 column)', columns: 1 },
  { label: '2 equal columns', columns: 2 },
  { label: '3 equal columns', columns: 3 },
  { label: '4-column metric row', columns: 4 },
];

/** Top-level editor for the report layout tree. The layout is always a
 *  single root grid (Phase 10) — the author drops blocks / nested grids
 *  into its slots instead of arranging floating siblings at the report
 *  root. */
export function GridContainer({
  definition,
  schemas,
  blockResults,
  reportId,
  filters,
  onChange,
}: GridContainerProps) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    })
  );

  // Phase 11: track block ids that were just created in this editor
  // mount so `BlockNodeEditor` can initialize them in `edit` mode. The
  // set is consumed (cleared) on first read inside the block editor,
  // so reopening the preview won't re-trigger edit mode.
  const [recentlyAddedIds, setRecentlyAddedIds] = useState<Set<string>>(
    () => new Set()
  );
  const markRecentlyAdded = (id: string) => {
    setRecentlyAddedIds((current) => {
      const next = new Set(current);
      next.add(id);
      return next;
    });
  };
  const consumeRecentlyAdded = (id: string) => {
    setRecentlyAddedIds((current) => {
      if (!current.has(id)) return current;
      const next = new Set(current);
      next.delete(id);
      return next;
    });
  };

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over) return;
    const sourceId = String(active.id);
    const overId = String(over.id);
    const result = resolveDrop(definition, { sourceId, overId });
    if (!result.apply) return;
    onChange(moveLayoutNode(definition, sourceId, result.target));
  };

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <div className="grid gap-4" data-testid="grid-container-root">
        <GridNodeEditor
          node={definition.layout}
          definition={definition}
          schemas={schemas}
          blockResults={blockResults}
          reportId={reportId}
          filters={filters}
          onChange={onChange}
          recentlyAddedIds={recentlyAddedIds}
          onMarkRecentlyAdded={markRecentlyAdded}
          onConsumeRecentlyAdded={consumeRecentlyAdded}
          isRoot
        />
      </div>
    </DndContext>
  );
}

interface SortableLayoutNodeProps {
  node: ReportLayoutNode;
  definition: ReportDefinition;
  schemas: Schema[];
  blockResults?: Partial<Record<string, ReportBlockResult>>;
  reportId?: string;
  filters: Record<string, unknown>;
  /** Set of block ids the wizard just created. `BlockNodeEditor` reads
   *  this once on mount and opens itself in edit mode if its block id
   *  is present, then consumes it. */
  recentlyAddedIds: Set<string>;
  onMarkRecentlyAdded: (id: string) => void;
  onConsumeRecentlyAdded: (id: string) => void;
  onChange: (definition: ReportDefinition) => void;
}

/** Wraps a layout node in a `useSortable` context so dnd-kit can drive
 *  drag + drop. The grip handle is forwarded to the editor's hover
 *  affordance row. */
function SortableLayoutNode(props: SortableLayoutNodeProps) {
  const sortable = useSortable({ id: props.node.id });
  const style: CSSProperties = {
    transform: CSS.Transform.toString(sortable.transform),
    transition: sortable.transition,
    opacity: sortable.isDragging ? 0.4 : 1,
  };
  return (
    <div ref={sortable.setNodeRef} style={style}>
      <LayoutNodeEditor
        {...props}
        dragHandleProps={{
          ...sortable.attributes,
          ...sortable.listeners,
        }}
      />
    </div>
  );
}

interface LayoutNodeEditorProps extends SortableLayoutNodeProps {
  dragHandleProps?: Record<string, unknown>;
}

function LayoutNodeEditor(props: LayoutNodeEditorProps) {
  if (props.node.type === 'block') {
    return <BlockNodeEditor {...props} node={props.node} />;
  }
  return <GridNodeEditor {...props} node={props.node} />;
}

interface BlockNodeEditorProps extends Omit<LayoutNodeEditorProps, 'node'> {
  node: Extract<ReportLayoutNode, { type: 'block' }>;
}

function BlockNodeEditor({
  node,
  definition,
  schemas,
  blockResults,
  reportId,
  filters,
  dragHandleProps,
  recentlyAddedIds,
  onConsumeRecentlyAdded,
  onChange,
}: BlockNodeEditorProps) {
  const block = definition.blocks.find((b) => b.id === node.blockId);
  // Phase 11: blocks the wizard just created open directly into edit
  // mode. We sample `recentlyAddedIds` once on mount (so toggling
  // Edit→Done→Edit doesn't loop), then consume the entry.
  const blockId = node.blockId;
  const [mode, setMode] = useState<'preview' | 'edit'>(() =>
    recentlyAddedIds.has(blockId) ? 'edit' : 'preview'
  );
  useEffect(() => {
    if (recentlyAddedIds.has(blockId)) {
      onConsumeRecentlyAdded(blockId);
    }
    // The consume call is the side-effect; we only want to fire once
    // per mount even if the parent set changes later.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!block) {
    return (
      <div className="rounded border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive">
        Block <code>{node.blockId}</code> is referenced by layout node{' '}
        <code>{node.id}</code> but missing from <code>definition.blocks</code>.
      </div>
    );
  }

  if (mode === 'edit') {
    return (
      <InlineBlockEditor
        block={block}
        schemas={schemas}
        datasets={definition.datasets ?? []}
        dragHandleProps={dragHandleProps}
        onDone={() => setMode('preview')}
        onChange={(next) =>
          onChange(updateBlock(definition, block.id, () => next))
        }
        onDelete={() => onChange(removeLayoutNode(definition, node.id))}
      />
    );
  }

  return (
    <BlockHostInEdit
      block={block}
      blockResult={blockResults?.[block.id]}
      reportId={reportId}
      filters={filters}
      dragHandleProps={dragHandleProps}
      onEdit={() => setMode('edit')}
      onDelete={() => onChange(removeLayoutNode(definition, node.id))}
    />
  );
}

interface GridNodeEditorProps extends Omit<LayoutNodeEditorProps, 'node'> {
  node: ReportGridLayoutNode;
  /** When true, this is the report-level root grid. The root grid is
   *  protected — no drag handle, no remove button. */
  isRoot?: boolean;
}

function GridNodeEditor({
  node,
  definition,
  schemas,
  blockResults,
  reportId,
  filters,
  dragHandleProps,
  isRoot,
  recentlyAddedIds,
  onMarkRecentlyAdded,
  onConsumeRecentlyAdded,
  onChange,
}: GridNodeEditorProps) {
  const [showSettings, setShowSettings] = useState(false);
  const columns = Math.max(node.columns ?? 1, 1);
  const widths = node.columnWidths;
  const template =
    widths && widths.length === columns
      ? widths.map((w) => `${Math.max(w, 0.0001)}fr`).join(' ')
      : `repeat(${columns}, minmax(0, 1fr))`;

  // How many rows the skeleton renders. The viewer renders rows
  // implicitly from items; the editor shows enough rows to fit
  // `items` *and* the user's explicit `rows` hint, never going below 1.
  // For naturalRows we have to consider both auto-flow item-cells AND
  // the deepest row touched by an explicit-position item.
  const itemCellCount = node.items.reduce((sum, item) => {
    const cs = Math.max(1, Math.min(item.colSpan ?? 1, columns));
    const rs = Math.max(1, item.rowSpan ?? 1);
    return sum + cs * rs;
  }, 0);
  const deepestExplicitRow = node.items.reduce((max, item) => {
    if (item.row == null) return max;
    const rs = Math.max(1, item.rowSpan ?? 1);
    return Math.max(max, item.row + rs - 1);
  }, 0);
  const naturalRows = Math.max(
    1,
    Math.max(Math.ceil(itemCellCount / columns), deepestExplicitRow)
  );
  const rows = Math.max(node.rows ?? naturalRows, naturalRows);
  const emptyCells = listEmptyCells(node.items, columns, rows);

  const handleDelete = () => {
    onChange(removeLayoutNode(definition, node.id));
  };

  // Phase 11: clicking an empty cell at (col, row) creates a new block
  // and pins it to that exact cell. The new block is marked as
  // "recently added" so `BlockNodeEditor` opens it directly in edit
  // mode for immediate configuration.
  const handleAddBlockToGrid = (col?: number, row?: number) => {
    const id = makeBlockId('block');
    const block = {
      id,
      type: 'markdown' as const,
      source: { schema: '' },
      markdown: { content: '' },
      title: 'New block',
    };
    let next = definition;
    next = { ...next, blocks: [...next.blocks, block] };
    next = addLayoutNode(
      next,
      { id: `n_${id}`, type: 'block', blockId: id },
      { parentGridId: node.id, col, row }
    );
    onMarkRecentlyAdded(id);
    onChange(next);
  };

  const handleAddGridToGrid = (
    subColumns: number,
    col?: number,
    row?: number
  ) => {
    const sub = newGrid({ columns: subColumns });
    onChange(
      addLayoutNode(definition, sub, { parentGridId: node.id, col, row })
    );
  };

  const setColumns = (next: number) => {
    const clamped = Math.max(1, Math.min(12, next));
    onChange(
      updateGrid(definition, node.id, (g) => ({
        ...g,
        columns: clamped,
        // Drop columnWidths when count changes — they would no longer
        // align with the new column count.
        columnWidths: undefined,
      }))
    );
  };

  const setRows = (next: number) => {
    const clamped = Math.max(naturalRows, Math.min(12, next));
    onChange(
      updateGrid(definition, node.id, (g) => ({
        ...g,
        rows: clamped,
      }))
    );
  };

  const itemChildIds = node.items.map((item) => item.child.id);

  return (
    <section
      className="rounded-lg border bg-card p-3"
      data-testid={`grid-${node.id}`}
      data-grid-id={node.id}
    >
      <header className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          {dragHandleProps && !isRoot ? (
            <button
              type="button"
              className="cursor-grab rounded p-0.5 text-muted-foreground hover:bg-muted active:cursor-grabbing"
              title="Drag to reorder"
              aria-label="Drag grid"
              {...dragHandleProps}
            >
              <GripVertical className="h-3.5 w-3.5" />
            </button>
          ) : null}
          <div className="min-w-0">
            {node.title ? (
              <h3 className="truncate text-sm font-semibold text-foreground">
                {node.title}
              </h3>
            ) : (
              <span className="text-xs uppercase tracking-wider text-muted-foreground">
                {isRoot ? `Report layout · ${columns}×${rows}` : `Grid · ${columns}×${rows}`}
              </span>
            )}
            {node.description ? (
              <p className="mt-1 text-xs text-muted-foreground">
                {node.description}
              </p>
            ) : null}
          </div>
        </div>
        <div className="flex items-center gap-2">
          <DimensionStepper
            label="Columns"
            value={columns}
            min={1}
            max={12}
            onChange={setColumns}
            decrementDisabledReason={
              columns <= 1 ? 'A grid needs at least one column' : undefined
            }
          />
          <DimensionStepper
            label="Rows"
            value={rows}
            min={naturalRows}
            max={12}
            onChange={setRows}
            decrementDisabledReason={
              rows <= naturalRows
                ? 'Can’t remove rows that still contain items'
                : undefined
            }
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            title="More grid settings"
            aria-label="Grid settings"
            onClick={() => setShowSettings((v) => !v)}
          >
            <Settings2 className="h-3.5 w-3.5" />
          </Button>
          {isRoot ? null : (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7 text-destructive"
              title="Remove grid"
              aria-label="Remove grid"
              onClick={handleDelete}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          )}
        </div>
      </header>
      {showSettings ? (
        <div className="mb-3 rounded border bg-muted/30 p-2">
          <GridSettingsPanel
            grid={node}
            onChange={(updater) =>
              onChange(updateGrid(definition, node.id, updater))
            }
          />
        </div>
      ) : null}
      <SortableContext
        items={itemChildIds}
        strategy={verticalListSortingStrategy}
      >
        <div
          className="grid w-full gap-3 rounded-md border border-dashed border-muted-foreground/20 bg-muted/10 p-2 [grid-template-columns:var(--report-grid-edit-cols)]"
          style={
            { '--report-grid-edit-cols': template } as CSSProperties
          }
        >
          {node.items.map((item) => {
            const colSpan =
              item.colSpan && item.colSpan > 1
                ? Math.min(item.colSpan, columns)
                : undefined;
            const rowSpan =
              item.rowSpan && item.rowSpan > 1 ? item.rowSpan : undefined;
            // Phase 11: explicit cell pinning. When an item has col/row,
            // CSS pins it to that cell instead of auto-flowing.
            const colCss =
              item.col != null
                ? `${item.col} / span ${colSpan ?? 1}`
                : colSpan
                  ? `span ${colSpan} / span ${colSpan}`
                  : 'auto';
            const rowCss =
              item.row != null
                ? `${item.row} / span ${rowSpan ?? 1}`
                : rowSpan
                  ? `span ${rowSpan} / span ${rowSpan}`
                  : 'auto';
            return (
              <div
                key={item.id}
                className="min-w-0 [grid-column:var(--report-grid-edit-col)] [grid-row:var(--report-grid-edit-row)]"
                style={
                  {
                    '--report-grid-edit-col': colCss,
                    '--report-grid-edit-row': rowCss,
                  } as CSSProperties
                }
              >
                <SortableLayoutNode
                  node={item.child}
                  definition={definition}
                  schemas={schemas}
                  blockResults={blockResults}
                  reportId={reportId}
                  filters={filters}
                  recentlyAddedIds={recentlyAddedIds}
                  onMarkRecentlyAdded={onMarkRecentlyAdded}
                  onConsumeRecentlyAdded={onConsumeRecentlyAdded}
                  onChange={onChange}
                />
              </div>
            );
          })}
          {emptyCells.map((cell) => (
            <EmptyCellPlaceholder
              key={`empty-${node.id}-${cell.row}-${cell.col}`}
              gridId={node.id}
              col={cell.col}
              row={cell.row}
              onAddBlock={() => handleAddBlockToGrid(cell.col, cell.row)}
              onAddGrid={(subColumns) =>
                handleAddGridToGrid(subColumns, cell.col, cell.row)
              }
            />
          ))}
        </div>
      </SortableContext>
      <p className="mt-2 text-[11px] text-muted-foreground">
        Tip: drag the <GripVertical className="inline h-3 w-3 align-text-bottom" /> grip on any block to reorder.
      </p>
    </section>
  );
}

interface DimensionStepperProps {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (next: number) => void;
  decrementDisabledReason?: string;
}

function DimensionStepper({
  label,
  value,
  min,
  max,
  onChange,
  decrementDisabledReason,
}: DimensionStepperProps) {
  const canDecrement = value > min;
  const canIncrement = value < max;
  return (
    <div className="inline-flex items-center gap-1 rounded-md border bg-background px-1 py-0.5 text-xs">
      <span className="px-1 text-muted-foreground">{label}</span>
      <button
        type="button"
        aria-label={`Remove ${label.toLowerCase()}`}
        title={decrementDisabledReason ?? `Remove ${label.toLowerCase()}`}
        disabled={!canDecrement}
        onClick={() => canDecrement && onChange(value - 1)}
        className="rounded p-0.5 text-muted-foreground hover:bg-muted disabled:cursor-not-allowed disabled:opacity-30"
      >
        <Minus className="h-3 w-3" />
      </button>
      <span className="min-w-[1ch] text-center font-medium tabular-nums">
        {value}
      </span>
      <button
        type="button"
        aria-label={`Add ${label.toLowerCase()}`}
        title={`Add ${label.toLowerCase()}`}
        disabled={!canIncrement}
        onClick={() => canIncrement && onChange(value + 1)}
        className="rounded p-0.5 text-muted-foreground hover:bg-muted disabled:cursor-not-allowed disabled:opacity-30"
      >
        <Plus className="h-3 w-3" />
      </button>
    </div>
  );
}

interface EmptyCellPlaceholderProps {
  gridId: string;
  col: number;
  row: number;
  onAddBlock: () => void;
  onAddGrid: (columns: number) => void;
}

function EmptyCellPlaceholder({
  gridId,
  col,
  row,
  onAddBlock,
  onAddGrid,
}: EmptyCellPlaceholderProps) {
  // Phase 11: each empty cell is its own drop target. When a draggable
  // layout node is dropped here, `resolveDrop` parses the composite id
  // and produces a target with explicit col/row, so `moveLayoutNode`
  // pins the moved item to this exact cell.
  const { setNodeRef, isOver } = useDroppable({
    id: `empty:${gridId}:${col}:${row}`,
  });
  return (
    <div
      ref={setNodeRef}
      data-testid={`empty-cell-${gridId}`}
      data-grid-col={col}
      data-grid-row={row}
      style={
        {
          gridColumn: `${col} / span 1`,
          gridRow: `${row} / span 1`,
        } as CSSProperties
      }
      className={
        'flex min-h-[80px] items-center justify-center rounded-md border border-dashed p-2 transition-colors ' +
        (isOver
          ? 'border-primary bg-primary/10'
          : 'border-muted-foreground/30 bg-background/30 hover:border-muted-foreground/60 hover:bg-muted/30')
      }
    >
      <div className="flex items-center gap-1">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-7 text-xs"
          onClick={onAddBlock}
        >
          <Plus className="mr-1 h-3 w-3" /> Add block
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-1.5 text-muted-foreground"
              title="Add nested grid"
              aria-label="Add nested grid"
            >
              <ChevronDown className="h-3 w-3" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="center">
            {NESTED_GRID_PRESETS.map((preset) => (
              <DropdownMenuItem
                key={preset.columns}
                onClick={() => onAddGrid(preset.columns)}
              >
                Nested {preset.label.toLowerCase()}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  );
}

// Exported for direct programmatic use (tests + future drag-and-drop).
export type { LayoutTarget };
export { pathToLayoutNode };
