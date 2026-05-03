import { Edge, Node, Position } from '@xyflow/react';
import { graphlib, layout as dagreLayout } from '@dagrejs/dagre';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
  STEP_TYPES,
} from '@/features/workflows/config/workflow.ts';
import {
  ExecutionGraphDto,
  ExecutionGraphStepDto,
  ExecutionGraphTransitionDto,
} from '@/features/workflows/types/execution-graph';
import { type Note, ValueType } from '@/generated/RuntaraRuntimeApi.ts';
import {
  parseSchema,
  buildSchemaFromFields,
} from '@/features/workflows/utils/schema';
import {
  snapToGrid,
  snapPositionToGrid,
  snapContainerHeightToGrid,
} from '@/features/workflows/config/workflow-editor';
import {
  migrateStartStep,
  needsStartStepMigration,
} from '@/features/workflows/utils/start-step-migration';
import { convertConditionArguments } from '@/shared/utils/condition-type-conversion';

// Base dimensions - must be multiples of SNAP_GRID_SIZE (12px) for proper alignment
export const BASE_WIDTH = 132; // 11 * 12 - compact pill shape
const BASE_HEIGHT = 36; // 3 * 12 - compact height

export const BASE_GROUP_WIDTH = 168; // 14 * 12
const BASE_GROUP_HEIGHT = 132; // 11 * 12

export type ExecutionGraphTransition = Required<ExecutionGraphTransitionDto>;

export type ExecutionGraphStep = ExecutionGraphStepDto &
  Required<
    Pick<
      ExecutionGraphStepDto,
      'id' | 'inputMapping' | 'name' | 'stepType' | 'renderingParameters'
    >
  > & { subgraph?: ExecutionGraph };

export interface ExecutionGraph {
  steps?: Record<string, ExecutionGraphStep>;
  executionPlan?: ExecutionGraphTransition[];
  entryPoint: string;
  notes?: Note[];
  // Static variables defined at workflow version level
  variables?: Record<string, any>;
}

// Switch match type mapping: UI lowercase <-> API uppercase
const MATCH_TYPE_UI_TO_API: Record<string, string> = {
  exact: 'EQ',
  ne: 'NE',
  in: 'IN',
  not_in: 'NOT_IN',
  gt: 'GT',
  gte: 'GTE',
  lt: 'LT',
  lte: 'LTE',
  starts_with: 'STARTS_WITH',
  ends_with: 'ENDS_WITH',
  contains: 'CONTAINS',
  is_defined: 'IS_DEFINED',
  is_empty: 'IS_EMPTY',
  is_not_empty: 'IS_NOT_EMPTY',
  between: 'BETWEEN',
  range: 'RANGE',
};

const MATCH_TYPE_API_TO_UI: Record<string, string> = Object.fromEntries(
  Object.entries(MATCH_TYPE_UI_TO_API).map(([k, v]) => [v, k])
);

function mapMatchTypeToAPI(matchType: string): string {
  // Already uppercase (from API) → pass through
  if (matchType === matchType.toUpperCase() && matchType.length > 1)
    return matchType;
  return MATCH_TYPE_UI_TO_API[matchType] || matchType.toUpperCase();
}

function mapMatchTypeFromAPI(matchType: string): string {
  return MATCH_TYPE_API_TO_UI[matchType] || matchType.toLowerCase();
}

// Layout configuration
const LAYOUT_CONFIG = {
  rankSep: 75, // Horizontal spacing between ranks (columns)
  nodeSep: 40, // Vertical spacing between nodes in same rank
  marginX: 40, // Left margin
  marginY: 40, // Top margin
  containerPadding: 24, // Padding inside containers
  containerNodeSep: 30, // Vertical spacing inside containers
  containerRankSep: 50, // Horizontal spacing inside containers
};

/**
 * Get the size of a node, handling containers recursively.
 * For containers, first layout children using Dagre to determine the required size.
 */
function getNodeSize(
  node: Node,
  allNodes: Node[],
  edges: Edge[],
  nodesByParent: Map<string, Node[]>,
  computedSizes: Map<string, { width: number; height: number }>
): { width: number; height: number } {
  // Check if already computed
  if (computedSizes.has(node.id)) {
    return computedSizes.get(node.id)!;
  }

  // For non-containers, use style or defaults
  if (node.type !== NODE_TYPES.ContainerNode) {
    const size = {
      width: (node.style?.width as number) ?? BASE_WIDTH,
      height: (node.style?.height as number) ?? BASE_HEIGHT,
    };
    computedSizes.set(node.id, size);
    return size;
  }

  // For containers, compute based on children using Dagre
  const children = nodesByParent.get(node.id) || [];
  if (children.length === 0) {
    const size = { width: BASE_GROUP_WIDTH, height: BASE_GROUP_HEIGHT };
    computedSizes.set(node.id, size);
    return size;
  }

  // First compute sizes for all children (recursively handles nested containers)
  const childSizes = new Map<string, { width: number; height: number }>();
  for (const child of children) {
    const size = getNodeSize(
      child,
      allNodes,
      edges,
      nodesByParent,
      computedSizes
    );
    childSizes.set(child.id, size);
  }

  // Layout children using Dagre to determine container size
  const childEdges = edges.filter((e) => {
    const src = children.find((n) => n.id === e.source);
    const tgt = children.find((n) => n.id === e.target);
    return src && tgt;
  });

  const childLayout = layoutWithDagre(
    children,
    childEdges,
    childSizes,
    LAYOUT_CONFIG.containerRankSep,
    LAYOUT_CONFIG.containerNodeSep
  );

  // Calculate bounding box (accounting for nodes that may have negative positions after branch adjustment)
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

  // If no children were laid out, use defaults
  if (minX === Infinity) {
    const defaultSize = { width: BASE_GROUP_WIDTH, height: BASE_GROUP_HEIGHT };
    computedSizes.set(node.id, defaultSize);
    return defaultSize;
  }

  const padding = LAYOUT_CONFIG.containerPadding;
  const contentWidth = maxX - minX;
  const contentHeight = maxY - minY;

  const size = {
    width: snapToGrid(Math.max(contentWidth + padding * 2, BASE_GROUP_WIDTH)),
    height: snapContainerHeightToGrid(
      Math.max(contentHeight + padding * 2, BASE_GROUP_HEIGHT)
    ),
  };
  computedSizes.set(node.id, size);
  return size;
}

/**
 * Get all descendants of a node following edges in the graph.
 */
function getDescendants(
  nodeId: string,
  edges: Edge[],
  nodeSet: Set<string>
): Set<string> {
  const descendants = new Set<string>();
  const queue = [nodeId];

  while (queue.length > 0) {
    const current = queue.shift()!;
    for (const edge of edges) {
      if (edge.source === current && nodeSet.has(edge.target)) {
        if (!descendants.has(edge.target)) {
          descendants.add(edge.target);
          queue.push(edge.target);
        }
      }
    }
  }

  return descendants;
}

/**
 * Find all connected components (subgraphs) in the graph.
 * Returns an array of sets, where each set contains node IDs in that component.
 */
