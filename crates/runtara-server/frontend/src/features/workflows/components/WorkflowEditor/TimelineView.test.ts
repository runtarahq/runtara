import { describe, expect, it } from 'vitest';
import type { Edge, Node } from '@xyflow/react';

import {
  buildTimelineItems,
  canOfferTimelineJoin,
  createMcpToolsetAddRequest,
  createTimelineJoinRequest,
  getHiddenNodeIds,
  getTimelineJoinTargets,
  getTimelineRouteAddActions,
  isTimelineContainerNode,
} from './TimelineView';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { NODE_TYPES } from '@/features/workflows/config/workflow.ts';

function makeNode(
  id: string,
  stepType: string,
  type: string = NODE_TYPES.BasicNode,
  extra: Partial<Node> = {}
): Node {
  return {
    id,
    type,
    position: { x: 0, y: 0 },
    data: { id, name: id, stepType },
    ...extra,
  } as Node;
}

function makeAiAgentNode(id: string): Node {
  return makeNode(id, 'AiAgent', NODE_TYPES.AiAgentNode);
}

function makeEdge(
  source: string,
  target: string,
  sourceHandle: string,
  data?: Record<string, unknown>
): Edge {
  return {
    id: `${source}-${target}-${sourceHandle}`,
    source,
    target,
    sourceHandle,
    ...(data ? { data } : {}),
  } as Edge;
}

function makeItem(node: Node, outgoingEdges: Edge[] = []) {
  return { node, children: [], outgoingEdges, lanes: [] };
}

describe('getHiddenNodeIds', () => {
  it('hides AiAgent tool, memory and mcp.<toolset> targets but keeps onError targets visible', () => {
    const nodes = [
      makeAiAgentNode('ai'),
      makeNode('tool_step', 'Agent'),
      makeNode('memory_step', 'Agent'),
      makeNode('mcp_step', 'Agent'),
      makeNode('error_handler', 'Error'),
      makeNode('next_step', 'Agent'),
    ];
    const edges = [
      makeEdge('ai', 'tool_step', 'search_tool'),
      makeEdge('ai', 'memory_step', 'memory'),
      makeEdge('ai', 'mcp_step', 'mcp.linear'),
      makeEdge('ai', 'error_handler', 'onError'),
      makeEdge('ai', 'next_step', 'source'),
    ];

    const hidden = getHiddenNodeIds(nodes, edges);

    expect(hidden.has('tool_step')).toBe(true);
    expect(hidden.has('memory_step')).toBe(true);
    expect(hidden.has('mcp_step')).toBe(true);
    // The onError route is a normal timeline branch — its handler must
    // remain visible.
    expect(hidden.has('error_handler')).toBe(false);
    expect(hidden.has('next_step')).toBe(false);
  });

  it('does not hide targets of non-AiAgent sources', () => {
    const nodes = [
      makeNode('agent', 'Agent'),
      makeNode('error_handler', 'Error'),
    ];
    const edges = [makeEdge('agent', 'error_handler', 'onError')];

    expect(getHiddenNodeIds(nodes, edges).size).toBe(0);
  });
});

