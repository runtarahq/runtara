import type { Edge, Node } from '@xyflow/react';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
} from '@/features/workflows/config/workflow.ts';
import {
  SNAP_GRID_SIZE,
  snapToGrid,
} from '@/features/workflows/config/workflow-editor';

export const BASE_WIDTH = 132;
export const BASE_HEIGHT = 36;
export const BASE_GROUP_WIDTH = 168;
export const BASE_GROUP_HEIGHT = 132;

export type LayoutPoint = { x: number; y: number };
export type LayoutSize = { width: number; height: number };

export type LayoutNodeType =
  | 'basic'
  | 'conditional'
  | 'switch'
  | 'container'
  | 'note';

export type LayoutPort = {
  id: string;
  side: 'left' | 'right' | 'bottom';
  label?: string;
  order: number;
  visibleLabel?: boolean;
};

export type LayoutNode = {
  id: string;
  type: LayoutNodeType;
  parentId?: string;
  size: LayoutSize;
  ports: LayoutPort[];
  children?: string[];
  order: number;
  positionHint: LayoutPoint;
};

export type LayoutEdgeKind =
  | 'sequence'
  | 'conditional-true'
  | 'conditional-false'
  | 'switch-case'
  | 'switch-default'
  | 'error'
  | 'tool';

export type LayoutEdge = {
  id: string;
  source: string;
  target: string;
  sourceHandle: string;
  kind: LayoutEdgeKind;
  order: number;
};

export type LayoutGraph = {
  nodes: LayoutNode[];
  edges: LayoutEdge[];
};

export function snapGapToGrid(value: number): number {
  return Math.max(
    SNAP_GRID_SIZE,
    Math.ceil(value / SNAP_GRID_SIZE) * SNAP_GRID_SIZE
  );
}

function toLayoutNodeType(nodeType?: string): LayoutNodeType {
  if (nodeType === NODE_TYPES.ConditionalNode) return 'conditional';
  if (nodeType === NODE_TYPES.SwitchNode) return 'switch';
  if (nodeType === NODE_TYPES.ContainerNode) return 'container';
  if (nodeType === NODE_TYPES.NoteNode) return 'note';
  return 'basic';
}

function getSwitchCases(node: Node): any[] {
  const inputMapping = (node.data as { inputMapping?: unknown } | undefined)
    ?.inputMapping;
  if (!Array.isArray(inputMapping)) return [];

  const casesField = inputMapping.find(
    (item: any) => item && item.type === 'cases'
  );
  return Array.isArray(casesField?.value) ? casesField.value : [];
}

function isSwitchRoutingMode(node: Node, cases: any[]): boolean {
  const inputMapping = (node.data as { inputMapping?: unknown } | undefined)
    ?.inputMapping;
  const routingModeField = Array.isArray(inputMapping)
    ? inputMapping.find((item: any) => item && item.type === 'routingMode')
    : undefined;

  return (
    routingModeField?.value === true ||
    cases.some((caseItem) => caseItem?.route && caseItem.route !== '')
  );
}

function formatNumber(value: number): string {
  if (value >= 1000000) return `${value / 1000000}M`;
  if (value >= 1000) return `${value / 1000}K`;
  return value.toString();
}

function formatRangeLabel(match: any): string {
  if (!match || typeof match !== 'object') return String(match || '');

  if ('min' in match || 'max' in match) {
    const min = match.min;
    const max = match.max;
    if (min !== undefined && max !== undefined) {
      return `${formatNumber(min)}-${formatNumber(max)}`;
    }
    if (min !== undefined) return `>= ${formatNumber(min)}`;
    if (max !== undefined) return `< ${formatNumber(max)}`;
  }

  const parts: string[] = [];
  if ('gte' in match) parts.push(`>= ${formatNumber(match.gte)}`);
  else if ('gt' in match) parts.push(`> ${formatNumber(match.gt)}`);
  if ('lt' in match) parts.push(`< ${formatNumber(match.lt)}`);
  else if ('lte' in match) parts.push(`<= ${formatNumber(match.lte)}`);

  return parts.length > 0 ? parts.join(', ') : JSON.stringify(match);
}

