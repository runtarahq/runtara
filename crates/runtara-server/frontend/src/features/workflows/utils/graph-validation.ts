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
