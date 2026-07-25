import { describe, it, expect } from 'vitest';
import {
  CUSTOM_FIELD_TYPES,
  customFieldTypeLabel,
} from './custom-field-types';

describe('CUSTOM_FIELD_TYPES', () => {
  it('has no duplicate values', () => {
    // Two entries once shared value 'json' ("JSON Object" and "Array"). A
    // Radix Select renders the text of every item matching the current value,
    // so the trigger read "JSON ObjectArray", and a DropdownMenu keyed by
    // value produced duplicate React keys.
    const values = CUSTOM_FIELD_TYPES.map((t) => t.value);
    expect(new Set(values).size).toBe(values.length);
  });

  it('has no duplicate labels', () => {
    const labels = CUSTOM_FIELD_TYPES.map((t) => t.label);
    expect(new Set(labels).size).toBe(labels.length);
  });

  it('only offers types the save path will keep', () => {
    // Mirrors VALID_VALUE_TYPES in CustomNodes/utils.tsx — isValidValueType
    // drops any hint outside this set, so offering e.g. 'array' here would
    // silently discard the user's choice on save.
    const validValueTypes = new Set([
      'string',
      'integer',
      'number',
      'boolean',
      'json',
      'file',
    ]);
    for (const option of CUSTOM_FIELD_TYPES) {
      expect(validValueTypes.has(option.value)).toBe(true);
    }
  });

  it('covers every valid value type exactly once', () => {
    expect(CUSTOM_FIELD_TYPES.map((t) => t.value).sort()).toEqual([
      'boolean',
      'file',
      'integer',
      'json',
      'number',
      'string',
    ]);
  });
});

describe('customFieldTypeLabel', () => {
  it('resolves known types', () => {
    expect(customFieldTypeLabel('json')).toBe('JSON (object or array)');
    expect(customFieldTypeLabel('string')).toBe('String');
  });

  it('falls back to the raw hint for unknown types', () => {
    expect(customFieldTypeLabel('auto')).toBe('auto');
    expect(customFieldTypeLabel('mystery')).toBe('mystery');
  });

  it('renders nothing for a missing hint', () => {
    expect(customFieldTypeLabel(undefined)).toBe('');
    expect(customFieldTypeLabel('')).toBe('');
  });
});
