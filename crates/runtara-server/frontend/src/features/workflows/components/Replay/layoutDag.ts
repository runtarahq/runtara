/**
 * Dagre wrapper — lays out the versioned replay DAG left→right (layered /
 * Sugiyama). Loop/back edges are excluded from ranking so the graph still
 * flows forward; they are still rendered (as curved edges) by React Flow.
 *
 * Returns top-left positions (React Flow's origin); dagre reports centers.
 */
import dagre from '@dagrejs/dagre';

export interface LayoutNodeInput {
  id: string;
  width: number;
  height: number;
}

export interface LayoutEdgeInput {
  source: string;
  target: string;
  isBackEdge?: boolean;
}

export interface LayoutResult {
  positions: Map<string, { x: number; y: number }>;
  width: number;
  height: number;
}

export interface LayoutOptions {
  direction?: 'LR' | 'TB';
  nodeSep?: number;
  rankSep?: number;
}

export function layoutDag(
  nodes: LayoutNodeInput[],
  edges: LayoutEdgeInput[],
  options: LayoutOptions = {}
): LayoutResult {
  const g = new dagre.graphlib.Graph({ multigraph: true });
  g.setGraph({
    rankdir: options.direction ?? 'LR',
    nodesep: options.nodeSep ?? 28,
    ranksep: options.rankSep ?? 80,
    marginx: 24,
    marginy: 24,
    acyclicer: 'greedy',
  });
  g.setDefaultEdgeLabel(() => ({}));

  for (const n of nodes) {
    g.setNode(n.id, { width: n.width, height: n.height });
  }
  const nodeIds = new Set(nodes.map((n) => n.id));
  let e = 0;
  for (const edge of edges) {
    if (edge.isBackEdge) continue; // keep ranking acyclic
    if (!nodeIds.has(edge.source) || !nodeIds.has(edge.target)) continue;
    g.setEdge(edge.source, edge.target, {}, `e${e++}`);
  }

  dagre.layout(g);

  const positions = new Map<string, { x: number; y: number }>();
  for (const n of nodes) {
    const dn = g.node(n.id) as { x: number; y: number } | undefined;
    if (!dn) {
      positions.set(n.id, { x: 0, y: 0 });
      continue;
    }
    // Dagre gives centers → convert to top-left.
    positions.set(n.id, { x: dn.x - n.width / 2, y: dn.y - n.height / 2 });
  }

  const graphLabel = g.graph() as { width?: number; height?: number };
  return {
    positions,
    width: graphLabel.width ?? 0,
    height: graphLabel.height ?? 0,
  };
}
