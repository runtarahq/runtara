import { describe, expect, it } from 'vitest';
import { deriveTypeHintFromSchemaType } from './finish-type-hints';

describe('deriveTypeHintFromSchemaType', () => {
  it('maps scalar schema types to their matching ValueType hints', () => {
    expect(deriveTypeHintFromSchemaType('string')).toBe('string');
    expect(deriveTypeHintFromSchemaType('integer')).toBe('integer');
    expect(deriveTypeHintFromSchemaType('number')).toBe('number');
    expect(deriveTypeHintFromSchemaType('boolean')).toBe('boolean');
    expect(deriveTypeHintFromSchemaType('file')).toBe('file');
  });

  it('maps object and array schema types to the json pass-through hint', () => {
    expect(deriveTypeHintFromSchemaType('object')).toBe('json');
    expect(deriveTypeHintFromSchemaType('array')).toBe('json');
    expect(deriveTypeHintFromSchemaType('array<string>')).toBe('json');
    expect(deriveTypeHintFromSchemaType('string[]')).toBe('json');
  });

  it('maps common type aliases', () => {
    expect(deriveTypeHintFromSchemaType('int')).toBe('integer');
    expect(deriveTypeHintFromSchemaType('float')).toBe('number');
    expect(deriveTypeHintFromSchemaType('double')).toBe('number');
    expect(deriveTypeHintFromSchemaType('bool')).toBe('boolean');
    expect(deriveTypeHintFromSchemaType('text')).toBe('string');
  });

  it('is case-insensitive', () => {
    expect(deriveTypeHintFromSchemaType('Number')).toBe('number');
    expect(deriveTypeHintFromSchemaType('BOOLEAN')).toBe('boolean');
  });

  it('omits the hint for unknown or missing schema types', () => {
    expect(deriveTypeHintFromSchemaType(undefined)).toBeUndefined();
    expect(deriveTypeHintFromSchemaType('')).toBeUndefined();
    expect(deriveTypeHintFromSchemaType('something-custom')).toBeUndefined();
  });
});
