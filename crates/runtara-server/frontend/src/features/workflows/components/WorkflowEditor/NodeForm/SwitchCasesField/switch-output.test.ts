import { describe, it, expect } from 'vitest';
import {
  readSwitchOutput,
  writeSwitchOutput,
  describeSwitchOutput,
  SWITCH_OUTPUT_MODES,
} from './switch-output';

describe('SWITCH_OUTPUT_MODES', () => {
  it('excludes template, which the runtime does not resolve', () => {
    // process_switch_output handles reference/immediate wrappers and recurses
    // everything else, so a {valueType:'template'} wrapper is emitted as a
    // literal object. Offering the mode would produce silently-broken output.
    expect(SWITCH_OUTPUT_MODES).not.toContain('template');
    expect([...SWITCH_OUTPUT_MODES]).toEqual([
      'immediate',
      'reference',
      'composite',
    ]);
  });
});

describe('readSwitchOutput', () => {
  it('reads a bare scalar as immediate', () => {
    expect(readSwitchOutput('Poland')).toEqual({
      mode: 'immediate',
      value: 'Poland',
    });
    expect(readSwitchOutput(42)).toEqual({ mode: 'immediate', value: 42 });
    expect(readSwitchOutput(true)).toEqual({ mode: 'immediate', value: true });
  });

  it('reads a reference wrapper', () => {
    expect(
      readSwitchOutput({ valueType: 'reference', value: 'steps.a.outputs.x' })
    ).toEqual({ mode: 'reference', value: 'steps.a.outputs.x' });
  });

  it('unwraps an immediate wrapper', () => {
    expect(readSwitchOutput({ valueType: 'immediate', value: 7 })).toEqual({
      mode: 'immediate',
      value: 7,
    });
  });

  it('reads a plain object or array as composite editor entries', () => {
    // The composite editor needs a {valueType, value} entry per field; a bare
    // literal used to render as "Unknown type" with a validation error.
    const obj = { label: 'x', id: { valueType: 'reference', value: 'a.b' } };
    expect(readSwitchOutput(obj)).toEqual({
      mode: 'composite',
      value: {
        label: { valueType: 'immediate', value: 'x' },
        id: { valueType: 'reference', value: 'a.b' },
      },
    });
    expect(readSwitchOutput([1, 2])).toEqual({
      mode: 'composite',
      value: [
        { valueType: 'immediate', value: 1 },
        { valueType: 'immediate', value: 2 },
      ],
    });
  });

  it('reads nested literal structures as nested composites', () => {
    expect(readSwitchOutput({ route: { queue: 'web', tags: ['a'] } })).toEqual({
      mode: 'composite',
      value: {
        route: {
          valueType: 'composite',
          value: {
            queue: { valueType: 'immediate', value: 'web' },
            tags: {
              valueType: 'composite',
              value: [{ valueType: 'immediate', value: 'a' }],
            },
          },
        },
      },
    });
  });

  it('keeps a string hint on strings that look like numbers or booleans', () => {
    expect(readSwitchOutput({ code: '123', flag: 'true' })).toEqual({
      mode: 'composite',
      value: {
        code: { valueType: 'immediate', value: '123', typeHint: 'string' },
        flag: { valueType: 'immediate', value: 'true', typeHint: 'string' },
      },
    });
  });

  it('treats an unknown wrapper as a plain object, matching the runtime', () => {
    const tpl = { valueType: 'template', value: 'Hello {{name}}' };
    expect(readSwitchOutput(tpl)).toEqual({
      mode: 'composite',
      value: {
        valueType: { valueType: 'immediate', value: 'template' },
        value: { valueType: 'immediate', value: 'Hello {{name}}' },
      },
    });
  });

  it('reads an editor composite wrapper as the structure it holds', () => {
    expect(
      readSwitchOutput({ valueType: 'composite', value: { queue: 'web' } })
    ).toEqual({
      mode: 'composite',
      value: { queue: { valueType: 'immediate', value: 'web' } },
    });
  });

  it('reads null/undefined as an empty immediate', () => {
    expect(readSwitchOutput(null)).toEqual({ mode: 'immediate', value: '' });
    expect(readSwitchOutput(undefined)).toEqual({
      mode: 'immediate',
      value: '',
    });
  });
});

describe('writeSwitchOutput', () => {
  it('keeps immediate scalars bare rather than wrapping them', () => {
    // The runtime's literal arm expects this, and every existing workflow
    // already stores it this way — wrapping would rewrite untouched cases.
    expect(writeSwitchOutput({ mode: 'immediate', value: 'Poland' })).toBe(
      'Poland'
    );
    expect(writeSwitchOutput({ mode: 'immediate', value: 0 })).toBe(0);
  });

  it('wraps a reference', () => {
    expect(
      writeSwitchOutput({ mode: 'reference', value: 'data.country' })
    ).toEqual({ valueType: 'reference', value: 'data.country' });
  });

  it('coerces a non-string reference to empty', () => {
    expect(writeSwitchOutput({ mode: 'reference', value: 42 })).toEqual({
      valueType: 'reference',
      value: '',
    });
  });

  it('unwraps composite editor entries to the shape the runtime reads', () => {
    expect(
      writeSwitchOutput({
        mode: 'composite',
        value: {
          queue: { valueType: 'immediate', value: 'web' },
          id: { valueType: 'reference', value: 'data.id' },
          nested: {
            valueType: 'composite',
            value: [{ valueType: 'immediate', value: 1 }],
          },
        },
      })
    ).toEqual({
      queue: 'web',
      id: { valueType: 'reference', value: 'data.id' },
      nested: [1],
    });
    expect(writeSwitchOutput({ mode: 'composite', value: 'oops' })).toEqual({});
  });

  it('applies the immediate type hint the composite editor defers', () => {
    expect(
      writeSwitchOutput({
        mode: 'composite',
        value: {
          count: { valueType: 'immediate', value: '12', typeHint: 'integer' },
          code: { valueType: 'immediate', value: '12', typeHint: 'string' },
          meta: { valueType: 'immediate', value: '{"a":1}', typeHint: 'json' },
        },
      })
    ).toEqual({
      count: 12,
      code: '12',
      meta: { valueType: 'immediate', value: { a: 1 } },
    });
  });

  it('round-trips every mode', () => {
    for (const stored of [
      'Poland',
      42,
      { valueType: 'reference', value: 'data.x' },
      { label: 'x' },
      { queue: 'web-team' },
      {
        code: '123',
        nested: {
          tags: ['a', 2, true],
          id: { valueType: 'reference', value: 'data.id' },
        },
      },
      [1, 2, 3],
    ]) {
      expect(writeSwitchOutput(readSwitchOutput(stored))).toEqual(stored);
    }
  });
});

describe('describeSwitchOutput', () => {
  it('summarizes each shape', () => {
    expect(describeSwitchOutput('Poland')).toBe('Poland');
    expect(describeSwitchOutput({ valueType: 'reference', value: 'a.b' })).toBe(
      'Reference: a.b'
    );
    expect(describeSwitchOutput({ a: 1, b: 2 })).toBe('2 fields');
    expect(describeSwitchOutput([1])).toBe('1 item');
    expect(describeSwitchOutput('')).toBe('(empty)');
    expect(describeSwitchOutput(null)).toBe('(empty)');
  });
});
