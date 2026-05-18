// Pure helpers that walk and mutate a ReportDefinition.layout tree and
// keep `blocks` in sync. The wizard v2 calls these instead of going through
// an intermediate WizardState model — every editing operation is a direct
// transformation on ReportDefinition.
//
// Phase 10 collapse: `definition.layout` is a single mandatory root grid
// (`ReportGridLayoutNode`). All blocks live inside `root.items[].child`,
// nested grids may live alongside blocks. Two layout-node variants:
//   - block: `{ type: "block", id, blockId, showWhen? }`
//   - grid:  `{ type: "grid", id, title?, description?, columns?,
//                rows?, columnWidths?, items: GridItem[], showWhen? }`
// where `GridItem = { id, colSpan?, rowSpan?, child: ReportLayoutNode }`.
//
// Functions are immutable (shallow clones along the path of change); the
// caller passes the new value to React state. The root grid itself is
// reachable via `definition.layout.id` and is protected against removal
// and against being replaced by a non-grid type.

import {
  ReportBlockDefinition,
  ReportDefinition,
  ReportGridLayoutItem,
  ReportGridLayoutNode,
  ReportLayoutNode,
} from '../../types';

// ============================================================================
// Constants
// ============================================================================

/** Stable id for the report-level root grid. Mirrors the server's
 *  `runtara_report_dsl::types::default_root_grid()` and the repository
 *  migration's wrapping behavior. */
export const ROOT_GRID_ID = 'root';

// ============================================================================
// Defaults
// ============================================================================

/** A fresh empty 1×1 root grid. New reports start here. */
export function newDefaultLayout(): ReportGridLayoutNode {
  return {
    id: ROOT_GRID_ID,
    columns: 1,
    rows: 1,
    items: [],
  };
}

// ============================================================================
// Walkers
// ============================================================================

/** Returns the ordered list of block ids that appear anywhere in the
 *  layout tree. Recurses from the root grid through every nested item. */
export function collectLayoutBlockIds(
  layout: ReportGridLayoutNode | undefined
): string[] {
  const ids: string[] = [];
  walkLayout(layout, (node) => {
    if (node.type === 'block') ids.push(node.blockId);
  });
  return ids;
}

/** Visits every layout node depth-first starting at the root grid's items
 *  (the root grid itself is not visited — callers that need it should
 *  read `definition.layout` directly). */
export function walkLayout(
  layout: ReportGridLayoutNode | undefined,
  visit: (node: ReportLayoutNode) => void
): void {
  if (!layout) return;
  walkItems(layout.items, visit);
}

function walkItems(
  items: ReportGridLayoutItem[] | undefined,
  visit: (node: ReportLayoutNode) => void
): void {
  for (const item of items ?? []) {
    visit(item.child);
    if (item.child.type === 'grid') {
      walkItems(item.child.items, visit);
    }
  }
}

// ============================================================================
// Block-side operations
// ============================================================================

/** Returns the visible-to-editor ordered list of blocks: each block from
 *  `definition.blocks` is listed in the order they appear in the layout tree.
 *  Blocks present in `blocks` but missing from `layout` are appended at the
 *  end (the wizard surfaces them as "unplaced"). */
export function orderedBlocksFromDefinition(
  definition: ReportDefinition
): ReportBlockDefinition[] {
  const layoutOrder = collectLayoutBlockIds(definition.layout);
  const byId = new Map(definition.blocks.map((block) => [block.id, block]));
  const ordered: ReportBlockDefinition[] = [];
  const consumed = new Set<string>();
  for (const id of layoutOrder) {
    const block = byId.get(id);
    if (block && !consumed.has(id)) {
      ordered.push(block);
      consumed.add(id);
    }
  }
  for (const block of definition.blocks) {
    if (!consumed.has(block.id)) ordered.push(block);
  }
  return ordered;
}

/** Replaces the block with `blockId` using `updater(prev)`. No-op if missing. */
export function updateBlock(
  definition: ReportDefinition,
  blockId: string,
  updater: (block: ReportBlockDefinition) => ReportBlockDefinition
): ReportDefinition {
  return {
    ...definition,
    blocks: definition.blocks.map((block) =>
      block.id === blockId ? updater(block) : block
    ),
  };
}

/** Appends `block` to `definition.blocks` and adds it as a new item at
 *  the end of the root grid (or a specified target grid). Returns the
 *  updated definition. */
