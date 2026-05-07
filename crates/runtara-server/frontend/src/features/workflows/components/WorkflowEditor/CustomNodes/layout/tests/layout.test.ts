import { describe, expect, it } from 'vitest';
import type { Edge, Node } from '@xyflow/react';
import {
  buildLayoutGraph,
  layoutReactFlowElements,
  rankScope,
  shouldHideDuplicateEdgeLabel,
} from '..';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
} from '@/features/workflows/config/workflow.ts';

function makeNode(
  id: string,
  type = NODE_TYPES.BasicNode,
  data: Record<string, unknown> = {}
): Node {
  const size = NODE_TYPE_SIZES[type] ?? NODE_TYPE_SIZES[NODE_TYPES.BasicNode];

  return {
    id,
    type,
    position: { x: 0, y: 0 },
    width: size.width,
    height: size.height,
    style: size,
    data: {
      id,
      name: id,
      stepType:
        type === NODE_TYPES.ConditionalNode
          ? 'Conditional'
          : type === NODE_TYPES.SwitchNode
            ? 'Switch'
            : 'Agent',
      ...data,
    },
  } as Node;
}

function makeEdge(
  id: string,
  source: string,
  target: string,
  sourceHandle = 'source'
): Edge {
  return { id, source, target, sourceHandle } as Edge;
}

function getNode(nodes: Node[], id: string): Node {
  const node = nodes.find((item) => item.id === id);
  expect(node).toBeDefined();
  return node!;
}

describe('workflow layout graph', () => {
  it('ranks a linear chain left-to-right from plain graph data', () => {
    const nodes = [makeNode('a'), makeNode('b'), makeNode('c')];
    const edges = [makeEdge('a-b', 'a', 'b'), makeEdge('b-c', 'b', 'c')];
    const graph = buildLayoutGraph(nodes, edges);
    const rankResult = rankScope(graph.nodes, graph.edges);

    expect(rankResult.rankById.get('a')).toBe(0);
    expect(rankResult.rankById.get('b')).toBe(1);
    expect(rankResult.rankById.get('c')).toBe(2);
  });

  it('detects back edges so loops do not constrain ranking', () => {
    const nodes = [makeNode('a'), makeNode('b'), makeNode('c')];
    const edges = [
      makeEdge('a-b', 'a', 'b'),
      makeEdge('b-c', 'b', 'c'),
      makeEdge('c-b', 'c', 'b'),
    ];
    const graph = buildLayoutGraph(nodes, edges);
    const rankResult = rankScope(graph.nodes, graph.edges);

    expect(rankResult.backEdgeIds.has('c-b')).toBe(true);
    expect(rankResult.rankById.get('c')).toBeGreaterThan(
      rankResult.rankById.get('b') ?? -1
    );
  });

  it('places true lanes above the conditional and false lanes below it', () => {
    const nodes = [
      makeNode('start'),
      makeNode('check', NODE_TYPES.ConditionalNode),
      makeNode('accept'),
      makeNode('reject'),
    ];
    const edges = [
      makeEdge('start-check', 'start', 'check'),
      makeEdge('check-accept', 'check', 'accept', 'true'),
      makeEdge('check-reject', 'check', 'reject', 'false'),
    ];

    const { nodes: layoutedNodes } = layoutReactFlowElements(nodes, edges);

    expect(getNode(layoutedNodes, 'accept').position.y).toBeLessThan(
      getNode(layoutedNodes, 'check').position.y
    );
    expect(getNode(layoutedNodes, 'check').position.y).toBeLessThan(
      getNode(layoutedNodes, 'reject').position.y
    );
  });

  it('returns deterministic orthogonal edge routes for the renderer', () => {
    const nodes = [makeNode('a'), makeNode('b')];
    const edges = [makeEdge('a-b', 'a', 'b')];

    const result = layoutReactFlowElements(nodes, edges);
    const route = result.edgeRoutes?.['a-b'];

    expect(route?.points).toHaveLength(4);
    expect(route!.points[0].x).toBeLessThan(route!.points[3].x);
    expect(route?.labelPoint).toBeDefined();
  });

  it('keeps switch cases ordered and hides duplicate case edge labels', () => {
    const switchNode = makeNode('route', NODE_TYPES.SwitchNode, {
      inputMapping: [
        {
          type: 'cases',
          value: [
            {
              match: 'express',
              matchType: 'exact',
              output: {},
              route: 'express',
            },
            {
              match: 'economy',
              matchType: 'exact',
              output: {},
              route: 'economy',
            },
            { match: 'bulk', matchType: 'exact', output: {}, route: 'bulk' },
          ],
        },
      ],
    });
    const nodes = [
      switchNode,
      makeNode('express'),
      makeNode('economy'),
      makeNode('bulk'),
      makeNode('fallback'),
    ];
    const edges = [
      makeEdge('route-express', 'route', 'express', 'case-0'),
      makeEdge('route-economy', 'route', 'economy', 'case-1'),
      makeEdge('route-bulk', 'route', 'bulk', 'case-2'),
      makeEdge('route-default', 'route', 'fallback', 'default'),
    ];

    const graph = buildLayoutGraph(nodes, edges);
    const switchLayoutNode = graph.nodes.find((node) => node.id === 'route');
    const { nodes: layoutedNodes } = layoutReactFlowElements(nodes, edges);

    expect(
      switchLayoutNode?.ports.find((port) => port.id === 'case-0')?.label
    ).toBe('express');
    expect(getNode(layoutedNodes, 'express').position.y).toBeLessThan(
      getNode(layoutedNodes, 'economy').position.y
    );
    expect(getNode(layoutedNodes, 'economy').position.y).toBeLessThan(
      getNode(layoutedNodes, 'bulk').position.y
    );
    expect(shouldHideDuplicateEdgeLabel({ sourceHandle: 'case-0' })).toBe(true);
    expect(shouldHideDuplicateEdgeLabel({ sourceHandle: 'default' })).toBe(
      true
    );
  });
});
