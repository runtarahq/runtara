// Pure helpers that walk and mutate a ReportDefinition.layout tree and
// keep `blocks` in sync. The wizard v2 calls these instead of going through
// an intermediate WizardState model — every editing operation is a direct
// transformation on ReportDefinition.
//
// Phase 9 collapse: the legacy `section` / `columns` / `metric_row` layout
// node types are gone. Two variants only:
//   - block: `{ type: "block", id, blockId, showWhen? }`
//   - grid:  `{ type: "grid", id, title?, description?, columns?,
//                columnWidths?, items: GridItem[], showWhen? }`
// where `GridItem = { id, colSpan?, rowSpan?, child: ReportLayoutNode }`
// (recursive — grids nest via `child`).
//
// Functions are immutable (shallow clones along the path of change); the
// caller passes the new value to React state.

import {
  ReportBlockDefinition,
  ReportDefinition,
  ReportGridLayoutItem,
  ReportGridLayoutNode,
  ReportLayoutNode,
} from '../../types';

// ============================================================================
// Walkers
// ============================================================================

/** Returns the ordered list of block ids that appear anywhere in the
 *  layout tree. Recurses into every grid item's child. */
export function collectLayoutBlockIds(
  layout: ReportLayoutNode[] | undefined
): string[] {
  const ids: string[] = [];
  walkLayout(layout, (node) => {
    if (node.type === 'block') ids.push(node.blockId);
  });
  return ids;
}

