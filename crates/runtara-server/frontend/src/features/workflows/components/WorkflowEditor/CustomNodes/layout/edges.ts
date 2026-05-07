import type { Edge, Node } from '@xyflow/react';
import {
  BASE_HEIGHT,
  BASE_WIDTH,
  getEdgeKind,
  getEdgeOrder,
  type LayoutEdgeKind,
  type LayoutPoint,
} from './graph';

export type OrthogonalRoute = {
  points: LayoutPoint[];
  labelPoint?: LayoutPoint;
};

type ObstacleBox = {
  id: string;
  left: number;
  top: number;
  right: number;
  bottom: number;
};

const NODE_AVOIDANCE_MARGIN = 18;
const EDGE_STUB = 36;
const EDGE_LANE_GAP = 14;

function getNodeSize(node: Node): { width: number; height: number } {
  const width =
    (typeof node.style?.width === 'number' ? node.style.width : undefined) ??
    (typeof node.width === 'number' ? node.width : undefined) ??
    BASE_WIDTH;
  const height =
    (typeof node.style?.height === 'number' ? node.style.height : undefined) ??
    (typeof node.height === 'number' ? node.height : undefined) ??
    BASE_HEIGHT;

  return { width, height };
}

function getAbsolutePosition(
  node: Node,
  nodeById: Map<string, Node>
): LayoutPoint {
  let x = node.position.x;
  let y = node.position.y;
  let parentId = node.parentId;

  while (parentId) {
    const parent = nodeById.get(parentId);
    if (!parent) break;
    x += parent.position.x;
    y += parent.position.y;
    parentId = parent.parentId;
  }

  return { x, y };
}

function getNodeBox(
  node: Node,
  nodeById: Map<string, Node>,
  margin = 0
): ObstacleBox {
  const position = getAbsolutePosition(node, nodeById);
  const size = getNodeSize(node);

  return {
    id: node.id,
    left: position.x - margin,
    top: position.y - margin,
    right: position.x + size.width + margin,
    bottom: position.y + size.height + margin,
  };
}

function getAncestorIds(node: Node, nodeById: Map<string, Node>): Set<string> {
  const ancestors = new Set<string>();
  let parentId = node.parentId;

  while (parentId) {
    ancestors.add(parentId);
    parentId = nodeById.get(parentId)?.parentId;
  }

  return ancestors;
}

function getSwitchCaseCount(node: Node): number {
  const inputMapping = (node.data as { inputMapping?: unknown } | undefined)
    ?.inputMapping;
  if (!Array.isArray(inputMapping)) return 0;

  const casesField = inputMapping.find(
    (item: any) => item && item.type === 'cases'
  );
  return Array.isArray(casesField?.value) ? casesField.value.length : 0;
}

function getSourceAnchorY(
  edge: Edge,
  source: Node,
  sourcePosition: LayoutPoint,
  sourceHeight: number
): number {
  const handle = edge.sourceHandle ?? 'source';
  if (handle.startsWith('case-')) {
    const caseIndex = Number.parseInt(handle.split('-')[1], 10);
    if (Number.isFinite(caseIndex)) {
      return sourcePosition.y + 24 + caseIndex * 18;
    }
  }

  if (handle === 'default') {
    return sourcePosition.y + 24 + getSwitchCaseCount(source) * 18;
  }

  if (handle === 'onError') {
    return sourcePosition.y + sourceHeight;
  }

  return sourcePosition.y + sourceHeight / 2;
}

function compareEdgesForRouting(a: Edge, b: Edge): number {
  const aHandle = (a.sourceHandle ?? 'source').toLowerCase();
  const bHandle = (b.sourceHandle ?? 'source').toLowerCase();
  const aOrder = getEdgeOrder(aHandle, getEdgeKind(aHandle));
  const bOrder = getEdgeOrder(bHandle, getEdgeKind(bHandle));

  if (aOrder !== bOrder) return aOrder - bOrder;
  return a.id.localeCompare(b.id);
}

