import { describe, it, expect } from 'vitest';
import { coerceValueForMode, nextValueMode } from './value-mode';

describe('nextValueMode', () => {
  it('cycles immediate → template → reference → composite → immediate', () => {
    expect(nextValueMode('immediate')).toBe('template');
    expect(nextValueMode('template')).toBe('reference');
    expect(nextValueMode('reference')).toBe('composite');
    expect(nextValueMode('composite')).toBe('immediate');
  });
});

describe('coerceValueForMode', () => {
  it('carries text across every string-mode transition', () => {
    // The bug this replaces: each of these used to write '' to the store, so
    // one stray click on a 36px icon deleted a saved reference path.
    const pairs = [
      ['immediate', 'template'],
      ['template', 'reference'],
      ['reference', 'immediate'],
      ['immediate', 'reference'],
      ['reference', 'template'],
      ['template', 'immediate'],
    ] as const;
    for (const [from, to] of pairs) {
      const result = coerceValueForMode('steps.parse.outputs', from, to);
      expect(result.changed).toBe(false);
      expect(result.value).toBe('steps.parse.outputs');
    }
  });

  it('is a no-op when the mode does not change', () => {
    const result = coerceValueForMode('x', 'immediate', 'immediate');
    expect(result).toEqual({ changed: false, value: 'x' });
  });

  it('seeds an empty object entering composite from a scalar', () => {
    expect(coerceValueForMode('', 'immediate', 'composite')).toEqual({
      changed: true,
      value: {},
    });
  });

  it('seeds an empty array entering composite on an array-typed field', () => {
    for (const fieldType of [
      'array',
      'array<string>',
      'string[]',
      '[string]',
    ]) {
      expect(
        coerceValueForMode('', 'immediate', 'composite', fieldType)
      ).toEqual({ changed: true, value: [] });
    }
  });

  it('keeps an existing structure entering composite', () => {
    const existing = { a: 1 };
    const result = coerceValueForMode(existing, 'immediate', 'composite');
    expect(result.changed).toBe(false);
    expect(result.value).toBe(existing);

    const arr = [1, 2];
    expect(coerceValueForMode(arr, 'reference', 'composite').value).toBe(arr);
  });

  it('serializes a composite when leaving for a string mode', () => {
    expect(coerceValueForMode({ a: 1 }, 'composite', 'immediate')).toEqual({
      changed: true,
      value: '{"a":1}',
    });
    expect(coerceValueForMode(['sku'], 'composite', 'immediate')).toEqual({
      changed: true,
      value: '["sku"]',
    });
  });

  it('clears when leaving an empty composite', () => {
    expect(coerceValueForMode({}, 'composite', 'immediate')).toEqual({
      changed: true,
      value: '',
    });
    expect(coerceValueForMode([], 'composite', 'immediate')).toEqual({
      changed: true,
      value: '',
    });
  });

  it('clears when leaving composite with a non-structural value', () => {
    expect(coerceValueForMode('oops', 'composite', 'immediate')).toEqual({
      changed: true,
      value: '',
    });
    expect(coerceValueForMode(null, 'composite', 'template')).toEqual({
      changed: true,
      value: '',
    });
  });

  it('round-trips a composite through immediate without losing content', () => {
    const original = { sku: 'A', qty: 2 };
    const out = coerceValueForMode(original, 'composite', 'immediate');
    expect(JSON.parse(out.value as string)).toEqual(original);
  });

  it('never returns changed:true for a string-mode pair, however odd', () => {
    // A template body read as a reference path is wrong, but it is visible and
    // fixable — which beats silently emptying the field.
    const out = coerceValueForMode('Hello {{name}}', 'template', 'reference');
    expect(out.changed).toBe(false);
    expect(out.value).toBe('Hello {{name}}');
  });
});
