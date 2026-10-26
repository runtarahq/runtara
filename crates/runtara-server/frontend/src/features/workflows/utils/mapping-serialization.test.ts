import { describe, expect, it } from 'vitest';

import {
  JSON_LITERAL_HINT,
  isJsonTextHint,
  jsonTextError,
  parseJsonText,
  processMappingEntry,
} from './mapping-serialization';

describe('isJsonTextHint', () => {
  it('is true for the hints the save parses as JSON', () => {
    expect(isJsonTextHint('json')).toBe(true);
    expect(isJsonTextHint('object')).toBe(true);
    expect(isJsonTextHint('array')).toBe(true);
    expect(isJsonTextHint(JSON_LITERAL_HINT)).toBe(true);
    expect(isJsonTextHint('string')).toBe(false);
    expect(isJsonTextHint('auto')).toBe(false);
    expect(isJsonTextHint(undefined)).toBe(false);
  });
});

describe('parseJsonText', () => {
  it('parses JSON text', () => {
    expect(parseJsonText('{"a": 1}')).toEqual({ a: 1 });
    expect(parseJsonText('12')).toBe(12);
  });

  it('keeps text holding {{ or text that does not parse', () => {
    expect(parseJsonText('["{{data.tags}}"]')).toBe('["{{data.tags}}"]');
    expect(parseJsonText('{"a":')).toBe('{"a":');
  });

  it("parses the JSON box's text whole, {{ included", () => {
    expect(parseJsonText('"hello {{name}}"', JSON_LITERAL_HINT)).toBe(
      'hello {{name}}'
    );
    expect(parseJsonText('["{{data.tags}}"]', JSON_LITERAL_HINT)).toEqual([
      '{{data.tags}}',
    ]);
    expect(parseJsonText('{"a":', JSON_LITERAL_HINT)).toBe('{"a":');
  });

  it('is the rule processMappingEntry saves immediates by', () => {
    for (const typeHint of ['json', JSON_LITERAL_HINT]) {
      for (const text of [
        '{"a": 1}',
        '["{{x}}"]',
        '"hi {{x}}"',
        '{"a":',
        '7',
      ]) {
        const [, saved] = processMappingEntry({
          type: 'field',
          value: text,
          valueType: 'immediate',
          typeHint,
        });
        expect((saved as { value: unknown; type?: string }).value).toEqual(
          parseJsonText(text, typeHint)
        );
        // Neither hint is a backend hint.
        expect((saved as { type?: string }).type).toBeUndefined();
      }
    }
  });
});

describe('jsonTextError', () => {
  const immediate = (value: unknown, typeHint?: string) => ({
    value,
    typeHint,
    valueType: 'immediate',
  });

  it('reports text under a JSON hint that does not parse', () => {
    expect(jsonTextError(immediate('{"a":', 'json'))).toMatch(
      /^Invalid JSON — /
    );
    expect(jsonTextError(immediate('{"a":', JSON_LITERAL_HINT))).toMatch(
      /^Invalid JSON — /
    );
  });

  it("checks the JSON box's text even when it holds {{", () => {
    expect(
      jsonTextError(immediate('{"a":"{{name}}",', JSON_LITERAL_HINT))
    ).toMatch(/^Invalid JSON — /);
    // Elsewhere that is template text, resolved at run time.
    expect(
      jsonTextError(immediate('{"a":"{{name}}",', 'json'))
    ).toBeUndefined();
  });

  it('leaves valid JSON, empty text, other hints and references alone', () => {
    expect(jsonTextError(immediate('[1]', JSON_LITERAL_HINT))).toBeUndefined();
    expect(jsonTextError(immediate('', JSON_LITERAL_HINT))).toBeUndefined();
    expect(jsonTextError(immediate('{"a":', 'string'))).toBeUndefined();
    expect(
      jsonTextError({
        value: 'data.x',
        typeHint: 'json',
        valueType: 'reference',
      })
    ).toBeUndefined();
  });
});