export function addBlock(
  definition: ReportDefinition,
  block: ReportBlockDefinition,
  target?: LayoutTarget
): ReportDefinition {
  const layoutNode: ReportLayoutNode = {
    id: `n_${block.id}`,
    type: 'block',
    blockId: block.id,
  };
  const withBlock = { ...definition, blocks: [...definition.blocks, block] };
  return addLayoutNode(
    withBlock,
    layoutNode,
    target ?? { parentGridId: ROOT_GRID_ID }
  );
}

/** Removes the block with `blockId` from both `blocks` and the layout
 *  tree. Strips every grid item whose `child` references that block. */
export function removeBlock(
  definition: ReportDefinition,
  blockId: string
): ReportDefinition {
  return {
    ...definition,
    blocks: definition.blocks.filter((block) => block.id !== blockId),
    layout: stripBlockReferencesFromGrid(definition.layout, blockId),
  };
}

function stripBlockReferencesFromGrid<T extends ReportGridLayoutNode>(
  grid: T | undefined,
  blockId: string
): T {
  if (!grid) return newDefaultLayout() as T;
  const items: ReportGridLayoutItem[] = (grid.items ?? [])
    .filter(
      (item) =>
        !(item.child.type === 'block' && item.child.blockId === blockId)
    )
    .map((item) => {
      if (item.child.type === 'grid') {
        return {
          ...item,
          child: stripBlockReferencesFromGrid(item.child, blockId),
        };
      }
      return item;
    });
  return { ...grid, items };
}

// ============================================================================
// Layout-node operations
// ============================================================================

/** Path of a layout node within the root grid tree. `parentGridId` is
 *  always set — every non-root node lives inside some grid. The root
 *  grid itself has no parent and is reachable as `{ parentGridId: null,
 *  itemIndex: null }`. */
export interface LayoutPath {
  parentGridId: string | null; // null only for the root grid itself
  itemIndex: number | null; // index inside parentGrid.items, null for root
}

export function pathToLayoutNode(
  definition: ReportDefinition,
  nodeId: string
): LayoutPath | null {
  if (definition.layout.id === nodeId) {
    return { parentGridId: null, itemIndex: null };
  }
  return findInGrid(definition.layout, nodeId);
}

function findInGrid(
  grid: ReportGridLayoutNode,
  nodeId: string
): LayoutPath | null {
  for (let i = 0; i < (grid.items ?? []).length; i++) {
    const item = grid.items[i];
    if (item.child.id === nodeId) {
      return { parentGridId: grid.id, itemIndex: i };
    }
    if (item.child.type === 'grid') {
      const nested = findInGrid(item.child, nodeId);
      if (nested) return nested;
    }
  }
  return null;
}

/** Target for `addLayoutNode` / `moveLayoutNode`. `parentGridId` must
 *  reference a grid in the tree (or be `null`, which resolves to the
 *  root grid). `index` is the position inside that grid's `items`. */
export interface LayoutTarget {
  parentGridId: string | null;
  index?: number;
}

export function addLayoutNode(
  definition: ReportDefinition,
  node: ReportLayoutNode,
  target: LayoutTarget
): ReportDefinition {
  const item: ReportGridLayoutItem = {
    id: `item_${node.id}_${Math.random().toString(36).slice(2, 6)}`,
    child: node,
  };
  const targetGridId = target.parentGridId ?? ROOT_GRID_ID;
  return {
    ...definition,
    layout: insertItemIntoGrid(
      definition.layout,
      targetGridId,
      item,
      target.index
    ),
  };
}

function insertItemIntoGrid<T extends ReportGridLayoutNode>(
  grid: T,
  targetGridId: string,
  item: ReportGridLayoutItem,
  index: number | undefined
): T {
  if (grid.id === targetGridId) {
    const items = [...(grid.items ?? [])];
    const at = Math.max(0, Math.min(index ?? items.length, items.length));
    items.splice(at, 0, item);
    return { ...grid, items };
  }
  const items: ReportGridLayoutItem[] = (grid.items ?? []).map((existing) => {
    if (existing.child.type !== 'grid') return existing;
    return {
      ...existing,
      child: insertItemIntoGrid(existing.child, targetGridId, item, index),
    };
  });
  return { ...grid, items };
}

/** Removes the layout node with `nodeId`. The root grid is protected —
 *  attempts to remove it return the definition unchanged. */
export function removeLayoutNode(
  definition: ReportDefinition,
  nodeId: string
): ReportDefinition {
  if (nodeId === definition.layout.id) return definition;
  return {
    ...definition,
    layout: removeNodeFromGrid(definition.layout, nodeId),
  };
}

