import { NODE_TYPES } from '@/features/workflows/config/workflow.ts';

/**
 * Minimal edge interface for validation functions.
 * This allows validation functions to work with both domain types and React Flow types.
 */
interface EdgeLike {
  source: string;
  target: string;
}

/**
 * Minimal node interface for validation functions.
 * This allows validation functions to work with both domain types and React Flow types.
 */
interface NodeLike {
  id: string;
  type?: string;
  parentId?: string;
  data?: {
    stepType?: string;
    name?: string;
    description?: string;
  };
}

/**
 * Finds all downstream nodes from a given node (nodes reachable by following edges forward).
 * Uses forward-only traversal (source -> target direction).
 *
 * @param edges - Current edges in the graph (supports both domain and React Flow types)
 * @param startNodeId - ID of the starting node
 * @returns Set of downstream node IDs (excluding the start node)
 */
export function getDownstreamNodes(
  edges: EdgeLike[],
  startNodeId: string
): Set<string> {
  const downstreamNodes = new Set<string>();
  const visited = new Set<string>();

  // Build forward-only adjacency list (source -> targets)
  const adjacencyList = new Map<string, Set<string>>();

  for (const edge of edges) {
    if (!adjacencyList.has(edge.source)) {
      adjacencyList.set(edge.source, new Set());
    }
    adjacencyList.get(edge.source)!.add(edge.target);
  }

  // BFS to find all downstream nodes
  const queue = [startNodeId];
  visited.add(startNodeId);

  while (queue.length > 0) {
    const currentId = queue.shift()!;
    const targets = adjacencyList.get(currentId) || new Set();

    for (const targetId of targets) {
      if (!visited.has(targetId)) {
        visited.add(targetId);
        downstreamNodes.add(targetId);
        queue.push(targetId);
      }
    }
  }

  return downstreamNodes;
}

/**
 * Checks if connecting from source to target would be a self-connection
 */
export function isSelfConnection(source: string, target: string): boolean {
  return source === target;
}

/**
 * Detects if adding an edge from source to target would create a cycle in the graph.
 * Uses Depth-First Search (DFS) to check if target can already reach source.
 * If target can reach source, then adding source->target would create a cycle.
 *
 * @param edges - Current edges in the graph (supports both domain and React Flow types)
 * @param source - ID of the source node
 * @param target - ID of the target node
 * @returns true if adding the edge would create a loop, false otherwise
 */
export function wouldCreateLoop(
  edges: EdgeLike[],
  source: string,
  target: string
): boolean {
  // Build adjacency list from current edges
  const adjacencyList = new Map<string, string[]>();

  for (const edge of edges) {
    if (!adjacencyList.has(edge.source)) {
      adjacencyList.set(edge.source, []);
    }
    adjacencyList.get(edge.source)!.push(edge.target);
  }

  // Check if adding edge source->target would create cycle
  // by checking if 'target' can already reach 'source'
  const visited = new Set<string>();

  function canReach(current: string, destination: string): boolean {
    if (current === destination) return true;
    if (visited.has(current)) return false;

    visited.add(current);
    const neighbors = adjacencyList.get(current) || [];

    for (const neighbor of neighbors) {
      if (canReach(neighbor, destination)) return true;
    }

    return false;
  }

  // If 'target' can already reach 'source', adding source->target creates a cycle
  return canReach(target, source);
}

/**
 * Validates if a connection can be made between two nodes.
 * Checks for both self-connections and circular dependencies.
 *
 * This function is library-agnostic and works with both domain types (WorkflowEdge/WorkflowNode)
 * and React Flow types (Edge/Node) thanks to the minimal interface requirements.
 *
 * @param edges - Current edges in the graph
 * @param _nodes - Current nodes in the graph (unused but kept for API compatibility)
 * @param source - ID of the source node
 * @param target - ID of the target node
 * @returns Object with validation result and error message if invalid
 */
export function validateConnection(
  edges: EdgeLike[],
  _nodes: NodeLike[],
  source: string,
  target: string
): { isValid: boolean; errorMessage?: string } {
  // Check for self-connection
  if (isSelfConnection(source, target)) {
    return {
      isValid: false,
      errorMessage: 'A step cannot connect to itself',
    };
  }

  // Check for circular dependency
  if (wouldCreateLoop(edges, source, target)) {
    return {
      isValid: false,
      errorMessage:
        'This connection would create a circular dependency in your workflow',
    };
  }

  return { isValid: true };
}

/**
 * Maps library node types to domain node type strings for validation purposes.
 * This allows validation to work with both React Flow NODE_TYPES constants
 * and domain WorkflowNodeType strings.
 */
const NOTE_NODE_TYPES = new Set([NODE_TYPES.NoteNode, 'note', 'NOTE_NODE']);
const CREATE_NODE_TYPES = new Set([
  NODE_TYPES.CreateNode,
  'create',
  'CREATE_NODE',
]);

/**
 * Result of workflow structure validation.
 * Includes both errors (blocking) and warnings (non-blocking).
 */
