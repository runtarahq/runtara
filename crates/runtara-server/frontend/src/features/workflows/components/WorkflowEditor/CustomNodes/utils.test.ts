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
import { normalizeMappingObject } from '../NodeForm/InputMappingField/mapping-entries';

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

  it('preserves a template nested inside a composite object', () => {
    // The composite editors create nested templates (CompositeValueItem /
    // CompositeArrayEditor); this locks the save/load contract they rely on.
    const graph = makeGraph({
      id: 's1',
      stepType: 'Agent',
      agentId: 'http',
      capabilityId: 'request',
      inputMapping: {
        headers: {
          valueType: 'composite',
          value: {
            authorization: {
              valueType: 'template',
              value: 'Bearer {{ steps.conn.outputs.api_key }}',
            },
            accept: { valueType: 'immediate', value: 'application/json' },
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.headers).toEqual({
      valueType: 'composite',
      value: {
        authorization: {
          valueType: 'template',
          value: 'Bearer {{ steps.conn.outputs.api_key }}',
        },
        accept: { valueType: 'immediate', value: 'application/json' },
      },
    });
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
  it('serializes graph metadata supplied by workflow settings', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'agent',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'agent',
            stepType: 'Agent',
            name: 'Agent',
            agentId: 'utils',
            capabilityId: 'noop',
            inputMapping: [],
          },
        },
      ] as any,
      [],
      {
        name: 'metadata-workflow',
        description: '',
        variables: {
          limit: {
            type: 'integer',
            value: 10,
            description: 'Max rows',
          },
        },
        inputSchema: {
          order_id: {
            type: 'string',
            required: true,
            default: 'ord_1',
            format: 'uuid',
          },
        },
        outputSchema: {
          ok: { type: 'boolean', required: true },
        },
        executionTimeoutSeconds: 120,
        rateLimitBudgetMs: 30_000,
        durable: false,
        entryPoint: 'agent',
      }
    );

    expect(graph).toMatchObject({
      name: 'metadata-workflow',
      description: '',
      variables: {
        limit: {
          type: 'integer',
          value: 10,
          description: 'Max rows',
        },
      },
      inputSchema: {
        order_id: {
          type: 'string',
          required: true,
          default: 'ord_1',
          format: 'uuid',
        },
      },
      outputSchema: {
        ok: { type: 'boolean', required: true },
      },
      executionTimeoutSeconds: 120,
      rateLimitBudgetMs: 30_000,
      durable: false,
      entryPoint: 'agent',
    });
  });

  it('serializes execution-plan edge conditions and priority', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'start',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'start',
            stepType: 'Agent',
            name: 'Start',
            agentId: 'utils',
            capabilityId: 'noop',
            inputMapping: [],
          },
        },
        {
          id: 'next',
          type: NODE_TYPES.BasicNode,
          position: { x: 240, y: 0 },
          data: {
            id: 'next',
            stepType: 'Agent',
            name: 'Next',
            agentId: 'utils',
            capabilityId: 'noop',
            inputMapping: [],
          },
        },
      ] as any,
      [
        {
          id: 'start-next',
          source: 'start',
          target: 'next',
          sourceHandle: 'source',
          data: {
            condition: {
              type: 'operation',
              op: 'EQ',
              arguments: ['data.status', 'ready'],
            },
            priority: 5,
          },
        },
      ] as any,
      { name: 'conditional-edge-workflow' }
    );

    expect(graph!.executionPlan?.[0]).toMatchObject({
      fromStep: 'start',
      toStep: 'next',
      label: 'next',
      condition: {
        type: 'operation',
        op: 'EQ',
        arguments: ['data.status', 'ready'],
      },
      priority: 5,
    });
  });

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

  it('preserves invalid Finish outputs so Rust validation can reject them', () => {
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
                type: 'orderId',
                value: '',
                typeHint: 'string',
                valueType: 'immediate',
              },
              {
                type: '',
                value: 'data.orderId',
                typeHint: 'string',
                valueType: 'reference',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'finish-output-invalid-fixture' }
    );

    const output = (graph!.steps as Record<string, any>).finish.inputMapping;
    expect(output.orderId).toEqual({
      valueType: 'immediate',
      value: '',
    });
    expect(output['']).toEqual({
      valueType: 'reference',
      value: 'data.orderId',
      type: 'string',
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

  it('round-trips Agent retry, timeout, and compensation fields', () => {
    const graph = makeGraph({
      id: 'agent',
      stepType: 'Agent',
      agentId: 'payments',
      capabilityId: 'charge-card',
      maxRetries: 2,
      retryDelay: 500,
      timeout: 30_000,
      compensation: {
        compensationStep: 'refund',
        compensationData: {
          chargeId: {
            valueType: 'reference',
            value: "steps['agent'].outputs.chargeId",
            type: 'string',
          },
        },
        trigger: 'on_downstream_error',
        order: 10,
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step).toMatchObject({
      maxRetries: 2,
      retryDelay: 500,
      timeout: 30_000,
      compensation: {
        compensationStep: 'refund',
        compensationData: {
          chargeId: {
            valueType: 'reference',
            value: "steps['agent'].outputs.chargeId",
            type: 'string',
          },
        },
        trigger: 'on_downstream_error',
        order: 10,
      },
    });
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

  it('round-trips Switch value reference type hint and default', () => {
    const graph = makeGraph({
      id: 'sw',
      stepType: 'Switch',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.country',
          type: 'string',
          default: 'US',
        },
        cases: [{ match: 'US', matchType: 'EQ', output: { region: 'NA' } }],
        default: { region: 'OTHER' },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.value).toEqual({
      valueType: 'reference',
      value: 'data.country',
      type: 'string',
      default: 'US',
    });
    expect(step.config.cases).toEqual([
      { match: 'US', matchType: 'EQ', output: { region: 'NA' } },
    ]);
  });

  it('does not fabricate a Switch default output on round-trip', () => {
    const graph = makeGraph({
      id: 'sw',
      stepType: 'Switch',
      config: {
        value: { valueType: 'reference', value: 'data.status' },
        cases: [{ match: 'approved', matchType: 'EQ', output: 'ok' }],
        // No default: at runtime an unmatched value must fail the step.
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config).not.toHaveProperty('default');
    expect(step.config.value).toEqual({
      valueType: 'reference',
      value: 'data.status',
    });
  });

  it('round-trips an authored Switch default output (including {})', () => {
    const emptyDefault = roundTripStep(
      makeGraph({
        id: 'sw',
        stepType: 'Switch',
        config: {
          value: { valueType: 'reference', value: 'data.status' },
          cases: [{ match: 'approved', matchType: 'EQ', output: 'ok' }],
          default: {},
        },
        renderingParameters: { x: 0, y: 0 },
      })
    );
    expect(emptyDefault.config.default).toEqual({});

    const objectDefault = roundTripStep(
      makeGraph({
        id: 'sw',
        stepType: 'Switch',
        config: {
          value: { valueType: 'reference', value: 'data.status' },
          cases: [{ match: 'approved', matchType: 'EQ', output: 'ok' }],
          default: { state: 'unknown' },
        },
        renderingParameters: { x: 0, y: 0 },
      })
    );
    expect(objectDefault.config.default).toEqual({ state: 'unknown' });
  });

  it('round-trips a composite Switch value', () => {
    const graph = makeGraph({
      id: 'sw',
      stepType: 'Switch',
      config: {
        value: {
          valueType: 'composite',
          value: {
            country: { valueType: 'reference', value: 'data.country' },
            tier: { valueType: 'immediate', value: 'gold' },
          },
        },
        cases: [
          {
            match: { country: 'US', tier: 'gold' },
            matchType: 'EQ',
            output: 'vip',
          },
        ],
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.value).toEqual({
      valueType: 'composite',
      value: {
        country: { valueType: 'reference', value: 'data.country' },
        tier: { valueType: 'immediate', value: 'gold' },
      },
    });
  });

  it('never emits a Switch config object lacking value', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'sw',
          type: NODE_TYPES.SwitchNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'sw',
            stepType: 'Switch',
            name: 'Route',
            inputMapping: [
              { type: 'value', value: '', typeHint: 'auto' },
              {
                type: 'cases',
                value: [{ match: 'a', matchType: 'exact', output: 'a' }],
              },
              { type: 'default', value: {} },
            ],
          },
        },
      ] as any,
      [],
      { name: 'switch-empty-value' }
    );

    // SwitchConfig.value is mandatory on the backend (deny_unknown_fields);
    // a config without it would be rejected by serde at save time.
    const step = (graph!.steps as Record<string, any>).sw;
    expect(step).not.toHaveProperty('config');
  });

  it('round-trips Delay duration MappingValue', () => {
    const graph = makeGraph({
      id: 'delay',
      stepType: 'Delay',
      durationMs: {
        valueType: 'reference',
        value: 'variables.delayMs',
        type: 'integer',
        default: 500,
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.durationMs).toEqual({
      valueType: 'reference',
      value: 'variables.delayMs',
      type: 'integer',
      default: 500,
    });
    expect(step).not.toHaveProperty('inputMapping');
  });

  it('round-trips Split source template MappingValue', () => {
    const graph = makeGraph({
      id: 'split',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'template',
          value: '{{ data.dynamicItems }}',
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.value).toEqual({
      valueType: 'template',
      value: '{{ data.dynamicItems }}',
    });
  });

  it('round-trips Filter source reference metadata', () => {
    const graph = makeGraph({
      id: 'filter',
      stepType: 'Filter',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
          type: 'json',
          default: [],
        },
        condition: {
          type: 'operation',
          op: 'EQ',
          arguments: [
            { valueType: 'reference', value: 'item.active' },
            { valueType: 'immediate', value: true },
          ],
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.value).toEqual({
      valueType: 'reference',
      value: 'data.items',
      type: 'json',
      default: [],
    });
  });

  it('round-trips Filter source immediate array MappingValue', () => {
    const graph = makeGraph({
      id: 'filter',
      stepType: 'Filter',
      config: {
        value: {
          valueType: 'immediate',
          value: [{ status: 'active' }, { status: 'pending' }],
        },
        condition: {
          type: 'operation',
          op: 'EQ',
          arguments: [
            { valueType: 'reference', value: 'item.status' },
            { valueType: 'immediate', value: 'active' },
          ],
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.value).toEqual({
      valueType: 'immediate',
      value: [{ status: 'active' }, { status: 'pending' }],
    });
  });

  it('round-trips GroupBy source composite MappingValue', () => {
    const graph = makeGraph({
      id: 'group',
      stepType: 'GroupBy',
      config: {
        value: {
          valueType: 'composite',
          value: [
            {
              valueType: 'reference',
              value: 'data.primary',
              type: 'json',
              default: [],
            },
            {
              valueType: 'reference',
              value: 'data.secondary',
              type: 'json',
            },
          ],
        },
        key: 'status',
        expectedKeys: ['active', 'pending'],
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.value).toEqual({
      valueType: 'composite',
      value: [
        {
          valueType: 'reference',
          value: 'data.primary',
          type: 'json',
          default: [],
        },
        {
          valueType: 'reference',
          value: 'data.secondary',
          type: 'json',
        },
      ],
    });
    expect(step.config.expectedKeys).toEqual(['active', 'pending']);
  });

  it('round-trips rich Split input and output schemas', () => {
    const inputSchema = {
      item: {
        type: 'object',
        required: true,
        description: 'Item payload',
        example: { sku: 'sku_1', quantity: 2 },
        properties: {
          sku: { type: 'string', required: true, pattern: '^sku_' },
          quantity: { type: 'integer', required: true, min: 1 },
        },
        visibleWhen: { field: 'mode', equals: 'manual' },
        'x-runtime': { source: 'input' },
      },
    };
    const outputSchema = {
      accepted: {
        type: 'boolean',
        required: true,
        label: 'Accepted',
        placeholder: 'true',
        order: 1,
      },
    };

    const graph = makeGraph({
      id: 'split',
      stepType: 'Split',
      inputSchema,
      outputSchema,
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputSchema).toEqual(inputSchema);
    expect(step.outputSchema).toEqual(outputSchema);
  });

  it('round-trips rich AiAgent structured output schema', () => {
    const outputSchema = {
      decision: {
        type: 'string',
        required: true,
        description: 'Routing decision',
        enum: ['approve', 'reject', { route: 'manual' }],
        example: 'approve',
        label: 'Decision',
        order: 1,
        'x-agent': { source: 'fixture' },
      },
    };

    const graph = makeGraph({
      id: 'ai',
      stepType: 'AiAgent',
      config: {
        systemPrompt: { valueType: 'immediate', value: 'You help.' },
        userPrompt: { valueType: 'template', value: '{{ data.prompt }}' },
        outputSchema,
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.outputSchema).toEqual(outputSchema);
  });

  it('round-trips rich WaitForSignal response schema', () => {
    const responseSchema = {
      files: {
        type: 'array',
        required: false,
        description: 'Uploaded evidence',
        items: { type: 'file' },
        example: [{ name: 'invoice.pdf' }],
        nullable: true,
        min: 0,
        max: 3,
      },
    };

    const graph = makeGraph({
      id: 'wait',
      stepType: 'WaitForSignal',
      signal: 'approval',
      responseSchema,
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.responseSchema).toEqual(responseSchema);
  });

  it('round-trips AiAgent retry settings in config', () => {
    const graph = makeGraph({
      id: 'ai',
      stepType: 'AiAgent',
      config: {
        systemPrompt: { valueType: 'immediate', value: 'You help.' },
        userPrompt: { valueType: 'template', value: '{{ data.prompt }}' },
        provider: 'openai',
        model: 'gpt-4.1-mini',
        maxRetries: 3,
        retryDelay: 250,
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config).toMatchObject({
      provider: 'openai',
      model: 'gpt-4.1-mini',
      maxRetries: 3,
      retryDelay: 250,
    });
  });

  it('round-trips WaitForSignal action metadata without synthesizing poll interval', () => {
    const graph = makeGraph({
      id: 'wait',
      stepType: 'WaitForSignal',
      signal: 'approval',
      action: {
        key: 'approve-order',
        correlation: {
          orderId: {
            valueType: 'reference',
            value: 'data.orderId',
            type: 'string',
          },
        },
        context: {
          requester: {
            valueType: 'reference',
            value: 'data.requester',
            type: 'string',
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.action).toEqual({
      key: 'approve-order',
      correlation: {
        orderId: {
          valueType: 'reference',
          value: 'data.orderId',
          type: 'string',
        },
      },
      context: {
        requester: {
          valueType: 'reference',
          value: 'data.requester',
          type: 'string',
        },
      },
    });
    expect(step).not.toHaveProperty('pollIntervalMs');
  });

  it('round-trips WaitForSignal onWait graph', () => {
    const onWait = {
      entryPoint: 'notify',
      steps: {
        notify: {
          id: 'notify',
          stepType: 'Log',
          message: 'waiting for approval',
          level: 'info',
        },
        done: {
          id: 'done',
          stepType: 'Finish',
          outputs: {},
        },
      },
      executionPlan: [
        {
          fromStep: 'notify',
          toStep: 'done',
          label: 'next',
        },
      ],
    };
    const graph = makeGraph({
      id: 'wait',
      stepType: 'WaitForSignal',
      signal: 'approval',
      onWait,
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.onWait).toEqual(onWait);
  });

  it('round-trips Log context metadata', () => {
    const graph = makeGraph({
      id: 'log',
      stepType: 'Log',
      message: 'created',
      level: 'info',
      context: {
        orderId: {
          valueType: 'reference',
          value: 'data.orderId',
          type: 'string',
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step).toMatchObject({
      message: 'created',
      level: 'info',
      context: {
        orderId: {
          valueType: 'reference',
          value: 'data.orderId',
          type: 'string',
        },
      },
    });
  });

  it('round-trips Error context metadata', () => {
    const graph = makeGraph({
      id: 'error',
      stepType: 'Error',
      code: 'ORDER_INVALID',
      message: 'Order is invalid',
      category: 'permanent',
      severity: 'error',
      context: {
        orderId: {
          valueType: 'reference',
          value: 'data.orderId',
          type: 'string',
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step).toMatchObject({
      code: 'ORDER_INVALID',
      message: 'Order is invalid',
      category: 'permanent',
      severity: 'error',
      context: {
        orderId: {
          valueType: 'reference',
          value: 'data.orderId',
          type: 'string',
        },
      },
    });
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

  it('round-trips advanced execution options', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
        },
        parallelism: 4,
        sequential: true,
        dontStopOnFailed: true,
        maxRetries: 2,
        retryDelay: 500,
        timeout: 10_000,
        allowNull: true,
        convertSingleValue: true,
        batchSize: 25,
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config).toMatchObject({
      parallelism: 4,
      sequential: true,
      dontStopOnFailed: true,
      maxRetries: 2,
      retryDelay: 500,
      timeout: 10_000,
      allowNull: true,
      convertSingleValue: true,
      batchSize: 25,
    });
  });

  it('round-trips a template variable', () => {
    const graph = makeGraph({
      id: 's1',
      stepType: 'Split',
      config: {
        value: {
          valueType: 'reference',
          value: 'data.items',
        },
        variables: {
          greeting: {
            valueType: 'template',
            value: 'Hello {{ data.name }}',
          },
        },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.config.variables.greeting).toEqual({
      valueType: 'template',
      value: 'Hello {{ data.name }}',
    });
  });

  it('serializes form-state template variables as template MappingValues', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'split',
          type: NODE_TYPES.ContainerNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'split',
            stepType: 'Split',
            name: 'Split',
            inputMapping: [
              { type: 'value', value: 'data.items', valueType: 'reference' },
            ],
            splitVariablesFields: [
              {
                name: 'greeting',
                value: 'Hi {{ data.name }}',
                valueType: 'template',
                type: 'string',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'split-template-variable-fixture' }
    );

    const step = (graph!.steps as Record<string, any>).split;
    expect(step.config.variables.greeting).toEqual({
      valueType: 'template',
      value: 'Hi {{ data.name }}',
    });
  });

  it('coerces typed immediate variables from their form strings', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'split',
          type: NODE_TYPES.ContainerNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'split',
            stepType: 'Split',
            name: 'Split',
            inputMapping: [
              { type: 'value', value: 'data.items', valueType: 'reference' },
            ],
            splitVariablesFields: [
              {
                name: 'count',
                value: '5',
                valueType: 'immediate',
                type: 'number',
              },
              {
                name: 'flag',
                value: 'true',
                valueType: 'immediate',
                type: 'boolean',
              },
              {
                name: 'label',
                value: 'plain',
                valueType: 'immediate',
                type: 'string',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'split-typed-immediates-fixture' }
    );

    const variables = (graph!.steps as Record<string, any>).split.config
      .variables;
    expect(variables.count).toEqual({ valueType: 'immediate', value: 5 });
    expect(variables.flag).toEqual({ valueType: 'immediate', value: true });
    expect(variables.label).toEqual({
      valueType: 'immediate',
      value: 'plain',
    });
  });

  it('never emits non-ValueType variable types as backend reference hints', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'split',
          type: NODE_TYPES.ContainerNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'split',
            stepType: 'Split',
            name: 'Split',
            inputMapping: [
              { type: 'value', value: 'data.items', valueType: 'reference' },
            ],
            splitVariablesFields: [
              {
                name: 'payload',
                value: 'data.payload',
                valueType: 'reference',
                // 'object' is a UI variable type but not a legal backend
                // ValueType — emitting it as `type` fails serde.
                type: 'object',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'split-illegal-type-hint-fixture' }
    );

    const variables = (graph!.steps as Record<string, any>).split.config
      .variables;
    expect(variables.payload).toEqual({
      valueType: 'reference',
      value: 'data.payload',
    });
  });
});

describe('Empty-string immediate preservation', () => {
  it('round-trips a JSON-authored immediate empty string on an Agent input', () => {
    const graph = makeGraph({
      id: 'agent',
      stepType: 'Agent',
      agentId: 'text',
      capabilityId: 'concat',
      inputMapping: {
        separator: { valueType: 'immediate', value: '' },
      },
      renderingParameters: { x: 0, y: 0 },
    });

    const step = roundTripStep(graph);
    expect(step.inputMapping.separator).toEqual({
      valueType: 'immediate',
      value: '',
    });
  });

  it('keeps an explicit (unflagged) immediate empty string from form state', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'agent',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'agent',
            stepType: 'Agent',
            name: 'Agent',
            agentId: 'text',
            capabilityId: 'concat',
            inputMapping: [
              {
                type: 'separator',
                value: '',
                typeHint: 'text',
                valueType: 'immediate',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'explicit-empty-string-fixture' }
    );

    const step = (graph!.steps as Record<string, any>).agent;
    expect(step.inputMapping.separator).toEqual({
      valueType: 'immediate',
      value: '',
    });
  });

  it('drops auto-seeded rows the user never filled in', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'agent',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'agent',
            stepType: 'Agent',
            name: 'Agent',
            agentId: 'text',
            capabilityId: 'concat',
            inputMapping: [
              {
                type: 'separator',
                value: '',
                typeHint: 'text',
                valueType: 'immediate',
                autoSeeded: true,
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'auto-seeded-empty-fixture' }
    );

    const step = (graph!.steps as Record<string, any>).agent;
    expect(step).not.toHaveProperty('inputMapping');
  });

  it('keeps filled auto-seeded rows without leaking the marker', () => {
    const graph = composeExecutionGraph(
      [
        {
          id: 'agent',
          type: NODE_TYPES.BasicNode,
          position: { x: 0, y: 0 },
          data: {
            id: 'agent',
            stepType: 'Agent',
            name: 'Agent',
            agentId: 'text',
            capabilityId: 'concat',
            inputMapping: [
              {
                type: 'separator',
                value: ', ',
                typeHint: 'text',
                valueType: 'immediate',
                autoSeeded: true,
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'auto-seeded-filled-fixture' }
    );

    const step = (graph!.steps as Record<string, any>).agent;
    expect(step.inputMapping.separator).toEqual({
      valueType: 'immediate',
      value: ', ',
    });
    expect(JSON.stringify(graph)).not.toContain('autoSeeded');
  });
});

describe('Finish object/array immediate outputs', () => {
  it('parses object/array hinted immediates into real JSON values', () => {
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
                type: 'payload',
                value: '{"a": 1}',
                typeHint: 'object',
                valueType: 'immediate',
              },
              {
                type: 'list',
                value: '[1, 2, 3]',
                typeHint: 'array',
                valueType: 'immediate',
              },
            ],
          },
        },
      ] as any,
      [],
      { name: 'finish-object-array-fixture' }
    );

    const mapping = (graph!.steps as Record<string, any>).finish.inputMapping;
    // Values are real JSON, and the form-level object/array hints are never
    // emitted as backend type hints (they are not legal ValueTypes).
    expect(mapping.payload).toEqual({
      valueType: 'immediate',
      value: { a: 1 },
    });
    expect(mapping.list).toEqual({
      valueType: 'immediate',
      value: [1, 2, 3],
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

describe('Subgraph-level ExecutionGraph field round-trip', () => {
  it('preserves Split subgraph variables, schemas, and metadata through load→save', () => {
    const graph = {
      name: 'subgraph-meta-fixture',
      entryPoint: 'split',
      executionPlan: [],
      steps: {
        split: {
          id: 'split',
          stepType: 'Split',
          config: {
            value: { valueType: 'reference', value: 'data.items' },
          },
          subgraph: {
            name: 'per-item',
            description: 'runs once per item',
            entryPoint: 'finish',
            variables: {
              threshold: { type: 'number', value: 5 },
            },
            inputSchema: { fields: [{ name: 'sku', type: 'string' }] },
            outputSchema: { fields: [{ name: 'ok', type: 'boolean' }] },
            steps: {
              finish: {
                id: 'finish',
                stepType: 'Finish',
                inputMapping: {
                  ok: { valueType: 'immediate', value: true },
                },
              },
            },
            executionPlan: [],
          },
        },
      },
    };

    const { nodes, edges } = executionGraphToReactFlow(graph as any);
    const round = composeExecutionGraph(nodes, edges, { name: graph.name });
    expect(round).not.toBeNull();

    const split = (round!.steps as Record<string, any>)['split'];
    expect(split).toBeDefined();
    expect(split.subgraph).toBeDefined();
    // Graph-level fields must survive the rebuild from child nodes.
    expect(split.subgraph.name).toBe('per-item');
    expect(split.subgraph.description).toBe('runs once per item');
    expect(split.subgraph.variables).toEqual({
      threshold: { type: 'number', value: 5 },
    });
    expect(split.subgraph.inputSchema).toEqual({
      fields: [{ name: 'sku', type: 'string' }],
    });
    expect(split.subgraph.outputSchema).toEqual({
      fields: [{ name: 'ok', type: 'boolean' }],
    });
    // Children are still rebuilt correctly.
    expect(split.subgraph.steps.finish).toBeDefined();
    expect(split.subgraph.steps.finish.stepType).toBe('Finish');
    // The UI-only carrier must not leak into the saved step.
    expect(split.subgraphMeta).toBeUndefined();
    expect(split.subgraph.subgraphMeta).toBeUndefined();
  });

  it('does not invent subgraph fields for fresh containers', () => {
    const container = makeLayoutNode('split', NODE_TYPES.ContainerNode);
    const child = makeLayoutNode('inner', NODE_TYPES.BasicNode, 'split');
    const round = composeExecutionGraph([container, child], [], {
      name: 'fresh-split',
    });
    expect(round).not.toBeNull();
    const split = (round!.steps as Record<string, any>)['split'];
    expect(split.subgraph).toBeDefined();
    expect(split.subgraph.name).toBeUndefined();
    expect(split.subgraph.variables).toBeUndefined();
    expect(split.subgraph.steps.inner).toBeDefined();
  });
});

describe('AiAgent onError and mcp.<toolset> edge round-trip', () => {
  /**
   * AiAgent error routing (`onError` label) and MCP toolset edges
   * (`mcp.<toolset>` label) are plain labelled edges in the DSL. The editor
   * must load them back into edges with the matching sourceHandle and
   * serialize them out unchanged — nothing may strip or rewrite the labels.
   */
  function makeAiAgentEdgeGraph(): ExecutionGraphDto & { entryPoint: string } {
    return {
      name: 'ai-agent-edge-fixture',
      steps: {
        ai: {
          id: 'ai',
          stepType: 'AiAgent',
          name: 'Assistant',
          connectionId: 'conn-1',
          config: {
            provider: 'openai',
            model: 'gpt-4o',
            userPrompt: { valueType: 'immediate', value: 'hi' },
          },
        },
        handler: {
          id: 'handler',
          stepType: 'Error',
          name: 'Error handler',
          inputMapping: [],
        },
        linear_mcp: {
          id: 'linear_mcp',
          stepType: 'Agent',
          name: 'Linear MCP toolset',
          agentId: 'mcp',
          capabilityId: 'mcp-tool-invoke',
          inputMapping: [],
        },
        finish: {
          id: 'finish',
          stepType: 'Finish',
          name: 'Finish',
          outputMapping: [],
        },
      } as any,
      executionPlan: [
        { fromStep: 'ai', toStep: 'finish', label: 'next' },
        { fromStep: 'ai', toStep: 'handler', label: 'onError' },
        { fromStep: 'ai', toStep: 'linear_mcp', label: 'mcp.linear' },
      ] as any,
      entryPoint: 'ai',
    };
  }

  it('loads onError and mcp.<toolset> labels into matching sourceHandles', () => {
    const { edges } = executionGraphToReactFlow(makeAiAgentEdgeGraph() as any);

    const onErrorEdge = edges.find(
      (edge) => edge.source === 'ai' && edge.sourceHandle === 'onError'
    );
    expect(onErrorEdge).toBeDefined();
    expect(onErrorEdge!.target).toBe('handler');

    const mcpEdge = edges.find(
      (edge) => edge.source === 'ai' && edge.sourceHandle === 'mcp.linear'
    );
    expect(mcpEdge).toBeDefined();
    expect(mcpEdge!.target).toBe('linear_mcp');
  });

  it('does not classify the onError route as an AiAgent tool on load', () => {
    const { nodes } = executionGraphToReactFlow(makeAiAgentEdgeGraph() as any);
    const aiNode = nodes.find((node) => node.id === 'ai');
    expect(aiNode).toBeDefined();

    const toolsField = ((aiNode!.data as any).inputMapping || []).find(
      (item: any) => item.type === 'tools'
    );
    const toolNames: string[] = Array.isArray(toolsField?.value)
      ? toolsField.value
      : [];
    expect(toolNames).not.toContain('onError');
  });

  it('round-trips onError and mcp.<toolset> edges through save', () => {
    const { nodes, edges } = executionGraphToReactFlow(
      makeAiAgentEdgeGraph() as any
    );
    const round = composeExecutionGraph(nodes, edges, {
      name: 'ai-agent-edge-fixture',
    });
    expect(round).not.toBeNull();

    const plan = (round!.executionPlan || []) as Array<{
      fromStep: string;
      toStep: string;
      label?: string;
    }>;

    expect(plan).toContainEqual(
      expect.objectContaining({
        fromStep: 'ai',
        toStep: 'handler',
        label: 'onError',
      })
    );
    expect(plan).toContainEqual(
      expect.objectContaining({
        fromStep: 'ai',
        toStep: 'linear_mcp',
        label: 'mcp.linear',
      })
    );
    expect(plan).toContainEqual(
      expect.objectContaining({
        fromStep: 'ai',
        toStep: 'finish',
        label: 'next',
      })
    );

    // The loaded steps survive the round-trip too.
    expect((round!.steps as Record<string, any>).handler.stepType).toBe(
      'Error'
    );
    expect((round!.steps as Record<string, any>).linear_mcp.agentId).toBe(
      'mcp'
    );
  });
});

describe('WaitForSignal clear-field round-trip', () => {
  /**
   * Clearing a WaitForSignal field in the form must actually remove the
   * corresponding top-level key on save. Loaded step data is spread into
   * node.data, so without an explicit delete the stale key resurrects
   * through the `...filteredRestData` spread (load → edit → save).
   */
  function makeWaitGraph(): ExecutionGraphDto & { entryPoint: string } {
    return makeGraph({
      id: 'wait',
      stepType: 'WaitForSignal',
      signal: 'approval',
      responseSchema: {
        approved: { type: 'boolean', required: true },
      },
      timeoutMs: { valueType: 'immediate', value: 60000 },
      pollIntervalMs: 500,
      action: {
        key: 'approve-order',
        correlation: {
          orderId: {
            valueType: 'reference',
            value: 'data.orderId',
            type: 'string',
          },
        },
        context: {
          requester: {
            valueType: 'reference',
            value: 'data.requester',
            type: 'string',
          },
        },
      },
      onWait: {
        entryPoint: 'notify',
        steps: {
          notify: {
            id: 'notify',
            stepType: 'Log',
            message: 'waiting',
            level: 'info',
          },
        },
        executionPlan: [],
      },
      renderingParameters: { x: 0, y: 0 },
    });
  }

  /** Patch an inputMapping entry on a loaded node, simulating a form edit. */
  function editEntry(
    node: Node,
    type: string,
    patch: Record<string, unknown>
  ) {
    const mapping = ((node.data as any).inputMapping || []) as any[];
    const idx = mapping.findIndex((item) => item.type === type);
    expect(idx, `inputMapping entry '${type}' missing`).toBeGreaterThanOrEqual(
      0
    );
    mapping[idx] = { ...mapping[idx], ...patch };
  }

  it('removes every cleared field key after load → clear → save', () => {
    const { nodes, edges } = executionGraphToReactFlow(makeWaitGraph() as any);
    const waitNode = nodes.find((n) => n.id === 'wait')!;
    expect(waitNode).toBeDefined();

    // Simulate the user clearing every optional field in the form
    editEntry(waitNode, 'responseSchema', { value: [] });
    editEntry(waitNode, 'timeoutMs', { value: '', valueType: 'immediate' });
    editEntry(waitNode, 'pollIntervalMs', { value: '' });
    editEntry(waitNode, 'actionKey', { value: '' });
    editEntry(waitNode, 'actionCorrelation', { value: '' });
    editEntry(waitNode, 'actionContext', { value: '' });
    // The onWait editor sets the form value to undefined when blanked
    (waitNode.data as any).onWait = undefined;

    const round = composeExecutionGraph(nodes, edges, { name: 'wait-clear' });
    expect(round).not.toBeNull();
    const step = (round!.steps as Record<string, any>).wait;

    expect(step).not.toHaveProperty('responseSchema');
    expect(step).not.toHaveProperty('timeoutMs');
    expect(step).not.toHaveProperty('pollIntervalMs');
    expect(step).not.toHaveProperty('action');
    expect(step).not.toHaveProperty('onWait');
    // Non-cleared fields survive
    expect(step.signal).toBe('approval');
  });

  it('keeps populated fields intact across an untouched round-trip', () => {
    const step = roundTripStep(makeWaitGraph());
    expect(step.responseSchema).toEqual({
      approved: { type: 'boolean', required: true },
    });
    expect(step.timeoutMs).toEqual({ valueType: 'immediate', value: 60000 });
    expect(step.pollIntervalMs).toBe(500);
    expect(step.action.key).toBe('approve-order');
    expect(step.onWait.entryPoint).toBe('notify');
  });

  it('serializes reference-mode timeoutMs as a reference MappingValue', () => {
    const { nodes, edges } = executionGraphToReactFlow(makeWaitGraph() as any);
    const waitNode = nodes.find((n) => n.id === 'wait')!;

    editEntry(waitNode, 'timeoutMs', {
      value: 'data.timeoutMs',
      valueType: 'reference',
    });

    const round = composeExecutionGraph(nodes, edges, { name: 'wait-ref' });
    const step = (round!.steps as Record<string, any>).wait;
    expect(step.timeoutMs).toEqual({
      valueType: 'reference',
      value: 'data.timeoutMs',
    });
  });

  it('never emits invalid JSON for a template-mode timeoutMs', () => {
    // The runtime requires timeoutMs to resolve to a number; a template
    // renders to a string and previously serialized as
    // {valueType:'template', value:NaN→null}, which serde rejects. The
    // serializer now drops the unrepresentable value entirely.
    const { nodes, edges } = executionGraphToReactFlow(makeWaitGraph() as any);
    const waitNode = nodes.find((n) => n.id === 'wait')!;

    editEntry(waitNode, 'timeoutMs', {
      value: '{{ data.timeout }}',
      valueType: 'template',
    });

    const round = composeExecutionGraph(nodes, edges, {
      name: 'wait-template',
    });
    const step = (round!.steps as Record<string, any>).wait;
    expect(step).not.toHaveProperty('timeoutMs');
  });

  it('rounds decimal pollIntervalMs to an integer (backend u64)', () => {
    const { nodes, edges } = executionGraphToReactFlow(makeWaitGraph() as any);
    const waitNode = nodes.find((n) => n.id === 'wait')!;

    editEntry(waitNode, 'pollIntervalMs', { value: '500.5' });

    const round = composeExecutionGraph(nodes, edges, { name: 'wait-poll' });
    const step = (round!.steps as Record<string, any>).wait;
    expect(step.pollIntervalMs).toBe(501);
    expect(Number.isInteger(step.pollIntervalMs)).toBe(true);
  });
});

describe('AiAgent memory removal round-trip', () => {
  const MEMORY_FIELD_TYPES = new Set([
    'memoryEnabled',
    'memoryConversationId',
    'memoryMaxMessages',
    'memoryStrategy',
    'memoryProviderStepId',
  ]);

  function makeMemoryGraph(
    compaction?: Record<string, unknown>
  ): ExecutionGraphDto & { entryPoint: string } {
    return {
      name: 'ai-memory-fixture',
      steps: {
        ai: {
          id: 'ai',
          stepType: 'AiAgent',
          name: 'Assistant',
          connectionId: 'conn-1',
          config: {
            provider: 'openai',
            model: 'gpt-4o',
            userPrompt: { valueType: 'immediate', value: 'hi' },
            memory: {
              conversationId: {
                valueType: 'reference',
                value: 'data.sessionId',
              },
              ...(compaction ? { compaction } : {}),
            },
          },
        },
        mem: {
          id: 'mem',
          stepType: 'Agent',
          name: 'Memory provider',
          agentId: 'object-model',
          capabilityId: 'conversation-memory',
          inputMapping: [],
        },
        finish: {
          id: 'finish',
          stepType: 'Finish',
          name: 'Finish',
          outputMapping: [],
        },
      } as any,
      executionPlan: [
        { fromStep: 'ai', toStep: 'finish', label: 'next' },
        { fromStep: 'ai', toStep: 'mem', label: 'memory' },
      ] as any,
      entryPoint: 'ai',
    };
  }

  it('omits config.memory after the memory entries and provider are removed', () => {
    const { nodes, edges } = executionGraphToReactFlow(
      makeMemoryGraph({ maxMessages: 50, strategy: 'summarize' }) as any
    );

    const aiNode = nodes.find((n) => n.id === 'ai')!;
    expect(aiNode).toBeDefined();
    // Sanity: memory was loaded into the form entries
    const loadedMapping = (aiNode.data as any).inputMapping as any[];
    expect(
      loadedMapping.find((item) => item.type === 'memoryEnabled')?.value
    ).toBe(true);
    // The stale config (incl. memory) is still spread into node.data — the
    // serializer must rebuild config from the entries, not resurrect it.
    expect((aiNode.data as any).config?.memory).toBeDefined();

    // Simulate the form's "Remove memory": strip memory entries, drop the
    // memory edge and the hidden provider node.
    (aiNode.data as any).inputMapping = loadedMapping.filter(
      (item) => !MEMORY_FIELD_TYPES.has(item.type)
    );
    const remainingEdges = edges.filter(
      (e) => !(e.source === 'ai' && e.sourceHandle === 'memory')
    );
    const remainingNodes = nodes.filter((n) => n.id !== 'mem');

    const round = composeExecutionGraph(remainingNodes, remainingEdges, {
      name: 'ai-memory-removed',
    });
    expect(round).not.toBeNull();

    const step = (round!.steps as Record<string, any>).ai;
    expect(step.config).not.toHaveProperty('memory');
    // The rest of the config survives
    expect(step.config).toMatchObject({
      provider: 'openai',
      model: 'gpt-4o',
    });
    // Provider step and memory edge are gone from the saved graph
    expect(round!.steps).not.toHaveProperty('mem');
    expect(round!.executionPlan).not.toContainEqual(
      expect.objectContaining({ label: 'memory' })
    );
  });

  it('round-trips memory without a compaction strategy and does not invent one', () => {
    // DSL default for CompactionConfig.strategy is SlidingWindow; the editor
    // must not silently materialize a different strategy on save.
    const step = roundTripStep(makeMemoryGraph({ maxMessages: 50 }) as any);
    expect(step.config.memory.conversationId).toEqual({
      valueType: 'reference',
      value: 'data.sessionId',
    });
    expect(step.config.memory.compaction).toEqual({ maxMessages: 50 });
    expect(step.config.memory.compaction).not.toHaveProperty('strategy');
  });
});

describe('Mapping-object round-trip (Log/Error/WaitForSignal contexts)', () => {
  /**
   * Finding 29: the four InputMapping-shaped JSON fields (Log.context,
   * Error.context, WaitForSignal.action.correlation/.context) are now edited
   * by MappingObjectField, which writes back the UI-format object produced by
   * normalizeMappingObject. These tests pin two contracts:
   *   1. a rich mapping object (reference with type+default, template,
   *      nested composite) survives load → save unchanged;
   *   2. writing the normalized UI-format object back into form state (what a
   *      structured edit does) serializes to the identical DSL output; and
   *   3. an empty editor value ({}) clears the key, like the empty textarea.
   */
  const RICH_MAPPING = {
    caseId: {
      valueType: 'reference',
      value: 'data.caseId',
      type: 'string',
      default: 'unknown-case',
    },
    summary: { valueType: 'template', value: 'Case {{ data.caseId }}' },
    meta: {
      valueType: 'composite',
      value: {
        flag: { valueType: 'immediate', value: true },
        nested: {
          valueType: 'composite',
          value: {
            inner: { valueType: 'reference', value: 'data.x' },
          },
        },
      },
    },
    count: { valueType: 'immediate', value: 5 },
  };

  function makeLogGraph() {
    return makeGraph({
      id: 'log',
      stepType: 'Log',
      message: 'hello',
      level: 'info',
      context: structuredClone(RICH_MAPPING),
      renderingParameters: { x: 0, y: 0 },
    });
  }

  function makeErrorGraph() {
    return makeGraph({
      id: 'err',
      stepType: 'Error',
      code: 'E_TEST',
      message: 'boom',
      category: 'permanent',
      severity: 'error',
      context: structuredClone(RICH_MAPPING),
      renderingParameters: { x: 0, y: 0 },
    });
  }

  function makeWaitGraph() {
    return makeGraph({
      id: 'wait',
      stepType: 'WaitForSignal',
      signal: 'approval',
      action: {
        key: 'approve',
        correlation: structuredClone(RICH_MAPPING),
        context: structuredClone(RICH_MAPPING),
      },
      renderingParameters: { x: 0, y: 0 },
    });
  }

  function editEntry(node: Node, type: string, patch: Record<string, unknown>) {
    const mapping = ((node.data as any).inputMapping || []) as any[];
    const idx = mapping.findIndex((item) => item.type === type);
    expect(idx, `inputMapping entry '${type}' missing`).toBeGreaterThanOrEqual(
      0
    );
    mapping[idx] = { ...mapping[idx], ...patch };
  }

  it('round-trips a rich Log.context unchanged', () => {
    const step = roundTripStep(makeLogGraph());
    expect(step.context).toEqual(RICH_MAPPING);
  });

  it('round-trips a rich Error.context unchanged', () => {
    const step = roundTripStep(makeErrorGraph());
    expect(step.context).toEqual(RICH_MAPPING);
  });

  it('round-trips rich WaitForSignal action correlation/context unchanged', () => {
    const step = roundTripStep(makeWaitGraph());
    expect(step.action.key).toBe('approve');
    expect(step.action.correlation).toEqual(RICH_MAPPING);
    expect(step.action.context).toEqual(RICH_MAPPING);
  });

  it('structured-editor write-back (normalized UI object) serializes identically', () => {
    // Simulate MappingObjectField: normalize the loaded value and write the
    // normalized object back into form state, then save.
    for (const [graph, stepId, entryType, extract] of [
      [makeLogGraph(), 'log', 'context', (s: any) => s.context],
      [makeErrorGraph(), 'err', 'context', (s: any) => s.context],
      [
        makeWaitGraph(),
        'wait',
        'actionCorrelation',
        (s: any) => s.action.correlation,
      ],
      [makeWaitGraph(), 'wait', 'actionContext', (s: any) => s.action.context],
    ] as const) {
      const { nodes, edges } = executionGraphToReactFlow(graph as any);
      const node = nodes.find((n) => n.id === stepId)!;
      const mapping = ((node.data as any).inputMapping || []) as any[];
      const entry = mapping.find((item) => item.type === entryType);
      expect(entry, `${stepId}.${entryType} entry missing`).toBeDefined();

      const normalized = normalizeMappingObject(entry.value);
      expect(normalized, `${stepId}.${entryType} not normalizable`).not.toBeNull();
      editEntry(node, entryType, { value: normalized, valueType: 'composite' });

      const round = composeExecutionGraph(nodes, edges, { name: 'norm' });
      expect(round).not.toBeNull();
      const step = (round!.steps as Record<string, any>)[stepId];
      expect(extract(step), `${stepId}.${entryType} drifted`).toEqual(
        RICH_MAPPING
      );
    }
  });

  it('an empty mapping editor ({}) clears Log.context like the empty textarea', () => {
    const { nodes, edges } = executionGraphToReactFlow(makeLogGraph() as any);
    const node = nodes.find((n) => n.id === 'log')!;
    editEntry(node, 'context', { value: {}, valueType: 'composite' });

    const round = composeExecutionGraph(nodes, edges, { name: 'log-clear' });
    const step = (round!.steps as Record<string, any>).log;
    expect(step).not.toHaveProperty('context');
    expect(step.message).toBe('hello');
  });

  it('an empty mapping editor ({}) clears Error.context like the empty textarea', () => {
    const { nodes, edges } = executionGraphToReactFlow(makeErrorGraph() as any);
    const node = nodes.find((n) => n.id === 'err')!;
    editEntry(node, 'context', { value: {}, valueType: 'composite' });

    const round = composeExecutionGraph(nodes, edges, { name: 'err-clear' });
    const step = (round!.steps as Record<string, any>).err;
    expect(step).not.toHaveProperty('context');
    expect(step.code).toBe('E_TEST');
  });

  it('empty mapping editors ({}) clear WaitForSignal action keys', () => {
    const { nodes, edges } = executionGraphToReactFlow(makeWaitGraph() as any);
    const node = nodes.find((n) => n.id === 'wait')!;
    editEntry(node, 'actionCorrelation', { value: {}, valueType: 'composite' });
    editEntry(node, 'actionContext', { value: {}, valueType: 'composite' });

    const round = composeExecutionGraph(nodes, edges, { name: 'wait-clear' });
    const step = (round!.steps as Record<string, any>).wait;
    // Key survives, the two cleared mapping objects are gone entirely.
    expect(step.action).toEqual({ key: 'approve' });

    // Clearing the key as well removes the whole action object.
    editEntry(node, 'actionKey', { value: '' });
    const round2 = composeExecutionGraph(nodes, edges, { name: 'wait-clear2' });
    const step2 = (round2!.steps as Record<string, any>).wait;
    expect(step2).not.toHaveProperty('action');
  });
});