function removeNodeFromGrid<T extends ReportGridLayoutNode>(
  grid: T,
  nodeId: string
): T {
  const items: ReportGridLayoutItem[] = (grid.items ?? [])
    .filter((item) => item.child.id !== nodeId)
    .map((item) => {
      if (item.child.type === 'grid') {
        return { ...item, child: removeNodeFromGrid(item.child, nodeId) };
      }
      return item;
    });
  return { ...grid, items };
}

/** Moves the node with `nodeId` to `target`. The root grid cannot be
 *  moved. When the source is already at the target position (same parent
 *  grid + same index), returns the definition unchanged so callers can
 *  detect no-op moves without the item-id churn that remove+add would
 *  otherwise introduce. */
export function moveLayoutNode(
  definition: ReportDefinition,
  nodeId: string,
  target: LayoutTarget
): ReportDefinition {
  if (nodeId === definition.layout.id) return definition;
  const path = pathToLayoutNode(definition, nodeId);
  if (!path || path.parentGridId == null || path.itemIndex == null) {
    return definition;
  }
  const targetParent = target.parentGridId ?? ROOT_GRID_ID;
  if (
    path.parentGridId === targetParent &&
    (target.index === undefined || target.index === path.itemIndex)
  ) {
    return definition;
  }
  // Capture the node before removing.
  let captured: ReportLayoutNode | null = null;
  walkLayout(definition.layout, (visited) => {
    if (visited.id === nodeId) captured = visited;
  });
  if (!captured) return definition;
  const removed = removeLayoutNode(definition, nodeId);
  return addLayoutNode(removed, captured, target);
}

/** Patches the grid with `gridId` via `updater(prev)`. Works on the root
 *  grid (when `gridId === definition.layout.id`) and every nested grid.
 *  No-op when the id is missing. */
export function updateGrid(
  definition: ReportDefinition,
  gridId: string,
  updater: (grid: ReportGridLayoutNode) => ReportGridLayoutNode
): ReportDefinition {
  return {
    ...definition,
    layout: updateGridInTree(definition.layout, gridId, updater),
  };
}

function updateGridInTree<T extends ReportGridLayoutNode>(
  grid: T,
  gridId: string,
  updater: (grid: ReportGridLayoutNode) => ReportGridLayoutNode
): T {
  if (grid.id === gridId) {
    // Preserve the input's discriminator (if any) by spreading the
    // updater result over the original grid.
    return { ...grid, ...updater(grid) };
  }
  const items: ReportGridLayoutItem[] = (grid.items ?? []).map((item) => {
    if (item.child.type !== 'grid') return item;
    return {
      ...item,
      child: updateGridInTree(item.child, gridId, updater),
    };
  });
  return { ...grid, items };
}

/** Patches a single grid item (col_span / row_span / id) inside any grid
 *  in the tree. Useful when a block's grid-cell sizing changes. */
export function updateGridItem(
  definition: ReportDefinition,
  itemId: string,
  updater: (item: ReportGridLayoutItem) => ReportGridLayoutItem
): ReportDefinition {
  return {
    ...definition,
    layout: updateGridItemInTree(definition.layout, itemId, updater),
  };
}

function updateGridItemInTree<T extends ReportGridLayoutNode>(
  grid: T,
  itemId: string,
  updater: (item: ReportGridLayoutItem) => ReportGridLayoutItem
): T {
  const items: ReportGridLayoutItem[] = (grid.items ?? []).map((item) => {
    if (item.id === itemId) return updater(item);
    if (item.child.type === 'grid') {
      return {
        ...item,
        child: updateGridItemInTree(item.child, itemId, updater),
      };
    }
    return item;
  });
  return { ...grid, items };
}

// ============================================================================
// Identifier helpers
// ============================================================================

/** Generates a stable-ish block id from a human title or counter. */
export function makeBlockId(seed: string): string {
  const cleaned = seed
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '');
  const suffix = Math.random().toString(36).slice(2, 6);
  return cleaned ? `${cleaned}_${suffix}` : `block_${suffix}`;
}

/** Generates a unique grid id. */
export function makeGridId(): string {
  return `grid_${Math.random().toString(36).slice(2, 7)}`;
}

/** Builds a fresh grid container with the given preset shape. Returned
 *  type is the discriminated-union variant so it can be passed directly
 *  to `addLayoutNode`. */
export function newGrid(opts: {
  columns?: number;
  rows?: number;
  columnWidths?: number[];
  title?: string;
  description?: string;
}): ReportLayoutNode {
  return {
    id: makeGridId(),
    type: 'grid',
    columns: opts.columns,
    rows: opts.rows,
    columnWidths: opts.columnWidths,
    title: opts.title,
    description: opts.description,
    items: [],
  };
}