function findConnectedComponents(
  nodeIds: string[],
  edges: Edge[]
): Set<string>[] {
  const visited = new Set<string>();
  const components: Set<string>[] = [];

  // Build undirected adjacency list
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

  // BFS to find each component
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

/**
 * Layout nodes using Dagre algorithm with post-processing for conditional branches.
 * Dagre provides automatic hierarchical layout, then we adjust to ensure:
 * - True branches are positioned above (lower Y) the conditional node
 * - False branches are positioned below (higher Y) the conditional node
 * - Disconnected subgraphs preserve their relative horizontal positions
 */
function layoutWithDagre(
  nodes: Node[],
  edges: Edge[],
  sizes: Map<string, { width: number; height: number }>,
  rankSep: number,
  nodeSep: number
): Map<string, { x: number; y: number }> {
  const positions = new Map<string, { x: number; y: number }>();

  if (nodes.length === 0) {
    return positions;
  }

  // Store original positions for preserving relative horizontal ordering
  const originalPositions = new Map<string, { x: number; y: number }>();
  for (const node of nodes) {
    originalPositions.set(node.id, { x: node.position.x, y: node.position.y });
  }

  // Find connected components to handle disconnected subgraphs
  const nodeIds = nodes.map((n) => n.id);
  const components = findConnectedComponents(nodeIds, edges);

  // If there's only one component, use standard Dagre layout
  if (components.length <= 1) {
    return layoutWithDagreInternal(nodes, edges, sizes, rankSep, nodeSep);
  }

  // Multiple components: layout each separately, then position based on original X
  // Sort components by their minimum original X position
  const componentData = components.map((component) => {
    const componentNodes = nodes.filter((n) => component.has(n.id));
    const componentEdges = edges.filter(
      (e) => component.has(e.source) && component.has(e.target)
    );

    // Find the minimum original X in this component (leftmost node)
    let minOriginalX = Infinity;
    for (const nodeId of component) {
      const origPos = originalPositions.get(nodeId);
      if (origPos && origPos.x < minOriginalX) {
        minOriginalX = origPos.x;
      }
    }

    return {
      component,
      nodes: componentNodes,
      edges: componentEdges,
      // Subtract margin since getLayoutedElements will re-add it after layout
      minOriginalX: Math.max(0, minOriginalX - LAYOUT_CONFIG.marginX),
    };
  });

  // Sort by original X position
  componentData.sort((a, b) => a.minOriginalX - b.minOriginalX);

  // Layout each component and position them horizontally
  let currentX = 0;
  const componentGap = rankSep; // Gap between disconnected components

  for (const {
    nodes: compNodes,
    edges: compEdges,
    minOriginalX,
  } of componentData) {
    const compPositions = layoutWithDagreInternal(
      compNodes,
      compEdges,
      sizes,
      rankSep,
      nodeSep
    );

    // Find the bounding box of this component after layout
    let compMinX = Infinity;
    let compMaxX = -Infinity;
    for (const [nodeId, pos] of compPositions) {
      const size = sizes.get(nodeId) || {
        width: BASE_WIDTH,
        height: BASE_HEIGHT,
      };
      compMinX = Math.min(compMinX, pos.x);
      compMaxX = Math.max(compMaxX, pos.x + size.width);
    }

    // Calculate offset to position this component
    // Use the larger of: (1) currentX to prevent overlap, or (2) original position to preserve spacing
    const targetX = Math.max(currentX, minOriginalX);
    const offsetX = targetX - compMinX;

    // Apply offset and add to final positions
    for (const [nodeId, pos] of compPositions) {
      positions.set(nodeId, {
        x: pos.x + offsetX,
        y: pos.y,
      });
    }

    // Update currentX for next component
    currentX = targetX + (compMaxX - compMinX) + componentGap;
  }

  return positions;
}

/**
 * Internal Dagre layout without connected component handling.
 */
function layoutWithDagreInternal(
  nodes: Node[],
  edges: Edge[],
  sizes: Map<string, { width: number; height: number }>,
  rankSep: number,
  nodeSep: number
): Map<string, { x: number; y: number }> {
  const positions = new Map<string, { x: number; y: number }>();

  if (nodes.length === 0) {
    return positions;
  }

  // Create a new Dagre graph
  const g = new graphlib.Graph().setDefaultEdgeLabel(() => ({}));

  // Configure the graph for left-to-right layout
  g.setGraph({
    rankdir: 'LR', // Left to Right
    nodesep: nodeSep, // Vertical separation between nodes
    ranksep: rankSep, // Horizontal separation between ranks
    marginx: 0,
    marginy: 0,
  });

  // Add nodes to the graph with their dimensions
  for (const node of nodes) {
    const size = sizes.get(node.id) || {
      width: BASE_WIDTH,
      height: BASE_HEIGHT,
    };
    g.setNode(node.id, {
      width: size.width,
      height: size.height,
    });
  }

  // Add edges to the graph
  for (const edge of edges) {
    // Only add edges where both source and target are in our node set
    if (g.hasNode(edge.source) && g.hasNode(edge.target)) {
      g.setEdge(edge.source, edge.target);
    }
  }

  // Run the Dagre layout algorithm
  dagreLayout(g);

  // Extract positions from the laid out graph
  // Dagre returns center positions, we need top-left
  for (const node of nodes) {
    const dagreNode = g.node(node.id);
    if (dagreNode) {
      const size = sizes.get(node.id) || {
        width: BASE_WIDTH,
        height: BASE_HEIGHT,
      };
      positions.set(node.id, {
        x: dagreNode.x - size.width / 2,
        y: dagreNode.y - size.height / 2,
      });
    }
  }

  // Post-process: Enforce true branches above, false branches below for conditional nodes
  // and center the conditional node between its branches
  // Process conditionals from right to left (inner/deeper ones first) so that
  // outer conditionals can account for repositioned inner branches
  const nodeSet = new Set(nodes.map((n) => n.id));
  const conditionalNodes = nodes
    .filter((n) => n.type === NODE_TYPES.ConditionalNode)
    .sort((a, b) => {
      const posA = positions.get(a.id);
      const posB = positions.get(b.id);
      // Sort by X descending (rightmost/deepest first)
      return (posB?.x || 0) - (posA?.x || 0);
    });

  for (const condNode of conditionalNodes) {
    const condPos = positions.get(condNode.id);
    if (!condPos) continue;

    const condSize = sizes.get(condNode.id) || {
      width: BASE_WIDTH,
      height: BASE_HEIGHT,
    };

    // Find true and false branch edges
    const trueEdge = edges.find(
      (e) => e.source === condNode.id && e.sourceHandle === 'true'
    );
    const falseEdge = edges.find(
      (e) => e.source === condNode.id && e.sourceHandle === 'false'
    );

    // Handle cases where one or both branches might not exist or have targets outside our node set
    const trueTargetId = trueEdge?.target;
    const falseTargetId = falseEdge?.target;

    const hasTrueBranch = trueTargetId && nodeSet.has(trueTargetId);
    const hasFalseBranch = falseTargetId && nodeSet.has(falseTargetId);

    // Skip if neither branch has a valid target in our node set
    if (!hasTrueBranch && !hasFalseBranch) continue;

    // Get all descendants of each branch
    const trueDescendantsAll = new Set<string>();
    if (hasTrueBranch) {
      const descendants = getDescendants(trueTargetId, edges, nodeSet);
      descendants.add(trueTargetId);
      descendants.forEach((d) => trueDescendantsAll.add(d));
    }

    const falseDescendantsAll = new Set<string>();
    if (hasFalseBranch) {
      const descendants = getDescendants(falseTargetId, edges, nodeSet);
      descendants.add(falseTargetId);
      descendants.forEach((d) => falseDescendantsAll.add(d));
    }

    // Exclude merge points (nodes reachable from both branches) from repositioning
    // These nodes should stay where Dagre placed them
    const mergePoints = new Set<string>();
    for (const nodeId of trueDescendantsAll) {
      if (falseDescendantsAll.has(nodeId)) {
        mergePoints.add(nodeId);
      }
    }

    const trueDescendants = new Set<string>();
    for (const nodeId of trueDescendantsAll) {
      if (!mergePoints.has(nodeId)) {
        trueDescendants.add(nodeId);
      }
    }

    const falseDescendants = new Set<string>();
    for (const nodeId of falseDescendantsAll) {
      if (!mergePoints.has(nodeId)) {
        falseDescendants.add(nodeId);
      }
    }

    // Calculate bounding boxes for each branch
    let trueBranchMinY = Infinity,
      trueBranchMaxY = -Infinity;
    let falseBranchMinY = Infinity,
      falseBranchMaxY = -Infinity;

    for (const nodeId of trueDescendants) {
      const pos = positions.get(nodeId);
      const size = sizes.get(nodeId) || {
        width: BASE_WIDTH,
        height: BASE_HEIGHT,
      };
      if (pos) {
        trueBranchMinY = Math.min(trueBranchMinY, pos.y);
        trueBranchMaxY = Math.max(trueBranchMaxY, pos.y + size.height);
      }
    }

    for (const nodeId of falseDescendants) {
      const pos = positions.get(nodeId);
      const size = sizes.get(nodeId) || {
        width: BASE_WIDTH,
        height: BASE_HEIGHT,
      };
      if (pos) {
        falseBranchMinY = Math.min(falseBranchMinY, pos.y);
        falseBranchMaxY = Math.max(falseBranchMaxY, pos.y + size.height);
      }
    }

    const trueBranchHeight =
      trueBranchMinY !== Infinity ? trueBranchMaxY - trueBranchMinY : 0;
    const falseBranchHeight =
      falseBranchMinY !== Infinity ? falseBranchMaxY - falseBranchMinY : 0;

    // Keep conditional at its current position and arrange branches around it
    // True branch: center should be at or above the conditional's top edge
    // False branch: center should be at or below the conditional's bottom edge
    const condTop = condPos.y;
    const condBottom = condPos.y + condSize.height;

    const minGap = nodeSep / 2;

    // Calculate and apply shifts for true branch
    if (trueBranchMinY !== Infinity && trueBranchHeight > 0) {
      const trueBranchCurrentCenterY = (trueBranchMinY + trueBranchMaxY) / 2;

      // True branch center should be above conditional center
      // For small branches, align center with conditional's top area
      // For large branches, position so bottom is above conditional top
      let trueBranchTargetCenterY: number;
      if (trueBranchHeight <= condSize.height) {
        // Small branch - align center with conditional's upper portion
        trueBranchTargetCenterY = condTop - minGap - trueBranchHeight / 2;
      } else {
        // Large branch - position so it's above conditional with gap
        trueBranchTargetCenterY = condTop - minGap - trueBranchHeight / 2;
      }

      const trueShift = trueBranchTargetCenterY - trueBranchCurrentCenterY;
      for (const nodeId of trueDescendants) {
        const pos = positions.get(nodeId);
        if (pos) {
          positions.set(nodeId, { x: pos.x, y: pos.y + trueShift });
        }
      }
    }

    // Calculate and apply shifts for false branch
    if (falseBranchMinY !== Infinity && falseBranchHeight > 0) {
      const falseBranchCurrentCenterY = (falseBranchMinY + falseBranchMaxY) / 2;

      // False branch center should be below conditional center
      let falseBranchTargetCenterY: number;
      if (falseBranchHeight <= condSize.height) {
        // Small branch - align center with conditional's lower portion
        falseBranchTargetCenterY = condBottom + minGap + falseBranchHeight / 2;
      } else {
        // Large branch - position so it's below conditional with gap
        falseBranchTargetCenterY = condBottom + minGap + falseBranchHeight / 2;
      }

      const falseShift = falseBranchTargetCenterY - falseBranchCurrentCenterY;
      for (const nodeId of falseDescendants) {
        const pos = positions.get(nodeId);
        if (pos) {
          positions.set(nodeId, { x: pos.x, y: pos.y + falseShift });
        }
      }
    }
  }

  return positions;
}

export function getLayoutedElements(nodes: Node[], edges: Edge[]) {
  // Separate notes from regular nodes - notes should not participate in auto-layout
  const noteNodes = nodes.filter((n) => n.type === NODE_TYPES.NoteNode);
  const layoutNodes = nodes.filter((n) => n.type !== NODE_TYPES.NoteNode);

  // Separate nodes by parent
  const nodesByParent = new Map<string, Node[]>();
  for (const node of layoutNodes) {
    const parentId = node.parentId || 'root';
    if (!nodesByParent.has(parentId)) {
      nodesByParent.set(parentId, []);
    }
    nodesByParent.get(parentId)!.push(node);
  }

  // Compute sizes for all nodes (handles nested containers recursively)
  const computedSizes = new Map<string, { width: number; height: number }>();
  for (const node of layoutNodes) {
    getNodeSize(node, layoutNodes, edges, nodesByParent, computedSizes);
  }

  // Layout root-level nodes
  const rootNodes = nodesByParent.get('root') || [];
  const rootEdges = edges.filter((e) => {
    const src = rootNodes.find((n) => n.id === e.source);
    const tgt = rootNodes.find((n) => n.id === e.target);
    return src && tgt;
  });

  const rootPositions = layoutWithDagre(
    rootNodes,
    rootEdges,
    computedSizes,
    LAYOUT_CONFIG.rankSep,
    LAYOUT_CONFIG.nodeSep
  );

  // Apply margin offset to root positions
  for (const [id, pos] of rootPositions) {
    rootPositions.set(id, {
      x: pos.x + LAYOUT_CONFIG.marginX,
      y: pos.y + LAYOUT_CONFIG.marginY,
    });
  }

  // Build result nodes array
  const resultNodes: Node[] = [];

  // Process layout nodes (excludes notes)
  for (const node of layoutNodes) {
    if (!node.parentId) {
      // Root level node
      const pos = rootPositions.get(node.id);
      const size = computedSizes.get(node.id)!;

      resultNodes.push({
        ...node,
        position: snapPositionToGrid(pos || { x: 0, y: 0 }),
        style: {
          ...node.style,
          width: size.width,
          height: size.height,
        },
        width: size.width,
        height: size.height,
        sourcePosition: edges.some((e) => e.source === node.id)
          ? Position.Right
          : undefined,
        targetPosition: edges.some((e) => e.target === node.id)
          ? Position.Left
          : undefined,
      });
    } else {
      // Child node - will be positioned later with container
      resultNodes.push(node);
    }
  }

  // Now layout children inside each container
  const containerNodes = layoutNodes.filter(
    (n) => n.type === NODE_TYPES.ContainerNode
  );

  for (const container of containerNodes) {
    const children = nodesByParent.get(container.id) || [];
    if (children.length === 0) continue;

    // Get sizes for children
    const childSizes = new Map<string, { width: number; height: number }>();
    for (const child of children) {
      childSizes.set(child.id, computedSizes.get(child.id)!);
    }

    // Layout children
    const childEdges = edges.filter((e) => {
      const src = children.find((n) => n.id === e.source);
      const tgt = children.find((n) => n.id === e.target);
      return src && tgt;
    });

    const childPositions = layoutWithDagre(
      children,
      childEdges,
      childSizes,
      LAYOUT_CONFIG.containerRankSep,
      LAYOUT_CONFIG.containerNodeSep
    );

    // Find minimum X and Y to normalize positions (some may be negative after branch adjustment)
    let minX = Infinity;
    let minY = Infinity;
    for (const [, pos] of childPositions) {
      minX = Math.min(minX, pos.x);
      minY = Math.min(minY, pos.y);
    }

    // Apply normalization and container padding offset
    const offsetX =
      LAYOUT_CONFIG.containerPadding - (minX === Infinity ? 0 : minX);
    const offsetY =
      LAYOUT_CONFIG.containerPadding - (minY === Infinity ? 0 : minY);

    for (const [id, pos] of childPositions) {
      childPositions.set(id, {
        x: pos.x + offsetX,
        y: pos.y + offsetY,
      });
    }

    // Update child nodes in result
    for (let i = 0; i < resultNodes.length; i++) {
      const node = resultNodes[i];
      if (node.parentId !== container.id) continue;

      const pos = childPositions.get(node.id);
      const size = computedSizes.get(node.id)!;

      resultNodes[i] = {
        ...node,
        position: snapPositionToGrid(pos || { x: 0, y: 0 }),
        style: {
          ...node.style,
          width: size.width,
          height: size.height,
        },
        width: size.width,
        height: size.height,
        sourcePosition: edges.some((e) => e.source === node.id)
          ? Position.Right
          : undefined,
        targetPosition: edges.some((e) => e.target === node.id)
          ? Position.Left
          : undefined,
      };
    }
  }

  // Final pass: align sequential edges by adjusting node centers.
  // Even without Y snapping, Dagre may not perfectly align nodes when
  // branching structures pull nodes in different directions.
  // Process edges left-to-right so alignments cascade through the chain.
  const sequentialEdges = edges
    .filter(
      (e) =>
        e.sourceHandle !== 'true' &&
        e.sourceHandle !== 'false' &&
        e.sourceHandle !== 'onError' &&
        !e.sourceHandle?.startsWith('case-') &&
        e.sourceHandle !== 'default'
    )
    .sort((a, b) => {
      const srcA = resultNodes.find((n) => n.id === a.source);
      const srcB = resultNodes.find((n) => n.id === b.source);
      return (srcA?.position.x || 0) - (srcB?.position.x || 0);
    });

  for (const edge of sequentialEdges) {
    const srcNode = resultNodes.find((n) => n.id === edge.source);
    const tgtNode = resultNodes.find((n) => n.id === edge.target);
    if (!srcNode || !tgtNode) continue;
    // Only align nodes at the same nesting level
    if (srcNode.parentId !== tgtNode.parentId) continue;

    const srcH =
      (srcNode.style?.height as number) || srcNode.height || BASE_HEIGHT;
    const tgtH =
      (tgtNode.style?.height as number) || tgtNode.height || BASE_HEIGHT;
    const srcCenterY = srcNode.position.y + srcH / 2;
    const tgtCenterY = tgtNode.position.y + tgtH / 2;

    if (Math.abs(srcCenterY - tgtCenterY) < 1) continue;

    // Move the smaller non-container/non-conditional node to align with the larger
    const isMovable = (n: Node) =>
      n.type !== NODE_TYPES.ContainerNode &&
      n.type !== NODE_TYPES.ConditionalNode &&
      n.type !== NODE_TYPES.SwitchNode;

    if (isMovable(tgtNode) && tgtH <= srcH) {
      tgtNode.position = {
        x: tgtNode.position.x,
        y: snapToGrid(srcCenterY - tgtH / 2),
      };
    } else if (isMovable(srcNode) && srcH <= tgtH) {
      srcNode.position = {
        x: srcNode.position.x,
        y: snapToGrid(tgtCenterY - srcH / 2),
      };
    }
  }

  // Add note nodes back unchanged - they keep their original positions
  return { nodes: [...resultNodes, ...noteNodes], edges };
}

// we have to make sure that parent nodes are rendered before their children
export const sortNodes = (a: Node, b: Node): number => {
  if (a.type === b.type) {
    return 0;
  }

  return a.type === NODE_TYPES.ContainerNode &&
    b.type !== NODE_TYPES.ContainerNode
    ? -1
    : 1;
};

export const getNodePositionInsideParent = (
  node: Partial<Node>,
  groupNode: Node
) => {
  const position = node.position ?? { x: 0, y: 0 };
  const nodeWidth = node.measured?.width ?? 0;
  const nodeHeight = node.measured?.height ?? 0;
  const groupWidth = groupNode.measured?.width ?? 0;
  const groupHeight = groupNode.measured?.height ?? 0;

  if (position.x < groupNode.position.x) {
    position.x = 0;
  } else if (position.x + nodeWidth > groupNode.position.x + groupWidth) {
    position.x = snapToGrid(groupWidth - nodeWidth);
  } else {
    position.x = snapToGrid(position.x - groupNode.position.x);
  }

  if (position.y < groupNode.position.y) {
    position.y = 0;
  } else if (position.y + nodeHeight > groupNode.position.y + groupHeight) {
    position.y = snapToGrid(groupHeight - nodeHeight);
  } else {
    position.y = snapToGrid(position.y - groupNode.position.y);
  }

  return position;
};

/*
  Payload schema

  |executionPlan                            |executionPlan
  |steps   -->   steps[id].subgraph   -->   |steps -->   ...
  |entryPoint                               |entryPoint
*/
export function composeExecutionGraph(
  nodes: Node[],
  edges: Edge[],
  options?: {
    name?: string;
    description?: string;
    variables?: Record<string, { type: string; value: string }>;
    inputSchema?: Record<string, unknown>;
    outputSchema?: Record<string, unknown>;
    executionTimeoutSeconds?: number;
    rateLimitBudgetMs?: number;
  }
): ExecutionGraph | null {
  const nodesMap: any = new Map();
  const executionGraph: any = {};

  // Include name and description in the execution graph
  // Name is required for updates, so always include it if provided
  if (options?.name !== undefined) {
    executionGraph.name = options.name;
  }
  // Only include description if it has a non-empty value
  if (options?.description) {
    executionGraph.description = options.description;
  }

  // Include variables and schemas in the execution graph if provided
  if (options?.variables) {
    executionGraph.variables = options.variables;
  }
  if (options?.inputSchema) {
    executionGraph.inputSchema = options.inputSchema;
  }
  if (options?.outputSchema) {
    executionGraph.outputSchema = options.outputSchema;
  }
  if (options?.executionTimeoutSeconds !== undefined) {
    executionGraph.executionTimeoutSeconds = options.executionTimeoutSeconds;
  }
  if (options?.rateLimitBudgetMs !== undefined) {
    executionGraph.rateLimitBudgetMs = options.rateLimitBudgetMs;
  }

  // Separate notes from regular nodes
  const noteNodes = nodes.filter((node) => node.type === NODE_TYPES.NoteNode);
  const nds = nodes.filter(
    (node) =>
      node.type !== NODE_TYPES.CreateNode && node.type !== NODE_TYPES.NoteNode
  );
  const stepNodeIds = new Set(nds.map((node) => node.id));

  if (nds.length > 0) {
    executionGraph.nodes = nds.map((node) => {
      const width =
        typeof node.style?.width === 'number'
          ? node.style.width
          : typeof node.width === 'number'
            ? node.width
            : undefined;
      const height =
        typeof node.style?.height === 'number'
          ? node.style.height
          : typeof node.height === 'number'
            ? node.height
            : undefined;

      return {
        id: node.id,
        type: node.type,
        position: node.position,
        ...(width !== undefined ? { width } : {}),
        ...(height !== undefined ? { height } : {}),
        ...(node.parentId ? { parentId: node.parentId } : {}),
      };
    });
  }

  const visualEdges = edges
    .filter(
      (edge) => stepNodeIds.has(edge.source) && stepNodeIds.has(edge.target)
    )
    .map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      ...(edge.sourceHandle ? { sourceHandle: edge.sourceHandle } : {}),
      ...(edge.targetHandle ? { targetHandle: edge.targetHandle } : {}),
    }));

  if (visualEdges.length > 0) {
    executionGraph.edges = visualEdges;
  }

  if (!nds.length && !noteNodes.length) {
    return null;
  }

  // Process notes into the execution graph
  // New format uses: { text, position: {x, y} }
  if (noteNodes.length > 0) {
    executionGraph.notes = noteNodes.map((node) => ({
      id: node.data.id || node.id,
      text: node.data.content || '',
      position: {
        x: node.position.x,
        y: node.position.y,
      },
      metadata: {
        width: node.width || NODE_TYPE_SIZES[NODE_TYPES.NoteNode]?.width || 240,
        height:
          node.height || NODE_TYPE_SIZES[NODE_TYPES.NoteNode]?.height || 120,
      },
    }));
  }

  if (!nds.length) {
    return executionGraph;
  }

  // Create deep copies of nodes to avoid mutating the originals
  nds.forEach((node) => {
    nodesMap.set(node.id, JSON.parse(JSON.stringify(node)));
  });

  nds.forEach((node) => {
    const stepType = node.data?.stepType;
    if (stepType === 'Split' || stepType === 'While') {
      const containerStep = nodesMap.get(node.id);
      if (containerStep && !containerStep.subgraph) {
        containerStep.subgraph = { steps: {} };
      }
    }
  });

  nds.forEach((node) => {
    if (node.parentId) {
      const parent = nodesMap.get(node.parentId);
      if (parent) {
        if (!parent.subgraph) {
          parent.subgraph = {};
          parent.subgraph.steps = {};
        }
        parent.subgraph.steps[node.id] = nodesMap.get(node.id);
      }
    } else {
      if (!executionGraph.steps) {
        executionGraph.steps = {};
      }
      executionGraph.steps[node.id] = nodesMap.get(node.id);
    }
  });

  edges.forEach((edge) => {
    const sourceNode = nodesMap.get(edge.source);
    const targetNode = nodesMap.get(edge.target);

    const groupId = sourceNode?.parentId || targetNode?.parentId;

    // Map edge sourceHandle to spec label
    // Per DSL v2.0.0: use "next" for sequential, "true"/"false" for Conditional
    const rawLabel = edge.sourceHandle || '';
    let specLabel =
      rawLabel === '' || rawLabel === 'source' ? 'next' : rawLabel;

    // For Switch routing mode, convert case-N handles to route labels
    if (rawLabel.startsWith('case-') && sourceNode) {
      const caseIndex = parseInt(rawLabel.split('-')[1], 10);
      const casesField = (sourceNode.data?.inputMapping || []).find(
        (item: any) => item.type === 'cases'
      );
      const cases = Array.isArray(casesField?.value) ? casesField.value : [];
      if (cases[caseIndex]?.route) {
        specLabel = cases[caseIndex].route;
      }
    }

    if (groupId && sourceNode?.parentId === targetNode?.parentId) {
      const parent = nodesMap.get(groupId);
      if (parent.subgraph) {
        if (!parent.subgraph.executionPlan) {
          parent.subgraph.executionPlan = [];
        }
        parent.subgraph.executionPlan.push({
          fromStep: edge.source,
          toStep: edge.target,
          label: specLabel,
        });
      }
    } else {
      if (!executionGraph.executionPlan) {
        executionGraph.executionPlan = [];
      }
      executionGraph.executionPlan.push({
        fromStep: edge.source,
        toStep: edge.target,
        label: specLabel,
      });
    }
  });

  executionGraph.steps = cleanNodeData(executionGraph.steps);

  addStarts(executionGraph);
  stripEditorOnlyStepFields(executionGraph.steps);

  return executionGraph;
}

