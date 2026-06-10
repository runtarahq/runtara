import { describe, expect, it } from 'vitest';
import type { Edge, Node } from '@xyflow/react';

import {
  createMcpToolsetAddRequest,
  getHiddenNodeIds,
  getTimelineRouteAddActions,
} from './TimelineView';
import { NODE_TYPES } from '@/features/workflows/config/workflow.ts';

function makeNode(
  id: string,
  stepType: string,
  type: string = NODE_TYPES.BasicNode
): Node {
  return {
    id,
    type,
    position: { x: 0, y: 0 },
    data: { id, name: id, stepType },
  } as Node;
}

function makeAiAgentNode(id: string): Node {
  return makeNode(id, 'AiAgent', NODE_TYPES.AiAgentNode);
}

function makeEdge(source: string, target: string, sourceHandle: string): Edge {
  return {
    id: `${source}-${target}-${sourceHandle}`,
    source,
    target,
    sourceHandle,
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
