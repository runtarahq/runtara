import type { Node } from '@xyflow/react';
import { NODE_TYPES } from '@/features/workflows/config/workflow';

/**
 * Where notes sit on the auto-laid-out canvas.
 *
 * Steps are re-laid out from the graph on every change, so a note stored at
 * absolute coordinates drifts away from whatever it described as soon as a
 * step is added before it. A note can instead be attached to a step: its
 * stored `position` is then an offset from that step's laid-out top-left
 * corner, and it moves with the step. A note with no anchor keeps absolute
 * coordinates.
 */

export interface Point {
  x: number;
  y: number;
}

export interface Rect extends Point {
  width: number;
  height: number;
}

export interface NotePlacement {
  /** Offset from the anchor step when anchored; absolute otherwise. */
  position: Point;
  anchorStepId: string | null;
}

/** A note dropped within this many px of a step's edge attaches to it. */
export const NOTE_ATTACH_DISTANCE = 96;

/** Gap between a step and a note created next to it. */
const NOTE_GAP = 24;

function nodeSize(node: Node): { width: number; height: number } {
  const width =
    node.measured?.width ??
    (typeof node.style?.width === 'number' ? node.style.width : undefined) ??
    node.width ??
    0;
  const height =
    node.measured?.height ??
    (typeof node.style?.height === 'number' ? node.style.height : undefined) ??
    node.height ??
    0;
  return { width, height };
}

/** Absolute canvas position of a node, following its parentId chain. */
export function absolutePosition(
  node: Node,
  byId: Map<string, Node>,
  depth = 0
): Point {
  if (!node.parentId || depth > 16) return node.position;
  const parent = byId.get(node.parentId);
  if (!parent) return node.position;
  const parentPosition = absolutePosition(parent, byId, depth + 1);
  return {
    x: parentPosition.x + node.position.x,
    y: parentPosition.y + node.position.y,
  };
}

/** Absolute rectangles of the step nodes notes can attach to. */
export function stepRects(nodes: Node[]): Map<string, Rect> {
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const rects = new Map<string, Rect>();
  for (const node of nodes) {
    if (
      node.type === NODE_TYPES.NoteNode ||
      node.type === NODE_TYPES.StartIndicatorNode ||
      node.type === NODE_TYPES.CreateNode
    ) {
      continue;
    }
    rects.set(node.id, { ...absolutePosition(node, byId), ...nodeSize(node) });
  }
  return rects;
}

export function noteAnchorStepId(node: Node): string | null {
  const anchor = (node.data as { anchorStepId?: unknown })?.anchorStepId;
  return typeof anchor === 'string' && anchor ? anchor : null;
}

/**
 * Display positions for notes on laid-out nodes: an anchored note is placed
 * at its step's laid-out position plus its stored offset. A note whose step
 * is not on the canvas is left where its stored position puts it.
 */
export function resolveNotePositions(nodes: Node[]): Node[] {
  if (!nodes.some((node) => noteAnchorStepId(node))) return nodes;
  const rects = stepRects(nodes);
  return nodes.map((node) => {
    const anchor = noteAnchorStepId(node);
    if (node.type !== NODE_TYPES.NoteNode || !anchor) return node;
    const rect = rects.get(anchor);
    if (!rect) return node;
    return {
      ...node,
      position: { x: rect.x + node.position.x, y: rect.y + node.position.y },
    };
  });
}

/** Distance between two rectangles' edges; 0 when they overlap. */
function rectGap(a: Rect, b: Rect): number {
  const dx = Math.max(
    0,
    Math.max(a.x - (b.x + b.width), b.x - (a.x + a.width))
  );
  const dy = Math.max(
    0,
    Math.max(a.y - (b.y + b.height), b.y - (a.y + a.height))
  );
  return Math.hypot(dx, dy);
}

/** The step a note dropped at `note` attaches to, or null for a free note. */
export function findAttachTarget(
  note: Rect,
  steps: Map<string, Rect>,
  maxDistance = NOTE_ATTACH_DISTANCE
): string | null {
  let best: { id: string; gap: number } | null = null;
  for (const [id, rect] of steps) {
    const gap = rectGap(note, rect);
    if (gap <= maxDistance && (!best || gap < best.gap)) {
      best = { id, gap };
    }
  }
  return best?.id ?? null;
}

/** What to store for a note now at absolute `noteTopLeft`. */
export function placementForDrop(
  noteTopLeft: Point,
  anchorStepId: string | null,
  steps: Map<string, Rect>
): NotePlacement {
  const anchor = anchorStepId ? steps.get(anchorStepId) : undefined;
  if (!anchorStepId || !anchor) {
    return { position: noteTopLeft, anchorStepId: null };
  }
  return {
    position: { x: noteTopLeft.x - anchor.x, y: noteTopLeft.y - anchor.y },
    anchorStepId,
  };
}

/** Offset for a note created next to a step: just above it, left-aligned. */
export function offsetNextToStep(noteHeight: number): Point {
  return { x: 0, y: -(noteHeight + NOTE_GAP) };
}