function stripEditorOnlyStepFields(steps?: Record<string, any>) {
  if (!steps) {
    return;
  }

  for (const step of Object.values(steps)) {
    delete step.renderingParameters;
    if (step.subgraph?.steps) {
      stripEditorOnlyStepFields(step.subgraph.steps);
    }
  }
}

function applyStoredNodeVisualState(nodes: Node[], storedNodes: unknown) {
  if (!Array.isArray(storedNodes)) {
    return nodes;
  }

  const visualStateById = new Map<string, any>();
  for (const storedNode of storedNodes) {
    if (
      storedNode &&
      typeof storedNode === 'object' &&
      typeof storedNode.id === 'string'
    ) {
      visualStateById.set(storedNode.id, storedNode);
    }
  }

  if (visualStateById.size === 0) {
    return nodes;
  }

  return nodes.map((node) => {
    const visualState = visualStateById.get(node.id);
    if (!visualState) {
      return node;
    }

    const position =
      visualState.position &&
      typeof visualState.position.x === 'number' &&
      typeof visualState.position.y === 'number'
        ? snapPositionToGrid({
            x: visualState.position.x,
            y: visualState.position.y,
          })
        : node.position;
    const width =
      typeof visualState.width === 'number'
        ? snapToGrid(visualState.width)
        : node.width;
    const height =
      typeof visualState.height === 'number'
        ? snapToGrid(visualState.height)
        : node.height;

    return {
      ...node,
      position,
      width,
      height,
      style: {
        ...node.style,
        ...(width !== undefined ? { width } : {}),
        ...(height !== undefined ? { height } : {}),
      },
    };
  });
}