describe('getTimelineRouteAddActions — error handler coverage', () => {
  it('offers the error route on every Agent step (no knownErrors gate)', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('agent', 'Agent'))
    );
    const errorAction = actions.find((action) => action.key === 'onError');

    expect(errorAction).toBeDefined();
    expect(errorAction!.request.sourceHandle).toBe('onError');
    expect(errorAction!.request.directStep?.stepType).toBe('Error');
  });

  it('offers the error route on AiAgent steps alongside tool/memory actions', () => {
    const actions = getTimelineRouteAddActions(makeItem(makeAiAgentNode('ai')));
    const keys = actions.map((action) => action.key);

    expect(keys).toContain('onError');
    expect(keys).toContain('ai-tool');
    expect(keys).toContain('ai-memory');

    const errorAction = actions.find((action) => action.key === 'onError')!;
    expect(errorAction.request.sourceNodeId).toBe('ai');
    expect(errorAction.request.sourceHandle).toBe('onError');
    expect(errorAction.request.directStep?.stepType).toBe('Error');
  });

  it('offers the error route on the full compiler-supported step set', () => {
    for (const stepType of [
      'Agent',
      'AiAgent',
      'EmbedWorkflow',
      'Split',
      'While',
      'WaitForSignal',
    ]) {
      const nodeType =
        stepType === 'AiAgent' ? NODE_TYPES.AiAgentNode : NODE_TYPES.BasicNode;
      const actions = getTimelineRouteAddActions(
        makeItem(makeNode('step', stepType, nodeType))
      );
      expect(
        actions.some((action) => action.key === 'onError'),
        `expected onError action for ${stepType}`
      ).toBe(true);
    }
  });

  it('does not offer the error route on step types the compiler rejects', () => {
    for (const stepType of [
      'Delay',
      'Log',
      'Filter',
      'GroupBy',
      'Conditional',
      'Switch',
      'Finish',
      'Error',
    ]) {
      const actions = getTimelineRouteAddActions(
        makeItem(makeNode('step', stepType))
      );
      expect(
        actions.some((action) => action.key === 'onError'),
        `expected no onError action for ${stepType}`
      ).toBe(false);
    }
  });

  it('does not offer a second error route when one already exists', () => {
    const node = makeAiAgentNode('ai');
    const actions = getTimelineRouteAddActions(
      makeItem(node, [makeEdge('ai', 'handler', 'onError')])
    );

    expect(actions.some((action) => action.key === 'onError')).toBe(false);
    // Tool/memory affordances are unaffected by the error route.
    expect(actions.some((action) => action.key === 'ai-tool')).toBe(true);
  });
});

describe('getTimelineRouteAddActions — parallel branch', () => {
  function findParallel(
    actions: ReturnType<typeof getTimelineRouteAddActions>
  ) {
    return actions.find((action) => action.key === 'parallel-branch');
  }

  it('offers the parallel branch exactly on a single unconditional outgoing edge', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('a', 'Agent'), [makeEdge('a', 'b', 'source')])
    );
    const parallel = findParallel(actions);

    expect(parallel).toBeDefined();
    // The request carries both edge endpoints so the editor can build the
    // E073-compliant diamond: S->N plus N->T while keeping S->T.
    expect(parallel!.request).toMatchObject({
      sourceNodeId: 'a',
      sourceHandle: 'source',
      targetNodeId: 'b',
      parallelBranch: true,
    });
  });

  it('still offers the parallel branch when an onError route exists alongside', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('a', 'Agent'), [
        makeEdge('a', 'b', 'source'),
        makeEdge('a', 'handler', 'onError'),
      ])
    );

    expect(findParallel(actions)).toBeDefined();
  });

  it('does not offer the parallel branch without an outgoing edge', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('a', 'Agent'))
    );

    expect(findParallel(actions)).toBeUndefined();
  });

  it('does not offer the parallel branch when the step already fans out', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('a', 'Agent'), [
        makeEdge('a', 'b', 'source'),
        makeEdge('a', 'c', 'source'),
      ])
    );

    expect(findParallel(actions)).toBeUndefined();
  });

  it('does not offer the parallel branch on a conditional (condition-carrying) edge', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('a', 'Agent'), [
        makeEdge('a', 'b', 'source', {
          condition: { type: 'operation', op: 'EQ', arguments: [] },
        }),
      ])
    );

    expect(findParallel(actions)).toBeUndefined();
  });

  it('does not offer the parallel branch on Conditional true/false routes', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('cond', 'Conditional', NODE_TYPES.ConditionalNode), [
        makeEdge('cond', 'a', 'true'),
        makeEdge('cond', 'b', 'false'),
      ])
    );

    expect(findParallel(actions)).toBeUndefined();
  });
});

