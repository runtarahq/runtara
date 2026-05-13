import { describe, it, expect } from 'vitest';
import type { Edge, Node } from '@xyflow/react';
import {
  composeExecutionGraph,
  executionGraphToReactFlow,
  getLayoutedElements,
} from './utils.tsx';
import type { ExecutionGraphDto } from '@/features/workflows/types/execution-graph';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
} from '@/features/workflows/config/workflow.ts';

/**
 * Round-trip tests for the workflow editor's save/load conversion.
 *
 * These tests use the REAL `composeExecutionGraph` / `executionGraphToReactFlow`
 * functions so that drift between load and save is caught. An earlier bug report
 * ("auto-layout + save mutates reference types") was caused by asymmetric conversion
 * paths; the tests below lock the symmetry in place.
 *
 * Invariant: `compose(reactFlow(graph)) ≈ graph` (modulo UI-synthesized width/height
 * inside `renderingParameters`, which the editor fills in from measured React Flow
 * dimensions).
 */

type StepWithId = {
  id: string;
  stepType: string;
  renderingParameters?: { x?: number; y?: number };
  [key: string]: unknown;
};

/** Build a minimal single-step execution graph for round-trip tests. */
function makeGraph(
  step: StepWithId
): ExecutionGraphDto & { entryPoint: string } {
  return {
    name: 'round-trip-fixture',
    steps: { [step.id]: step as any },
    executionPlan: [],
    entryPoint: step.id,
  };
}

/** Run graph through load→save and return the resulting graph's single step. */
function roundTripStep(graph: ExecutionGraphDto & { entryPoint: string }) {
  const { nodes, edges } = executionGraphToReactFlow(graph as any);
  const round = composeExecutionGraph(nodes, edges, { name: graph.name });
  expect(round).not.toBeNull();
  const stepId = graph.entryPoint;
  return (round!.steps as Record<string, any>)[stepId];
}

function makeLayoutNode(
  id: string,
  type = NODE_TYPES.BasicNode,
  parentId?: string
): Node {
  const size = NODE_TYPE_SIZES[type] ?? NODE_TYPE_SIZES[NODE_TYPES.BasicNode];
  const stepType =
    type === NODE_TYPES.ConditionalNode
      ? 'Conditional'
      : type === NODE_TYPES.ContainerNode
        ? 'Split'
        : 'Agent';

  return {
    id,
    type,
    parentId,
    position: { x: 0, y: 0 },
    data: {
      id,
      name: id,
      stepType,
    },
    width: size.width,
    height: size.height,
    style: {
      width: size.width,
      height: size.height,
    },
  } as Node;
}

function makeLayoutEdge(
  id: string,
  source: string,
  target: string,
  sourceHandle = 'source'
): Edge {
  return {
    id,
    source,
    target,
    sourceHandle,
  } as Edge;
}

function getLayoutNode(nodes: Node[], id: string): Node {
  const node = nodes.find((item) => item.id === id);
  expect(node).toBeDefined();
  return node!;
}

function getLayoutSize(node: Node): { width: number; height: number } {
  return {
    width:
      (node.style?.width as number | undefined) ??
      (node.width as number | undefined) ??
      NODE_TYPE_SIZES[NODE_TYPES.BasicNode].width,
    height:
      (node.style?.height as number | undefined) ??
      (node.height as number | undefined) ??
      NODE_TYPE_SIZES[NODE_TYPES.BasicNode].height,
  };
}

function nodesOverlap(a: Node, b: Node): boolean {
  const aSize = getLayoutSize(a);
  const bSize = getLayoutSize(b);

  return (
    a.position.x < b.position.x + bSize.width &&
    a.position.x + aSize.width > b.position.x &&
    a.position.y < b.position.y + bSize.height &&
    a.position.y + aSize.height > b.position.y
  );
}

function expectNoSiblingOverlaps(nodes: Node[]): void {
  for (let i = 0; i < nodes.length; i++) {
    for (let j = i + 1; j < nodes.length; j++) {
      const a = nodes[i];
      const b = nodes[j];
      if ((a.parentId ?? 'root') !== (b.parentId ?? 'root')) continue;

      expect(nodesOverlap(a, b), `${a.id} overlaps ${b.id}`).toBe(false);
    }
  }
}

