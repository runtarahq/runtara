import { describe, it, expect } from 'vitest';
import {
  carryConditionArgs,
  emptyConditionArgument,
  operatorChangeDropsArgs,
} from './condition-args';

const arg = (value: string) => ({
  valueType: 'immediate' as const,
  value,
  immediateType: 'string' as const,
});

describe('carryConditionArgs', () => {
  it('keeps both operands across a BINARY → BINARY change', () => {
    // Equals → Not equals used to wipe both sides.
    const existing = [arg('steps.a.outputs.status'), arg('active')];
    expect(carryConditionArgs(existing, 'BINARY')).toEqual(existing);
  });

  it('keeps the first operand when narrowing BINARY → UNARY', () => {
    const existing = [arg('steps.a.outputs.value'), arg('42')];
    const out = carryConditionArgs(existing, 'UNARY');
    expect(out).toHaveLength(1);
    expect(out[0]).toEqual(arg('steps.a.outputs.value'));
  });

  it('pads when widening UNARY → BINARY, keeping what was there', () => {
    const out = carryConditionArgs([arg('steps.a.outputs.value')], 'BINARY');
    expect(out).toHaveLength(2);
    expect(out[0]).toEqual(arg('steps.a.outputs.value'));
    expect(out[1]).toEqual(emptyConditionArgument());
  });

  it('keeps every nested condition across AND → OR', () => {
    // VARIADIC → VARIADIC: the classic case where sub-conditions vanished.
    const nested = [
      { type: 'operation', op: 'EQ', arguments: [arg('a'), arg('b')] },
      { type: 'operation', op: 'GT', arguments: [arg('c'), arg('1')] },
      { type: 'operation', op: 'LT', arguments: [arg('d'), arg('9')] },
    ];
    expect(carryConditionArgs(nested, 'VARIADIC')).toEqual(nested);
  });

  it('gives VARIADIC one empty slot when there is nothing to carry', () => {
    expect(carryConditionArgs([], 'VARIADIC')).toEqual([
      emptyConditionArgument(),
    ]);
    expect(carryConditionArgs(undefined, 'VARIADIC')).toEqual([
      emptyConditionArgument(),
    ]);
  });

  it('fills a required arity from nothing', () => {
    expect(carryConditionArgs(undefined, 'BINARY')).toEqual([
      emptyConditionArgument(),
      emptyConditionArgument(),
    ]);
    expect(carryConditionArgs([], 'UNARY')).toEqual([emptyConditionArgument()]);
  });

  it('does not mutate the input', () => {
    const existing = [arg('a'), arg('b'), arg('c')];
    const copy = [...existing];
    carryConditionArgs(existing, 'UNARY');
    expect(existing).toEqual(copy);
  });

  it('keeps a VARIADIC list of any length', () => {
    const many = Array.from({ length: 7 }, (_, i) => arg(String(i)));
    expect(carryConditionArgs(many, 'VARIADIC')).toHaveLength(7);
  });
});

describe('operatorChangeDropsArgs', () => {
  it('reports truncation only when operands would actually be lost', () => {
    expect(operatorChangeDropsArgs([arg('a'), arg('b')], 'UNARY')).toBe(true);
    expect(operatorChangeDropsArgs([arg('a')], 'UNARY')).toBe(false);
    expect(operatorChangeDropsArgs([arg('a'), arg('b')], 'BINARY')).toBe(false);
  });

  it('never reports truncation for VARIADIC', () => {
    const many = Array.from({ length: 9 }, (_, i) => arg(String(i)));
    expect(operatorChangeDropsArgs(many, 'VARIADIC')).toBe(false);
  });

  it('handles a missing list', () => {
    expect(operatorChangeDropsArgs(undefined, 'UNARY')).toBe(false);
  });
});
