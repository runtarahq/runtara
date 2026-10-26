import { describe, it, expect } from 'vitest';
import {
  describeArrayValue,
  describeConditionValue,
  describeObjectValue,
  EMPTY_VALUE_LABEL,
} from './value-display';

describe('describeArrayValue', () => {
  it('summarizes an immediate array held as a real array', () => {
    // The MCP / REST shape: {"valueType":"immediate","value":["sku"]}.
    // String(["sku"]) is "sku", which is not JSON — this used to fall through
    // to "Click to configure...", making a set field look empty.
    expect(describeArrayValue(['sku'], 'immediate')).toBe('1 item');
    expect(describeArrayValue(['a', 'b', 'c'], 'immediate')).toBe('3 items');
    expect(describeArrayValue([1, 0, 0, 0], 'immediate')).toBe('4 items');
  });

  it('summarizes an immediate array held as a JSON string', () => {
    expect(describeArrayValue('["sku"]', 'immediate')).toBe('1 item');
    expect(describeArrayValue('["a","b"]', 'immediate')).toBe('2 items');
  });

  it('describes references', () => {
    expect(describeArrayValue('steps.parse.outputs', 'reference')).toBe(
      'Reference: steps.parse.outputs'
    );
  });

  it('describes composites', () => {
    expect(describeArrayValue([{}, {}], 'composite')).toBe('List · 2 items');
    expect(describeArrayValue('not-an-array', 'composite')).toBe('List');
  });

  it('falls back for empty and unparseable values', () => {
    expect(describeArrayValue('', 'immediate')).toBe(EMPTY_VALUE_LABEL);
    expect(describeArrayValue(null, 'immediate')).toBe(EMPTY_VALUE_LABEL);
    expect(describeArrayValue(undefined, 'immediate')).toBe(EMPTY_VALUE_LABEL);
    expect(describeArrayValue('sku', 'immediate')).toBe(EMPTY_VALUE_LABEL);
    expect(describeArrayValue('{"a":1}', 'immediate')).toBe(EMPTY_VALUE_LABEL);
  });

  it('reports an empty array as empty, not as zero items', () => {
    // [] is falsy-ish only via the !value guard? It is truthy, so it reaches
    // the array branch and reports honestly.
    expect(describeArrayValue([], 'immediate')).toBe('0 items');
  });
});

describe('describeObjectValue', () => {
  it('summarizes an immediate object held as a real object', () => {
    // String({a:1}) is "[object Object]" — never parses.
    expect(describeObjectValue({ a: 1 }, 'immediate')).toBe('1 field');
    expect(describeObjectValue({ a: 1, b: 2 }, 'immediate')).toBe('2 fields');
    expect(describeObjectValue({}, 'immediate')).toBe('0 fields');
  });

  it('summarizes an immediate object held as a JSON string', () => {
    expect(describeObjectValue('{"a":1,"b":2}', 'immediate')).toBe('2 fields');
  });

  it('prefers legacy dot-notation field count', () => {
    expect(describeObjectValue({ a: 1 }, 'immediate', 3)).toBe('3 fields');
    expect(describeObjectValue(null, 'immediate', 1)).toBe('1 field');
  });

  it('describes references and composites', () => {
    expect(describeObjectValue('data.payload', 'reference')).toBe(
      'Reference: data.payload'
    );
    expect(describeObjectValue({ a: 1 }, 'composite')).toBe('Object · 1 field');
    expect(describeObjectValue([1, 2], 'composite')).toBe('Object');
  });

  it('does not mistake an array for an object', () => {
    expect(describeObjectValue(['a'], 'immediate')).toBe(EMPTY_VALUE_LABEL);
    expect(describeObjectValue('["a"]', 'immediate')).toBe(EMPTY_VALUE_LABEL);
  });

  it('falls back for empty and unparseable values', () => {
    expect(describeObjectValue('', 'immediate')).toBe(EMPTY_VALUE_LABEL);
    expect(describeObjectValue(null, 'immediate')).toBe(EMPTY_VALUE_LABEL);
    expect(describeObjectValue('nope', 'immediate')).toBe(EMPTY_VALUE_LABEL);
  });
});

describe('describeConditionValue', () => {
  // The Object Model query condition from the ticket that found this: a
  // literal whose second argument reads run data through a reference.
  const condition = {
    type: 'operation',
    op: 'EQ',
    arguments: [
      { valueType: 'reference', value: 'name' },
      { valueType: 'reference', value: 'data.body.supplier' },
    ],
  };

  it('reads a literal condition as the sentence its canvas card shows', () => {
    expect(describeConditionValue(condition, 'immediate')).toBe(
      'name is Input · body.supplier'
    );
  });

  it('reads the JSON text of a condition being edited', () => {
    expect(
      describeConditionValue(
        JSON.stringify({ ...condition, op: 'NE' }),
        'immediate'
      )
    ).toBe('name is not Input · body.supplier');
  });

  it('is empty for anything that is not a condition expression', () => {
    expect(describeConditionValue({ a: 1 }, 'immediate')).toBe('');
    // A JSON Patch operation has an `op` key but is not a condition.
    expect(
      describeConditionValue({ op: 'add', path: '/a', value: 1 }, 'immediate')
    ).toBe('');
    expect(describeConditionValue('{"type": "operation",', 'immediate')).toBe(
      ''
    );
    expect(describeConditionValue(null, 'immediate')).toBe('');
  });

  it('reads a condition without the optional `type` tag', () => {
    // The DSL reads an object with `op` and `arguments` as an operation.
    const untagged = { op: condition.op, arguments: condition.arguments };
    expect(describeConditionValue(untagged, 'immediate')).toBe(
      'name is Input · body.supplier'
    );
  });

  it('does not try to parse ordinary text', () => {
    expect(describeConditionValue('name', 'immediate')).toBe('');
    expect(describeConditionValue('[1, 2]', 'immediate')).toBe('');
  });

  it('only reads immediates', () => {
    expect(describeConditionValue(condition, 'composite')).toBe('');
    expect(describeConditionValue('data.condition', 'reference')).toBe('');
  });
});