describe('MappingValue round-trip', () => {
  it('preserves `type` on top-level reference', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        count: {
          valueType: 'reference',
          value: 'steps.prev.outputs.count',
          type: 'integer',
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.count).toEqual({
      valueType: 'reference',
      value: 'steps.prev.outputs.count',
      type: 'integer',
    });
  });

  it('preserves `default` on top-level reference', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        limit: {
          valueType: 'reference',
          value: 'data.limit',
          type: 'integer',
          default: 10,
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.limit).toEqual({
      valueType: 'reference',
      value: 'data.limit',
      type: 'integer',
      default: 10,
    });
  });

  it('preserves `default` (object) on top-level reference', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        settings: {
          valueType: 'reference',
          value: 'data.settings',
          default: { foo: 1, bar: 'baz' },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.settings).toEqual({
      valueType: 'reference',
      value: 'data.settings',
      default: { foo: 1, bar: 'baz' },
    });
  });

  it('preserves `type` on reference inside composite object', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        payload: {
          valueType: 'composite',
          value: {
            userId: {
              valueType: 'reference',
              value: 'steps.api.outputs.user.id',
              type: 'integer',
            },
            name: {
              valueType: 'immediate',
              value: 'Alice',
              type: 'string',
            },
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.payload).toEqual({
      valueType: 'composite',
      value: {
        userId: {
          valueType: 'reference',
          value: 'steps.api.outputs.user.id',
          type: 'integer',
        },
        name: {
          valueType: 'immediate',
          value: 'Alice',
        },
      },
    });
  });

  it('preserves `default` on reference inside composite object', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        payload: {
          valueType: 'composite',
          value: {
            limit: {
              valueType: 'reference',
              value: 'data.limit',
              type: 'integer',
              default: 25,
            },
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.payload.value.limit).toEqual({
      valueType: 'reference',
      value: 'data.limit',
      type: 'integer',
      default: 25,
    });
  });

  it('preserves `type` on reference inside composite array', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        items: {
          valueType: 'composite',
          value: [
            {
              valueType: 'reference',
              value: 'data.first',
              type: 'integer',
            },
            {
              valueType: 'immediate',
              value: 42,
              type: 'integer',
            },
          ],
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.items.value).toEqual([
      {
        valueType: 'reference',
        value: 'data.first',
        type: 'integer',
      },
      {
        valueType: 'immediate',
        value: 42,
      },
    ]);
  });

  it('preserves template valueType', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        greeting: {
          valueType: 'template',
          value: 'Hello {{ data.name }}',
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.greeting).toEqual({
      valueType: 'template',
      value: 'Hello {{ data.name }}',
    });
  });

  it('does not emit backend type hints on immediate values', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        timeout: {
          valueType: 'immediate',
          value: 5000,
          type: 'integer',
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.timeout).toEqual({
      valueType: 'immediate',
      value: 5000,
    });
  });

  it('preserves immediate object without type', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'http-request',
      inputMapping: {
        headers: {
          valueType: 'immediate',
          value: { 'X-API-Key': 'abc' },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.headers).toEqual({
      valueType: 'immediate',
      value: { 'X-API-Key': 'abc' },
    });
  });
});

/**
 * Regression: when a user types `"5"` in an integer field and saves, the save path
 * should coerce it to the number `5`. These tests run the SAVE path directly
 * (bypassing load) with form-shaped input to lock that coercion in place.
 */