function getSwitchCaseLabel(
  caseItem: any,
  index: number,
  preferRoute: boolean
): string {
  if (preferRoute && caseItem?.route) return caseItem.route;

  const matchType = caseItem?.matchType || 'exact';
  const match = caseItem?.match;

  switch (matchType) {
    case 'range':
      return formatRangeLabel(match);
    case 'exact':
      return String(match || `Case ${index + 1}`);
    case 'ne':
      return `!= ${match || ''}`;
    case 'in':
      return Array.isArray(match)
        ? match.join(', ')
        : String(match || `Case ${index + 1}`);
    case 'not_in':
      return Array.isArray(match)
        ? `not: ${match.join(', ')}`
        : String(match || `Case ${index + 1}`);
    case 'gt':
      return `> ${match}`;
    case 'gte':
      return `>= ${match}`;
    case 'lt':
      return `< ${match}`;
    case 'lte':
      return `<= ${match}`;
    case 'between':
      return Array.isArray(match) && match.length >= 2
        ? `${match[0]}-${match[1]}`
        : String(match || `Case ${index + 1}`);
    case 'starts_with':
      return `^${match || ''}`;
    case 'ends_with':
      return `${match || ''}$`;
    case 'contains':
      return `*${match || ''}*`;
    case 'is_defined':
      return 'defined?';
    case 'is_empty':
      return 'empty?';
    case 'is_not_empty':
      return 'not empty?';
    default:
      return `Case ${index + 1}`;
  }
}

export function getReactFlowNodeSize(node: Node): LayoutSize {
  const configuredSize =
    NODE_TYPE_SIZES[node.type || NODE_TYPES.BasicNode] ??
    NODE_TYPE_SIZES[NODE_TYPES.BasicNode];
  const styleWidth =
    typeof node.style?.width === 'number' ? node.style.width : undefined;
  const styleHeight =
    typeof node.style?.height === 'number' ? node.style.height : undefined;
  const nodeWidth = typeof node.width === 'number' ? node.width : undefined;
  const nodeHeight = typeof node.height === 'number' ? node.height : undefined;

  let width = styleWidth ?? nodeWidth ?? configuredSize.width;
  let height = styleHeight ?? nodeHeight ?? configuredSize.height;

  if (node.type === NODE_TYPES.SwitchNode) {
    const cases = getSwitchCases(node);
    if (isSwitchRoutingMode(node, cases)) {
      const handleSpacing = 18;
      const firstHandleTop = 24;
      const totalHandles = cases.length + 1;
      const lastHandleTop = firstHandleTop + (totalHandles - 1) * handleSpacing;
      height = Math.max(height, snapToGrid(lastHandleTop + 24));
    }
  }

  return {
    width: snapToGrid(width),
    height: snapToGrid(height),
  };
}

function buildPorts(node: Node): LayoutPort[] {
  const ports: LayoutPort[] = [{ id: 'target', side: 'left', order: 0 }];

  if (node.type === NODE_TYPES.ConditionalNode) {
    ports.push(
      { id: 'true', side: 'right', label: 'True', order: 0 },
      { id: 'false', side: 'right', label: 'False', order: 1 }
    );
    return ports;
  }

  if (node.type === NODE_TYPES.SwitchNode) {
    const cases = getSwitchCases(node);
    const routingMode = isSwitchRoutingMode(node, cases);
    if (routingMode) {
      for (let index = 0; index < cases.length; index++) {
        ports.push({
          id: `case-${index}`,
          side: 'right',
          label: getSwitchCaseLabel(cases[index], index, true),
          order: index,
          visibleLabel: true,
        });
      }
      ports.push({
        id: 'default',
        side: 'right',
        label: 'default',
        order: cases.length,
        visibleLabel: true,
      });
      return ports;
    }
  }

  if (node.type === NODE_TYPES.ContainerNode) {
    ports.push(
      { id: 'source', side: 'right', order: 0 },
      { id: 'onError', side: 'bottom', label: 'Error', order: 1 }
    );
    return ports;
  }

  ports.push({ id: 'source', side: 'right', order: 0 });
  return ports;
}

export function getEdgeHandle(edge: Edge | LayoutEdge): string {
  if ('sourceHandle' in edge && typeof edge.sourceHandle === 'string') {
    return edge.sourceHandle.trim().toLowerCase();
  }

  const label =
    'label' in edge && typeof edge.label === 'string' ? edge.label : undefined;
  return String(label ?? 'source')
    .trim()
    .toLowerCase();
}

export function getCaseHandleIndex(handle: string): number | null {
  const match = /^case-(\d+)$/.exec(handle);
  return match ? Number(match[1]) : null;
}

export function getEdgeKind(handle: string): LayoutEdgeKind {
  if (handle === 'true') return 'conditional-true';
  if (handle === 'false') return 'conditional-false';
  if (getCaseHandleIndex(handle) !== null) return 'switch-case';
  if (handle === 'default') return 'switch-default';
  if (handle === 'onerror' || handle === 'on-error') return 'error';
  if (handle !== 'source' && handle !== 'next' && handle !== '') return 'tool';
  return 'sequence';
}

