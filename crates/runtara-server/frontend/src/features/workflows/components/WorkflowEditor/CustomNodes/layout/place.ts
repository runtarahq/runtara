import {
  BASE_HEIGHT,
  BASE_WIDTH,
  buildScopedEdges,
  compareLayoutNodes,
  findConnectedComponents,
  snapGapToGrid,
  type LayoutEdge,
  type LayoutNode,
  type LayoutPoint,
  type LayoutSize,
} from './graph';
import { orderRanks } from './order';
import { rankScope } from './rank';
import { snapToGrid } from '@/features/workflows/config/workflow-editor';

function placeOrderedRanks(
  ranks: string[][],
  sizes: Map<string, LayoutSize>,
  branchLaneBias: Map<string, number>,
  rankSep: number,
  nodeSep: number
): Map<string, LayoutPoint> {
  const positions = new Map<string, LayoutPoint>();
  const horizontalGap = snapGapToGrid(rankSep);
  const verticalGap = snapGapToGrid(nodeSep);
  const laneUnit = snapGapToGrid(BASE_HEIGHT + nodeSep);
  const rankWidths = ranks.map((rankIds) =>
    Math.max(
      0,
      ...rankIds.map((nodeId) => sizes.get(nodeId)?.width ?? BASE_WIDTH)
    )
  );
  let currentX = 0;
  let minY = Infinity;

  const placementsByRank = ranks.map((rankIds) => {
    const placements = rankIds.map((nodeId) => {
      const size = sizes.get(nodeId) ?? {
        width: BASE_WIDTH,
        height: BASE_HEIGHT,
      };
      const desiredCenterY = (branchLaneBias.get(nodeId) ?? 0) * laneUnit;

      return {
        nodeId,
        size,
        desiredCenterY,
        y: desiredCenterY - size.height / 2,
      };
    });

    const totalHeight =
      placements.reduce((sum, placement) => sum + placement.size.height, 0) +
      Math.max(0, placements.length - 1) * verticalGap;
    const desiredRange =
      placements.length > 0
        ? Math.max(...placements.map((placement) => placement.desiredCenterY)) -
          Math.min(...placements.map((placement) => placement.desiredCenterY))
        : 0;

    if (placements.length > 1 && desiredRange < totalHeight) {
      const averageCenter =
        placements.reduce(
          (sum, placement) => sum + placement.desiredCenterY,
          0
        ) / placements.length;
      let nextY = averageCenter - totalHeight / 2;

      for (const placement of placements) {
        placement.y = nextY;
        nextY += placement.size.height + verticalGap;
      }
    } else {
      let previousBottom = -Infinity;
      for (const placement of placements) {
        if (previousBottom !== -Infinity) {
          placement.y = Math.max(placement.y, previousBottom + verticalGap);
        }
        previousBottom = placement.y + placement.size.height;
      }
    }

    for (const placement of placements) {
      placement.y = snapToGrid(placement.y);
      minY = Math.min(minY, placement.y);
    }

    return placements;
  });

  const yOffset = minY === Infinity ? 0 : -minY;

  for (let rank = 0; rank < ranks.length; rank++) {
    const rankWidth = rankWidths[rank];
    const rankX = snapToGrid(currentX);

    for (const placement of placementsByRank[rank]) {
      positions.set(placement.nodeId, {
        x: rankX,
        y: snapToGrid(placement.y + yOffset),
      });
    }

    currentX = rankX + rankWidth + horizontalGap;
  }

  return positions;
}

function layoutConnectedComponent(
  nodes: LayoutNode[],
  edges: LayoutEdge[],
  sizes: Map<string, LayoutSize>,
  rankSep: number,
  nodeSep: number
): Map<string, LayoutPoint> {
  if (nodes.length === 0) return new Map();

  const scopedEdges = buildScopedEdges(nodes, edges);
  const rankResult = rankScope(nodes, scopedEdges);
  const orderedRanks = orderRanks(
    rankResult.orderedNodes,
    scopedEdges,
    rankResult
  );

  return placeOrderedRanks(
    orderedRanks.ranks,
    sizes,
    orderedRanks.branchLaneBias,
    rankSep,
    nodeSep
  );
}

export function layoutScope(
  nodes: LayoutNode[],
  edges: LayoutEdge[],
  sizes: Map<string, LayoutSize>,
  rankSep: number,
  nodeSep: number
): Map<string, LayoutPoint> {
  const positions = new Map<string, LayoutPoint>();

  if (nodes.length === 0) {
    return positions;
  }

  const scopedEdges = buildScopedEdges(nodes, edges);
  const nodeIds = nodes.map((node) => node.id);
  const components = findConnectedComponents(nodeIds, scopedEdges);

  if (components.length <= 1) {
    return layoutConnectedComponent(
      nodes,
      scopedEdges,
      sizes,
      rankSep,
      nodeSep
    );
  }

  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const componentData = components.map((component) => {
    const componentNodes = nodes.filter((node) => component.has(node.id));
    const componentEdges = scopedEdges.filter(
      (edge) => component.has(edge.source) && component.has(edge.target)
    );
    const minOriginalX = Math.min(
      ...componentNodes.map((node) => node.positionHint.x)
    );
    const minOriginalY = Math.min(
      ...componentNodes.map((node) => node.positionHint.y)
    );

    return {
      nodes: componentNodes.sort(compareLayoutNodes),
      edges: componentEdges,
      minOriginalX,
      minOriginalY,
    };
  });

  componentData.sort((a, b) => {
    if (a.minOriginalX !== b.minOriginalX) {
      return a.minOriginalX - b.minOriginalX;
    }
    return a.minOriginalY - b.minOriginalY;
  });

  let currentX = 0;
  const componentGap = snapGapToGrid(rankSep);

  for (const component of componentData) {
    const componentPositions = layoutConnectedComponent(
      component.nodes,
      component.edges,
      sizes,
      rankSep,
      nodeSep
    );

    let minX = Infinity;
    let maxX = -Infinity;
    for (const [nodeId, pos] of componentPositions) {
      const size = sizes.get(nodeId) ?? {
        width: BASE_WIDTH,
        height: BASE_HEIGHT,
      };
      minX = Math.min(minX, pos.x);
      maxX = Math.max(maxX, pos.x + size.width);
    }

    if (minX === Infinity) continue;

    const originalLeft = Math.max(0, component.minOriginalX);
    const targetX = snapToGrid(Math.max(currentX, originalLeft));
    const offsetX = targetX - minX;

    for (const [nodeId, pos] of componentPositions) {
      if (!nodeById.has(nodeId)) continue;
      positions.set(nodeId, {
        x: snapToGrid(pos.x + offsetX),
        y: pos.y,
      });
    }

    currentX = targetX + (maxX - minX) + componentGap;
  }

  return positions;
}
