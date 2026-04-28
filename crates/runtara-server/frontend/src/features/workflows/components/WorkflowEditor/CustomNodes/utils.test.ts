import { describe, it, expect } from 'vitest';
import { composeExecutionGraph, executionGraphToReactFlow } from './utils.tsx';
import type { ExecutionGraphDto } from '@/features/workflows/types/execution-graph';

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
          type: 'string',
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
        type: 'integer',
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

  it('preserves immediate with integer type', () => {
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
      type: 'integer',
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
    expect(out.type).toBe('integer');
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
});

describe('Split variable round-trip', () => {
  it('preserves numeric immediate variable (no JSON.stringify on load)', () => {
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
      type: 'integer',
    });
  });

  it('preserves boolean immediate variable', () => {
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
      type: 'boolean',
    });
  });

  it('preserves composite array variable', () => {
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
    expect(step.config.variables.payload.type).toBe('array');
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
