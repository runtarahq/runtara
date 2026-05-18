// Pure resolver for `DndContext.onDragEnd` events. Takes the active
// (source) + over (destination) layout-node ids, walks the live
// `definition.layout` tree, and returns the `moveLayoutNode` target
// the wizard should commit. Extracted so unit tests can exercise the
// drop logic without React or dnd-kit.
//
// Phase 10: `definition.layout` is a single mandatory root grid; the
// walker descends from that root rather than iterating an array.

import { ReportDefinition, ReportGridLayoutNode } from '../../types';
import { LayoutTarget } from './layoutOps';

export interface ResolveDropArgs {
  /** Layout-node id of the dragged source. */
  sourceId: string;
  /** Layout-node id of the slot under the pointer at drop time. May be
   *  a sibling node (in which case the source lands before it inside
   *  the sibling's parent grid) or a grid container itself (in which
   *  case the source is appended into that grid). */
  overId: string;
}

export type ResolveDropResult =
  | { apply: false }
  | { apply: true; target: LayoutTarget };

/** Walks `definition.layout` to figure out where the dragged node
 *  should land. Returns `apply: false` when the drop is a no-op (source
 *  dropped on itself, source already at the destination position, or
 *  the over-target can't be found). */
export function resolveDrop(
  definition: ReportDefinition,
  args: ResolveDropArgs
): ResolveDropResult {
  if (args.sourceId === args.overId) return { apply: false };

  // Phase 11: drop on an empty cell pins the source to that cell.
  // `EmptyCellPlaceholder` registers itself as a droppable with id
  // `empty:<gridId>:<col>:<row>`. Parsing it here keeps the resolver
  // pure (no DOM lookups, no dnd-kit knowledge of the data shape).
  const emptyCell = parseEmptyCellId(args.overId);
  if (emptyCell) {
    return {
      apply: true,
      target: {
        parentGridId: emptyCell.gridId,
        col: emptyCell.col,
        row: emptyCell.row,
      },
    };
  }

  const overContainer = findContainerById(definition.layout, args.overId);
  if (overContainer) {
    // Drop landed on a grid container — append into its items.
    return { apply: true, target: { parentGridId: args.overId } };
  }

  const siblingLocation = findSiblingLocation(definition.layout, args.overId);
  if (!siblingLocation) return { apply: false };

  // `moveLayoutNode` is remove-then-add. Passing the over-sibling's
  // *original* index means: after remove pushes everything below the
  // source up by one, insert at the over-sibling's original slot. That
  // matches dnd-kit's `arrayMove(items, sourceIndex, overIndex)`
  // semantic — dragging down past N items lands the source after
  // those N items.
  return {
    apply: true,
    target: {
      parentGridId: siblingLocation.parentGridId,
      index: siblingLocation.index,
    },
  };
}

function parseEmptyCellId(
  id: string
): { gridId: string; col: number; row: number } | null {
  if (!id.startsWith('empty:')) return null;
  const parts = id.slice('empty:'.length).split(':');
  if (parts.length < 3) return null;
  const col = Number(parts[parts.length - 2]);
  const row = Number(parts[parts.length - 1]);
  const gridId = parts.slice(0, -2).join(':');
  if (!gridId || !Number.isFinite(col) || !Number.isFinite(row)) return null;
  return { gridId, col, row };
}

interface SiblingLocation {
  parentGridId: string;
  index: number;
}

function findSiblingLocation(
  grid: ReportGridLayoutNode,
  nodeId: string
): SiblingLocation | null {
  for (let i = 0; i < (grid.items ?? []).length; i++) {
    if (grid.items[i].child.id === nodeId) {
      return { parentGridId: grid.id, index: i };
    }
  }
  for (const item of grid.items ?? []) {
    if (item.child.type === 'grid') {
      const nested = findSiblingLocation(item.child, nodeId);
      if (nested) return nested;
    }
  }
  return null;
}

function findContainerById(
  grid: ReportGridLayoutNode,
  nodeId: string
): ReportGridLayoutNode | null {
  if (grid.id === nodeId) return grid;
  for (const item of grid.items ?? []) {
    if (item.child.type === 'grid') {
      const found = findContainerById(item.child, nodeId);
      if (found) return found;
    }
  }
  return null;
}