function buildSourceLaneIndexes(edges: Edge[]): Map<string, number> {
  const outgoingBySource = new Map<string, Edge[]>();

  for (const edge of edges) {
    const outgoing = outgoingBySource.get(edge.source) ?? [];
    outgoing.push(edge);
    outgoingBySource.set(edge.source, outgoing);
  }

  const indexes = new Map<string, number>();
  for (const outgoing of outgoingBySource.values()) {
    outgoing.sort(compareEdgesForRouting);
    outgoing.forEach((edge, index) => indexes.set(edge.id, index));
  }

  return indexes;
}

function segmentIntersectsBox(
  start: LayoutPoint,
  end: LayoutPoint,
  box: ObstacleBox
): boolean {
  if (start.x === end.x) {
    const top = Math.min(start.y, end.y);
    const bottom = Math.max(start.y, end.y);
    return (
      start.x > box.left &&
      start.x < box.right &&
      bottom > box.top &&
      top < box.bottom
    );
  }

  if (start.y === end.y) {
    const left = Math.min(start.x, end.x);
    const right = Math.max(start.x, end.x);
    return (
      start.y > box.top &&
      start.y < box.bottom &&
      right > box.left &&
      left < box.right
    );
  }

  return false;
}

function routeIntersectsObstacle(
  points: LayoutPoint[],
  obstacles: ObstacleBox[]
): boolean {
  for (let index = 1; index < points.length; index++) {
    const previous = points[index - 1];
    const current = points[index];
    if (obstacles.some((box) => segmentIntersectsBox(previous, current, box))) {
      return true;
    }
  }

  return false;
}

function compactPoints(points: LayoutPoint[]): LayoutPoint[] {
  const deduped: LayoutPoint[] = [];

  for (const point of points) {
    const previous = deduped[deduped.length - 1];
    if (previous && previous.x === point.x && previous.y === point.y) continue;
    deduped.push(point);
  }

  const compacted: LayoutPoint[] = [];
  for (const point of deduped) {
    const a = compacted[compacted.length - 2];
    const b = compacted[compacted.length - 1];
    if (
      a &&
      b &&
      ((a.x === b.x && b.x === point.x) || (a.y === b.y && b.y === point.y))
    ) {
      compacted[compacted.length - 1] = point;
    } else {
      compacted.push(point);
    }
  }

  return compacted;
}

function getRoutableObstacles(
  edge: Edge,
  nodeById: Map<string, Node>
): ObstacleBox[] {
  const source = nodeById.get(edge.source);
  const target = nodeById.get(edge.target);
  if (!source || !target) return [];

  const excluded = new Set([
    edge.source,
    edge.target,
    ...getAncestorIds(source, nodeById),
    ...getAncestorIds(target, nodeById),
  ]);

  return [...nodeById.values()]
    .filter((node) => !excluded.has(node.id))
    .map((node) => getNodeBox(node, nodeById, NODE_AVOIDANCE_MARGIN));
}

function isRouteClear(
  points: LayoutPoint[],
  obstacles: ObstacleBox[]
): boolean {
  return !routeIntersectsObstacle(points, obstacles);
}

function getCorridorObstacles(
  start: LayoutPoint,
  end: LayoutPoint,
  obstacles: ObstacleBox[]
): ObstacleBox[] {
  const left = Math.min(start.x, end.x);
  const right = Math.max(start.x, end.x);

  return obstacles.filter((box) => box.right > left && box.left < right);
}

function getOuterLaneY(
  start: LayoutPoint,
  end: LayoutPoint,
  obstacles: ObstacleBox[],
  side: 'top' | 'bottom',
  laneIndex: number
): number {
  const corridorObstacles = getCorridorObstacles(start, end, obstacles);
  const lanePadding =
    NODE_AVOIDANCE_MARGIN + Math.abs(laneIndex) * EDGE_LANE_GAP;

  if (corridorObstacles.length === 0) {
    const baseline =
      side === 'top' ? Math.min(start.y, end.y) : Math.max(start.y, end.y);
    return side === 'top' ? baseline - lanePadding : baseline + lanePadding;
  }

  if (side === 'top') {
    return Math.min(...corridorObstacles.map((box) => box.top)) - lanePadding;
  }

  return Math.max(...corridorObstacles.map((box) => box.bottom)) + lanePadding;
}

