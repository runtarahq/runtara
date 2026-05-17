// Phase 9: single layout-editor primitive. Walks `definition.layout`
// recursively, rendering each `grid` as a CSS grid with the configured
// `columns` / `columnWidths`. Each item slot hosts either a block
// editor (`BlockHostInEdit`) or a nested `GridContainer`. Drag-and-drop
// between slots is powered by `@dnd-kit/core` + `@dnd-kit/sortable`
// with cross-grid moves dispatched through `moveLayoutNode`.

import {
  DndContext,
  DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
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
import { GripVertical, Plus, Settings2, Trash2 } from 'lucide-react';
import { CSSProperties, useState } from 'react';
import {
  ReportBlockResult,
  ReportDefinition,
  ReportGridLayoutNode,
  ReportLayoutNode,
} from '../../types';
import { BlockEditor } from './blocks/BlockEditor';
import { BlockHostInEdit } from './BlockHostInEdit';
import { GridSettingsPanel } from './GridSettingsPanel';
import { resolveDrop } from './dndResolve';
import {
  LayoutTarget,
  addBlock,
  addLayoutNode,
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

const PRESETS = [
  { label: 'Section (1 column)', columns: 1 },
  { label: '2 equal columns', columns: 2 },
  { label: '3 equal columns', columns: 3 },
  { label: '4-column metric row', columns: 4 },
];

/** Top-level editor for the report layout tree. Renders each root-level
 *  layout node and offers "Add grid" / "Add block" affordances. */
export function GridContainer({
  definition,
  schemas,
  blockResults,
  reportId,
  filters,
  onChange,
}: GridContainerProps) {
  const layout = definition.layout ?? [];
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    })
  );

  const handleAddRootGrid = (columns: number) => {
    const grid = newGrid({ columns });
    onChange(addLayoutNode(definition, grid, { parentGridId: null }));
  };

  const handleAddRootBlock = () => {
    const id = makeBlockId('block');
    onChange(
      addBlock(definition, {
        id,
        type: 'markdown',
        source: { schema: '' },
        markdown: { content: '' },
        title: 'New block',
      })
    );
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

  const rootIds = layout.map((node) => node.id);

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <div className="grid gap-4" data-testid="grid-container-root">
        {layout.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No layout yet. Add a block or a grid below to start arranging your
            report.
          </p>
        ) : (
          <SortableContext
            items={rootIds}
            strategy={verticalListSortingStrategy}
          >
            <div className="grid gap-3">
              {layout.map((node) => (
                <SortableLayoutNode
                  key={node.id}
                  node={node}
                  definition={definition}
                  schemas={schemas}
                  blockResults={blockResults}
                  reportId={reportId}
                  filters={filters}
                  onChange={onChange}
                />
              ))}
            </div>
          </SortableContext>
        )}
        <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-muted/30 p-3">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={handleAddRootBlock}
          >
            <Plus className="mr-1 h-3.5 w-3.5" /> Add block
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button type="button" variant="outline" size="sm">
                <Plus className="mr-1 h-3.5 w-3.5" /> Add grid
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {PRESETS.map((preset) => (
                <DropdownMenuItem
                  key={preset.columns}
                  onClick={() => handleAddRootGrid(preset.columns)}
                >
                  {preset.label}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
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
  onChange,
}: BlockNodeEditorProps) {
  const [expanded, setExpanded] = useState(false);
  const block = definition.blocks.find((b) => b.id === node.blockId);
  if (!block) {
    return (
      <div className="rounded border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive">
        Block <code>{node.blockId}</code> is referenced by layout node{' '}
        <code>{node.id}</code> but missing from <code>definition.blocks</code>.
      </div>
    );
  }
  return (
    <div className="grid gap-2">
      <BlockHostInEdit
        block={block}
        blockResult={blockResults?.[block.id]}
        reportId={reportId}
        filters={filters}
        dragHandleProps={dragHandleProps}
        onConfigure={() => setExpanded((v) => !v)}
        onDelete={() => onChange(removeLayoutNode(definition, node.id))}
      />
      {expanded ? (
        <div className="rounded border bg-card p-3">
          <BlockEditor
            block={block}
            schemas={schemas}
            datasets={definition.datasets ?? []}
            onChange={(next) =>
              onChange(updateBlock(definition, block.id, () => next))
            }
          />
        </div>
      ) : null}
    </div>
  );
}

interface GridNodeEditorProps extends Omit<LayoutNodeEditorProps, 'node'> {
  node: ReportGridLayoutNode & { type: 'grid' };
}

function GridNodeEditor({
  node,
  definition,
  schemas,
  blockResults,
  reportId,
  filters,
  dragHandleProps,
  onChange,
}: GridNodeEditorProps) {
  const [showSettings, setShowSettings] = useState(false);
  const columns = Math.max(node.columns ?? 1, 1);
  const widths = node.columnWidths;
  const template =
    widths && widths.length === columns
      ? widths.map((w) => `${Math.max(w, 0.0001)}fr`).join(' ')
      : `repeat(${columns}, minmax(0, 1fr))`;

  const handleDelete = () => {
    onChange(removeLayoutNode(definition, node.id));
  };

  const handleAddBlockToGrid = () => {
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
      { parentGridId: node.id }
    );
    onChange(next);
  };

  const handleAddGridToGrid = (subColumns: number) => {
    const sub = newGrid({ columns: subColumns });
    onChange(addLayoutNode(definition, sub, { parentGridId: node.id }));
  };

  const itemChildIds = node.items.map((item) => item.child.id);

  return (
    <section
      className="rounded-lg border bg-card p-3"
      data-testid={`grid-${node.id}`}
      data-grid-id={node.id}
    >
      <header className="mb-3 flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          {dragHandleProps ? (
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
                Grid · {columns} {columns === 1 ? 'column' : 'columns'}
              </span>
            )}
            {node.description ? (
              <p className="mt-1 text-xs text-muted-foreground">
                {node.description}
              </p>
            ) : null}
          </div>
        </div>
        <div className="flex items-center gap-1">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            title="Grid settings"
            onClick={() => setShowSettings((v) => !v)}
          >
            <Settings2 className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 text-destructive"
            title="Remove grid"
            onClick={handleDelete}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
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
          className="grid w-full gap-3 [grid-template-columns:var(--report-grid-edit-cols)]"
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
            return (
              <div
                key={item.id}
                className="min-w-0 [grid-column:var(--report-grid-edit-col)] [grid-row:var(--report-grid-edit-row)]"
                style={
                  {
                    '--report-grid-edit-col': colSpan
                      ? `span ${colSpan} / span ${colSpan}`
                      : 'auto',
                    '--report-grid-edit-row': rowSpan
                      ? `span ${rowSpan} / span ${rowSpan}`
                      : 'auto',
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
                  onChange={onChange}
                />
              </div>
            );
          })}
        </div>
      </SortableContext>
      <div className="mt-3 flex flex-wrap items-center gap-2 border-t pt-3">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7"
          onClick={handleAddBlockToGrid}
        >
          <Plus className="mr-1 h-3 w-3" /> Add block
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" size="sm" className="h-7">
              <Plus className="mr-1 h-3 w-3" /> Add nested grid
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {PRESETS.map((preset) => (
              <DropdownMenuItem
                key={preset.columns}
                onClick={() => handleAddGridToGrid(preset.columns)}
              >
                {preset.label}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </section>
  );
}

// Exported for direct programmatic use (tests + future drag-and-drop).
export type { LayoutTarget };
export { pathToLayoutNode };
