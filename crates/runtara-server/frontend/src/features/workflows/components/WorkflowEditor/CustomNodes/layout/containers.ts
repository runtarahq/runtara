import { Position, type Edge, type Node } from '@xyflow/react';
import { NODE_TYPES } from '@/features/workflows/config/workflow.ts';
import {
  snapContainerHeightToGrid,
  snapPositionToGrid,
  snapToGrid,
} from '@/features/workflows/config/workflow-editor';
import {
  BASE_GROUP_HEIGHT,
  BASE_GROUP_WIDTH,
  BASE_HEIGHT,
  BASE_WIDTH,
  buildLayoutGraph,
  buildScopedEdges,
  type LayoutEdge,
  type LayoutGraph,
  type LayoutNode,
  type LayoutPoint,
  type LayoutSize,
} from './graph';
import { routeOrthogonalEdges, type OrthogonalRoute } from './edges';
import { layoutScope } from './place';

const LAYOUT_CONFIG = {
  rankSep: 75,
  nodeSep: 40,
  marginX: 40,
  marginY: 40,
  containerPadding: 24,
  containerNodeSep: 48,
  containerRankSep: 50,
};

export type WorkflowLayoutResult = {
  nodes: Node[];
  edges: Edge[];
  edgeRoutes?: Record<string, OrthogonalRoute>;
};

function groupNodesByParent(nodes: LayoutNode[]): Map<string, LayoutNode[]> {
  const nodesByParent = new Map<string, LayoutNode[]>();

  for (const node of nodes) {
    const parentId = node.parentId || 'root';
    const siblings = nodesByParent.get(parentId) ?? [];
    siblings.push(node);
    nodesByParent.set(parentId, siblings);
  }

  return nodesByParent;
}

function getContainerChildEdges(
  children: LayoutNode[],
  edges: LayoutEdge[]
): LayoutEdge[] {
  return buildScopedEdges(children, edges);
}

function computeNodeSize(
  node: LayoutNode,
  graph: LayoutGraph,
  nodesByParent: Map<string, LayoutNode[]>,
  computedSizes: Map<string, LayoutSize>
): LayoutSize {
  const cached = computedSizes.get(node.id);
  if (cached) return cached;

  if (node.type !== 'container') {
    computedSizes.set(node.id, node.size);
    return node.size;
  }

  const children = nodesByParent.get(node.id) || [];
  if (children.length === 0) {
    const emptySize = {
      width: BASE_GROUP_WIDTH,
      height: BASE_GROUP_HEIGHT,
    };
    computedSizes.set(node.id, emptySize);
    return emptySize;
  }

  const childSizes = new Map<string, LayoutSize>();
  for (const child of children) {
    childSizes.set(
      child.id,
      computeNodeSize(child, graph, nodesByParent, computedSizes)
    );
  }

  const childEdges = getContainerChildEdges(children, graph.edges);
  const childLayout = layoutScope(
    children,
    childEdges,
    childSizes,
    LAYOUT_CONFIG.containerRankSep,
    LAYOUT_CONFIG.containerNodeSep
  );

  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;

  for (const [nodeId, pos] of childLayout) {
    const nodeSize = childSizes.get(nodeId)!;
    minX = Math.min(minX, pos.x);
    minY = Math.min(minY, pos.y);
    maxX = Math.max(maxX, pos.x + nodeSize.width);
    maxY = Math.max(maxY, pos.y + nodeSize.height);
  }

  if (minX === Infinity || minY === Infinity) {
    const emptySize = {
      width: BASE_GROUP_WIDTH,
      height: BASE_GROUP_HEIGHT,
    };
    computedSizes.set(node.id, emptySize);
    return emptySize;
  }

  const contentWidth = maxX - minX;
  const contentHeight = maxY - minY;
  const size = {
    width: snapToGrid(
      Math.max(
        contentWidth + LAYOUT_CONFIG.containerPadding * 2,
        BASE_GROUP_WIDTH
      )
    ),
    height: snapContainerHeightToGrid(
      Math.max(
        contentHeight + LAYOUT_CONFIG.containerPadding * 2,
        BASE_GROUP_HEIGHT
      )
    ),
  };
  computedSizes.set(node.id, size);
  return size;
}

function getReactFlowSize(size: LayoutSize | undefined): LayoutSize {
  return {
    width: size?.width ?? BASE_WIDTH,
    height: size?.height ?? BASE_HEIGHT,
  };
}

function applyNodeLayout(
  node: Node,
  position: LayoutPoint | undefined,
  size: LayoutSize | undefined,
  edges: Edge[]
): Node {
  const resolvedSize = getReactFlowSize(size);

  return {
    ...node,
    position: snapPositionToGrid(position || { x: 0, y: 0 }),
    style: {
      ...node.style,
      width: resolvedSize.width,
      height: resolvedSize.height,
    },
    width: resolvedSize.width,
    height: resolvedSize.height,
    sourcePosition: edges.some((edge) => edge.source === node.id)
      ? Position.Right
      : undefined,
    targetPosition: edges.some((edge) => edge.target === node.id)
      ? Position.Left
      : undefined,
  };
}

function normalizeChildPositions(
  positions: Map<string, LayoutPoint>
): Map<string, LayoutPoint> {
  let minX = Infinity;
  let minY = Infinity;
  for (const [, pos] of positions) {
    minX = Math.min(minX, pos.x);
    minY = Math.min(minY, pos.y);
  }

  const offsetX =
    LAYOUT_CONFIG.containerPadding - (minX === Infinity ? 0 : minX);
  const offsetY =
    LAYOUT_CONFIG.containerPadding - (minY === Infinity ? 0 : minY);
  const normalized = new Map<string, LayoutPoint>();

  for (const [id, pos] of positions) {
    normalized.set(id, {
      x: pos.x + offsetX,
      y: pos.y + offsetY,
    });
  }

  return normalized;
}