function getPreferredOuterSides(
  kind: LayoutEdgeKind,
  start: LayoutPoint,
  end: LayoutPoint
): Array<'top' | 'bottom'> {
  if (kind === 'conditional-true') return ['top', 'bottom'];
  if (
    kind === 'conditional-false' ||
    kind === 'error' ||
    kind === 'switch-default'
  ) {
    return ['bottom', 'top'];
  }

  return end.y < start.y ? ['top', 'bottom'] : ['bottom', 'top'];
}

function buildCandidateRoutes(
  edge: Edge,
  start: LayoutPoint,
  end: LayoutPoint,
  obstacles: ObstacleBox[],
  laneIndex: number
): LayoutPoint[][] {
  const leftToRight = end.x >= start.x;
  const direction = leftToRight ? 1 : -1;
  const laneOffset = laneIndex * EDGE_LANE_GAP;
  const sourceLaneX = start.x + direction * (EDGE_STUB + laneOffset);
  const targetLaneX = end.x - direction * (EDGE_STUB + laneOffset);
  const midX = start.x + (end.x - start.x) / 2 + laneOffset;
  const kind = getEdgeKind((edge.sourceHandle ?? 'source').toLowerCase());
  const candidates: LayoutPoint[][] = [];

  if (start.y === end.y) {
    candidates.push([start, end]);
  }

  for (const laneX of [sourceLaneX, targetLaneX, midX]) {
    candidates.push([
      start,
      { x: laneX, y: start.y },
      { x: laneX, y: end.y },
      end,
    ]);
  }

  for (const side of getPreferredOuterSides(kind, start, end)) {
    const outerY = getOuterLaneY(start, end, obstacles, side, laneIndex);
    candidates.push([
      start,
      { x: sourceLaneX, y: start.y },
      { x: sourceLaneX, y: outerY },
      { x: targetLaneX, y: outerY },
      { x: targetLaneX, y: end.y },
      end,
    ]);
  }

  return candidates.map(compactPoints);
}

function selectRoute(
  candidates: LayoutPoint[][],
  obstacles: ObstacleBox[]
): LayoutPoint[] {
  return (
    candidates.find((points) => isRouteClear(points, obstacles)) ??
    candidates[candidates.length - 1]
  );
}

export function routeOrthogonalEdges(
  nodes: Node[],
  edges: Edge[]
): Record<string, OrthogonalRoute> {
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const routeCountsBySource = new Map<string, number>();
  const sourceLaneIndexes = buildSourceLaneIndexes(edges);
  const routes: Record<string, OrthogonalRoute> = {};

  for (const edge of edges) {
    const source = nodeById.get(edge.source);
    const target = nodeById.get(edge.target);
    if (!source || !target) continue;

    const sourceBox = getNodeSize(source);
    const targetBox = getNodeSize(target);
    const sourcePosition = getAbsolutePosition(source, nodeById);
    const targetPosition = getAbsolutePosition(target, nodeById);
    const sourceKey = `${edge.source}:${edge.sourceHandle ?? 'source'}`;
    const duplicateLaneIndex = routeCountsBySource.get(sourceKey) ?? 0;
    routeCountsBySource.set(sourceKey, duplicateLaneIndex + 1);

    const laneOffset = duplicateLaneIndex * 6;
    const start = {
      x: sourcePosition.x + sourceBox.width,
      y:
        getSourceAnchorY(edge, source, sourcePosition, sourceBox.height) +
        laneOffset,
    };
    const end = {
      x: targetPosition.x,
      y: targetPosition.y + targetBox.height / 2 + laneOffset,
    };
    const sourceLaneIndex = sourceLaneIndexes.get(edge.id) ?? 0;
    const obstacles = getRoutableObstacles(edge, nodeById);
    const points = selectRoute(
      buildCandidateRoutes(edge, start, end, obstacles, sourceLaneIndex),
      obstacles
    );

    routes[edge.id] = {
      points,
      labelPoint: {
        x: start.x + Math.sign(end.x - start.x || 1) * 24,
        y: start.y,
      },
    };
  }

  return routes;
}