describe('Form-input coercion on save', () => {
  function saveWithInput(field: {
    type: string;
    value: unknown;
    typeHint?: string;
    valueType?: string;
  }) {
    const graph = {
      name: 'coercion-fixture',
      steps: {
        s1: {
          id: 's1',
          stepType: 'Agent',
          agentId: 'x',
          capabilityId: 'y',
          inputMapping: {
            // placeholder — we replace via load, then override in UI form
          },
          renderingParameters: { x: 0, y: 0 },
        },
      },
      executionPlan: [],
      entryPoint: 's1',
    };
    const { nodes, edges } = executionGraphToReactFlow(graph as any);
    // Simulate a user-typed form entry
    (nodes[0].data as any).inputMapping = [field];
    const round = composeExecutionGraph(nodes, edges, { name: graph.name });
    return (round!.steps as Record<string, any>).s1.inputMapping[field.type];
  }

  it('coerces string "5" to number 5 when typeHint is integer', () => {
    const out = saveWithInput({
      type: 'count',
      value: '5',
      typeHint: 'integer',
      valueType: 'immediate',
    });
    expect(out.value).toBe(5);
    expect(out).not.toHaveProperty('type');
  });

  it('coerces string "true" to boolean true when typeHint is boolean', () => {
    const out = saveWithInput({
      type: 'enabled',
      value: 'true',
      typeHint: 'boolean',
      valueType: 'immediate',
    });
    expect(out.value).toBe(true);
  });

  it('parses JSON string only when typeHint is explicitly json', () => {
    const withJson = saveWithInput({
      type: 'data',
      value: '{"k":"v"}',
      typeHint: 'json',
      valueType: 'immediate',
    });
    expect(withJson.value).toEqual({ k: 'v' });

    const withoutJson = saveWithInput({
      type: 'text',
      value: '{"k":"v"}',
      typeHint: undefined,
      valueType: 'immediate',
    });
    expect(withoutJson.value).toBe('{"k":"v"}');
  });

  it('preserves reference path string even with integer typeHint', () => {
    const out = saveWithInput({
      type: 'count',
      value: 'data.count',
      typeHint: 'integer',
      valueType: 'reference',
    });
    expect(out.value).toBe('data.count');
    expect(out.type).toBe('integer');
  });
});

