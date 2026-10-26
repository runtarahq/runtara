import { describe, expect, it } from 'vitest';

import {
  isJsonTextHint,
  parseJsonText,
  processMappingEntry,
} from './mapping-serialization';

describe('isJsonTextHint', () => {
  it('is true for the hints the save parses as JSON', () => {
    expect(isJsonTextHint('json')).toBe(true);
    expect(isJsonTextHint('object')).toBe(true);
    expect(isJsonTextHint('array')).toBe(true);
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

  it('is the rule processMappingEntry saves immediates by', () => {
    for (const text of ['{"a": 1}', '["{{x}}"]', '{"a":', '7']) {
      const [, saved] = processMappingEntry({
        type: 'field',
        value: text,
        valueType: 'immediate',
        typeHint: 'json',
      });
      expect((saved as { value: unknown }).value).toEqual(parseJsonText(text));
    }
  });
});