function addStarts(executionGraph: ExecutionGraphDto) {
  function findStart(
    steps: Record<string, ExecutionGraphStepDto>,
    executionPlan: ExecutionGraphTransitionDto[]
  ) {
    const allIds = new Set(Object.keys(steps));
    const targets = new Set<string>();

    for (const { toStep = '' } of executionPlan) {
      if (allIds.has(toStep)) {
        targets.add(toStep);
      }
    }

    // Find all candidate entry points (nodes without incoming edges)
    const candidates: string[] = [];
    for (const id of allIds) {
      if (!targets.has(id)) {
        candidates.push(id);
      }
    }

    if (candidates.length === 0) {
      return '';
    }

    // If multiple candidates, pick the leftmost one (smallest x position)
    // This ensures the entry point stays stable when edges are deleted
    if (candidates.length === 1) {
      return candidates[0];
    }

    return candidates.reduce((leftmostId, id) => {
      const leftmostX = steps[leftmostId]?.renderingParameters?.x ?? Infinity;
      const currentX = steps[id]?.renderingParameters?.x ?? Infinity;
      return currentX < leftmostX ? id : leftmostId;
    });
  }

  function findAllStarts(executionGraph: ExecutionGraphDto) {
    const { steps = {}, executionPlan = [] } = executionGraph;
    const entry = findStart(steps, executionPlan);
    executionGraph.entryPoint = entry;

    for (const step of Object.values(steps)) {
      if (step.subgraph) {
        findAllStarts(step.subgraph);
      }
    }
  }

  findAllStarts(executionGraph);
}

// Valid ValueType values from the spec
const VALID_VALUE_TYPES = new Set(Object.values(ValueType));

/**
 * Coerces a value to match the given type hint (using API ValueType convention).
 * e.g., "150" with type "integer" becomes 150
 */
function coerceValueToType(value: any, typeHint?: string): any {
  if (typeHint === ValueType.Integer || typeHint === ValueType.Number) {
    const numValue = Number(value);
    if (!isNaN(numValue)) {
      return typeHint === ValueType.Integer ? Math.trunc(numValue) : numValue;
    }
  }
  if (typeHint === ValueType.Boolean && typeof value === 'string') {
    const lower = value.toLowerCase();
    if (lower === 'true' || lower === '1') return true;
    if (lower === 'false' || lower === '0') return false;
  }
  return value;
}

// Check if a typeHint is a valid ValueType
function isValidValueType(typeHint?: string): typeHint is ValueType {
  return typeHint !== undefined && VALID_VALUE_TYPES.has(typeHint as ValueType);
}

function normalizeConditionExpression(condition: any): any {
  if (!condition || typeof condition !== 'object') return condition;
  if (!('op' in condition) || !Array.isArray(condition.arguments)) {
    return condition;
  }

  return {
    ...condition,
    type: condition.type || 'operation',
    arguments: convertConditionArguments(condition.op, condition.arguments),
  };
}

