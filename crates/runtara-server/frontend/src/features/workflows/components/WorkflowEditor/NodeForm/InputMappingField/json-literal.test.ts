import { describe, expect, it } from 'vitest';

import {
  isJsonLiteral,
  jsonLiteralText,
  parseJsonStructure,
} from './json-literal';

describe('isJsonLiteral', () => {
  it('is true for an immediate holding an object or an array', () => {
    expect(isJsonLiteral({ type: 'operation' }, 'immediate', 'auto')).toBe(
      true
    );
    expect(isJsonLiteral(['sku'], 'immediate', 'json')).toBe(true);
    // The field's own hint does not matter once the value is a structure.
    expect(isJsonLiteral({ a: 1 }, 'immediate', undefined)).toBe(true);
  });

  it('is false for a build or a reference, whatever the value', () => {
    expect(isJsonLiteral({ a: 1 }, 'composite', 'json-literal')).toBe(false);
    expect(isJsonLiteral('data.x', 'reference', 'json-literal')).toBe(false);
    expect(isJsonLiteral('{"a":1}', 'template', 'json-literal')).toBe(false);
  });

  it('is false for a plain scalar', () => {
    expect(isJsonLiteral('hello', 'immediate', 'auto')).toBe(false);
    expect(isJsonLiteral(5, 'immediate', 'integer')).toBe(false);
    expect(isJsonLiteral(null, 'immediate', undefined)).toBe(false);
  });

  it('reads any text from the JSON box as a literal, empty included', () => {
    expect(isJsonLiteral('{"a": 1', 'immediate', 'json-literal')).toBe(true);
    expect(isJsonLiteral('"hello {{name}}"', 'immediate', 'json-literal')).toBe(
      true
    );
    expect(isJsonLiteral('', 'immediate', 'json-literal')).toBe(true);
  });

  it('leaves a typed object field seed under `json` to Build', () => {
    // Typed object/array fields are seeded with the json hint and an empty
    // string; only the JSON box's own hint marks a literal.
    expect(isJsonLiteral('', 'immediate', 'json')).toBe(false);
    expect(isJsonLiteral('{"a": 1}', 'immediate', 'json')).toBe(false);
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

describe('parseJsonStructure', () => {
  it('returns the object or array the text describes', () => {
    expect(parseJsonStructure('{"a": "{{x}}"}')).toEqual({ a: '{{x}}' });
    expect(parseJsonStructure('[1, 2]')).toEqual([1, 2]);
  });

  it('is undefined for a JSON scalar, an empty string, or broken JSON', () => {
    expect(parseJsonStructure('"abc"')).toBeUndefined();
    expect(parseJsonStructure('12')).toBeUndefined();
    expect(parseJsonStructure('null')).toBeUndefined();
    expect(parseJsonStructure('')).toBeUndefined();
    expect(parseJsonStructure('{"a":')).toBeUndefined();
  });
});
