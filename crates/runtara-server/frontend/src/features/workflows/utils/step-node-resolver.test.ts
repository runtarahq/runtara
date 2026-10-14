import { describe, expect, it } from 'vitest';
import { executionGraphToReactFlow } from '@/features/workflows/components/WorkflowEditor/CustomNodes/utils.tsx';
import {
  createGraphNodeResolver,
  createStepNodeResolver,
} from './step-node-resolver';
import { convertServerErrors } from './validation-helpers';

const finish = (id: string) => ({ id, stepType: 'Finish', inputMapping: {} });
const container = (id: string, stepType: string, steps: object) => ({
  id,
  stepType,
  config: { value: { valueType: 'reference', value: 'data.rows' } },
  subgraph: { entryPoint: 'iter_finish', steps, executionPlan: [] },
});

describe('createStepNodeResolver', () => {
  // Root `finish`, Splits `a` and `b` with an `iter_finish` each, and a While
  // `w` nested in `b` that also holds an `iter_finish`.
  const graph = {
    name: 'scopes',
    entryPoint: 'a',
    steps: {
      a: container('a', 'Split', { iter_finish: finish('iter_finish') }),
      b: container('b', 'Split', {
        iter_finish: finish('iter_finish'),
        w: container('w', 'While', { iter_finish: finish('iter_finish') }),
      }),
      finish: finish('finish'),
    },
    executionPlan: [],
  };
  const { nodes } = executionGraphToReactFlow(graph as any);
  const resolve = createStepNodeResolver(nodes);
  const parentOf = (nodeId: string) =>
    nodes.find((node) => node.id === nodeId)?.parentId;

  it('resolves a shared step id by the iteration scope of the event', () => {
    expect(parentOf(resolve('iter_finish', 'sc_a_0'))).toBe('a');
    expect(parentOf(resolve('iter_finish', 'sc_b_12'))).toBe('b');
    const inWhile = resolve('iter_finish', 'sc_b_1_w_3');
    expect(nodes.find((n) => n.id === parentOf(inWhile))?.data.id).toBe('w');
  });

  it('keeps unshared step ids and unscoped events on the plain node', () => {
    expect(resolve('finish', null)).toBe('finish');
    expect(resolve('a', undefined)).toBe('a');
    expect(resolve('iter_finish', null)).toBe('iter_finish');
    expect(resolve('not_on_canvas', 'sc_a_0')).toBe('not_on_canvas');
  });
});

describe('createGraphNodeResolver', () => {
  const graph = {
    name: 'graphs',
    entryPoint: 'a',
    steps: {
      a: container('a', 'Split', { iter_finish: finish('iter_finish') }),
      b: container('b', 'Split', {
        iter_finish: finish('iter_finish'),
        w: container('w', 'While', { iter_finish: finish('iter_finish') }),
      }),
      finish: finish('finish'),
    },
    executionPlan: [],
  };
  const { nodes } = executionGraphToReactFlow(graph as any);
  const find = createGraphNodeResolver(nodes);
  const parentStepOf = (nodeId?: string) =>
    nodes.find((n) => n.id === nodes.find((m) => m.id === nodeId)?.parentId)
      ?.data.id;

  it('resolves a shared step id by the reported body', () => {
    expect(parentStepOf(find('iter_finish', ['a']))).toBe('a');
    expect(parentStepOf(find('iter_finish', ['b']))).toBe('b');
    expect(parentStepOf(find('iter_finish', ['b', 'w']))).toBe('w');
  });

  it('resolves unshared ids without a path and misses unknown ones', () => {
    expect(find('finish')).toBe('finish');
    expect(find('nope', ['a'])).toBeUndefined();
  });

  it('points server errors at the node in the reported body', () => {
    const [message] = convertServerErrors(
      [
        {
          code: 'E003',
          message: 'unreachable',
          stepId: 'iter_finish',
          graphPath: ['b'],
        },
      ],
      nodes
    );
    expect(parentStepOf(message.stepId ?? undefined)).toBe('b');
  });
});