describe('timeline join (connect to existing step)', () => {
  it('is offered only on steps without an unconditional continuation', () => {
    expect(canOfferTimelineJoin(makeItem(makeNode('a', 'Agent')))).toBe(true);
    expect(
      canOfferTimelineJoin(
        makeItem(makeNode('a', 'Agent'), [makeEdge('a', 'h', 'onError')])
      )
    ).toBe(true);
    expect(
      canOfferTimelineJoin(
        makeItem(makeNode('a', 'Agent'), [makeEdge('a', 'b', 'source')])
      )
    ).toBe(false);
    // Terminal / inherently-labeled steps never get the unconditional join.
    expect(canOfferTimelineJoin(makeItem(makeNode('f', 'Finish')))).toBe(false);
    expect(
      canOfferTimelineJoin(
        makeItem(makeNode('c', 'Conditional', NODE_TYPES.ConditionalNode))
      )
    ).toBe(false);
  });

  it('lists same-scope targets and excludes self, upstream (cycle) and connected steps', () => {
    // root -> cond; cond -true-> a; cond -false-> b; b -> shared
    const nodes = [
      makeNode('root', 'Agent'),
      makeNode('cond', 'Conditional', NODE_TYPES.ConditionalNode),
      makeNode('a', 'Agent'),
      makeNode('b', 'Agent'),
      makeNode('shared', 'Agent'),
      makeNode('child', 'Agent', NODE_TYPES.BasicNode, {
        parentId: 'split-1',
      }),
    ];
    const edges = [
      makeEdge('root', 'cond', 'source'),
      makeEdge('cond', 'a', 'true'),
      makeEdge('cond', 'b', 'false'),
      makeEdge('b', 'shared', 'source'),
    ];

    const targetIds = getTimelineJoinTargets(
      nodes.find((node) => node.id === 'a')!,
      nodes,
      edges
    ).map((node) => node.id);

    // Valid: the sibling lane and the shared continuation.
    expect(targetIds).toContain('b');
    expect(targetIds).toContain('shared');
    // Excluded: self, upstream steps (would create a cycle) and other scopes.
    expect(targetIds).not.toContain('a');
    expect(targetIds).not.toContain('root');
    expect(targetIds).not.toContain('cond');
    expect(targetIds).not.toContain('child');
  });

  it('excludes targets the step already routes to and hidden AiAgent attachments', () => {
    const nodes = [
      makeNode('a', 'Agent'),
      makeNode('done', 'Finish'),
      makeAiAgentNode('ai'),
      makeNode('tool_step', 'Agent'),
    ];
    const edges = [
      makeEdge('a', 'done', 'source'),
      makeEdge('ai', 'tool_step', 'search_tool'),
    ];

    const targetIds = getTimelineJoinTargets(
      nodes.find((node) => node.id === 'a')!,
      nodes,
      edges
    ).map((node) => node.id);

    expect(targetIds).not.toContain('done');
    expect(targetIds).not.toContain('tool_step');
    expect(targetIds).toContain('ai');
  });

  it('produces an edge-only request with the unconditional source handle', () => {
    const request = createTimelineJoinRequest(
      makeNode('a', 'Agent'),
      makeNode('shared', 'Agent')
    );

    expect(request).toEqual({
      sourceNodeId: 'a',
      targetNodeId: 'shared',
      sourceHandle: 'source',
    });
  });
});

describe('timeline route deletion', () => {
  it('removes the edge through the store remove path the canvas uses', () => {
    const nodes = [
      makeNode('a', 'Agent'),
      makeNode('b', 'Agent'),
      makeNode('c', 'Agent'),
    ];
    const edges = [makeEdge('a', 'b', 'source'), makeEdge('b', 'c', 'source')];
    useWorkflowStore.setState({ nodes, edges, isDirty: false });

    useWorkflowStore
      .getState()
      .onEdgesChange([{ id: 'a-b-source', type: 'remove' }]);

    const state = useWorkflowStore.getState();
    expect(state.edges.map((edge) => edge.id)).toEqual(['b-c-source']);
    expect(state.isDirty).toBe(true);
  });

  it('keeps rendering the timeline when deletion orphans a branch', () => {
    // After deleting a->b, the b->c branch dangles: the validator flags it,
    // the timeline must still render every step as a root item.
    const nodes = [
      makeNode('a', 'Agent'),
      makeNode('b', 'Agent'),
      makeNode('c', 'Agent'),
    ];
    const edges = [makeEdge('b', 'c', 'source')];

    const items = buildTimelineItems(nodes, edges, undefined, new Set());
    const renderedIds = items.map((item) => item.node.id);

    expect(renderedIds).toContain('a');
    expect(renderedIds).toContain('b');
    expect(renderedIds).toContain('c');
  });
});

