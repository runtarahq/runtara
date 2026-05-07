import {
  buildIncomingEdges,
  buildOutgoingEdges,
  type LayoutEdge,
  type LayoutNode,
} from './graph';
import type { RankResult } from './rank';

const CONDITIONAL_BRANCH_LANE_BIAS_SCALE = 3;
const SWITCH_BRANCH_LANE_BIAS_SCALE = 1.2;

export type OrderedRanks = {
  ranks: string[][];
  rowById: Map<string, number>;
  branchLaneBias: Map<string, number>;
  edgeLaneOffsets: Map<string, number>;
};

function average(values: number[]): number | null {
  if (values.length === 0) return null;
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

function updateRankRows(ranks: string[][], rowById: Map<string, number>): void {
  for (const rankIds of ranks) {
    rankIds.forEach((nodeId, index) => rowById.set(nodeId, index));
  }
}

function buildEdgeLaneOffsets(
  outgoingBySource: Map<string, LayoutEdge[]>
): Map<string, number> {
  const offsets = new Map<string, number>();

  for (const outgoingEdges of outgoingBySource.values()) {
    if (outgoingEdges.length <= 1) {
      if (outgoingEdges[0]) offsets.set(outgoingEdges[0].id, 0);
      continue;
    }

    const center = (outgoingEdges.length - 1) / 2;
    for (let index = 0; index < outgoingEdges.length; index++) {
      offsets.set(outgoingEdges[index].id, (index - center) * 0.9);
    }
  }

  return offsets;
}

function getBranchLaneBiasScale(outgoingEdges: LayoutEdge[]): number {
  const hasConditionalHandle = outgoingEdges.some(
    (edge) =>
      edge.kind === 'conditional-true' || edge.kind === 'conditional-false'
  );

  return hasConditionalHandle
    ? CONDITIONAL_BRANCH_LANE_BIAS_SCALE
    : SWITCH_BRANCH_LANE_BIAS_SCALE;
}

function getBranchLaneBiasOffset(
  edge: LayoutEdge,
  sourceBranches: LayoutEdge[],
  edgeLaneOffsets: Map<string, number>
): number {
  if (sourceBranches.length <= 1) return 0;

  return (
    (edgeLaneOffsets.get(edge.id) ?? 0) * getBranchLaneBiasScale(sourceBranches)
  );
}

function buildBranchLaneBias(
  nodes: LayoutNode[],
  incomingByTarget: Map<string, LayoutEdge[]>,
  outgoingBySource: Map<string, LayoutEdge[]>,
  edgeLaneOffsets: Map<string, number>
): Map<string, number> {
  const biasByNode = new Map<string, number>();

  for (const node of nodes) {
    const incomingEdges = incomingByTarget.get(node.id) ?? [];
    const candidateBiases: number[] = [];

    for (const edge of incomingEdges) {
      if (edge.source === edge.target) continue;
      const sourceBias = biasByNode.get(edge.source) ?? 0;
      const sourceBranches = outgoingBySource.get(edge.source) ?? [];
      const branchOffset = getBranchLaneBiasOffset(
        edge,
        sourceBranches,
        edgeLaneOffsets
      );

      candidateBiases.push(sourceBias + branchOffset);
    }

    if (candidateBiases.length > 0) {
      const inheritedBias = average(candidateBiases) ?? 0;
      biasByNode.set(node.id, inheritedBias);
    } else if (!biasByNode.has(node.id)) {
      biasByNode.set(node.id, 0);
    }

    const outgoingEdges = outgoingBySource.get(node.id) ?? [];
    if (outgoingEdges.length > 1) {
      const sourceBias = biasByNode.get(node.id) ?? 0;
      for (const edge of outgoingEdges) {
        const targetCandidate =
          sourceBias +
          getBranchLaneBiasOffset(edge, outgoingEdges, edgeLaneOffsets);
        const currentTargetBias = biasByNode.get(edge.target);
        if (currentTargetBias === undefined) {
          biasByNode.set(edge.target, targetCandidate);
        }
      }
    }
  }

  return biasByNode;
}

function getOrderingScore(
  nodeId: string,
  direction: 'incoming' | 'outgoing',
  rowById: Map<string, number>,
  incomingByTarget: Map<string, LayoutEdge[]>,
  outgoingBySource: Map<string, LayoutEdge[]>,
  edgeLaneOffsets: Map<string, number>
): number {
  const scores: number[] = [];

  if (direction === 'incoming') {
    for (const edge of incomingByTarget.get(nodeId) ?? []) {
      const sourceRow = rowById.get(edge.source);
      if (sourceRow === undefined) continue;
      scores.push(sourceRow + (edgeLaneOffsets.get(edge.id) ?? 0));
    }
  } else {
    for (const edge of outgoingBySource.get(nodeId) ?? []) {
      const targetRow = rowById.get(edge.target);
      if (targetRow === undefined) continue;
      scores.push(targetRow - (edgeLaneOffsets.get(edge.id) ?? 0));
    }
  }

  return average(scores) ?? rowById.get(nodeId) ?? 0;
}

function getDeclaredIncomingOrder(
  nodeId: string,
  incomingByTarget: Map<string, LayoutEdge[]>
): number {
  const incomingEdges = incomingByTarget.get(nodeId) ?? [];
  if (incomingEdges.length === 0) return Number.POSITIVE_INFINITY;
  return Math.min(...incomingEdges.map((edge) => edge.order));
}

function applyHardOrderingConstraints(
  rankIds: string[],
  incomingByTarget: Map<string, LayoutEdge[]>,
  branchLaneBias: Map<string, number>,
  orderIndex: Map<string, number>
): void {
  rankIds.sort((a, b) => {
    const biasA = branchLaneBias.get(a) ?? 0;
    const biasB = branchLaneBias.get(b) ?? 0;
    if (Math.abs(biasA - biasB) > 0.001) return biasA - biasB;

    const declaredA = getDeclaredIncomingOrder(a, incomingByTarget);
    const declaredB = getDeclaredIncomingOrder(b, incomingByTarget);
    if (declaredA !== declaredB) return declaredA - declaredB;

    return (orderIndex.get(a) ?? 0) - (orderIndex.get(b) ?? 0);
  });
}

export function orderRanks(
  nodes: LayoutNode[],
  edges: LayoutEdge[],
  rankResult: RankResult
): OrderedRanks {
  const outgoingBySource = buildOutgoingEdges(edges);
  const incomingByTarget = buildIncomingEdges(edges);
  const ranks = rankResult.ranks.map((rankIds) => [...rankIds]);
  const edgeLaneOffsets = buildEdgeLaneOffsets(outgoingBySource);
  const branchLaneBias = buildBranchLaneBias(
    nodes,
    incomingByTarget,
    outgoingBySource,
    edgeLaneOffsets
  );
  const rowById = new Map<string, number>();
  updateRankRows(ranks, rowById);

  const sortRank = (rankIds: string[], direction: 'incoming' | 'outgoing') => {
    rankIds.sort((a, b) => {
      const scoreA =
        getOrderingScore(
          a,
          direction,
          rowById,
          incomingByTarget,
          outgoingBySource,
          edgeLaneOffsets
        ) + (branchLaneBias.get(a) ?? 0);
      const scoreB =
        getOrderingScore(
          b,
          direction,
          rowById,
          incomingByTarget,
          outgoingBySource,
          edgeLaneOffsets
        ) + (branchLaneBias.get(b) ?? 0);

      if (Math.abs(scoreA - scoreB) > 0.001) return scoreA - scoreB;
      return (
        (rankResult.orderIndex.get(a) ?? 0) -
        (rankResult.orderIndex.get(b) ?? 0)
      );
    });

    applyHardOrderingConstraints(
      rankIds,
      incomingByTarget,
      branchLaneBias,
      rankResult.orderIndex
    );
  };

  for (let iteration = 0; iteration < 6; iteration++) {
    for (let rank = 1; rank < ranks.length; rank++) {
      sortRank(ranks[rank], 'incoming');
      updateRankRows(ranks, rowById);
    }

    for (let rank = ranks.length - 2; rank >= 0; rank--) {
      sortRank(ranks[rank], 'outgoing');
      updateRankRows(ranks, rowById);
    }
  }

  return {
    ranks,
    rowById,
    branchLaneBias,
    edgeLaneOffsets,
  };
}