describe('Backend DSL serialization', () => {
  it('does not emit backend type hints for immediate Finish outputs', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'finish',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'finish',
            stepType: 'Finish',
            name: 'Finish',
            inputMapping: [
              {
                type: 'status',
                value: 'ok',
                typeHint: 'string',
                valueType: 'immediate',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'finish-output-fixture' }
    );

    const output = (graph!.steps as Record<string, any>).finish.inputMapping
      .status;
    expect(output).toEqual({
      valueType: 'immediate',
      value: 'ok',
    });
  });

  it('preserves literal null Finish outputs', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'finish',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'finish',
            stepType: 'Finish',
            name: 'Finish',
            inputMapping: [
              {
                type: 'optionalPayload',
                value: null,
                typeHint: 'json',
                valueType: 'immediate',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'finish-output-null-fixture' }
    );

    const output = (graph!.steps as Record<string, any>).finish.inputMapping
      .optionalPayload;
    expect(output).toEqual({
      valueType: 'immediate',
      value: null,
    });
  });

  it('preserves constructed array Finish outputs', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'finish',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'finish',
            stepType: 'Finish',
            name: 'Finish',
            inputMapping: [
              {
                type: 'items',
                value: [
                  { valueType: 'immediate', value: 'created' },
                  {
                    valueType: 'reference',
                    value: "steps['fetch'].outputs.item",
                  },
                ],
                typeHint: 'array',
                valueType: 'composite',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'finish-output-array-fixture' }
    );

    const output = (graph!.steps as Record<string, any>).finish.inputMapping
      .items;
    expect(output).toEqual({
      valueType: 'composite',
      value: [
        { valueType: 'immediate', value: 'created' },
        {
          valueType: 'reference',
          value: "steps['fetch'].outputs.item",
        },
      ],
    });
  });

  it('does not leak editor-only form defaults into Agent steps', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'random',
          type: 'basic',
          position: { x: 24, y: 48 },
          data: {
            id: 'random',
            stepType: 'Agent',
            name: 'Random double',
            agentId: 'utils',
            capabilityId: 'random-double',
            inputMapping: [],
            childWorkflowId: '',
            childVersion: 'latest',
            embedWorkflowConfig: undefined,
            executionTimeout: 120,
            retryStrategy: 'Linear',
            groupByKey: '',
            groupByExpectedKeys: [],
            splitVariablesFields: [],
            splitParallelism: 0,
            splitSequential: false,
            splitDontStopOnFailed: false,
            selectedTriggerId: '',
          },
        },
      ] as any,
      [],
      { name: 'random-workflow' }
    );

    const step = (graph!.steps as Record<string, any>).random;
    expect(step).toMatchObject({
      id: 'random',
      stepType: 'Agent',
      name: 'Random double',
      agentId: 'utils',
      capabilityId: 'random-double',
    });
    expect(step).not.toHaveProperty('childWorkflowId');
    expect(step).not.toHaveProperty('childVersion');
    expect(step).not.toHaveProperty('embedWorkflowConfig');
    expect(step).not.toHaveProperty('executionTimeout');
    expect(step).not.toHaveProperty('retryStrategy');
    expect(step).not.toHaveProperty('groupByKey');
    expect(step).not.toHaveProperty('groupByExpectedKeys');
    expect(step).not.toHaveProperty('renderingParameters');
    expect((graph as any).nodes[0].position).toEqual({ x: 24, y: 48 });
  });

  it('does not preserve stale direct Error fields after form values are cleared', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'error',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'error',
            stepType: 'Error',
            name: 'Validation Target',
            code: 'PREVIOUS_CODE',
            message: 'Previous message',
            category: 'permanent',
            severity: 'error',
            inputMapping: [
              {
                type: 'code',
                value: '',
                typeHint: 'string',
                valueType: 'immediate',
              },
              {
                type: 'message',
                value: '',
                typeHint: 'string',
                valueType: 'immediate',
              },
              {
                type: 'category',
                value: 'permanent',
                typeHint: 'string',
                valueType: 'immediate',
              },
              {
                type: 'severity',
                value: 'error',
                typeHint: 'string',
                valueType: 'immediate',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'error-workflow' }
    );

    const step = (graph!.steps as Record<string, any>).error;
    expect(step).not.toHaveProperty('code');
    expect(step).not.toHaveProperty('message');
    expect(step).toMatchObject({
      id: 'error',
      stepType: 'Error',
      name: 'Validation Target',
      category: 'permanent',
      severity: 'error',
    });
  });

  it('keeps child workflow fields only on EmbedWorkflow steps', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'embed',
          type: 'basic',
          position: { x: 12, y: 36 },
          data: {
            id: 'embed',
            stepType: 'EmbedWorkflow',
            name: 'Run child',
            agentId: '',
            capabilityId: '',
            childWorkflowId: 'child-workflow',
            childVersion: '2',
            inputMapping: [],
            executionTimeout: 120,
            retryStrategy: 'Linear',
            groupByKey: '',
            groupByExpectedKeys: [],
          },
        },
      ] as any,
      [],
      { name: 'parent-workflow' }
    );

    const step = (graph!.steps as Record<string, any>).embed;
    expect(step).toMatchObject({
      id: 'embed',
      stepType: 'EmbedWorkflow',
      name: 'Run child',
      childWorkflowId: 'child-workflow',
      childVersion: 2,
    });
    expect(step).not.toHaveProperty('agentId');
    expect(step).not.toHaveProperty('capabilityId');
    expect(step).not.toHaveProperty('executionTimeout');
    expect(step).not.toHaveProperty('retryStrategy');
    expect(step).not.toHaveProperty('groupByKey');
    expect(step).not.toHaveProperty('groupByExpectedKeys');
    expect(step).not.toHaveProperty('renderingParameters');
  });

  it('does not leak Switch routing UI state into backend DSL', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'route',
          type: NODE_TYPES.SwitchNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'route',
            stepType: 'Switch',
            name: 'Route',
            switchRoutingMode: true,
            inputMapping: [
              {
                type: 'value',
                valueType: 'reference',
                value: 'data.status',
              },
              {
                type: 'cases',
                value: [
                  {
                    match: 'approved',
                    matchType: 'exact',
                    output: 'approved',
                    route: 'approved',
                  },
                ],
              },
              {
                type: 'default',
                value: 'fallback',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'switch-routing-workflow' }
    );

    const step = (graph!.steps as Record<string, any>).route;
    expect(step).toMatchObject({
      id: 'route',
      stepType: 'Switch',
      name: 'Route',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.status',
        },
        cases: [
          {
            match: 'approved',
            matchType: 'EQ',
            output: 'approved',
            route: 'approved',
          },
        ],
        default: 'fallback',
      },
    });
    expect(step).not.toHaveProperty('switchRoutingMode');
    expect(step).not.toHaveProperty('inputMapping');
  });
});