function cleanNodeData(steps: Record<string, any>) {
  const cleaned: Record<string, any> = {};

  if (!steps) {
    return {};
  }

  for (const [id, step] of Object.entries(steps)) {
    const { measured, position, subgraph, data } = step;
    // Destructure to exclude UI-only fields from the cleaned data
    const {
      inputMapping = [],
      inputSchema,
      outputSchema,
      childWorkflowId,
      childVersion,
      inputSchemaFields: _1,
      variablesFields: _2,
      splitInputSchemaFields: _3,
      splitOutputSchemaFields: _4,
      embedWorkflowConfig: _5,
      splitVariablesFields: _6,
      splitParallelism: _7,
      splitSequential: _8,
      splitDontStopOnFailed: _9,
      formTabs: _10,
      startMode: _11,
      selectedTriggerId: _12,
      executionTimeout: _13,
      retryStrategy: _14,
      groupByKey,
      groupByExpectedKeys,
      filterCondition,
      whileCondition,
      whileMaxIterations,
      whileTimeout,
      ...restData
    } = data;
    // Suppress unused variable warnings for destructured exclusions
    void _1;
    void _2;
    void _3;
    void _4;
    void _5;
    void _6;
    void _7;
    void _8;
    void _9;
    void _10;
    void _11;
    void _12;
    void _13;
    void _14;

    if (subgraph) {
      data.subgraph = {
        ...(subgraph || {}),
        steps: cleanNodeData(subgraph.steps),
      };
    }

    // Handle inputMapping - convert array format to object format
    let cleanedInputMapping = inputMapping;
    // console.log("[DEBUG] cleanNodeData - inputMapping for node', id, ':', inputMapping);

    // Helper function to recursively process composite values.
    // Mirror of convertCompositeToUIFormat below — preserves typeHint/defaultValue for
    // every non-composite valueType (not only `immediate`) so the UI→backend round-trip is lossless.
    const processCompositeValue = (
      compositeVal: any
    ): {
      valueType: 'reference' | 'immediate' | 'composite';
      value: any;
      type?: string;
    } => {
      const processEntry = (val: any) => {
        if (
          typeof val !== 'object' ||
          val === null ||
          !('valueType' in (val as Record<string, unknown>))
        ) {
          return {
            valueType: 'immediate',
            value: val,
          };
        }

        const typedVal = val as {
          valueType: 'reference' | 'immediate' | 'composite' | 'template';
          value: any;
          typeHint?: string;
          defaultValue?: any;
        };

        if (typedVal.valueType === 'composite') {
          const nestedValue =
            typedVal.value && typeof typedVal.value === 'object'
              ? typedVal.value
              : {};
          return {
            valueType: 'composite',
            value: processCompositeValue(nestedValue).value,
          };
        }

        const coercedValue =
          typedVal.valueType === 'immediate' && typedVal.typeHint
            ? coerceValueToType(typedVal.value, typedVal.typeHint)
            : typedVal.value === undefined || typedVal.value === null
              ? ''
              : typedVal.value;

        const out: {
          valueType: string;
          value: any;
          type?: string;
          default?: any;
        } = {
          valueType: typedVal.valueType || 'immediate',
          value: coercedValue,
        };
        if (isValidValueType(typedVal.typeHint)) {
          out.type = typedVal.typeHint;
        }
        if (
          typedVal.valueType === 'reference' &&
          typedVal.defaultValue !== undefined
        ) {
          out.default = typedVal.defaultValue;
        }
        return out;
      };

      // Handle composite object
      if (
        compositeVal &&
        typeof compositeVal === 'object' &&
        !Array.isArray(compositeVal)
      ) {
        const processedObject: Record<string, any> = {};
        for (const [key, val] of Object.entries(compositeVal)) {
          processedObject[key] = processEntry(val);
        }
        return { valueType: 'composite', value: processedObject };
      }

      // Handle composite array
      if (Array.isArray(compositeVal)) {
        return {
          valueType: 'composite',
          value: compositeVal.map(processEntry),
        };
      }

      // Fallback - shouldn't happen for properly structured data
      return { valueType: 'immediate', value: compositeVal };
    };

    // Helper function to process a single mapping entry
    const processMappingEntry = ({
      type,
      value,
      typeHint,
      valueType,
      defaultValue,
    }: {
      type: string;
      value: any;
      typeHint?: string;
      valueType?: 'reference' | 'immediate' | 'composite' | 'template';
      defaultValue?: any;
    }) => {
      // Handle template values - always a string, no type coercion
      if (valueType === 'template') {
        return [type, { valueType: 'template', value: String(value) }];
      }

      // Handle composite values - process recursively
      if (valueType === 'composite') {
        const processed = processCompositeValue(value);
        const mappingValue: {
          valueType: 'composite';
          value: any;
          type?: string;
        } = {
          valueType: 'composite',
          value: processed.value,
        };
        // Add type if it's a valid ValueType from the spec
        if (isValidValueType(typeHint)) {
          mappingValue.type = typeHint;
        }
        return [type, mappingValue];
      }

      // Parse JSON strings into actual arrays/objects before sending to backend
      let finalValue = value;

      if (typeof value === 'string' && value) {
        // Skip parsing for template variables (they're resolved at runtime)
        const isTemplate = value.includes('{{');

        if (!isTemplate) {
          // For non-template strings, only parse as JSON if typeHint is explicitly 'json'
          // No auto-detection - explicit typeHint required
          if (typeHint === ValueType.Json) {
            try {
              finalValue = JSON.parse(value);
            } catch {
              // If parsing fails, keep as string
              finalValue = value;
            }
          }

          // Convert numeric strings to actual numbers for integer/number type hints
          if (typeHint === ValueType.Integer || typeHint === ValueType.Number) {
            const numValue = Number(value);
            if (!isNaN(numValue)) {
              // For integers, ensure we get a whole number
              finalValue =
                typeHint === ValueType.Integer
                  ? Math.trunc(numValue)
                  : numValue;
            }
          }

          // Convert boolean strings to actual booleans for boolean type hint
          if (typeHint === ValueType.Boolean) {
            const lowerValue = value.toLowerCase();
            if (lowerValue === 'true' || lowerValue === '1') {
              finalValue = true;
            } else if (lowerValue === 'false' || lowerValue === '0') {
              finalValue = false;
            }
          }
        }
      }

      // Use explicit valueType from UI, fallback to auto-detection for backward compatibility
      const resolvedValueType: 'reference' | 'immediate' | 'template' =
        valueType ||
        (typeof finalValue === 'string' && finalValue.includes('{{')
          ? 'reference'
          : 'immediate');

      // Create the new format per DSL v2.0.0 spec: { valueType, value, type?, default? }
      const mappingValue: {
        valueType: 'reference' | 'immediate' | 'template';
        value: any;
        type?: string;
        default?: any;
      } = {
        valueType: resolvedValueType,
        value: finalValue,
      };

      // Add type if it's a valid ValueType from the spec
      if (isValidValueType(typeHint)) {
        mappingValue.type = typeHint;
      }

      // Preserve ReferenceValue.default — only references carry this field on the backend.
      if (resolvedValueType === 'reference' && defaultValue !== undefined) {
        mappingValue.default = defaultValue;
      }

      return [type, mappingValue];
    };

    // Filter out empty optional fields that shouldn't be sent to the API
    const optionalFieldsToFilterIfEmpty = [
      'agentId',
      'capabilityId',
      'connectionId',
    ];
    const filteredRestData = Object.fromEntries(
      Object.entries(restData).filter(
        ([key, value]) =>
          !(optionalFieldsToFilterIfEmpty.includes(key) && value === '')
      )
    );

    if (Array.isArray(inputMapping)) {
      const filteredMapping = inputMapping.filter(
        ({
          type,
          value,
          valueType,
        }: {
          type: string;
          value: any;
          valueType?: string;
        }) => {
          // Filter out entries with empty keys (field names)
          if (!type || type.trim() === '') {
            return false;
          }
          // Keep reference/template entries even with empty values — they're resolved at runtime
          if (valueType === 'reference' || valueType === 'template') {
            return true;
          }
          // Filter out empty optional fields
          if (value === undefined || value === null || value === '') {
            return false;
          }
          return true;
        }
      );

      // Error steps need direct field values, not InputMapping wrapping
      if (data.stepType === 'Error') {
        // Extract error fields as direct values to match backend DSL
        const errorFields = ['code', 'message', 'category', 'severity'];
        filteredMapping.forEach(
          ({ type, value }: { type: string; value: any }) => {
            if (errorFields.includes(type)) {
              filteredRestData[type] = value; // Direct string value
            }
          }
        );
        // Don't include inputMapping for Error steps
        cleanedInputMapping = undefined;
      } else if (data.stepType === 'Log') {
        // Log steps need direct field values (message, level) to match backend DSL
        const logFields = ['message', 'level'];
        filteredMapping.forEach(
          ({ type, value }: { type: string; value: any }) => {
            if (logFields.includes(type)) {
              filteredRestData[type] = value;
            }
          }
        );
        // Don't include inputMapping for Log steps
        cleanedInputMapping = undefined;
      } else {
        // Regular steps - flat object format with InputMapping wrapping
        const mappingObject = Object.fromEntries(
          filteredMapping.map(processMappingEntry)
        );
        // Only include inputMapping if it has entries
        cleanedInputMapping =
          Object.keys(mappingObject).length > 0 ? mappingObject : undefined;
      }
    }

    const normalizedInputSchema =
      inputSchema &&
      typeof inputSchema === 'object' &&
      Object.keys(inputSchema).length > 0
        ? inputSchema
        : undefined;

    cleaned[id] = {
      ...filteredRestData,
      ...(normalizedInputSchema ? { inputSchema: normalizedInputSchema } : {}),
      ...(cleanedInputMapping !== undefined
        ? { inputMapping: cleanedInputMapping }
        : {}),
      renderingParameters: {
        ...measured,
        ...position,
      },
    };

    if (restData.stepType === 'Agent' && data.capabilityId) {
      cleaned[id].capabilityId = data.capabilityId;
    }

    if (restData.stepType !== 'Agent') {
      delete cleaned[id].agentId;
      delete cleaned[id].capabilityId;
      delete cleaned[id].compensation;
    }
    if (restData.stepType !== 'Agent' && restData.stepType !== 'AiAgent') {
      delete cleaned[id].connectionId;
    }
    if (
      restData.stepType !== 'Agent' &&
      restData.stepType !== 'EmbedWorkflow'
    ) {
      delete cleaned[id].maxRetries;
      delete cleaned[id].retryDelay;
      delete cleaned[id].timeout;
    }
    if (restData.stepType !== 'Split') {
      delete cleaned[id].inputSchema;
    }

    // Include processed subgraph for container steps (Split)
    // The subgraph is reconstructed by composeExecutionGraph from child nodes with parentId,
    // and processed via the recursive cleanNodeData call at lines 504-510.
    if (subgraph) {
      cleaned[id].subgraph = data.subgraph;
    }

    if (restData.stepType === 'Conditional' && (restData as any).condition) {
      delete cleaned[id].inputMapping;
      cleaned[id].condition = normalizeConditionExpression(
        (restData as any).condition
      );
    }

    // Ensure EmbedWorkflow has childWorkflowId and childVersion at root level (DSL v2.0.0 requirement)
    if (restData.stepType === 'EmbedWorkflow') {
      if (childWorkflowId) {
        cleaned[id].childWorkflowId = childWorkflowId;
      }
      if (childVersion !== undefined) {
        // Backend ChildVersion is an untagged enum: string ("latest"/"current") or integer.
        // Convert numeric strings to integers so serde deserializes to Specific(i32).
        const v = childVersion;
        const num = Number(v);
        cleaned[id].childVersion =
          typeof v === 'string' &&
          v !== '' &&
          !isNaN(num) &&
          v !== 'latest' &&
          v !== 'current'
            ? num
            : v;
      }
    }

    // Split step: use config instead of inputMapping, include schemas
    if (restData.stepType === 'Split') {
      // Remove inputMapping for Split steps - we use config instead
      delete cleaned[id].inputMapping;
      const existingSplitConfig = (restData as any).config;

      // Build the config object for Split step
      const splitConfig: {
        value?: {
          valueType: 'reference' | 'immediate';
          value: unknown;
          type?: string;
          default?: unknown;
        };
        parallelism?: number;
        sequential?: boolean;
        dontStopOnFailed?: boolean;
        variables?: Record<
          string,
          {
            valueType: 'reference' | 'immediate' | 'composite';
            value: unknown;
            type?: string;
          }
        >;
      } = {};

      // Get the array source value from inputMapping (which was the array format)
      if (Array.isArray(inputMapping) && inputMapping.length > 0) {
        const firstMapping = inputMapping[0];
        const hasSplitSourceValue =
          firstMapping &&
          firstMapping.value !== undefined &&
          firstMapping.value !== null &&
          !(
            typeof firstMapping.value === 'string' &&
            firstMapping.value.trim() === ''
          );
        if (hasSplitSourceValue) {
          splitConfig.value = {
            valueType: firstMapping.valueType || 'reference',
            value: firstMapping.value,
            ...(isValidValueType(firstMapping.typeHint)
              ? { type: firstMapping.typeHint }
              : {}),
            ...(firstMapping.valueType === 'reference' &&
            firstMapping.defaultValue !== undefined
              ? { default: firstMapping.defaultValue }
              : {}),
          };
        }
      }

      // Keep existing value if form inputMapping is temporarily empty.
      // This prevents emitting invalid Split config without the required source value.
      if (
        !splitConfig.value &&
        existingSplitConfig?.value &&
        existingSplitConfig.value.value !== undefined &&
        existingSplitConfig.value.value !== null &&
        !(
          typeof existingSplitConfig.value.value === 'string' &&
          existingSplitConfig.value.value.trim() === ''
        )
      ) {
        splitConfig.value = {
          valueType:
            existingSplitConfig.value.valueType === 'immediate'
              ? 'immediate'
              : 'reference',
          value: existingSplitConfig.value.value,
          ...(existingSplitConfig.value.type
            ? { type: existingSplitConfig.value.type }
            : {}),
          ...(existingSplitConfig.value.default !== undefined
            ? { default: existingSplitConfig.value.default }
            : {}),
        };
      }

      // Add execution options from the form data
      if (data.splitParallelism !== undefined && data.splitParallelism !== 0) {
        splitConfig.parallelism = data.splitParallelism;
      }
      if (data.splitSequential === true) {
        splitConfig.sequential = true;
      }
      if (data.splitDontStopOnFailed === true) {
        splitConfig.dontStopOnFailed = true;
      }

      // Add variables from splitVariablesFields
      if (
        Array.isArray(data.splitVariablesFields) &&
        data.splitVariablesFields.length > 0
      ) {
        const variables: Record<
          string,
          {
            valueType: 'reference' | 'immediate' | 'composite';
            value: unknown;
            type?: string;
          }
        > = {};
        for (const varField of data.splitVariablesFields) {
          const variableName =
            typeof varField.name === 'string' ? varField.name.trim() : '';
          if (variableName && varField.value !== undefined) {
            const resolvedValueType: 'reference' | 'immediate' | 'composite' =
              varField.valueType ||
              (typeof varField.value === 'object' && varField.value !== null
                ? 'composite'
                : 'immediate');
            variables[variableName] = {
              valueType: resolvedValueType,
              value: varField.value,
              ...(varField.type ? { type: varField.type } : {}),
            };
          }
        }
        if (Object.keys(variables).length > 0) {
          splitConfig.variables = variables;
        }
      }

      // Backend requires config.value to exist for Split config.
      // Preserve variables/options even when source is not chosen yet by sending an empty value placeholder.
      if (
        !splitConfig.value &&
        (splitConfig.variables ||
          splitConfig.parallelism !== undefined ||
          splitConfig.sequential !== undefined ||
          splitConfig.dontStopOnFailed !== undefined)
      ) {
        splitConfig.value = {
          valueType: 'reference',
          value: '',
        };
      }

      // Preserve split config when any split settings were provided.
      if (
        splitConfig.value ||
        splitConfig.variables ||
        splitConfig.parallelism !== undefined ||
        splitConfig.sequential !== undefined ||
        splitConfig.dontStopOnFailed !== undefined
      ) {
        cleaned[id].config = splitConfig;
      }

      // Add outputSchema if defined
      if (
        outputSchema &&
        typeof outputSchema === 'object' &&
        Object.keys(outputSchema).length > 0
      ) {
        cleaned[id].outputSchema = outputSchema;
      }
    }

    // Switch step: use config instead of inputMapping
    if (restData.stepType === 'Switch') {
      // Remove inputMapping for Switch steps - we use config instead
      delete cleaned[id].inputMapping;

      const switchConfig: {
        value?: { valueType: string; value: unknown };
        cases?: Array<{
          match: any;
          matchType: string;
          output: any;
          route?: string;
        }>;
        default?: any;
      } = {};

      if (Array.isArray(inputMapping)) {
        // Extract value field
        const valueItem = inputMapping.find(
          (item: any) => item.type === 'value'
        );
        if (valueItem?.value !== undefined && valueItem.value !== '') {
          const isRef =
            typeof valueItem.value === 'string' &&
            valueItem.value.includes('{{');
          switchConfig.value = {
            valueType:
              valueItem.valueType || (isRef ? 'reference' : 'immediate'),
            value: valueItem.value,
          };
        }

        // Extract cases
        const casesItem = inputMapping.find(
          (item: any) => item.type === 'cases'
        );
        if (
          casesItem?.value &&
          Array.isArray(casesItem.value) &&
          casesItem.value.length > 0
        ) {
          switchConfig.cases = casesItem.value.map((c: any) => ({
            match: c.match,
            matchType: mapMatchTypeToAPI(c.matchType),
            output: c.output,
            ...(c.route ? { route: c.route } : {}),
          }));
        }

        // Extract default
        const defaultItem = inputMapping.find(
          (item: any) => item.type === 'default'
        );
        if (defaultItem?.value !== undefined) {
          switchConfig.default = defaultItem.value;
        }
      }

      // Add config if it has any meaningful fields (value, cases, or default)
      if (
        switchConfig.value ||
        switchConfig.cases ||
        switchConfig.default !== undefined
      ) {
        cleaned[id].config = switchConfig;
      }
    }

    // Filter step: use config instead of inputMapping
    if (restData.stepType === 'Filter') {
      delete cleaned[id].inputMapping;
      delete cleaned[id].filterCondition;

      const filterConfig: {
        value?: { valueType: 'reference' | 'immediate'; value: unknown };
        condition?: any;
      } = {};

      // Get the array source value from inputMapping
      if (Array.isArray(inputMapping) && inputMapping.length > 0) {
        const firstMapping = inputMapping[0];
        if (firstMapping?.value) {
          filterConfig.value = {
            valueType: firstMapping.valueType || 'reference',
            value: firstMapping.value,
          };
        }
      }

      // Add condition from form data
      if (filterCondition) {
        filterConfig.condition = normalizeConditionExpression(filterCondition);
      }

      // Only add config if it has the required fields
      if (filterConfig.value && filterConfig.condition) {
        cleaned[id].config = filterConfig;
      }
    }

    // While step: serialize condition and config
    if (restData.stepType === 'While') {
      delete cleaned[id].inputMapping;
      delete cleaned[id].whileCondition;
      delete cleaned[id].whileMaxIterations;
      delete cleaned[id].whileTimeout;

      // Set condition at root level (API expects WhileStep.condition)
      if (whileCondition) {
        cleaned[id].condition = normalizeConditionExpression(whileCondition);
      }

      // Build config object
      const whileConfig: { maxIterations?: number; timeout?: number | null } =
        {};
      if (whileMaxIterations !== undefined && whileMaxIterations !== null) {
        whileConfig.maxIterations = whileMaxIterations;
      }
      if (whileTimeout !== undefined && whileTimeout !== null) {
        whileConfig.timeout = whileTimeout;
      }

      if (Object.keys(whileConfig).length > 0) {
        cleaned[id].config = whileConfig;
      }
    }

    // GroupBy step: use config instead of inputMapping
    if (restData.stepType === 'GroupBy') {
      delete cleaned[id].inputMapping;
      delete cleaned[id].groupByKey;
      delete cleaned[id].groupByExpectedKeys;

      const groupByConfig: {
        value?: { valueType: 'reference' | 'immediate'; value: unknown };
        key?: string;
        expectedKeys?: unknown[];
      } = {};

      // Get the array source value from inputMapping
      if (Array.isArray(inputMapping) && inputMapping.length > 0) {
        const firstMapping = inputMapping[0];
        if (firstMapping?.value) {
          groupByConfig.value = {
            valueType: firstMapping.valueType || 'reference',
            value: firstMapping.value,
          };
        }
      }

      // Add group key from form data
      if (groupByKey) {
        groupByConfig.key = groupByKey;
      }

      // Add expected keys from form data (already an array)
      if (
        Array.isArray(groupByExpectedKeys) &&
        groupByExpectedKeys.length > 0
      ) {
        groupByConfig.expectedKeys = groupByExpectedKeys;
      }

      // Only add config if it has the required fields
      if (groupByConfig.value && groupByConfig.key) {
        cleaned[id].config = groupByConfig;
      }
    }

    // AiAgent step: use config instead of inputMapping
    if (restData.stepType === 'AiAgent') {
      delete cleaned[id].inputMapping;

      const aiAgentConfig: {
        systemPrompt?: { valueType: string; value: unknown };
        userPrompt?: { valueType: string; value: unknown };
        provider?: string;
        model?: string | null;
        maxIterations?: number | null;
        temperature?: number | null;
        maxTokens?: number | null;
      } = {};

      if (Array.isArray(inputMapping)) {
        const systemPromptItem = inputMapping.find(
          (item: any) => item.type === 'systemPrompt'
        );
        if (
          systemPromptItem?.value !== undefined &&
          systemPromptItem.value !== ''
        ) {
          aiAgentConfig.systemPrompt = {
            valueType: systemPromptItem.valueType || 'immediate',
            value: systemPromptItem.value,
          };
        }

        const userPromptItem = inputMapping.find(
          (item: any) => item.type === 'userPrompt'
        );
        if (
          userPromptItem?.value !== undefined &&
          userPromptItem.value !== ''
        ) {
          aiAgentConfig.userPrompt = {
            valueType: userPromptItem.valueType || 'immediate',
            value: userPromptItem.value,
          };
        }

        const providerItem = inputMapping.find(
          (item: any) => item.type === 'provider'
        );
        if (providerItem?.value) {
          aiAgentConfig.provider = String(providerItem.value);
        }

        const modelItem = inputMapping.find(
          (item: any) => item.type === 'model'
        );
        if (modelItem?.value) {
          aiAgentConfig.model = String(modelItem.value);
        }

        const maxIterationsItem = inputMapping.find(
          (item: any) => item.type === 'maxIterations'
        );
        if (
          maxIterationsItem?.value !== undefined &&
          maxIterationsItem.value !== ''
        ) {
          aiAgentConfig.maxIterations = Number(maxIterationsItem.value);
        }

        const temperatureItem = inputMapping.find(
          (item: any) => item.type === 'temperature'
        );
        if (
          temperatureItem?.value !== undefined &&
          temperatureItem.value !== ''
        ) {
          aiAgentConfig.temperature = Number(temperatureItem.value);
        }

        const maxTokensItem = inputMapping.find(
          (item: any) => item.type === 'maxTokens'
        );
        if (maxTokensItem?.value !== undefined && maxTokensItem.value !== '') {
          aiAgentConfig.maxTokens = Number(maxTokensItem.value);
        }
      }

      // Memory config: serialize from inputMapping entries into config.memory
      const memoryEnabledItem = inputMapping.find(
        (item: any) => item.type === 'memoryEnabled'
      );
      if (memoryEnabledItem?.value === true) {
        const memoryConfig: {
          conversationId?: { valueType: string; value: unknown };
          compaction?: { maxMessages?: number; strategy?: string };
        } = {};

        const conversationIdItem = inputMapping.find(
          (item: any) => item.type === 'memoryConversationId'
        );
        // Always include conversationId when memory is enabled — backend requires it
        memoryConfig.conversationId = {
          valueType: conversationIdItem?.valueType || 'reference',
          value: conversationIdItem?.value ?? '',
        };

        const maxMessagesItem = inputMapping.find(
          (item: any) => item.type === 'memoryMaxMessages'
        );
        const strategyItem = inputMapping.find(
          (item: any) => item.type === 'memoryStrategy'
        );
        if (
          (maxMessagesItem?.value !== undefined &&
            maxMessagesItem.value !== '') ||
          (strategyItem?.value !== undefined && strategyItem.value !== '')
        ) {
          memoryConfig.compaction = {};
          if (
            maxMessagesItem?.value !== undefined &&
            maxMessagesItem.value !== ''
          ) {
            memoryConfig.compaction.maxMessages = Number(maxMessagesItem.value);
          }
          if (strategyItem?.value) {
            memoryConfig.compaction.strategy = String(strategyItem.value);
          }
        }

        (aiAgentConfig as any).memory = memoryConfig;
      }

      // Output schema: convert SchemaField[] → Record<string, SchemaField>
      const outputSchemaItem = inputMapping.find(
        (item: any) => item.type === 'outputSchema'
      );
      if (
        outputSchemaItem?.value &&
        Array.isArray(outputSchemaItem.value) &&
        outputSchemaItem.value.length > 0
      ) {
        (aiAgentConfig as any).outputSchema = buildSchemaFromFields(
          outputSchemaItem.value
        );
      }

      cleaned[id].config = aiAgentConfig;
    }

    // WaitForSignal step: fields are top-level (not nested under config)
    if (restData.stepType === 'WaitForSignal') {
      delete cleaned[id].inputMapping;

      if (Array.isArray(inputMapping)) {
        // responseSchema: convert SchemaField[] → Record<string, SchemaField>
        const responseSchemaItem = inputMapping.find(
          (item: any) => item.type === 'responseSchema'
        );
        if (
          responseSchemaItem?.value &&
          Array.isArray(responseSchemaItem.value) &&
          responseSchemaItem.value.length > 0
        ) {
          cleaned[id].responseSchema = buildSchemaFromFields(
            responseSchemaItem.value
          );
        }

        // timeoutMs: serialize as MappingValue if present
        const timeoutItem = inputMapping.find(
          (item: any) => item.type === 'timeoutMs'
        );
        if (timeoutItem?.value !== undefined && timeoutItem.value !== '') {
          cleaned[id].timeoutMs = {
            valueType: timeoutItem.valueType || 'immediate',
            value:
              timeoutItem.valueType === 'reference'
                ? timeoutItem.value
                : Number(timeoutItem.value),
          };
        }

        // pollIntervalMs: serialize as plain number
        const pollItem = inputMapping.find(
          (item: any) => item.type === 'pollIntervalMs'
        );
        if (pollItem?.value !== undefined && pollItem.value !== '') {
          cleaned[id].pollIntervalMs = Number(pollItem.value);
        }
      }
    }
  }

  return cleaned;
}

