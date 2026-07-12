import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  __resetStepOutputShapesForTests,
  __setStepOutputShapesForTests,
  OutputShapeJson,
} from '@/features/workflows/utils/step-output-shapes';
import { composePreviousSteps } from './shared';
import { composeVariableSuggestions } from '../NodeForm/InputMappingValueField/VariableSuggestions';
import type { ExecutionGraph } from '../CustomNodes/utils.tsx';

/**
 * Mirrors what runtara-dsl's output_shape_json emits for the step types the
 * tests exercise. The real payload is asserted against the WASM in
 * rust-workflow-validation.test.ts ("serves typed output shapes…"); these
 * fixtures only need the same structure.
 */
const SHAPES: Record<string, OutputShapeJson> = {
  While: {
    outputs: {
      kind: 'object',
      fields: [
        { name: 'iterations', type: 'integer' },
        { name: 'outputs', type: 'dynamic' },
      ],
    },
    siblingFields: [],
  },
  Split: {
    outputs: { kind: 'array' },
    siblingFields: [
      { name: 'data', type: 'object', gatedBy: 'dontStopOnFailed' },
      { name: 'stats', type: 'object', gatedBy: 'dontStopOnFailed' },
      { name: 'hasFailures', type: 'boolean', gatedBy: 'dontStopOnFailed' },
    ],
  },
  Conditional: {
    outputs: {
      kind: 'object',
      fields: [{ name: 'result', type: 'boolean' }],
    },
    siblingFields: [],
  },
  Switch: {
    outputs: { kind: 'dynamic' },
    siblingFields: [{ name: 'route', type: 'string' }],
  },
  Filter: {
    outputs: {
      kind: 'object',
      fields: [
        { name: 'items', type: 'array' },
        { name: 'count', type: 'integer' },
      ],
    },
    siblingFields: [],
  },
};

function graphWithUpstream(
  stepId: string,
  stepType: string,
  name: string,
  config?: Record<string, unknown>
): ExecutionGraph {
  return {
    entryPoint: stepId,
    executionPlan: [{ fromStep: stepId, toStep: 'probe' }],
    steps: {
      [stepId]: { id: stepId, name, stepType, config },
      probe: { id: 'probe', name: 'Probe', stepType: 'Agent' },
    },
  } as unknown as ExecutionGraph;
}

function previousStepsFor(graph: ExecutionGraph) {
  return composePreviousSteps({
    stepId: 'probe',
    agents: [],
    executionGraph: graph,
    workflows: [],
  });
}

describe('composePreviousSteps control-step output shapes', () => {
  beforeEach(() => {
    __setStepOutputShapesForTests(SHAPES);
  });

  afterEach(() => {
    __resetStepOutputShapesForTests();
  });

  it('suggests While outputs under steps.<id>.outputs, not the null sibling path', () => {
    const [loop] = previousStepsFor(
      graphWithUpstream('loop', 'While', 'Retry loop')
    );

    const paths = loop.outputs.map((o) => o.path);
    // The old hand-coded suggestion steps['loop'].iterations resolves to null
    // at runtime; the canonical location is under outputs.
    expect(paths).toContain("steps['loop'].outputs.iterations");
    expect(paths).not.toContain("steps['loop'].iterations");

    const iterations = loop.outputs.find((o) => o.name === 'iterations');
    expect(iterations?.type).toBe('integer');

    // The last iteration's Finish outputs are runtime-shaped: no fake type.
    const lastOutputs = loop.outputs.find(
      (o) => o.path === "steps['loop'].outputs.outputs"
    );
    expect(lastOutputs?.type).toBeUndefined();
  });

  it('types Split outputs as an array and suggests siblings when the gate is on', () => {
    const [split] = previousStepsFor(
      graphWithUpstream('split', 'Split', 'Split items', {
        dontStopOnFailed: true,
      })
    );

    const whole = split.outputs.find(
      (o) => o.path === "steps['split'].outputs"
    );
    expect(whole?.type).toBe('array');

    const byPath = Object.fromEntries(
      split.outputs.map((o) => [o.path, o.type])
    );
    expect(byPath["steps['split'].data"]).toBe('object');
    expect(byPath["steps['split'].stats"]).toBe('object');
    expect(byPath["steps['split'].hasFailures"]).toBe('boolean');
  });

  it('hides config-gated Split siblings when dontStopOnFailed is off', () => {
    // Without the gate the runtime never writes data/stats/hasFailures —
    // suggesting them would recreate the silent-null reference class.
    const [split] = previousStepsFor(
      graphWithUpstream('split', 'Split', 'Split items')
    );

    const paths = split.outputs.map((o) => o.path);
    expect(paths).toContain("steps['split'].outputs");
    expect(paths).not.toContain("steps['split'].hasFailures");
    expect(paths).not.toContain("steps['split'].stats");
    expect(paths).not.toContain("steps['split'].data");
  });

  it('suggests Conditional result as boolean', () => {
    const [branch] = previousStepsFor(
      graphWithUpstream('branch', 'Conditional', 'Branch on flag')
    );

    expect(branch.outputs).toEqual([
      expect.objectContaining({
        name: 'result',
        type: 'boolean',
        path: "steps['branch'].outputs.result",
      }),
    ]);
  });

  it('suggests Switch route sibling and leaves dynamic outputs untyped', () => {
    const [sw] = previousStepsFor(graphWithUpstream('sw', 'Switch', 'Route'));

    const whole = sw.outputs.find((o) => o.path === "steps['sw'].outputs");
    expect(whole).toBeDefined();
    expect(whole?.type).toBeUndefined();

    const route = sw.outputs.find((o) => o.path === "steps['sw'].route");
    expect(route?.type).toBe('string');
  });

  it('types Filter fields from the shape table', () => {
    const [filter] = previousStepsFor(
      graphWithUpstream('filt', 'Filter', 'Filter results')
    );

    const byName = Object.fromEntries(
      filter.outputs.map((o) => [o.name, o.type])
    );
    expect(byName['items']).toBe('array');
    expect(byName['count']).toBe('integer');
  });

  it('falls back to a generic outputs suggestion when the shape cache is cold', () => {
    __resetStepOutputShapesForTests();

    const [loop] = previousStepsFor(
      graphWithUpstream('loop', 'While', 'Retry loop')
    );

    expect(loop.outputs).toEqual([
      expect.objectContaining({
        path: "steps['loop'].outputs",
        type: 'object',
      }),
    ]);
  });
});

