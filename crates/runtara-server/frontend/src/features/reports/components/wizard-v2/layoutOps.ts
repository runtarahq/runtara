// Pure helpers that walk and mutate a ReportDefinition.layout tree and
// keep `blocks` in sync. The wizard v2 calls these instead of going through
// an intermediate WizardState model — every editing operation is a direct
// transformation on ReportDefinition.
//
// The functions are immutable (shallow clones along the path of change); the
// caller passes the new value to React state.

import {
  ReportBlockDefinition,
  ReportDefinition,
  ReportLayoutNode,
} from '../../types';

/** Returns the ordered list of block ids that appear under any `block` node
 * anywhere in the layout tree. */
export function collectLayoutBlockIds(
  layout: ReportLayoutNode[] | undefined
): string[] {
  const ids: string[] = [];
  walkLayout(layout, (node) => {
    if (node.type === 'block') ids.push(node.blockId);
  });
  return ids;
}

/** Calls `visit` on every node in the layout tree, depth-first. */
export function walkLayout(
  layout: ReportLayoutNode[] | undefined,
  visit: (node: ReportLayoutNode) => void
): void {
  for (const node of layout ?? []) {
    visit(node);
    if (node.type === 'section') walkLayout(node.children, visit);
    if (node.type === 'columns') {
      for (const column of node.columns ?? []) {
        walkLayout(column.children, visit);
      }
    }
  }
}

/** Removes any `block` nodes referencing `blockId` from the layout tree.
 * Preserves all surrounding structure (sections, columns, other blocks). */
export function removeBlockFromLayout(
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
  if (node.type === 'section') {
    return {
      ...node,
      children: (node.children ?? [])
        .map((child) => stripBlockFromNode(child, blockId))
        .filter((child): child is ReportLayoutNode => child !== null),
    };
  }
  if (node.type === 'columns') {
    return {
      ...node,
      columns: (node.columns ?? []).map((column) => ({
        ...column,
        children: (column.children ?? [])
          .map((child) => stripBlockFromNode(child, blockId))
          .filter((child): child is ReportLayoutNode => child !== null),
      })),
    };
  }
  return node;
}

/** Appends a top-level `block` layout node so a freshly-added block has a
 * place to render. Unknown id collisions are caller's responsibility. */
export function appendBlockToLayout(
  layout: ReportLayoutNode[] | undefined,
  blockId: string
): ReportLayoutNode[] {
  const node: ReportLayoutNode = {
    id: `n_${blockId}`,
    type: 'block',
    blockId,
  };
  return [...(layout ?? []), node];
}

/** Maps `layout`'s block-nodes to point at `nextOrder` instead of the current
 * one, preserving every non-block node and the surrounding structure.
 *
 * Used by reorder operations: callers pass the same set of block ids in a
 * new order. If the set differs (the layout had blocks that aren't in
 * `nextOrder` or vice versa) the function falls back to: removing surplus
 * block nodes from the layout, then appending missing ones as top-level
 * block nodes. */
export function reorderLayoutBlocks(
  layout: ReportLayoutNode[] | undefined,
  nextOrder: string[]
): ReportLayoutNode[] {
  const current = collectLayoutBlockIds(layout);
  const sameSet =
    current.length === nextOrder.length &&
    current.every((id) => nextOrder.includes(id));
  if (!sameSet) {
    let next = layout ?? [];
    for (const id of current) {
      if (!nextOrder.includes(id)) next = removeBlockFromLayout(next, id);
    }
    for (const id of nextOrder) {
      if (!current.includes(id)) next = appendBlockToLayout(next, id);
    }
    return remapBlockNodes(next, nextOrder);
  }
  return remapBlockNodes(layout ?? [], nextOrder);
}

function remapBlockNodes(
  layout: ReportLayoutNode[],
  nextOrder: string[]
): ReportLayoutNode[] {
  const queue = [...nextOrder];
  return layout.map((node) => remapNode(node, queue));
}

function remapNode(
  node: ReportLayoutNode,
  queue: string[]
): ReportLayoutNode {
  if (node.type === 'block') {
    const nextId = queue.shift();
    if (!nextId) return node;
    return { ...node, blockId: nextId, id: node.id || `n_${nextId}` };
  }
  if (node.type === 'section') {
    return {
      ...node,
      children: (node.children ?? []).map((child) => remapNode(child, queue)),
    };
  }
  if (node.type === 'columns') {
    return {
      ...node,
      columns: (node.columns ?? []).map((column) => ({
        ...column,
        children: (column.children ?? []).map((child) =>
          remapNode(child, queue)
        ),
      })),
    };
  }
  return node;
}

/** Returns the visible-to-editor ordered list of blocks: each block from
 * `definition.blocks` is listed in the order they appear in the layout tree.
 * Blocks present in `blocks` but missing from `layout` are appended at the
 * end (the wizard surfaces them as "unplaced"). */
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
 * node pointing at it. Returns the updated definition. */
export function addBlock(
  definition: ReportDefinition,
  block: ReportBlockDefinition
): ReportDefinition {
  return {
    ...definition,
    blocks: [...definition.blocks, block],
    layout: appendBlockToLayout(definition.layout, block.id),
  };
}

/** Removes the block with `blockId` from both `blocks` and `layout`. */
export function removeBlock(
  definition: ReportDefinition,
  blockId: string
): ReportDefinition {
  return {
    ...definition,
    blocks: definition.blocks.filter((block) => block.id !== blockId),
    layout: removeBlockFromLayout(definition.layout, blockId),
  };
}

/** Moves the block with `blockId` to a new index in the editor's ordered
 * block list. The layout tree is updated to reflect the new order. */
export function moveBlock(
  definition: ReportDefinition,
  blockId: string,
  toIndex: number
): ReportDefinition {
  const order = orderedBlocksFromDefinition(definition).map((b) => b.id);
  const fromIndex = order.indexOf(blockId);
  if (fromIndex < 0) return definition;
  const clamped = Math.max(0, Math.min(toIndex, order.length - 1));
  if (clamped === fromIndex) return definition;
  const [picked] = order.splice(fromIndex, 1);
  order.splice(clamped, 0, picked);
  return {
    ...definition,
    layout: reorderLayoutBlocks(definition.layout, order),
  };
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