export function executionGraphToReactFlow(
  executionGraph: ExecutionGraphDto & { notes?: Note[]; nodes?: unknown }
) {
  // Migrate legacy Start steps if present
  let graphToProcess = executionGraph;
  if (needsStartStepMigration(executionGraph)) {
    const migrationResult = migrateStartStep(executionGraph);
    graphToProcess = migrationResult.executionGraph as ExecutionGraphDto & {
      notes?: Note[];
    };

    // Log migration for debugging
    if (migrationResult.wasMigrated) {
      console.info(
        'Migrated legacy Start step from execution graph.',
        migrationResult.extractedInputSchema ? 'Extracted inputSchema.' : '',
        migrationResult.extractedVariables ? 'Extracted variables.' : ''
      );
    }
  }

  const { steps = {}, executionPlan = [], notes = [] } = graphToProcess;
  const { nodes: parsedNodes, edges } = normalizeNodesAndEdges(
    steps,
    executionPlan || []
  );
  const nodes = applyStoredNodeVisualState(parsedNodes, graphToProcess.nodes);

  // Convert notes to React Flow nodes
  // New format uses: { text, position: {x, y} }
  const noteNodes: Node[] = (notes || []).map((note: any) => {
    const defaultSize = NODE_TYPE_SIZES[NODE_TYPES.NoteNode] || {
      width: 240,
      height: 120,
    };
    const width = snapToGrid(note.metadata?.width ?? defaultSize.width);
    const height = snapToGrid(note.metadata?.height ?? defaultSize.height);

    // Handle position - new format uses position.x/y
    const x = note.position?.x ?? note.x ?? 0;
    const y = note.position?.y ?? note.y ?? 0;

    // Handle content - new format uses "text" field
    const content = note.text ?? note.content ?? '';

    return {
      id: note.id,
      type: NODE_TYPES.NoteNode,
      position: snapPositionToGrid({ x, y }),
      data: {
        id: note.id,
        content,
      },
      width,
      height,
      style: {
        width,
        height,
      },
    };
  });

  return { nodes: [...nodes, ...noteNodes], edges };
}