describe('composePreviousSteps agent output nesting', () => {
  const AGENTS = [
    {
      id: 'http',
      name: 'HTTP',
      supportedCapabilities: {
        'http-request': {
          id: 'http-request',
          inputs: [],
          output: {
            type: 'object',
            fields: [
              { name: 'status_code', type: 'integer' },
              {
                name: 'body',
                type: 'object',
                fields: [
                  { name: 'token', type: 'string' },
                  {
                    name: 'meta',
                    type: 'object',
                    fields: [{ name: 'count', type: 'integer' }],
                  },
                ],
              },
              { name: 'headers', type: 'object' },
            ],
          },
        },
      },
    },
  ] as any;

  function agentGraph(): ExecutionGraph {
    return {
      entryPoint: 'fetch',
      executionPlan: [{ fromStep: 'fetch', toStep: 'probe' }],
      steps: {
        fetch: {
          id: 'fetch',
          name: 'Fetch page',
          stepType: 'Agent',
          agentId: 'http',
          capabilityId: 'http-request',
        },
        probe: { id: 'probe', name: 'Probe', stepType: 'Agent' },
      },
    } as unknown as ExecutionGraph;
  }

  it('recurses nested output fields with types', () => {
    const [fetch] = composePreviousSteps({
      stepId: 'probe',
      agents: AGENTS,
      executionGraph: agentGraph(),
      workflows: [],
    });

    const suggestions = composeVariableSuggestions([fetch]);
    const byValue = Object.fromEntries(
      suggestions
        .filter((s) => s.group === 'Step Outputs')
        .map((s) => [s.value, s])
    );

    expect(byValue["steps['fetch'].outputs.status_code"]?.type).toBe(
      'integer'
    );
    // Nested object fields are suggested with dotted labels and types.
    expect(byValue["steps['fetch'].outputs.body.token"]?.type).toBe('string');
    expect(byValue["steps['fetch'].outputs.body.token"]?.label).toBe(
      'body.token'
    );
    expect(byValue["steps['fetch'].outputs.body.meta.count"]?.type).toBe(
      'integer'
    );
    // Objects without declared children stay leaf-level.
    expect(byValue["steps['fetch'].outputs.headers"]?.type).toBe('object');
  });
});

describe('composeVariableSuggestions sibling labels', () => {
  beforeEach(() => {
    __setStepOutputShapesForTests(SHAPES);
  });

  afterEach(() => {
    __resetStepOutputShapesForTests();
  });

  it('labels sibling fields by name instead of the raw path', () => {
    const previousSteps = previousStepsFor(
      graphWithUpstream('split', 'Split', 'Split items', {
        dontStopOnFailed: true,
      })
    );

    const suggestions = composeVariableSuggestions(previousSteps);
    const stats = suggestions.find(
      (s) => s.value === "steps['split'].stats"
    );
    expect(stats?.label).toBe('stats');
    expect(stats?.group).toBe('Step Outputs');
    expect(stats?.type).toBe('object');

    const hasFailures = suggestions.find(
      (s) => s.value === "steps['split'].hasFailures"
    );
    expect(hasFailures?.label).toBe('hasFailures');
    expect(hasFailures?.type).toBe('boolean');
  });
});