/** Visits every layout node depth-first, including each grid item's child. */
export function walkLayout(
  layout: ReportLayoutNode[] | undefined,
  visit: (node: ReportLayoutNode) => void
): void {
  for (const node of layout ?? []) {
    visit(node);
    if (node.type === 'grid') {
      for (const item of node.items ?? []) {
        walkLayout([item.child], visit);
      }
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

/** Appends `block` to `definition.blocks` and adds a top-level layout
 *  node pointing at it. Returns the updated definition. */
export function addBlock(
  definition: ReportDefinition,
  block: ReportBlockDefinition
): ReportDefinition {
  const layoutNode: ReportLayoutNode = {
    id: `n_${block.id}`,
    type: 'block',
    blockId: block.id,
  };
  return {
    ...definition,
    blocks: [...definition.blocks, block],
    layout: [...(definition.layout ?? []), layoutNode],
  };
}

/** Removes the block with `blockId` from both `blocks` and `layout`. The
 *  layout entry can be either a top-level `block` node or a `block` child
 *  of any grid item — both are stripped. */
export function removeBlock(
  definition: ReportDefinition,
  blockId: string
): ReportDefinition {
  return {
    ...definition,
    blocks: definition.blocks.filter((block) => block.id !== blockId),
    layout: stripBlockReferences(definition.layout, blockId),
  };
}

/** Moves the block with `blockId` to a new index in the editor's flat
 *  ordered block list. Works only when the block lives at the top level
 *  of the layout — nested blocks need `moveLayoutNode` with an explicit
 *  target. */
export function moveBlock(
  definition: ReportDefinition,
  blockId: string,
  toIndex: number
): ReportDefinition {
  const layout = definition.layout ?? [];
  type BlockEntry = { node: ReportLayoutNode; layoutIndex: number };
  const topLevelBlocks: BlockEntry[] = [];
  layout.forEach((node, i) => {
    if (node.type === 'block' && node.blockId === blockId) {
      topLevelBlocks.push({ node, layoutIndex: i });
    } else if (node.type === 'block') {
      topLevelBlocks.push({ node, layoutIndex: i });
    }
  });
  const subjectIndex = topLevelBlocks.findIndex(
    (entry) =>
      entry.node.type === 'block' && entry.node.blockId === blockId
  );
  if (subjectIndex < 0) return definition;
  const clamped = Math.max(0, Math.min(toIndex, topLevelBlocks.length - 1));
  if (clamped === subjectIndex) return definition;
  const [picked] = topLevelBlocks.splice(subjectIndex, 1);
  topLevelBlocks.splice(clamped, 0, picked);
  const nextLayout = [...layout];
  for (let i = 0; i < topLevelBlocks.length; i++) {
    nextLayout[topLevelBlocks[i].layoutIndex] = topLevelBlocks[i].node;
  }
  // Note: layoutIndex preserves the *slot* of each top-level block; we
  // just rewrite which block sits in each slot, leaving non-block
  // siblings (grids) untouched.
  void layout;
  // Reassign in occurrence order against the slot positions.
  const slotIndices = layout
    .map((node, i) => (node.type === 'block' ? i : -1))
    .filter((v) => v >= 0);
  for (let i = 0; i < slotIndices.length; i++) {
    nextLayout[slotIndices[i]] = topLevelBlocks[i].node;
  }
  return { ...definition, layout: nextLayout };
}

function stripBlockReferences(
  layout: ReportLayoutNode[] | undefined,
  blockId: string
): ReportLayoutNode[] {
  return (layout ?? [])
    .map((node) => stripBlockFromNode(node, blockId))
    .filter((node): node is ReportLayoutNode => node !== null);
}

function stripBlockFromNode(
  node: ReportLayoutNode,
  blockId: string
): ReportLayoutNode | null {
  if (node.type === 'block') {
    return node.blockId === blockId ? null : node;
  }
  if (node.type === 'grid') {
    const items = (node.items ?? [])
      .map((item) => stripBlockFromItem(item, blockId))
      .filter((item): item is ReportGridLayoutItem => item !== null);
    return { ...node, items };
  }
  return node;
}

function stripBlockFromItem(
  item: ReportGridLayoutItem,
  blockId: string
): ReportGridLayoutItem | null {
  if (item.child.type === 'block') {
    return item.child.blockId === blockId ? null : item;
  }
  const stripped = stripBlockFromNode(item.child, blockId);
  if (!stripped) return null;
  return { ...item, child: stripped };
}

// ============================================================================
// Grid (layout-node) operations
// ============================================================================

/** Returns a path from the root layout array to the node with `nodeId`.
 *  The path's `parentGridId` is `null` when the node lives at the root.
 *  Returns `null` if no node with that id exists. */
export interface LayoutPath {
  parentGridId: string | null;
  itemIndex: number | null; // index in parentGrid.items, or null at root
  rootIndex: number | null; // index in definition.layout, or null when nested
}

export function pathToLayoutNode(
  definition: ReportDefinition,
  nodeId: string
): LayoutPath | null {
  const layout = definition.layout ?? [];
  for (let i = 0; i < layout.length; i++) {
    const node = layout[i];
    if (layoutNodeId(node) === nodeId) {
      return { parentGridId: null, itemIndex: null, rootIndex: i };
    }
    if (node.type === 'grid') {
      const found = findInGrid(node, nodeId);
      if (found) return found;
    }
  }
  return null;
}

function findInGrid(grid: ReportGridLayoutNode, nodeId: string): LayoutPath | null {
  for (let i = 0; i < (grid.items ?? []).length; i++) {
    const item = grid.items[i];
    if (layoutNodeId(item.child) === nodeId) {
      return { parentGridId: grid.id, itemIndex: i, rootIndex: null };
    }
    if (item.child.type === 'grid') {
      const nested = findInGrid(item.child, nodeId);
      if (nested) return nested;
    }
  }
  return null;
}

function layoutNodeId(node: ReportLayoutNode): string {
  return node.id;
}

/** Inserts `node` at the given target slot. Target shapes:
 *   - `{ parentGridId: null, index }` → insert at root layout array
 *   - `{ parentGridId: "g1", index }` → wrap in a grid item, insert into g1.items
 */
export interface LayoutTarget {
  parentGridId: string | null;
  index?: number;
}

export function addLayoutNode(
  definition: ReportDefinition,
  node: ReportLayoutNode,
  target: LayoutTarget
): ReportDefinition {
  if (target.parentGridId === null) {
    const layout = [...(definition.layout ?? [])];
    const index = target.index ?? layout.length;
    layout.splice(Math.max(0, Math.min(index, layout.length)), 0, node);
    return { ...definition, layout };
  }
  const item: ReportGridLayoutItem = {
    id: `item_${layoutNodeId(node)}_${Math.random().toString(36).slice(2, 6)}`,
    child: node,
  };
  return updateGrid(definition, target.parentGridId, (grid) => {
    const items = [...(grid.items ?? [])];
    const index = target.index ?? items.length;
    items.splice(Math.max(0, Math.min(index, items.length)), 0, item);
    return { ...grid, items };
  });
}

/** Removes the layout node with `nodeId` from wherever it appears.
 *  Returns the updated definition; no-op when the id is missing. */
export function removeLayoutNode(
  definition: ReportDefinition,
  nodeId: string
): ReportDefinition {
  return {
    ...definition,
    layout: removeNodeFromTree(definition.layout, nodeId),
  };
}

function removeNodeFromTree(
  layout: ReportLayoutNode[] | undefined,
  nodeId: string
): ReportLayoutNode[] {
  return (layout ?? [])
    .filter((node) => layoutNodeId(node) !== nodeId)
    .map((node): ReportLayoutNode => {
      if (node.type !== 'grid') return node;
      const items: ReportGridLayoutItem[] = (node.items ?? [])
        .filter((item) => layoutNodeId(item.child) !== nodeId)
        .map((item): ReportGridLayoutItem => {
          if (item.child.type === 'grid') {
            return {
              ...item,
              child: removeNodeFromGrid(item.child, nodeId),
            };
          }
          return item;
        });
      return { ...node, type: 'grid', items };
    });
}

function removeNodeFromGrid(
  grid: ReportGridLayoutNode & { type: 'grid' },
  nodeId: string
): ReportLayoutNode {
  const items: ReportGridLayoutItem[] = (grid.items ?? [])
    .filter((item) => layoutNodeId(item.child) !== nodeId)
    .map((item): ReportGridLayoutItem => {
      if (item.child.type === 'grid') {
        return {
          ...item,
          child: removeNodeFromGrid(item.child, nodeId),
        };
      }
      return item;
    });
  return { ...grid, type: 'grid', items };
}

/** Moves the node with `nodeId` to `target`. Convenience wrapper around
 *  `removeLayoutNode` + `addLayoutNode`. */
export function moveLayoutNode(
  definition: ReportDefinition,
  nodeId: string,
  target: LayoutTarget
): ReportDefinition {
  const path = pathToLayoutNode(definition, nodeId);
  if (!path) return definition;
  // Find the actual node value before removing.
  const captured: { node: ReportLayoutNode | null } = { node: null };
  walkLayout(definition.layout, (visited) => {
    if (layoutNodeId(visited) === nodeId) captured.node = visited;
  });
  if (!captured.node) return definition;
  const removed = removeLayoutNode(definition, nodeId);
  return addLayoutNode(removed, captured.node, target);
}

/** Patches the grid with `gridId` via `updater(prev)`. Walks every nesting
 *  level. No-op when the id is missing. */
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

function updateGridInTree(
  layout: ReportLayoutNode[] | undefined,
  gridId: string,
  updater: (grid: ReportGridLayoutNode) => ReportGridLayoutNode
): ReportLayoutNode[] {
  return (layout ?? []).map((node): ReportLayoutNode => {
    if (node.type !== 'grid') return node;
    if (node.id === gridId) {
      return { ...updater(node), type: 'grid' };
    }
    const items: ReportGridLayoutItem[] = (node.items ?? []).map((item) => {
      if (item.child.type !== 'grid') return item;
      const replaced = updateGridInTree([item.child], gridId, updater)[0];
      return { ...item, child: replaced };
    });
    return { ...node, type: 'grid', items };
  });
}

/** Patches a single grid item (col_span / row_span) inside any grid.
 *  Useful when a block's grid-cell sizing changes. */
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

function updateGridItemInTree(
  layout: ReportLayoutNode[] | undefined,
  itemId: string,
  updater: (item: ReportGridLayoutItem) => ReportGridLayoutItem
): ReportLayoutNode[] {
  return (layout ?? []).map((node): ReportLayoutNode => {
    if (node.type !== 'grid') return node;
    const items: ReportGridLayoutItem[] = (node.items ?? []).map((item) => {
      if (item.id === itemId) return updater(item);
      if (item.child.type === 'grid') {
        const replaced = updateGridItemInTree([item.child], itemId, updater)[0];
        return { ...item, child: replaced };
      }
      return item;
    });
    return { ...node, type: 'grid', items };
  });
}

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
  columnWidths?: number[];
  title?: string;
  description?: string;
}): ReportLayoutNode {
  return {
    id: makeGridId(),
    type: 'grid',
    columns: opts.columns,
    columnWidths: opts.columnWidths,
    title: opts.title,
    description: opts.description,
    items: [],
  };
}
