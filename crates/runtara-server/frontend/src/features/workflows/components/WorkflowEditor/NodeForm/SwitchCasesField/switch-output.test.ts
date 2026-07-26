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

  it('reads a plain object or array as composite', () => {
    const obj = { label: 'x', id: { valueType: 'reference', value: 'a.b' } };
    expect(readSwitchOutput(obj)).toEqual({ mode: 'composite', value: obj });
    const arr = [1, 2];
    expect(readSwitchOutput(arr)).toEqual({ mode: 'composite', value: arr });
  });

  it('treats an unknown wrapper as a plain object, matching the runtime', () => {
    const tpl = { valueType: 'template', value: 'Hello {{name}}' };
    expect(readSwitchOutput(tpl)).toEqual({ mode: 'composite', value: tpl });
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

  it('passes a structure through for composite', () => {
    const obj = { a: 1 };
    expect(writeSwitchOutput({ mode: 'composite', value: obj })).toBe(obj);
    expect(writeSwitchOutput({ mode: 'composite', value: 'oops' })).toEqual({});
  });

  it('round-trips every mode', () => {
    for (const stored of [
      'Poland',
      42,
      { valueType: 'reference', value: 'data.x' },
      { label: 'x' },
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