describe('Split variable round-trip', () => {
  it('preserves numeric immediate variable without emitting backend type metadata', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
        },
        variables: {
          counter: {
            valueType: 'immediate',
            value: 5,
            type: 'integer',
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.variables.counter).toEqual({
      valueType: 'immediate',
      value: 5,
    });
  });

  it('preserves boolean immediate variable without emitting backend type metadata', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
        },
        variables: {
          active: {
            valueType: 'immediate',
            value: true,
            type: 'boolean',
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.variables.active).toEqual({
      valueType: 'immediate',
      value: true,
    });
  });

  it('preserves composite array variable without emitting backend type metadata', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
        },
        variables: {
          payload: {
            valueType: 'composite',
            value: [
              { valueType: 'immediate', value: 'a' },
              { valueType: 'immediate', value: 'b' },
            ],
            type: 'array',
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.variables.payload.valueType).toBe('composite');
    expect(Array.isArray(step.config.variables.payload.value)).toBe(true);
    expect(step.config.variables.payload.value).toEqual([
      { valueType: 'immediate', value: 'a' },
      { valueType: 'immediate', value: 'b' },
    ]);
    expect(step.config.variables.payload).not.toHaveProperty('type');
  });

  it('does not synthesize `type: "string"` when backend omitted it', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
        },
        variables: {
          name: {
            valueType: 'immediate',
            value: 'Alice',
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.variables.name).not.toHaveProperty('type');
  });

  it('preserves reference variable with type hint', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
        },
        variables: {
          counter: {
            valueType: 'reference',
            value: 'variables.counter',
            type: 'integer',
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.variables.counter).toEqual({
      valueType: 'reference',
      value: 'variables.counter',
      type: 'integer',
    });
  });

  it('preserves type hint on Split config.value reference', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
          type: 'json',
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.value).toMatchObject({
      valueType: 'reference',
      value: 'data.items',
      type: 'json',
    });
  });
});

