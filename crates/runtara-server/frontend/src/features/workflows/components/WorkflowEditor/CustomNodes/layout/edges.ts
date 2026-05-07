import type { Edge, Node } from '@xyflow/react';
import {
  BASE_HEIGHT,
  BASE_WIDTH,
  getEdgeKind,
  getEdgeOrder,
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
const ROUTE_BEND_PENALTY = 36;
const ROUTE_BACKTRACK_PENALTY = 400;
const SOURCE_SIDE_VERTICAL_PENALTY = 900;
const TARGET_SIDE_VERTICAL_PENALTY = 80;

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
  const normalizedHandle = handle.toLowerCase();

  if (normalizedHandle === 'true') {
    return sourcePosition.y + sourceHeight * 0.3;
  }

  if (normalizedHandle === 'false') {
    return sourcePosition.y + sourceHeight * 0.7;
  }

  if (handle.startsWith('case-')) {
    const caseIndex = Number.parseInt(handle.split('-')[1], 10);
    if (Number.isFinite(caseIndex)) {
      return sourcePosition.y + 24 + caseIndex * 18;
    }
  }

  if (normalizedHandle === 'default') {
    return sourcePosition.y + 24 + getSwitchCaseCount(source) * 18;
  }

  if (normalizedHandle === 'onerror') {
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

type RouteAxis = 'horizontal' | 'vertical' | 'none';

type RouteCandidate = {
  point: LayoutPoint;
  axis: Exclude<RouteAxis, 'none'>;
};

type RouteQueueItem = {
  key: string;
  point: LayoutPoint;
  axis: RouteAxis;
  cost: number;
  sequence: number;
};

class RoutePriorityQueue {
  private items: RouteQueueItem[] = [];

  push(item: RouteQueueItem): void {
    this.items.push(item);
    this.bubbleUp(this.items.length - 1);
  }

  pop(): RouteQueueItem | undefined {
    const first = this.items[0];
    const last = this.items.pop();
    if (!first || !last) return first;

    if (this.items.length > 0) {
      this.items[0] = last;
      this.sinkDown(0);
    }

    return first;
  }

  get length(): number {
    return this.items.length;
  }

  private compare(a: RouteQueueItem, b: RouteQueueItem): number {
    if (a.cost !== b.cost) return a.cost - b.cost;
    return a.sequence - b.sequence;
  }

  private bubbleUp(index: number): void {
    while (index > 0) {
      const parentIndex = Math.floor((index - 1) / 2);
      if (this.compare(this.items[index], this.items[parentIndex]) >= 0) break;
      [this.items[index], this.items[parentIndex]] = [
        this.items[parentIndex],
        this.items[index],
      ];
      index = parentIndex;
    }
  }

  private sinkDown(index: number): void {
    while (true) {
      const leftIndex = index * 2 + 1;
      const rightIndex = leftIndex + 1;
      let smallestIndex = index;

      if (
        leftIndex < this.items.length &&
        this.compare(this.items[leftIndex], this.items[smallestIndex]) < 0
      ) {
        smallestIndex = leftIndex;
      }
      if (
        rightIndex < this.items.length &&
        this.compare(this.items[rightIndex], this.items[smallestIndex]) < 0
      ) {
        smallestIndex = rightIndex;
      }
      if (smallestIndex === index) break;

      [this.items[index], this.items[smallestIndex]] = [
        this.items[smallestIndex],
        this.items[index],
      ];
      index = smallestIndex;
    }
  }
}

function coordinateKey(value: number): string {
  return Number.isInteger(value) ? `${value}` : value.toFixed(3);
}

function pointKey(point: LayoutPoint): string {
  return `${coordinateKey(point.x)},${coordinateKey(point.y)}`;
}

function stateKey(point: LayoutPoint, axis: RouteAxis): string {
  return `${pointKey(point)}|${axis}`;
}

function uniqueSortedCoordinates(values: number[]): number[] {
  return [...new Set(values.map((value) => Number(value.toFixed(3))))].sort(
    (a, b) => a - b
  );
}

function buildRoutingGrid(
  start: LayoutPoint,
  end: LayoutPoint,
  obstacles: ObstacleBox[],
  laneIndex: number
): { xs: number[]; ys: number[] } {
  const leftToRight = end.x >= start.x;
  const direction: 1 | -1 = leftToRight ? 1 : -1;
  const laneOffset = laneIndex * EDGE_LANE_GAP;
  const sourceLaneX = start.x + direction * (EDGE_STUB + laneOffset);
  const targetLaneX = end.x - direction * (EDGE_STUB + laneOffset);
  const midX = start.x + (end.x - start.x) / 2 + laneOffset;
  const xs = [start.x, sourceLaneX, midX, targetLaneX, end.x];
  const ys = [start.y, end.y];
  const outerPadding =
    NODE_AVOIDANCE_MARGIN + EDGE_STUB + Math.abs(laneIndex) * EDGE_LANE_GAP;

  if (obstacles.length > 0) {
    xs.push(Math.min(start.x, end.x) - outerPadding);
    xs.push(Math.max(start.x, end.x) + outerPadding);
    ys.push(Math.min(start.y, end.y) - outerPadding);
    ys.push(Math.max(start.y, end.y) + outerPadding);
  }

  for (const box of obstacles) {
    xs.push(
      box.left - EDGE_LANE_GAP,
      box.left,
      box.right,
      box.right + EDGE_LANE_GAP
    );
    ys.push(
      box.top - EDGE_LANE_GAP,
      box.top,
      box.bottom,
      box.bottom + EDGE_LANE_GAP
    );
  }

  return {
    xs: uniqueSortedCoordinates(xs),
    ys: uniqueSortedCoordinates(ys),
  };
}

function routeSegmentCost(
  from: LayoutPoint,
  to: LayoutPoint,
  axis: Exclude<RouteAxis, 'none'>,
  previousAxis: RouteAxis,
  start: LayoutPoint,
  end: LayoutPoint,
  direction: 1 | -1
): number {
  const distance = Math.abs(to.x - from.x) + Math.abs(to.y - from.y);
  let cost = distance;

  if (previousAxis !== 'none' && previousAxis !== axis) {
    cost += ROUTE_BEND_PENALTY;
  }

  if (axis === 'horizontal') {
    const delta = to.x - from.x;
    if (delta !== 0 && Math.sign(delta) !== direction) {
      cost += ROUTE_BACKTRACK_PENALTY + Math.abs(delta);
    }
  } else {
    const sourceProgress =
      direction === 1 ? from.x - start.x : start.x - from.x;
    const targetGap = direction === 1 ? end.x - from.x : from.x - end.x;

    if (sourceProgress < EDGE_STUB) {
      cost += SOURCE_SIDE_VERTICAL_PENALTY;
    }
    if (targetGap < EDGE_STUB / 2) {
      cost += TARGET_SIDE_VERTICAL_PENALTY;
    }
  }

  return cost;
}

function getRouteCandidates(
  point: LayoutPoint,
  grid: { xs: number[]; ys: number[] },
  direction: 1 | -1,
  end: LayoutPoint
): RouteCandidate[] {
  const xIndex = grid.xs.indexOf(point.x);
  const yIndex = grid.ys.indexOf(point.y);
  const candidates: RouteCandidate[] = [];

  const addHorizontal = (index: number) => {
    if (index < 0 || index >= grid.xs.length) return;
    candidates.push({
      point: { x: grid.xs[index], y: point.y },
      axis: 'horizontal',
    });
  };

  const addVertical = (index: number) => {
    if (index < 0 || index >= grid.ys.length) return;
    candidates.push({
      point: { x: point.x, y: grid.ys[index] },
      axis: 'vertical',
    });
  };

  if (direction === 1) {
    addHorizontal(xIndex + 1);
    addVertical(end.y >= point.y ? yIndex + 1 : yIndex - 1);
    addVertical(end.y >= point.y ? yIndex - 1 : yIndex + 1);
    addHorizontal(xIndex - 1);
  } else {
    addHorizontal(xIndex - 1);
    addVertical(end.y >= point.y ? yIndex + 1 : yIndex - 1);
    addVertical(end.y >= point.y ? yIndex - 1 : yIndex + 1);
    addHorizontal(xIndex + 1);
  }

  return candidates.filter(
    (candidate) => candidate.point.x !== point.x || candidate.point.y !== point.y
  );
}

function reconstructRoute(
  endStateKey: string,
  previousByState: Map<string, string | null>,
  pointByState: Map<string, LayoutPoint>
): LayoutPoint[] {
  const points: LayoutPoint[] = [];
  let currentKey: string | null | undefined = endStateKey;

  while (currentKey) {
    const point = pointByState.get(currentKey);
    if (point) points.push(point);
    currentKey = previousByState.get(currentKey);
  }

  return compactPoints(points.reverse());
}

function routeOnGrid(
  start: LayoutPoint,
  end: LayoutPoint,
  obstacles: ObstacleBox[],
  laneIndex: number
): LayoutPoint[] {
  if (start.y === end.y && !routeIntersectsObstacle([start, end], obstacles)) {
    return [start, end];
  }

  const direction: 1 | -1 = end.x >= start.x ? 1 : -1;
  const grid = buildRoutingGrid(start, end, obstacles, laneIndex);
  const queue = new RoutePriorityQueue();
  const bestCostByState = new Map<string, number>();
  const previousByState = new Map<string, string | null>();
  const pointByState = new Map<string, LayoutPoint>();
  const startStateKey = stateKey(start, 'none');
  const endPointKey = pointKey(end);
  let sequence = 0;

  bestCostByState.set(startStateKey, 0);
  previousByState.set(startStateKey, null);
  pointByState.set(startStateKey, start);
  queue.push({
    key: startStateKey,
    point: start,
    axis: 'none',
    cost: 0,
    sequence: sequence++,
  });

  while (queue.length > 0) {
    const current = queue.pop();
    if (!current) break;

    if (current.cost > (bestCostByState.get(current.key) ?? Infinity)) {
      continue;
    }

    if (pointKey(current.point) === endPointKey) {
      return reconstructRoute(current.key, previousByState, pointByState);
    }

    for (const candidate of getRouteCandidates(
      current.point,
      grid,
      direction,
      end
    )) {
      if (routeIntersectsObstacle([current.point, candidate.point], obstacles)) {
        continue;
      }

      const candidateCost =
        current.cost +
        routeSegmentCost(
          current.point,
          candidate.point,
          candidate.axis,
          current.axis,
          start,
          end,
          direction
        );
      const candidateStateKey = stateKey(candidate.point, candidate.axis);

      if (
        candidateCost >= (bestCostByState.get(candidateStateKey) ?? Infinity)
      ) {
        continue;
      }

      bestCostByState.set(candidateStateKey, candidateCost);
      previousByState.set(candidateStateKey, current.key);
      pointByState.set(candidateStateKey, candidate.point);
      queue.push({
        key: candidateStateKey,
        point: candidate.point,
        axis: candidate.axis,
        cost: candidateCost,
        sequence: sequence++,
      });
    }
  }

  const fallbackX =
    start.x + (end.x - start.x) / 2 + laneIndex * EDGE_LANE_GAP;
  return compactPoints([
    start,
    { x: fallbackX, y: start.y },
    { x: fallbackX, y: end.y },
    end,
  ]);
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
    const points = routeOnGrid(start, end, obstacles, sourceLaneIndex);

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