export interface WorkflowValidationResult {
  /** Whether the workflow is valid (only considers errors, not warnings) */
  isValid: boolean;
  /** Error messages that block saving */
  errors: string[];
  /** Warning messages that don't block saving but inform the user */
  warnings: string[];
}

/**
 * Validates the overall structure of a workflow.
 * Checks for:
 * - Orphaned nodes that are not connected to the workflow
 * - Valid entry point (exists, has no incoming edges)
 * - Valid Finish steps (have no outgoing edges)
 * - No deprecated Start step types
 *
 * Also generates warnings for:
 * - Steps without descriptions
 * - Steps with very long names
 *
 * This function is library-agnostic and works with both domain types (WorkflowNode/WorkflowEdge)
 * and React Flow types (Node/Edge).
 *
 * @param nodes - All nodes in the workflow
 * @param edges - All edges in the workflow
 * @returns Object with validation result, error messages, and warning messages
 */
export function validateWorkflowStructure(
  nodes: NodeLike[],
  edges: EdgeLike[]
): WorkflowValidationResult {
  const errors: string[] = [];
  const warnings: string[] = [];

  // Filter out note nodes and create nodes from validation
  const workflowNodes = nodes.filter(
    (n) =>
      !NOTE_NODE_TYPES.has(n.type || '') && !CREATE_NODE_TYPES.has(n.type || '')
  );

  if (workflowNodes.length === 0) {
    // Empty workflow is not valid when saving - at least one step is required
    return {
      isValid: false,
      errors: ['Workflow should have at least one step'],
      warnings: [],
    };
  }

  // Build sets for connectivity analysis
  const nodesWithIncomingEdges = new Set<string>();
  const nodesWithOutgoingEdges = new Set<string>();
  const connectedNodes = new Set<string>();

  edges.forEach((edge) => {
    nodesWithIncomingEdges.add(edge.target);
    nodesWithOutgoingEdges.add(edge.source);
    connectedNodes.add(edge.source);
    connectedNodes.add(edge.target);
  });

  // Check for deprecated Start step type
  const startSteps = workflowNodes.filter((n) => n.data?.stepType === 'Start');
  if (startSteps.length > 0) {
    errors.push(
      'Workflow contains deprecated Start step type. Start steps should be migrated - entry point now points directly to the first real step.'
    );
  }

  // Find entry points (nodes with no incoming edges, excluding Start steps)
  const potentialEntryPoints = workflowNodes.filter(
    (n) => !nodesWithIncomingEdges.has(n.id) && n.data?.stepType !== 'Start'
  );

  // If there are no entry points and we have workflow nodes, that's a problem
  // (unless all nodes are Start steps, which we already flagged above)
  if (potentialEntryPoints.length === 0 && startSteps.length === 0) {
    errors.push(
      'Workflow has no entry point. At least one step must have no incoming connections.'
    );
  }

  // Check that Finish steps have no outgoing edges
  const finishSteps = workflowNodes.filter(
    (n) => n.data?.stepType === 'Finish'
  );
  const finishStepsWithOutgoing = finishSteps.filter((n) =>
    nodesWithOutgoingEdges.has(n.id)
  );
  if (finishStepsWithOutgoing.length > 0) {
    const names = finishStepsWithOutgoing
      .map((n) => n.data?.name || n.id)
      .join(', ');
    errors.push(`Finish steps cannot have outgoing connections: ${names}`);
  }

  // Check for orphaned nodes (nodes not connected to any other node)
  // Exclude nodes that have a parentId - they are inside a container and don't need edge connections
  if (workflowNodes.length > 1) {
    const orphanedNodes = workflowNodes.filter(
      (n) => !connectedNodes.has(n.id) && !n.parentId
    );

    if (orphanedNodes.length > 0) {
      const orphanedNames = orphanedNodes
        .map((n) => n.data?.name || n.id)
        .join(', ');
      errors.push(
        `Some steps are not connected to the workflow: ${orphanedNames}`
      );
    }
  }

  // === WARNINGS ===

  // Check for steps without descriptions
  const stepsWithoutDescription = workflowNodes.filter(
    (n) =>
      !n.data?.description &&
      n.data?.stepType !== 'Finish' &&
      n.data?.stepType !== 'Start'
  );
  if (stepsWithoutDescription.length > 0) {
    const names = stepsWithoutDescription
      .map((n) => n.data?.name || n.id)
      .join(', ');
    warnings.push(`Steps without description: ${names}`);
  }

  // Check for steps with very long names (> 50 characters)
  const stepsWithLongNames = workflowNodes.filter(
    (n) => (n.data?.name as string)?.length > 50
  );
  if (stepsWithLongNames.length > 0) {
    const names = stepsWithLongNames
      .map((n) => n.data?.name || n.id)
      .join(', ');
    warnings.push(`Steps with long names (> 50 chars): ${names}`);
  }

  return {
    isValid: errors.length === 0,
    errors,
    warnings,
  };
}

// =============================================================================
// Type-safe overloads for domain types
// =============================================================================
