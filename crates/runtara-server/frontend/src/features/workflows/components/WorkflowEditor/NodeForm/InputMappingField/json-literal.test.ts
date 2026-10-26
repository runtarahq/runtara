import { describe, expect, it } from 'vitest';

import { isJsonLiteral, jsonLiteralText } from './json-literal';

describe('isJsonLiteral', () => {
  it('is true for an immediate holding an object or an array', () => {
    expect(
      isJsonLiteral({ type: 'operation' }, 'immediate', 'auto', true)
    ).toBe(true);
    expect(isJsonLiteral(['sku'], 'immediate', 'json', false)).toBe(true);
    // The field's own hint does not matter once the value is a structure.
    expect(isJsonLiteral({ a: 1 }, 'immediate', undefined, false)).toBe(true);
  });

  it('is false for a build or a reference, whatever the value', () => {
    expect(isJsonLiteral({ a: 1 }, 'composite', 'json', true)).toBe(false);
    expect(isJsonLiteral('data.x', 'reference', 'json', true)).toBe(false);
    expect(isJsonLiteral('{"a":1}', 'template', 'json', true)).toBe(false);
  });

  it('is false for a plain scalar without the json hint', () => {
    expect(isJsonLiteral('hello', 'immediate', 'auto', true)).toBe(false);
    expect(isJsonLiteral(5, 'immediate', 'integer', true)).toBe(false);
    expect(isJsonLiteral(null, 'immediate', undefined, true)).toBe(false);
  });

  it('reads the text of a literal being edited as a literal', () => {
    expect(isJsonLiteral('{"a": 1', 'immediate', 'json', true)).toBe(true);
    expect(isJsonLiteral('{"a": 1', 'immediate', 'json', false)).toBe(true);
  });

  it('keeps an untyped field in JSON once cleared, but not a typed seed', () => {
    // On an `any` field only an edit sets the json hint, so the cleared text
    // is still the literal being written.
    expect(isJsonLiteral('', 'immediate', 'json', true)).toBe(true);
    // A typed object/array field is seeded with the json hint and an empty
    // string — that is the untouched field, which opens in Build.
    expect(isJsonLiteral('', 'immediate', 'json', false)).toBe(false);
  });
});

describe('jsonLiteralText', () => {
  it('pretty-prints a structure', () => {
    expect(jsonLiteralText({ op: 'EQ', arguments: [1] })).toBe(
      '{\n  "op": "EQ",\n  "arguments": [\n    1\n  ]\n}'
    );
    expect(jsonLiteralText(['a', 'b'])).toBe('[\n  "a",\n  "b"\n]');
  });

  it('leaves text being edited exactly as typed', () => {
    expect(jsonLiteralText('{"op":')).toBe('{"op":');
  });

  it('shows nothing for null or undefined', () => {
    expect(jsonLiteralText(null)).toBe('');
    expect(jsonLiteralText(undefined)).toBe('');
  });
});
