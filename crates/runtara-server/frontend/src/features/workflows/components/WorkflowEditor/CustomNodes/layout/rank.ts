import {
  buildOutgoingEdges,
  compareLayoutNodes,
  type LayoutEdge,
  type LayoutNode,
} from './graph';

export type RankResult = {
  orderedNodes: LayoutNode[];
  orderIndex: Map<string, number>;
  rankById: Map<string, number>;
  ranks: string[][];
  backEdgeIds: Set<string>;
};

function getTopologicalNodeOrder(
  nodes: LayoutNode[],
  edges: LayoutEdge[]
): LayoutNode[] {
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const indegree = new Map(nodes.map((node) => [node.id, 0]));
  const outgoing = buildOutgoingEdges(edges);

  for (const edge of edges) {
    if (edge.source === edge.target) continue;
    indegree.set(edge.target, (indegree.get(edge.target) ?? 0) + 1);
  }

  const queue = nodes
    .filter((node) => (indegree.get(node.id) ?? 0) === 0)
    .sort(compareLayoutNodes);
  const ordered: LayoutNode[] = [];
  const visited = new Set<string>();

  while (queue.length > 0) {
    queue.sort(compareLayoutNodes);
    const current = queue.shift()!;
    if (visited.has(current.id)) continue;

    visited.add(current.id);
    ordered.push(current);

    for (const edge of outgoing.get(current.id) ?? []) {
      if (edge.source === edge.target) continue;

      const nextInDegree = (indegree.get(edge.target) ?? 0) - 1;
      indegree.set(edge.target, nextInDegree);

      if (nextInDegree <= 0) {
        const target = nodeById.get(edge.target);
        if (target && !visited.has(target.id)) queue.push(target);
      }
    }
  }

  const remaining = nodes
    .filter((node) => !visited.has(node.id))
    .sort(compareLayoutNodes);

  return [...ordered, ...remaining];
}

export function rankScope(
  nodes: LayoutNode[],
  edges: LayoutEdge[]
): RankResult {
  const orderedNodes = getTopologicalNodeOrder(nodes, edges);
  const orderIndex = new Map(
    orderedNodes.map((node, index) => [node.id, index])
  );
  const outgoingBySource = buildOutgoingEdges(edges);
  const rankById = new Map(orderedNodes.map((node) => [node.id, 0]));
  const backEdgeIds = new Set<string>();

  for (const node of orderedNodes) {
    const sourceRank = rankById.get(node.id) ?? 0;
    const sourceIndex = orderIndex.get(node.id) ?? 0;

    for (const edge of outgoingBySource.get(node.id) ?? []) {
      const targetIndex = orderIndex.get(edge.target);
      if (targetIndex === undefined || targetIndex <= sourceIndex) {
        backEdgeIds.add(edge.id);
        continue;
      }

      rankById.set(
        edge.target,
        Math.max(rankById.get(edge.target) ?? 0, sourceRank + 1)
      );
    }
  }

  const maxRank = Math.max(0, ...rankById.values());
  const ranks = Array.from({ length: maxRank + 1 }, () => [] as string[]);

  for (const node of orderedNodes) {
    ranks[rankById.get(node.id) ?? 0].push(node.id);
  }

  return {
    orderedNodes,
    orderIndex,
    rankById,
    ranks,
    backEdgeIds,
  };
}