function normalizeNodesAndEdges(
  steps: Record<string, ExecutionGraphStepDto>,
  executionPlan: ExecutionGraphTransitionDto[],
  parentId?: string
) {
  const nodes: Node[] = [];
  const edges: Edge[] = [];

  // nodes
  for (const [id, step] of Object.entries(steps)) {
    const { subgraph, ...data } = step;
    const { inputMapping = {} } = data;

    const nodeType = step.stepType
      ? STEP_TYPES[step.stepType] || NODE_TYPES.BasicNode
      : NODE_TYPES.BasicNode;

    // Helper function to safely parse potentially double-stringified values
    const safeParseValue = (value: any): any => {
      if (typeof value !== 'string') return value;
      try {
        const parsed = JSON.parse(value);
        // If it's still a string after parsing, it might be double-stringified
        if (typeof parsed === 'string') {
          return parsed;
        }
        return parsed;
      } catch {
        return value;
      }
    };

    // Helper function to convert composite values from API format (type) to UI format (typeHint)
    const convertCompositeToUIFormat = (compositeVal: any): any => {
      const convertEntry = (val: any) => {
        const typedVal = val as {
          valueType: 'reference' | 'immediate' | 'composite' | 'template';
          value: any;
          type?: string;
          default?: any;
        };
        if (typedVal.valueType === 'composite') {
          return {
            valueType: 'composite',
            value: convertCompositeToUIFormat(typedVal.value),
            ...(typedVal.type ? { typeHint: typedVal.type } : {}),
          };
        }
        const out: Record<string, any> = {
          valueType: typedVal.valueType,
          value: typedVal.value,
        };
        // Convert backend `type` → UI `typeHint` for every non-composite variant,
        // not only `immediate` — references/templates can carry type hints too.
        if (typedVal.type !== undefined) {
          out.typeHint = typedVal.type;
        }
        // Preserve ReferenceValue.default so it survives the UI round-trip.
        if (
          typedVal.valueType === 'reference' &&
          typedVal.default !== undefined
        ) {
          out.defaultValue = typedVal.default;
        }
        return out;
      };

      // Handle composite object
      if (
        compositeVal &&
        typeof compositeVal === 'object' &&
        !Array.isArray(compositeVal)
      ) {
        const convertedObject: Record<string, any> = {};
        for (const [key, val] of Object.entries(compositeVal)) {
          convertedObject[key] = convertEntry(val);
        }
        return convertedObject;
      }

      // Handle composite array
      if (Array.isArray(compositeVal)) {
        return compositeVal.map(convertEntry);
      }

      // Return as-is if not a composite structure
      return compositeVal;
    };

    // Get correct size for this node type
    const nodeSize = NODE_TYPE_SIZES[nodeType] || {
      width: BASE_WIDTH,
      height: BASE_HEIGHT,
    };

    // Type assertion for extended properties that may exist at runtime
    const extendedData = data as any;
    const parsedInputSchema = safeParseValue((data as any).inputSchema);

    const node: Node = {
      id,
      type: nodeType,
      data: {
        ...data,
        ...(parsedInputSchema ? { inputSchema: parsedInputSchema } : {}),
        // Fix potentially double-stringified values for EmbedWorkflow steps
        ...(extendedData.childWorkflowId && {
          childWorkflowId: safeParseValue(extendedData.childWorkflowId),
        }),
        ...(extendedData.childVersion && {
          childVersion: safeParseValue(extendedData.childVersion),
        }),
        // Handle inputMapping conversion - flat object format
        inputMapping: Object.keys(inputMapping).map((input) => {
          const mappingValue = inputMapping[input];

          // Handle new format: { valueType, value, type?, default? }
          if (
            typeof mappingValue === 'object' &&
            mappingValue !== null &&
            'value' in mappingValue
          ) {
            const typedValue = mappingValue as {
              value: any;
              type?: string;
              default?: any;
              valueType?: 'reference' | 'immediate' | 'composite' | 'template';
            };

            // For composite values, convert nested 'type' fields to 'typeHint'
            if (typedValue.valueType === 'composite') {
              return {
                type: input,
                value: convertCompositeToUIFormat(typedValue.value),
                typeHint: typedValue.type as ValueType | undefined,
                valueType: 'composite' as const,
              };
            }

            const resolvedValueType = typedValue.valueType || 'immediate';
            const entry: {
              type: string;
              value: any;
              typeHint: ValueType | undefined;
              valueType: 'reference' | 'immediate' | 'template';
              defaultValue?: any;
            } = {
              type: input,
              value: typedValue.value, // Can be string, array, object, or composite structure
              typeHint: typedValue.type as ValueType | undefined,
              valueType: resolvedValueType as
                | 'reference'
                | 'immediate'
                | 'template',
            };
            // Preserve ReferenceValue.default so a subsequent save doesn't drop it.
            if (
              resolvedValueType === 'reference' &&
              typedValue.default !== undefined
            ) {
              entry.defaultValue = typedValue.default;
            }
            return entry;
          }

          // Handle legacy format: string value
          return {
            type: input,
            value: mappingValue,
            typeHint: undefined,
            valueType: 'immediate' as const,
          };
        }),
        // For Split steps, parse config, inputSchema and outputSchema into UI fields
        ...(step.stepType === 'Split'
          ? (() => {
              const config = (data as any).config;
              // Convert config.value to inputMapping format for the UI.
              // Carry the backend `type` through so the save path can round-trip it.
              const splitInputMapping = config?.value
                ? [
                    {
                      type: 'value',
                      value: config.value.value,
                      typeHint: config.value.type ?? 'auto',
                      valueType: config.value.valueType || 'reference',
                      ...(config.value.default !== undefined
                        ? { defaultValue: config.value.default }
                        : {}),
                    },
                  ]
                : [];

              // Convert config.variables to splitVariablesFields format
              const splitVariablesFields = config?.variables
                ? Object.entries(config.variables).map(([name, varDef]) => {
                    const typedVarDef = varDef as {
                      valueType?: 'reference' | 'immediate' | 'composite';
                      value: unknown;
                      type?: string;
                    };
                    const resolvedValueType:
                      | 'reference'
                      | 'immediate'
                      | 'composite' =
                      typedVarDef.valueType ||
                      (typeof typedVarDef.value === 'object' &&
                      typedVarDef.value !== null
                        ? 'composite'
                        : 'reference');

                    // Keep immediates/references as-is so round-trip is lossless.
                    // Composites: arrays stay arrays; object-shaped values stay objects.
                    // SplitStepField renders scalars via JSON.stringify when needed, so
                    // we don't need to coerce at load time.
                    const resolvedValue =
                      resolvedValueType === 'composite'
                        ? Array.isArray(typedVarDef.value)
                          ? typedVarDef.value
                          : typeof typedVarDef.value === 'object' &&
                              typedVarDef.value !== null
                            ? typedVarDef.value
                            : {}
                        : typedVarDef.value === undefined
                          ? ''
                          : typedVarDef.value;

                    return {
                      name,
                      value: resolvedValue,
                      // Only forward `type` when backend had it; don't synthesize a default
                      // or we'll asymmetrically inject a field that wasn't there on load.
                      ...(typedVarDef.type !== undefined
                        ? { type: typedVarDef.type }
                        : {}),
                      valueType: resolvedValueType,
                    };
                  })
                : [];

              return {
                // Override inputMapping with config.value for Split steps
                inputMapping: splitInputMapping,
                splitInputSchemaFields: parsedInputSchema
                  ? Object.entries(parsedInputSchema).map(
                      ([name, typeDef]) => ({
                        name,
                        type:
                          typeof typeDef === 'object' && typeDef !== null
                            ? (typeDef as any).type || 'string'
                            : 'string',
                      })
                    )
                  : [],
                splitOutputSchemaFields: (data as any).outputSchema
                  ? Object.entries((data as any).outputSchema).map(
                      ([name, typeDef]) => ({
                        name,
                        type:
                          typeof typeDef === 'object' && typeDef !== null
                            ? (typeDef as any).type || 'string'
                            : 'string',
                      })
                    )
                  : [],
                outputSchema: safeParseValue((data as any).outputSchema),
                // Config fields
                splitVariablesFields,
                splitParallelism: config?.parallelism ?? 0,
                splitSequential: config?.sequential ?? false,
                splitDontStopOnFailed: config?.dontStopOnFailed ?? false,
              };
            })()
          : {}),
        // For Switch steps, parse config into inputMapping format for the UI
        ...((step.stepType as string) === 'Switch'
          ? (() => {
              const config = (data as any).config;
              const switchInputMapping: any[] = [];

              // Convert config.value to value field
              if (config?.value) {
                switchInputMapping.push({
                  type: 'value',
                  value: config.value.value,
                  typeHint: config.value.type || ValueType.String,
                  valueType: config.value.valueType || 'reference',
                });
              } else {
                switchInputMapping.push({
                  type: 'value',
                  value: '',
                  typeHint: ValueType.String,
                });
              }

              // Convert config.cases to cases field with UI match types
              const uiCases = (config?.cases || []).map((c: any) => ({
                match: c.match,
                matchType: mapMatchTypeFromAPI(c.matchType),
                output: c.output,
                ...(c.route ? { route: c.route } : {}),
              }));
              switchInputMapping.push({
                type: 'cases',
                value: uiCases,
                typeHint: ValueType.Json,
              });

              // Convert config.default
              switchInputMapping.push({
                type: 'default',
                value: config?.default ?? {},
                typeHint: ValueType.Json,
              });

              // Detect routing mode: any case with a route field
              const hasRoutes = uiCases.some(
                (c: any) => c.route && c.route !== ''
              );

              return {
                inputMapping: switchInputMapping,
                ...(hasRoutes
                  ? {
                      switchRoutingMode: true,
                    }
                  : {}),
              };
            })()
          : {}),
        // For Filter steps, parse config into form fields
        ...((step.stepType as string) === 'Filter'
          ? (() => {
              const config = (data as any).config;
              const filterInputMapping = config?.value
                ? [
                    {
                      type: 'value',
                      value: config.value.value,
                      typeHint: 'auto',
                      valueType: config.value.valueType || 'reference',
                    },
                  ]
                : [];

              return {
                inputMapping: filterInputMapping,
                filterCondition: config?.condition,
              };
            })()
          : {}),
        // For While steps, parse condition and config into form fields
        ...((step.stepType as string) === 'While'
          ? (() => {
              const config = (data as any).config;
              return {
                inputMapping: [],
                whileCondition: (data as any).condition,
                whileMaxIterations: config?.maxIterations ?? 10,
                whileTimeout: config?.timeout ?? null,
              };
            })()
          : {}),
        // For GroupBy steps, parse config into form fields
        ...((step.stepType as string) === 'GroupBy'
          ? (() => {
              const config = (data as any).config;
              const groupByInputMapping = config?.value
                ? [
                    {
                      type: 'value',
                      value: config.value.value,
                      typeHint: 'auto',
                      valueType: config.value.valueType || 'reference',
                    },
                  ]
                : [];

              return {
                inputMapping: groupByInputMapping,
                groupByKey: config?.key || '',
                groupByExpectedKeys: Array.isArray(config?.expectedKeys)
                  ? config.expectedKeys
                  : [],
              };
            })()
          : {}),
        // For AiAgent steps, parse config into form fields
        ...((step.stepType as string) === 'AiAgent'
          ? (() => {
              const config = (data as any).config;
              const aiInputMapping: any[] = [];

              if (config?.systemPrompt) {
                aiInputMapping.push({
                  type: 'systemPrompt',
                  value: config.systemPrompt.value ?? '',
                  valueType: config.systemPrompt.valueType || 'immediate',
                  typeHint: 'string',
                });
              }

              if (config?.userPrompt) {
                aiInputMapping.push({
                  type: 'userPrompt',
                  value: config.userPrompt.value ?? '',
                  valueType: config.userPrompt.valueType || 'immediate',
                  typeHint: 'string',
                });
              }

              if (config?.provider) {
                aiInputMapping.push({
                  type: 'provider',
                  value: config.provider,
                  valueType: 'immediate',
                  typeHint: 'string',
                });
              }

              if (config?.model) {
                aiInputMapping.push({
                  type: 'model',
                  value: config.model,
                  valueType: 'immediate',
                  typeHint: 'string',
                });
              }

              if (
                config?.maxIterations !== undefined &&
                config.maxIterations !== null
              ) {
                aiInputMapping.push({
                  type: 'maxIterations',
                  value: config.maxIterations,
                  valueType: 'immediate',
                  typeHint: 'integer',
                });
              }

              if (
                config?.temperature !== undefined &&
                config.temperature !== null
              ) {
                aiInputMapping.push({
                  type: 'temperature',
                  value: config.temperature,
                  valueType: 'immediate',
                  typeHint: 'number',
                });
              }

              if (
                config?.maxTokens !== undefined &&
                config.maxTokens !== null
              ) {
                aiInputMapping.push({
                  type: 'maxTokens',
                  value: config.maxTokens,
                  valueType: 'immediate',
                  typeHint: 'integer',
                });
              }

              // Memory config: deserialize config.memory into form fields
              if (config?.memory) {
                aiInputMapping.push({
                  type: 'memoryEnabled',
                  value: true,
                  valueType: 'immediate',
                  typeHint: 'boolean',
                });

                if (config.memory.conversationId) {
                  aiInputMapping.push({
                    type: 'memoryConversationId',
                    value: config.memory.conversationId.value ?? '',
                    valueType:
                      config.memory.conversationId.valueType || 'reference',
                    typeHint: 'string',
                  });
                }

                if (config.memory.compaction) {
                  if (config.memory.compaction.maxMessages !== undefined) {
                    aiInputMapping.push({
                      type: 'memoryMaxMessages',
                      value: config.memory.compaction.maxMessages,
                      valueType: 'immediate',
                      typeHint: 'integer',
                    });
                  }
                  if (config.memory.compaction.strategy) {
                    aiInputMapping.push({
                      type: 'memoryStrategy',
                      value: config.memory.compaction.strategy,
                      valueType: 'immediate',
                      typeHint: 'string',
                    });
                  }
                }

                // Find the memory provider step from execution plan
                const memoryEdge = (executionPlan || []).find(
                  (e) => e.fromStep === id && e.label === 'memory'
                );
                if (memoryEdge?.toStep) {
                  aiInputMapping.push({
                    type: 'memoryProviderStepId',
                    value: memoryEdge.toStep,
                    valueType: 'immediate',
                    typeHint: 'string',
                  });
                }
              }

              // Build tools array from executionPlan edges with labels
              // Filter out 'memory' label — it's not a tool
              const toolNames = (executionPlan || [])
                .filter(
                  (e) =>
                    e.fromStep === id &&
                    e.label &&
                    e.label !== 'next' &&
                    e.label !== 'default' &&
                    e.label !== 'memory'
                )
                .map((e) => e.label as string);

              if (toolNames.length > 0) {
                aiInputMapping.push({
                  type: 'tools',
                  value: toolNames,
                  valueType: 'immediate',
                  typeHint: 'json',
                });
              }

              // Output schema: convert Record<string, SchemaField> → SchemaField[]
              if (
                config?.outputSchema &&
                typeof config.outputSchema === 'object'
              ) {
                const schemaFields = parseSchema(config.outputSchema);
                aiInputMapping.push({
                  type: 'outputSchema',
                  value: schemaFields.map((f) => ({
                    name: f.name,
                    type: f.type || 'string',
                    required: f.required !== false,
                    description: f.description || '',
                    ...(Array.isArray(f.enum) && f.enum.length > 0
                      ? { enum: f.enum }
                      : {}),
                  })),
                  valueType: 'immediate',
                  typeHint: 'json',
                });
              }

              return { inputMapping: aiInputMapping };
            })()
          : {}),
        // For WaitForSignal steps, parse top-level fields into inputMapping
        ...((step.stepType as string) === 'WaitForSignal'
          ? (() => {
              const waitStep = data as any;
              const waitInputMapping: any[] = [];

              // Convert responseSchema (Record<string,SchemaField>) → SchemaField[]
              const schemaFields = parseSchema(waitStep.responseSchema);
              waitInputMapping.push({
                type: 'responseSchema',
                value: schemaFields.map((f) => ({
                  name: f.name,
                  type: f.type || 'string',
                  required: f.required !== false,
                  description: f.description || '',
                })),
                valueType: 'immediate',
                typeHint: 'json',
              });

              // Convert timeoutMs (MappingValue)
              if (waitStep.timeoutMs) {
                waitInputMapping.push({
                  type: 'timeoutMs',
                  value: waitStep.timeoutMs.value ?? '',
                  valueType: waitStep.timeoutMs.valueType || 'immediate',
                  typeHint: 'number',
                });
              } else {
                waitInputMapping.push({
                  type: 'timeoutMs',
                  value: '',
                  valueType: 'immediate',
                  typeHint: 'number',
                });
              }

              // Convert pollIntervalMs (number)
              waitInputMapping.push({
                type: 'pollIntervalMs',
                value:
                  waitStep.pollIntervalMs !== undefined &&
                  waitStep.pollIntervalMs !== null
                    ? String(waitStep.pollIntervalMs)
                    : '1000',
                valueType: 'immediate',
                typeHint: 'number',
              });

              return { inputMapping: waitInputMapping };
            })()
          : {}),
        // For Log steps, parse top-level fields into inputMapping
        ...((step.stepType as string) === 'Log'
          ? (() => {
              const logStep = data as any;
              const logInputMapping: any[] = [
                {
                  type: 'message',
                  value: logStep.message || '',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
                {
                  type: 'level',
                  value: logStep.level || 'info',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
              ];

              return { inputMapping: logInputMapping };
            })()
          : {}),
        // For Error steps, parse top-level fields into inputMapping
        ...((step.stepType as string) === 'Error'
          ? (() => {
              const errorStep = data as any;
              const errorInputMapping: any[] = [
                {
                  type: 'code',
                  value: errorStep.code || '',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
                {
                  type: 'message',
                  value: errorStep.message || '',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
                {
                  type: 'category',
                  value: errorStep.category || 'permanent',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
                {
                  type: 'severity',
                  value: errorStep.severity || 'error',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
              ];

              return { inputMapping: errorInputMapping };
            })()
          : {}),
      },
      // Resizable nodes (Container, Note) use saved dimensions; others always use config size
      width: snapToGrid(
        nodeType === NODE_TYPES.ContainerNode ||
          nodeType === NODE_TYPES.NoteNode
          ? (data.renderingParameters?.width ?? nodeSize.width)
          : nodeSize.width
      ),
      height: snapToGrid(
        nodeType === NODE_TYPES.ContainerNode ||
          nodeType === NODE_TYPES.NoteNode
          ? (data.renderingParameters?.height ?? nodeSize.height)
          : nodeSize.height
      ),
      style: {
        width: snapToGrid(
          nodeType === NODE_TYPES.ContainerNode ||
            nodeType === NODE_TYPES.NoteNode
            ? (data.renderingParameters?.width ?? nodeSize.width)
            : nodeSize.width
        ),
        height: snapToGrid(
          nodeType === NODE_TYPES.ContainerNode ||
            nodeType === NODE_TYPES.NoteNode
            ? (data.renderingParameters?.height ?? nodeSize.height)
            : nodeSize.height
        ),
      },
      position: snapPositionToGrid({
        x: data.renderingParameters?.x ?? 0,
        y: data.renderingParameters?.y ?? 0,
      }),
    };

    if (parentId) {
      node.parentId = parentId;
      node.extent = 'parent';
      node.expandParent = true;
    }

    nodes.push(node);

    if (subgraph) {
      const { nodes: childNodes, edges: childEdges } = normalizeNodesAndEdges(
        subgraph.steps || {},
        subgraph.executionPlan || [],
        id
      );
      nodes.push(...childNodes);
      edges.push(...childEdges);
    }
  }

  // edges
  for (let i = 0; i < executionPlan.length; i++) {
    const edge = executionPlan[i];
    const sourceStep = steps[edge.fromStep ?? ''];
    const isSwitchSource = (sourceStep?.stepType as string) === 'Switch';

    let sourceHandle: string;

    if (isSwitchSource) {
      // Switch nodes: map edge labels to the correct sourceHandle
      const label = edge.label || 'next';
      if (label === 'default') {
        sourceHandle = 'default';
      } else if (label === 'next') {
        // Value mode: single output
        sourceHandle = 'source';
      } else if (label.startsWith('case-')) {
        // Direct case index label
        sourceHandle = label;
      } else {
        // Route label: find matching case index by route name
        const config = (sourceStep as any).config;
        const cases = config?.cases || [];
        const caseIndex = cases.findIndex((c: any) => c.route === label);
        sourceHandle = caseIndex >= 0 ? `case-${caseIndex}` : label;
      }
    } else {
      // Non-Switch: convert spec labels back to React Flow sourceHandle
      // "next" -> "source" for sequential edges, keep "true"/"false" for Conditional
      sourceHandle =
        !edge.label || edge.label === 'default' || edge.label === 'next'
          ? 'source'
          : edge.label;
    }

    edges.push({
      id: `${edge.fromStep}-${edge.toStep}-${edge.label || 'default'}-${i}`,
      source: edge.fromStep ?? '',
      target: edge.toStep ?? '',
      sourceHandle,
    });
  }

  // Post-process: ensure container nodes are large enough to contain their children.
  // When renderingParameters don't include width/height, containers default to a small size
  // but children may have positions far outside those bounds. Without this fix,
  // React Flow's expandParent triggers unexpected movement on selection/click.
  const containerIds = new Set(
    nodes.filter((n) => n.type === NODE_TYPES.ContainerNode).map((n) => n.id)
  );

  for (const containerId of containerIds) {
    const container = nodes.find((n) => n.id === containerId);
    if (!container) continue;

    const children = nodes.filter((n) => n.parentId === containerId);
    if (children.length === 0) continue;

    // Calculate bounding box of children
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

    // Add padding and snap to grid
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

  return { nodes, edges };
}
