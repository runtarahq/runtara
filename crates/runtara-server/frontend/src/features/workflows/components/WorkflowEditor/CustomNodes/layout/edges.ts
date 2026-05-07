import type { Edge, Node } from '@xyflow/react';
import { BASE_HEIGHT, BASE_WIDTH, type LayoutPoint } from './graph';

export type OrthogonalRoute = {
  points: LayoutPoint[];
  labelPoint?: LayoutPoint;
};

function getNodeBox(node: Node): {
  x: number;
  y: number;
  width: number;
  height: number;
} {
  const width =
    (typeof node.style?.width === 'number' ? node.style.width : undefined) ??
    (typeof node.width === 'number' ? node.width : undefined) ??
    BASE_WIDTH;
  const height =
    (typeof node.style?.height === 'number' ? node.style.height : undefined) ??
    (typeof node.height === 'number' ? node.height : undefined) ??
    BASE_HEIGHT;

  return {
    x: node.position.x,
    y: node.position.y,
    width,
    height,
  };
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

export function routeOrthogonalEdges(
  nodes: Node[],
  edges: Edge[]
): Record<string, OrthogonalRoute> {
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const routeCountsBySource = new Map<string, number>();
  const routes: Record<string, OrthogonalRoute> = {};

  for (const edge of edges) {
    const source = nodeById.get(edge.source);
    const target = nodeById.get(edge.target);
    if (!source || !target) continue;

    const sourceBox = getNodeBox(source);
    const targetBox = getNodeBox(target);
    const sourcePosition = getAbsolutePosition(source, nodeById);
    const targetPosition = getAbsolutePosition(target, nodeById);
    const sourceKey = `${edge.source}:${edge.sourceHandle ?? 'source'}`;
    const laneIndex = routeCountsBySource.get(sourceKey) ?? 0;
    routeCountsBySource.set(sourceKey, laneIndex + 1);

    const laneOffset = laneIndex * 6;
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
    const midX = start.x + Math.max(24, (end.x - start.x) / 2);

    routes[edge.id] = {
      points: [start, { x: midX, y: start.y }, { x: midX, y: end.y }, end],
      labelPoint: {
        x: start.x + 24,
        y: start.y,
      },
    };
  }

  return routes;
}
