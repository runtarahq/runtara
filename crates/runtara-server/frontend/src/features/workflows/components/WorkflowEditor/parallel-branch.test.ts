import { describe, expect, it } from 'vitest';
import { findBranchJoin, unconditionalTargets } from './parallel-branch';

const e = (source: string, target: string, sourceHandle = 'source') => ({
  source,
  target,
  sourceHandle,
});

describe('findBranchJoin', () => {
  it("is the route's target for a step with a single route", () => {
    expect(findBranchJoin([e('a', 'c')], 'a')).toBe('c');
  });

  it('is where an existing fan-out re-converges', () => {
    // A → B → C, A → B1.1 → C: a third branch rejoins at C.
    const edges = [e('a', 'b'), e('a', 'b11'), e('b', 'c'), e('b11', 'c')];
    expect(findBranchJoin(edges, 'a')).toBe('c');
  });

  it('picks the nearest common step when branches have different lengths', () => {
    const edges = [
      e('a', 'b'),
      e('b', 'b2'),
      e('b2', 'c'),
      e('a', 'x'),
      e('x', 'c'),
      e('c', 'd'),
    ];
    expect(findBranchJoin(edges, 'a')).toBe('c');
  });

  it('treats a branch that goes straight to the join as reaching it', () => {
    const edges = [e('a', 'b'), e('a', 'c'), e('b', 'c')];
    expect(findBranchJoin(edges, 'a')).toBe('c');
  });

  it('is null when the branches never meet', () => {
    const edges = [e('a', 'b'), e('a', 'x')];
    expect(findBranchJoin(edges, 'a')).toBeNull();
  });

  it('ignores conditional and non-default routes', () => {
    const edges = [e('a', 'c'), e('a', 'err', 'onError'), e('a', 'z')];
    const conditional = (edge: { target: string }) => edge.target !== 'z';
    expect(unconditionalTargets(edges, 'a', conditional)).toEqual(['c']);
    expect(findBranchJoin(edges, 'a', conditional)).toBe('c');
  });
});