export function getEdgeOrder(
  handle: string,
  kind = getEdgeKind(handle)
): number {
  const caseIndex = getCaseHandleIndex(handle);
  if (kind === 'conditional-true') return 0;
  if (kind === 'conditional-false') return 1;
  if (kind === 'switch-case' && caseIndex !== null) return 10 + caseIndex;
  if (kind === 'sequence') return 100;
  if (kind === 'switch-default') return 200;
  if (kind === 'error') return 300;
  return 400;
}

export function compareLayoutEdges(a: LayoutEdge, b: LayoutEdge): number {
  if (a.order !== b.order) return a.order - b.order;
  if (a.sourceHandle !== b.sourceHandle) {
    return a.sourceHandle.localeCompare(b.sourceHandle);
  }
  return a.id.localeCompare(b.id);
}

export function buildLayoutGraph(nodes: Node[], edges: Edge[]): LayoutGraph {
  const childrenByParent = new Map<string, string[]>();
  nodes.forEach((node) => {
    if (!node.parentId) return;
    const children = childrenByParent.get(node.parentId) ?? [];
    children.push(node.id);
    childrenByParent.set(node.parentId, children);
  });

  const layoutNodes = nodes.map<LayoutNode>((node, index) => ({
    id: node.id,
    type: toLayoutNodeType(node.type),
    parentId: node.parentId,
    size: getReactFlowNodeSize(node),
    ports: buildPorts(node),
    children: childrenByParent.get(node.id),
    order: index,
    positionHint: {
      x: node.position.x,
      y: node.position.y,
    },
  }));

  const layoutEdges = edges.map<LayoutEdge>((edge, index) => {
    const sourceHandle = getEdgeHandle(edge);
    const kind = getEdgeKind(sourceHandle);

    return {
      id: edge.id,
      source: edge.source,
      target: edge.target,
      sourceHandle,
      kind,
      order: getEdgeOrder(sourceHandle, kind) + index / 1000,
    };
  });

  return {
    nodes: layoutNodes,
    edges: layoutEdges,
  };
}

export function buildScopedEdges(
  nodes: LayoutNode[],
  edges: LayoutEdge[]
): LayoutEdge[] {
  const nodeSet = new Set(nodes.map((node) => node.id));
  return edges.filter(
    (edge) => nodeSet.has(edge.source) && nodeSet.has(edge.target)
  );
}

export function buildOutgoingEdges(
  edges: LayoutEdge[]
): Map<string, LayoutEdge[]> {
  const outgoing = new Map<string, LayoutEdge[]>();

  for (const edge of edges) {
    const list = outgoing.get(edge.source) ?? [];
    list.push(edge);
    outgoing.set(edge.source, list);
  }

  for (const list of outgoing.values()) {
    list.sort(compareLayoutEdges);
  }

  return outgoing;
}

export function buildIncomingEdges(
  edges: LayoutEdge[]
): Map<string, LayoutEdge[]> {
  const incoming = new Map<string, LayoutEdge[]>();

  for (const edge of edges) {
    const list = incoming.get(edge.target) ?? [];
    list.push(edge);
    incoming.set(edge.target, list);
  }

  for (const list of incoming.values()) {
    list.sort(compareLayoutEdges);
  }

  return incoming;
}

export function compareLayoutNodes(a: LayoutNode, b: LayoutNode): number {
  if (a.positionHint.x !== b.positionHint.x) {
    return a.positionHint.x - b.positionHint.x;
  }
  if (a.positionHint.y !== b.positionHint.y) {
    return a.positionHint.y - b.positionHint.y;
  }
  return a.order - b.order || a.id.localeCompare(b.id);
}

export function findConnectedComponents(
  nodeIds: string[],
  edges: LayoutEdge[]
): Set<string>[] {
  const visited = new Set<string>();
  const components: Set<string>[] = [];
  const adjacency = new Map<string, Set<string>>();

  for (const id of nodeIds) {
    adjacency.set(id, new Set());
  }

  for (const edge of edges) {
    if (adjacency.has(edge.source) && adjacency.has(edge.target)) {
      adjacency.get(edge.source)!.add(edge.target);
      adjacency.get(edge.target)!.add(edge.source);
    }
  }

  for (const startId of nodeIds) {
    if (visited.has(startId)) continue;

    const component = new Set<string>();
    const queue = [startId];

    while (queue.length > 0) {
      const current = queue.shift()!;
      if (visited.has(current)) continue;

      visited.add(current);
      component.add(current);

      for (const neighbor of adjacency.get(current) || []) {
        if (!visited.has(neighbor)) {
          queue.push(neighbor);
        }
      }
    }

    components.push(component);
  }

  return components;
}
