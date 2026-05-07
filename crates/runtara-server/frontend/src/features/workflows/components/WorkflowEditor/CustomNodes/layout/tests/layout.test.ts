import { describe, expect, it } from 'vitest';
import type { Edge, Node } from '@xyflow/react';
import {
  buildLayoutGraph,
  layoutReactFlowElements,
  rankScope,
  routeOrthogonalEdges,
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

function segmentIntersectsNode(
  start: { x: number; y: number },
  end: { x: number; y: number },
  node: Node
): boolean {
  const left = node.position.x;
  const top = node.position.y;
  const right = left + ((node.style?.width as number) ?? node.width ?? 0);
  const bottom = top + ((node.style?.height as number) ?? node.height ?? 0);

  if (start.x === end.x) {
    const segmentTop = Math.min(start.y, end.y);
    const segmentBottom = Math.max(start.y, end.y);
    return (
      start.x > left &&
      start.x < right &&
      segmentBottom > top &&
      segmentTop < bottom
    );
  }

  if (start.y === end.y) {
    const segmentLeft = Math.min(start.x, end.x);
    const segmentRight = Math.max(start.x, end.x);
    return (
      start.y > top &&
      start.y < bottom &&
      segmentRight > left &&
      segmentLeft < right
    );
  }

  return false;
}

function getVerticalSegmentXs(
  points: Array<{ x: number; y: number }>
): number[] {
  const xs: number[] = [];

  for (let index = 1; index < points.length; index++) {
    const previous = points[index - 1];
    const current = points[index];
    if (previous.x === current.x && previous.y !== current.y) {
      xs.push(previous.x);
    }
  }

  return xs;
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

    expect(route?.points.length).toBeGreaterThanOrEqual(2);
    expect(route!.points[0].x).toBeLessThan(
      route!.points[route!.points.length - 1].x
    );
    expect(route?.labelPoint).toBeDefined();
  });

  it('routes around occupied node boxes instead of through them', () => {
    const source = makeNode('source');
    source.position = { x: 0, y: 0 };
    source.style = { width: 96, height: 36 };
    source.width = 96;
    source.height = 36;

    const blocker = makeNode('blocker');
    blocker.position = { x: 150, y: -24 };
    blocker.style = { width: 84, height: 84 };
    blocker.width = 84;
    blocker.height = 84;

    const target = makeNode('target');
    target.position = { x: 320, y: 0 };
    target.style = { width: 96, height: 36 };
    target.width = 96;
    target.height = 36;

    const route = routeOrthogonalEdges(
      [source, blocker, target],
      [makeEdge('source-target', 'source', 'target')]
    )['source-target'];

    for (let index = 1; index < route.points.length; index++) {
      expect(
        segmentIntersectsNode(
          route.points[index - 1],
          route.points[index],
          blocker
        )
      ).toBe(false);
    }
  });

  it('routes conditional true and false edges from distinct handles', () => {
    const source = makeNode('source', NODE_TYPES.ConditionalNode);
    source.position = { x: 0, y: 0 };
    source.style = { width: 132, height: 36 };
    source.width = 132;
    source.height = 36;

    const trueTarget = makeNode('true-target');
    trueTarget.position = { x: 300, y: -80 };
    trueTarget.style = { width: 132, height: 36 };
    trueTarget.width = 132;
    trueTarget.height = 36;

    const falseTarget = makeNode('false-target');
    falseTarget.position = { x: 300, y: 80 };
    falseTarget.style = { width: 132, height: 36 };
    falseTarget.width = 132;
    falseTarget.height = 36;

    const routes = routeOrthogonalEdges(
      [source, trueTarget, falseTarget],
      [
        makeEdge('source-true', 'source', 'true-target', 'true'),
        makeEdge('source-false', 'source', 'false-target', 'false'),
      ]
    );

    expect(routes['source-true'].points[0].y).toBeLessThan(
      routes['source-false'].points[0].y
    );
    expect(
      routes['source-false'].points[0].y - routes['source-true'].points[0].y
    ).toBeGreaterThan(8);
  });

  it('uses the middle corridor for branch-stack merge routes', () => {
    const source = makeNode('source');
    source.position = { x: 0, y: 0 };
    source.style = { width: 96, height: 36 };
    source.width = 96;
    source.height = 36;

    const sibling = makeNode('sibling');
    sibling.position = { x: 0, y: 80 };
    sibling.style = { width: 96, height: 36 };
    sibling.width = 96;
    sibling.height = 36;

    const target = makeNode('target');
    target.position = { x: 320, y: 120 };
    target.style = { width: 96, height: 36 };
    target.width = 96;
    target.height = 36;

    const route = routeOrthogonalEdges(
      [source, sibling, target],
      [makeEdge('source-target', 'source', 'target')]
    )['source-target'];

    const verticalSegmentXs = getVerticalSegmentXs(route.points);
    expect(verticalSegmentXs.some((x) => x > 250 && x < target.position.x)).toBe(
      true
    );
  });

  it('uses a shared target-side bus for multi-incoming merges', () => {
    const topSource = makeNode('top-source');
    topSource.position = { x: 0, y: -80 };
    topSource.style = { width: 96, height: 36 };
    topSource.width = 96;
    topSource.height = 36;

    const bottomSource = makeNode('bottom-source');
    bottomSource.position = { x: 0, y: 80 };
    bottomSource.style = { width: 96, height: 36 };
    bottomSource.width = 96;
    bottomSource.height = 36;

    const target = makeNode('target');
    target.position = { x: 320, y: 0 };
    target.style = { width: 96, height: 36 };
    target.width = 96;
    target.height = 36;

    const routes = routeOrthogonalEdges(
      [topSource, bottomSource, target],
      [
        makeEdge('top-target', 'top-source', 'target'),
        makeEdge('bottom-target', 'bottom-source', 'target'),
      ]
    );

    const topBusXs = getVerticalSegmentXs(routes['top-target'].points);
    const bottomBusXs = getVerticalSegmentXs(routes['bottom-target'].points);

    expect(topBusXs).toContain(284);
    expect(bottomBusXs).toContain(284);
  });

  it('gives long multi-incoming merge edges a bypass before the merge bus', () => {
    const farSource = makeNode('far-source');
    farSource.position = { x: 0, y: 0 };
    farSource.style = { width: 96, height: 36 };
    farSource.width = 96;
    farSource.height = 36;

    const nearSource = makeNode('near-source');
    nearSource.position = { x: 560, y: 120 };
    nearSource.style = { width: 96, height: 36 };
    nearSource.width = 96;
    nearSource.height = 36;

    const target = makeNode('target');
    target.position = { x: 760, y: 80 };
    target.style = { width: 96, height: 36 };
    target.width = 96;
    target.height = 36;

    const routes = routeOrthogonalEdges(
      [farSource, nearSource, target],
      [
        makeEdge('far-target', 'far-source', 'target'),
        makeEdge('near-target', 'near-source', 'target'),
      ]
    );

    expect(
      routes['far-target'].points.some(
        (point) => point.y < routes['far-target'].points[0].y
      )
    ).toBe(true);
    expect(getVerticalSegmentXs(routes['near-target'].points)).toContain(724);
  });

  it('keeps same-container long bypasses inside the container gutter', () => {
    const container = makeNode('container', NODE_TYPES.ContainerNode);
    container.position = { x: 100, y: 100 };
    container.style = { width: 900, height: 300 };
    container.width = 900;
    container.height = 300;

    const source = makeNode('source');
    source.parentId = 'container';
    source.position = { x: 40, y: 120 };
    source.style = { width: 96, height: 36 };
    source.width = 96;
    source.height = 36;

    const target = makeNode('target');
    target.parentId = 'container';
    target.position = { x: 760, y: 160 };
    target.style = { width: 96, height: 36 };
    target.width = 96;
    target.height = 36;

    const route = routeOrthogonalEdges(
      [container, source, target],
      [makeEdge('source-target', 'source', 'target')]
    )['source-target'];

    expect(route.points.every((point) => point.y >= container.position.y)).toBe(
      true
    );
    expect(
      route.points.every(
        (point) => point.y <= container.position.y + (container.height ?? 0)
      )
    ).toBe(true);
  });

  it('routes long same-row edges through a bypass lane', () => {
    const source = makeNode('source');
    source.position = { x: 0, y: 0 };
    source.style = { width: 96, height: 36 };
    source.width = 96;
    source.height = 36;

    const target = makeNode('target');
    target.position = { x: 760, y: 0 };
    target.style = { width: 96, height: 36 };
    target.width = 96;
    target.height = 36;

    const route = routeOrthogonalEdges(
      [source, target],
      [makeEdge('source-target', 'source', 'target')]
    )['source-target'];

    expect(route.points.length).toBeGreaterThan(2);
    expect(route.points.some((point) => point.y !== route.points[0].y)).toBe(
      true
    );
  });

  it('routes long sequence bypasses away from their branch side', () => {
    const upperSource = makeNode('upper-source');
    upperSource.position = { x: 0, y: 0 };
    upperSource.style = { width: 96, height: 36 };
    upperSource.width = 96;
    upperSource.height = 36;

    const lowerSource = makeNode('lower-source');
    lowerSource.position = { x: 0, y: 160 };
    lowerSource.style = { width: 96, height: 36 };
    lowerSource.width = 96;
    lowerSource.height = 36;

    const upperTarget = makeNode('upper-target-node');
    upperTarget.position = { x: 760, y: 80 };
    upperTarget.style = { width: 96, height: 36 };
    upperTarget.width = 96;
    upperTarget.height = 36;

    const upperRoutes = routeOrthogonalEdges(
      [upperSource, upperTarget],
      [makeEdge('upper-target', 'upper-source', 'upper-target-node')]
    );

    expect(
      upperRoutes['upper-target'].points.some(
        (point) => point.y < upperRoutes['upper-target'].points[0].y
      )
    ).toBe(true);

    const lowerRoutes = routeOrthogonalEdges(
      [lowerSource, upperTarget],
      [makeEdge('lower-target', 'lower-source', 'upper-target-node')]
    );

    expect(
      lowerRoutes['lower-target'].points.some(
        (point) => point.y > lowerRoutes['lower-target'].points[0].y
      )
    ).toBe(true);
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

  it('spaces switch case route anchors by the rendered handle rhythm', () => {
    const switchNode = makeNode('route', NODE_TYPES.SwitchNode, {
      inputMapping: [
        {
          type: 'cases',
          value: [
            { match: 'express', matchType: 'exact', output: {}, route: 'a' },
            { match: 'economy', matchType: 'exact', output: {}, route: 'b' },
          ],
        },
      ],
    });
    switchNode.position = { x: 0, y: 0 };
    switchNode.style = { width: 132, height: 96 };
    switchNode.width = 132;
    switchNode.height = 96;

    const firstTarget = makeNode('first');
    firstTarget.position = { x: 300, y: -80 };
    const secondTarget = makeNode('second');
    secondTarget.position = { x: 300, y: 80 };

    const routes = routeOrthogonalEdges(
      [switchNode, firstTarget, secondTarget],
      [
        makeEdge('route-first', 'route', 'first', 'case-0'),
        makeEdge('route-second', 'route', 'second', 'case-1'),
      ]
    );

    expect(
      routes['route-second'].points[0].y - routes['route-first'].points[0].y
    ).toBe(24);
  });
});
