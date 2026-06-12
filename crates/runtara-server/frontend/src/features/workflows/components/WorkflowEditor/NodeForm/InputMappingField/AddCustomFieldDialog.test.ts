import { describe, expect, it } from 'vitest';
import { validateCustomFieldName } from './AddCustomFieldDialog';

/**
 * Dotted mapping keys are legal DSL: validation checks them by root segment
 * (validation.rs) and the runtime builds nested objects from them
 * (direct_json.rs insert_nested). The dialog must accept them while still
 * rejecting malformed segments.
 */
describe('validateCustomFieldName', () => {
  it('accepts plain identifiers', () => {
    expect(validateCustomFieldName('my_parameter')).toBeNull();
    expect(validateCustomFieldName('_private')).toBeNull();
    expect(validateCustomFieldName('camelCase9')).toBeNull();
  });

  it('accepts dot-separated segments that build nested objects', () => {
    expect(validateCustomFieldName('payload.user.name')).toBeNull();
    expect(validateCustomFieldName('data.field_name')).toBeNull();
    expect(validateCustomFieldName('a.b')).toBeNull();
  });

  it('rejects an empty name', () => {
    expect(validateCustomFieldName('')).toBe('Field name is required');
  });

  it('rejects segments that are not valid identifiers', () => {
    const message =
      'Each dot-separated segment must start with a letter or underscore and contain only letters, numbers, and underscores';
    expect(validateCustomFieldName('1abc')).toBe(message);
    expect(validateCustomFieldName('a-b')).toBe(message);
    expect(validateCustomFieldName('a..b')).toBe(message);
    expect(validateCustomFieldName('.leading')).toBe(message);
    expect(validateCustomFieldName('trailing.')).toBe(message);
    expect(validateCustomFieldName('a.1b')).toBe(message);
    expect(validateCustomFieldName('a b')).toBe(message);
  });
});