describe('Workflow canvas auto-layout', () => {
  it('keeps a branching graph topologically left-to-right without sibling overlaps', () => {
    const nodes = [
      makeLayoutNode('start'),
      makeLayoutNode('check', NODE_TYPES.ConditionalNode),
      makeLayoutNode('true-a'),
      makeLayoutNode('false-a'),
      makeLayoutNode('true-b'),
      makeLayoutNode('false-b'),
      makeLayoutNode('join'),
      makeLayoutNode('finish'),
    ];
    const edges = [
      makeLayoutEdge('start-check', 'start', 'check'),
      makeLayoutEdge('check-true', 'check', 'true-a', 'true'),
      makeLayoutEdge('check-false', 'check', 'false-a', 'false'),
      makeLayoutEdge('true-a-true-b', 'true-a', 'true-b'),
      makeLayoutEdge('false-a-false-b', 'false-a', 'false-b'),
      makeLayoutEdge('true-b-join', 'true-b', 'join'),
      makeLayoutEdge('false-b-join', 'false-b', 'join'),
      makeLayoutEdge('join-finish', 'join', 'finish'),
    ];

    const { nodes: layoutedNodes } = getLayoutedElements(nodes, edges);

    expectNoSiblingOverlaps(layoutedNodes);

    const start = getLayoutNode(layoutedNodes, 'start');
    const check = getLayoutNode(layoutedNodes, 'check');
    const trueA = getLayoutNode(layoutedNodes, 'true-a');
    const falseA = getLayoutNode(layoutedNodes, 'false-a');
    const trueB = getLayoutNode(layoutedNodes, 'true-b');
    const falseB = getLayoutNode(layoutedNodes, 'false-b');
    const join = getLayoutNode(layoutedNodes, 'join');
    const finish = getLayoutNode(layoutedNodes, 'finish');

    expect(start.position.x).toBeLessThan(check.position.x);
    expect(check.position.x).toBeLessThan(trueA.position.x);
    expect(check.position.x).toBeLessThan(falseA.position.x);
    expect(trueA.position.x).toBeLessThan(trueB.position.x);
    expect(falseA.position.x).toBeLessThan(falseB.position.x);
    expect(trueB.position.x).toBeLessThan(join.position.x);
    expect(falseB.position.x).toBeLessThan(join.position.x);
    expect(join.position.x).toBeLessThan(finish.position.x);

    expect(trueA.position.y).toBeLessThan(falseA.position.y);
    expect(trueB.position.y).toBeLessThan(falseB.position.y);
  });

  it('places exclusive true-path nodes above the branch source', () => {
    const nodes = [
      makeLayoutNode('start'),
      makeLayoutNode('check', NODE_TYPES.ConditionalNode),
      makeLayoutNode('review'),
      makeLayoutNode('approved', NODE_TYPES.ConditionalNode),
      makeLayoutNode('continue'),
      makeLayoutNode('reject'),
      makeLayoutNode('finish'),
    ];
    const edges = [
      makeLayoutEdge('start-check', 'start', 'check'),
      makeLayoutEdge('check-review', 'check', 'review', 'true'),
      makeLayoutEdge('check-continue', 'check', 'continue', 'false'),
      makeLayoutEdge('review-approved', 'review', 'approved'),
      makeLayoutEdge('approved-continue', 'approved', 'continue', 'true'),
      makeLayoutEdge('approved-reject', 'approved', 'reject', 'false'),
      makeLayoutEdge('continue-finish', 'continue', 'finish'),
    ];

    const { nodes: layoutedNodes } = getLayoutedElements(nodes, edges);

    expectNoSiblingOverlaps(layoutedNodes);
    expect(getLayoutNode(layoutedNodes, 'review').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'check').position.y
    );
    expect(getLayoutNode(layoutedNodes, 'approved').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'check').position.y
    );
    expect(getLayoutNode(layoutedNodes, 'continue').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'reject').position.y
    );
  });

  it('keeps nested true branches above an already-biased branch source', () => {
    const nodes = [
      makeLayoutNode('start'),
      makeLayoutNode('outer-check', NODE_TYPES.ConditionalNode),
      makeLayoutNode('inner-check', NODE_TYPES.ConditionalNode),
      makeLayoutNode('outer-false'),
      makeLayoutNode('inner-true'),
      makeLayoutNode('inner-false'),
      makeLayoutNode('finish'),
    ];
    const edges = [
      makeLayoutEdge('start-outer', 'start', 'outer-check'),
      makeLayoutEdge('outer-inner', 'outer-check', 'inner-check', 'true'),
      makeLayoutEdge('outer-false', 'outer-check', 'outer-false', 'false'),
      makeLayoutEdge('inner-true', 'inner-check', 'inner-true', 'true'),
      makeLayoutEdge('inner-false', 'inner-check', 'inner-false', 'false'),
      makeLayoutEdge('inner-true-finish', 'inner-true', 'finish'),
      makeLayoutEdge('inner-false-finish', 'inner-false', 'finish'),
      makeLayoutEdge('outer-false-finish', 'outer-false', 'finish'),
    ];

    const { nodes: layoutedNodes } = getLayoutedElements(nodes, edges);

    expectNoSiblingOverlaps(layoutedNodes);
    expect(getLayoutNode(layoutedNodes, 'inner-check').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'outer-check').position.y
    );
    expect(getLayoutNode(layoutedNodes, 'inner-true').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'inner-check').position.y
    );
    expect(getLayoutNode(layoutedNodes, 'inner-true').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'inner-false').position.y
    );
  });

  it('left-aligns small siblings in a column with a wide container', () => {
    const nodes = [
      makeLayoutNode('start'),
      makeLayoutNode('screen', NODE_TYPES.ConditionalNode),
      makeLayoutNode('review'),
      makeLayoutNode('approved', NODE_TYPES.ConditionalNode),
      makeLayoutNode('split', NODE_TYPES.ContainerNode),
      makeLayoutNode('reject'),
      makeLayoutNode('finish'),
      makeLayoutNode('child-1', NODE_TYPES.BasicNode, 'split'),
      makeLayoutNode('child-2', NODE_TYPES.BasicNode, 'split'),
      makeLayoutNode('child-3', NODE_TYPES.BasicNode, 'split'),
      makeLayoutNode('child-4', NODE_TYPES.BasicNode, 'split'),
      makeLayoutNode('child-5', NODE_TYPES.BasicNode, 'split'),
    ];
    const edges = [
      makeLayoutEdge('start-screen', 'start', 'screen'),
      makeLayoutEdge('screen-review', 'screen', 'review', 'true'),
      makeLayoutEdge('screen-split', 'screen', 'split', 'false'),
      makeLayoutEdge('review-approved', 'review', 'approved'),
      makeLayoutEdge('approved-split', 'approved', 'split', 'true'),
      makeLayoutEdge('approved-reject', 'approved', 'reject', 'false'),
      makeLayoutEdge('split-finish', 'split', 'finish'),
      makeLayoutEdge('child-1-child-2', 'child-1', 'child-2'),
      makeLayoutEdge('child-2-child-3', 'child-2', 'child-3'),
      makeLayoutEdge('child-3-child-4', 'child-3', 'child-4'),
      makeLayoutEdge('child-4-child-5', 'child-4', 'child-5'),
    ];

    const { nodes: layoutedNodes } = getLayoutedElements(nodes, edges);
    const split = getLayoutNode(layoutedNodes, 'split');
    const reject = getLayoutNode(layoutedNodes, 'reject');

    expectNoSiblingOverlaps(layoutedNodes);
    expect(getLayoutSize(split).width).toBeGreaterThan(
      getLayoutSize(reject).width * 2
    );
    expect(reject.position.x).toBe(split.position.x);
  });

  it('spreads multi-way switch routes into ordered vertical lanes', () => {
    const nodes = [
      makeLayoutNode('route', NODE_TYPES.SwitchNode),
      makeLayoutNode('case-0-step'),
      makeLayoutNode('case-1-step'),
      makeLayoutNode('case-2-step'),
      makeLayoutNode('default-step'),
      makeLayoutNode('finish'),
    ];
    const edges = [
      makeLayoutEdge('route-case-0', 'route', 'case-0-step', 'case-0'),
      makeLayoutEdge('route-case-1', 'route', 'case-1-step', 'case-1'),
      makeLayoutEdge('route-case-2', 'route', 'case-2-step', 'case-2'),
      makeLayoutEdge('route-default', 'route', 'default-step', 'default'),
      makeLayoutEdge('case-0-finish', 'case-0-step', 'finish'),
      makeLayoutEdge('case-1-finish', 'case-1-step', 'finish'),
      makeLayoutEdge('case-2-finish', 'case-2-step', 'finish'),
      makeLayoutEdge('default-finish', 'default-step', 'finish'),
    ];

    const { nodes: layoutedNodes } = getLayoutedElements(nodes, edges);

    expectNoSiblingOverlaps(layoutedNodes);
    expect(getLayoutNode(layoutedNodes, 'case-0-step').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'case-1-step').position.y
    );
    expect(getLayoutNode(layoutedNodes, 'case-1-step').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'case-2-step').position.y
    );
    expect(getLayoutNode(layoutedNodes, 'case-2-step').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'default-step').position.y
    );
    expect(
      getLayoutNode(layoutedNodes, 'default-step').position.y -
        getLayoutNode(layoutedNodes, 'case-0-step').position.y
    ).toBeLessThan(300);
  });

  it('lays out container children without overlaps and expands the parent', () => {
    const nodes = [
      makeLayoutNode('split', NODE_TYPES.ContainerNode),
      makeLayoutNode('inner-check', NODE_TYPES.ConditionalNode, 'split'),
      makeLayoutNode('inner-true', NODE_TYPES.BasicNode, 'split'),
      makeLayoutNode('inner-false', NODE_TYPES.BasicNode, 'split'),
      makeLayoutNode('inner-join', NODE_TYPES.BasicNode, 'split'),
      makeLayoutNode('finish'),
    ];
    const edges = [
      makeLayoutEdge('split-finish', 'split', 'finish'),
      makeLayoutEdge('inner-check-true', 'inner-check', 'inner-true', 'true'),
      makeLayoutEdge(
        'inner-check-false',
        'inner-check',
        'inner-false',
        'false'
      ),
      makeLayoutEdge('inner-true-join', 'inner-true', 'inner-join'),
      makeLayoutEdge('inner-false-join', 'inner-false', 'inner-join'),
    ];

    const { nodes: layoutedNodes } = getLayoutedElements(nodes, edges);
    const children = layoutedNodes.filter((node) => node.parentId === 'split');

    expectNoSiblingOverlaps(layoutedNodes);
    expect(getLayoutNode(layoutedNodes, 'inner-true').position.y).toBeLessThan(
      getLayoutNode(layoutedNodes, 'inner-false').position.y
    );

    const split = getLayoutNode(layoutedNodes, 'split');
    const splitSize = getLayoutSize(split);
    const childRight = Math.max(
      ...children.map((node) => node.position.x + getLayoutSize(node).width)
    );
    const childBottom = Math.max(
      ...children.map((node) => node.position.y + getLayoutSize(node).height)
    );

    expect(splitSize.width).toBeGreaterThan(childRight);
    expect(splitSize.height).toBeGreaterThan(childBottom);
  });
});