function rootEdgesForNodes(rootNodes: LayoutNode[], edges: LayoutEdge[]) {
  return buildScopedEdges(rootNodes, edges);
}

export function layoutReactFlowElements(
  nodes: Node[],
  edges: Edge[]
): WorkflowLayoutResult {
  const noteNodes = nodes.filter((node) => node.type === NODE_TYPES.NoteNode);
  const layoutReactFlowNodes = nodes.filter(
    (node) => node.type !== NODE_TYPES.NoteNode
  );
  const reactFlowNodeById = new Map(
    layoutReactFlowNodes.map((node) => [node.id, node])
  );
  const graph = buildLayoutGraph(layoutReactFlowNodes, edges);
  const nodesByParent = groupNodesByParent(graph.nodes);
  const computedSizes = new Map<string, LayoutSize>();

  for (const node of graph.nodes) {
    computeNodeSize(node, graph, nodesByParent, computedSizes);
  }

  const rootNodes = nodesByParent.get('root') || [];
  const rootPositions = layoutScope(
    rootNodes,
    rootEdgesForNodes(rootNodes, graph.edges),
    computedSizes,
    LAYOUT_CONFIG.rankSep,
    LAYOUT_CONFIG.nodeSep
  );

  for (const [id, pos] of rootPositions) {
    rootPositions.set(id, {
      x: pos.x + LAYOUT_CONFIG.marginX,
      y: pos.y + LAYOUT_CONFIG.marginY,
    });
  }

  const resultNodes = graph.nodes.map((layoutNode) => {
    const reactFlowNode = reactFlowNodeById.get(layoutNode.id);
    if (!reactFlowNode) return null;

    if (!layoutNode.parentId) {
      return applyNodeLayout(
        reactFlowNode,
        rootPositions.get(layoutNode.id),
        computedSizes.get(layoutNode.id),
        edges
      );
    }

    return reactFlowNode;
  });

  const compactResultNodes = resultNodes.filter((node): node is Node =>
    Boolean(node)
  );
  const resultNodeIndexById = new Map(
    compactResultNodes.map((node, index) => [node.id, index])
  );
  const containerNodes = graph.nodes.filter(
    (node) => node.type === 'container'
  );

  for (const container of containerNodes) {
    const children = nodesByParent.get(container.id) || [];
    if (children.length === 0) continue;

    const childSizes = new Map<string, LayoutSize>();
    for (const child of children) {
      childSizes.set(child.id, computedSizes.get(child.id)!);
    }

    const childPositions = normalizeChildPositions(
      layoutScope(
        children,
        getContainerChildEdges(children, graph.edges),
        childSizes,
        LAYOUT_CONFIG.containerRankSep,
        LAYOUT_CONFIG.containerNodeSep
      )
    );

    for (const child of children) {
      const resultIndex = resultNodeIndexById.get(child.id);
      const reactFlowNode = reactFlowNodeById.get(child.id);
      if (resultIndex === undefined || !reactFlowNode) continue;

      compactResultNodes[resultIndex] = applyNodeLayout(
        reactFlowNode,
        childPositions.get(child.id),
        computedSizes.get(child.id),
        edges
      );
    }
  }

  const outputNodes = [...compactResultNodes, ...noteNodes];

  return {
    nodes: outputNodes,
    edges,
    edgeRoutes: routeOrthogonalEdges(outputNodes, edges),
  };
}

export function ensureContainersContainChildren(nodes: Node[]): Node[] {
  const resultNodes = nodes.map((node) => ({
    ...node,
    style: node.style ? { ...node.style } : node.style,
  }));
  const containerIds = new Set(
    resultNodes
      .filter((node) => node.type === NODE_TYPES.ContainerNode)
      .map((node) => node.id)
  );

  for (const containerId of containerIds) {
    const container = resultNodes.find((node) => node.id === containerId);
    if (!container) continue;

    const children = resultNodes.filter(
      (node) => node.parentId === containerId
    );
    if (children.length === 0) continue;

    let maxRight = 0;
    let maxBottom = 0;
    for (const child of children) {
      const childWidth =
        (child.style?.width as number) || child.width || BASE_WIDTH;
      const childHeight =
        (child.style?.height as number) || child.height || BASE_HEIGHT;
      maxRight = Math.max(maxRight, child.position.x + childWidth);
      maxBottom = Math.max(maxBottom, child.position.y + childHeight);
    }

    const requiredWidth = snapToGrid(maxRight + LAYOUT_CONFIG.containerPadding);
    const requiredHeight = snapContainerHeightToGrid(
      maxBottom + LAYOUT_CONFIG.containerPadding
    );
    const currentWidth =
      (container.style?.width as number) || container.width || BASE_GROUP_WIDTH;
    const currentHeight =
      (container.style?.height as number) ||
      container.height ||
      BASE_GROUP_HEIGHT;

    if (requiredWidth > currentWidth || requiredHeight > currentHeight) {
      const newWidth = Math.max(currentWidth, requiredWidth);
      const newHeight = Math.max(currentHeight, requiredHeight);
      container.width = newWidth;
      container.height = newHeight;
      container.style = {
        ...container.style,
        width: newWidth,
        height: newHeight,
      };
    }
  }

  return resultNodes;
}
