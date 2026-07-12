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
      { name: 'data', type: 'object' },
      { name: 'stats', type: 'object' },
      { name: 'hasFailures', type: 'boolean' },
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
  name: string
): ExecutionGraph {
  return {
    entryPoint: stepId,
    executionPlan: [{ fromStep: stepId, toStep: 'probe' }],
    steps: {
      [stepId]: { id: stepId, name, stepType },
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

  it('types Split outputs as an array and suggests its sibling fields', () => {
    const [split] = previousStepsFor(
      graphWithUpstream('split', 'Split', 'Split items')
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

describe('composeVariableSuggestions sibling labels', () => {
  beforeEach(() => {
    __setStepOutputShapesForTests(SHAPES);
  });

  afterEach(() => {
    __resetStepOutputShapesForTests();
  });

  it('labels sibling fields by name instead of the raw path', () => {
    const previousSteps = previousStepsFor(
      graphWithUpstream('split', 'Split', 'Split items')
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