describe('WaitForSignal on-wait flow affordance', () => {
  function findOnWait(actions: ReturnType<typeof getTimelineRouteAddActions>) {
    return actions.find((action) => action.key === 'on-wait');
  }

  it('offers the on-wait flow action on a WaitForSignal step without a container', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('wait', 'WaitForSignal'))
    );
    const onWait = findOnWait(actions);

    expect(onWait).toBeDefined();
    // The request is the regular empty-scope insertion (first child of the
    // freshly converted container); the conversion happens at click time.
    expect(onWait!.request).toEqual({ parentId: 'wait' });
    expect(onWait!.convertsToContainer).toBe(true);
  });

  it('does not offer the on-wait flow action once the step is a container', () => {
    const actions = getTimelineRouteAddActions(
      makeItem(makeNode('wait', 'WaitForSignal', NODE_TYPES.ContainerNode))
    );

    expect(findOnWait(actions)).toBeUndefined();
  });

  it('does not offer the on-wait flow action when the step already has children', () => {
    const item = {
      ...makeItem(makeNode('wait', 'WaitForSignal')),
      children: [
        makeItem(
          makeNode('child', 'Log', NODE_TYPES.BasicNode, { parentId: 'wait' })
        ),
      ],
    };

    expect(findOnWait(getTimelineRouteAddActions(item))).toBeUndefined();
  });

  it('does not offer the on-wait flow action on other step types', () => {
    for (const stepType of ['Agent', 'Split', 'Delay', 'Finish']) {
      const actions = getTimelineRouteAddActions(
        makeItem(makeNode('step', stepType))
      );
      expect(
        findOnWait(actions),
        `expected no on-wait action for ${stepType}`
      ).toBeUndefined();
    }
  });
});

describe('isTimelineContainerNode', () => {
  it('treats Split/While scope step types as containers regardless of node type', () => {
    expect(isTimelineContainerNode(makeNode('s', 'Split'))).toBe(true);
    expect(isTimelineContainerNode(makeNode('w', 'While'))).toBe(true);
  });

  it('is node-data-aware for WaitForSignal: container exactly when the node type is the container node', () => {
    expect(isTimelineContainerNode(makeNode('wait', 'WaitForSignal'))).toBe(
      false
    );
    expect(
      isTimelineContainerNode(
        makeNode('wait', 'WaitForSignal', NODE_TYPES.ContainerNode)
      )
    ).toBe(true);
  });

  it('does not flag plain steps as containers', () => {
    expect(isTimelineContainerNode(makeNode('a', 'Agent'))).toBe(false);
  });
});

describe('createMcpToolsetAddRequest', () => {
  it('creates an mcp.<toolset> edge request targeting a new mcp Agent step', () => {
    const request = createMcpToolsetAddRequest(makeAiAgentNode('ai'), 'linear');

    expect(request.sourceNodeId).toBe('ai');
    expect(request.sourceHandle).toBe('mcp.linear');
    // The validator requires the target to be an Agent step with
    // agentId === 'mcp' (validation.rs MCP edge rules).
    expect(request.directStep).toMatchObject({
      stepType: 'Agent',
      agentId: 'mcp',
    });
  });

  it('propagates the source node parent scope', () => {
    const node = {
      ...makeAiAgentNode('ai'),
      parentId: 'split-1',
    } as Node;

    expect(createMcpToolsetAddRequest(node, 'slack').parentId).toBe('split-1');
  });
});
