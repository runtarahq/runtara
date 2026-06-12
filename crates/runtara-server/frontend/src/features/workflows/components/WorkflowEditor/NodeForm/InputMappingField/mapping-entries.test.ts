import { describe, expect, it } from 'vitest';
import {
  formatMappingObjectJson,
  normalizeMappingObject,
  normalizeMappingValue,
  parseMappingObjectJson,
  toEditorInitialData,
  toFormMappingEntries,
  type SeededMappingEntry,
} from './mapping-entries';
import type { InputMappingEntry } from '@/features/workflows/stores/nodeFormStore';

describe('toFormMappingEntries', () => {
  it('preserves defaultValue on reference entries', () => {
    const entries: InputMappingEntry[] = [
      {
        type: 'orderId',
        value: "steps['fetch'].outputs.orderId",
        valueType: 'reference',
        typeHint: 'string',
        defaultValue: 'unknown-order',
      },
    ];

    const result = toFormMappingEntries(entries);

    expect(result).toEqual([
      {
        type: 'orderId',
        value: "steps['fetch'].outputs.orderId",
        valueType: 'reference',
        typeHint: 'string',
        defaultValue: 'unknown-order',
      },
    ]);
  });

  it('preserves non-string JSON-authored defaults verbatim', () => {
    const entries: InputMappingEntry[] = [
      {
        type: 'retries',
        value: "steps['cfg'].outputs.retries",
        valueType: 'reference',
        typeHint: 'integer',
        defaultValue: 3,
      },
      {
        type: 'payload',
        value: "steps['cfg'].outputs.payload",
        valueType: 'reference',
        typeHint: 'json',
        defaultValue: { fallback: true },
      },
    ];

    const result = toFormMappingEntries(entries);

    expect(result[0].defaultValue).toBe(3);
    expect(result[1].defaultValue).toEqual({ fallback: true });
  });

  it('omits the defaultValue key when the entry has none', () => {
    const entries: InputMappingEntry[] = [
      {
        type: 'name',
        value: 'literal',
        valueType: 'immediate',
        typeHint: 'string',
      },
    ];

    const result = toFormMappingEntries(entries);

    expect(result[0]).not.toHaveProperty('defaultValue');
    expect(result[0]).toEqual({
      type: 'name',
      value: 'literal',
      valueType: 'immediate',
      typeHint: 'string',
    });
  });
});

describe('toEditorInitialData', () => {
  it('preserves defaultValue when converting form items to editor data', () => {
    const result = toEditorInitialData([
      {
        type: 'orderId',
        value: "steps['fetch'].outputs.orderId",
        valueType: 'reference',
        typeHint: 'string',
        defaultValue: 'unknown-order',
      },
    ]);

    expect(result[0].defaultValue).toBe('unknown-order');
  });

  it('applies value/valueType fallbacks without inventing a defaultValue', () => {
    const result = toEditorInitialData([{ type: 'name' }]);

    expect(result[0]).toEqual({
      type: 'name',
      value: '',
      valueType: 'immediate',
      typeHint: undefined,
    });
    expect(result[0]).not.toHaveProperty('defaultValue');
  });

  it('round-trips defaultValue through editor data and back to form entries', () => {
    const formItems = [
      {
        type: 'amount',
        value: "steps['calc'].outputs.amount",
        valueType: 'reference' as const,
        typeHint: 'number',
        defaultValue: 0,
      },
    ];

    const roundTripped = toFormMappingEntries(toEditorInitialData(formItems));

    expect(roundTripped).toEqual(formItems);
  });
});

describe('autoSeeded passthrough', () => {
  it('carries the autoSeeded marker editor → form → editor', () => {
    const seeded: SeededMappingEntry[] = [
      {
        type: 'note',
        value: '',
        valueType: 'immediate',
        typeHint: 'text',
        autoSeeded: true,
      },
    ];

    const formEntries = toFormMappingEntries(seeded);
    expect(formEntries[0].autoSeeded).toBe(true);

    const editorEntries = toEditorInitialData(formEntries);
    expect(editorEntries[0].autoSeeded).toBe(true);
  });

  it('does not invent autoSeeded on entries that never had it', () => {
    const entries: SeededMappingEntry[] = [
      {
        type: 'note',
        value: '',
        valueType: 'immediate',
        typeHint: 'text',
      },
    ];

    expect(toFormMappingEntries(entries)[0]).not.toHaveProperty('autoSeeded');
    expect(toEditorInitialData(entries)[0]).not.toHaveProperty('autoSeeded');
  });
});

describe('normalizeMappingValue', () => {
  it('converts raw DSL reference aliases (type/default) to UI format', () => {
    expect(
      normalizeMappingValue({
        valueType: 'reference',
        value: 'data.caseId',
        type: 'string',
        default: 'unknown',
      })
    ).toEqual({
      valueType: 'reference',
      value: 'data.caseId',
      typeHint: 'string',
      defaultValue: 'unknown',
    });
  });

  it('passes UI-format entries through unchanged (idempotent)', () => {
    const entry = {
      valueType: 'reference' as const,
      value: 'data.caseId',
      typeHint: 'string',
      defaultValue: 'unknown',
    };
    const once = normalizeMappingValue(entry);
    expect(once).toEqual(entry);
    expect(normalizeMappingValue(once)).toEqual(once);
  });

  it('wraps bare literals as immediate values', () => {
    expect(normalizeMappingValue('hello')).toEqual({
      valueType: 'immediate',
      value: 'hello',
    });
    expect(normalizeMappingValue(5)).toEqual({
      valueType: 'immediate',
      value: 5,
    });
    expect(normalizeMappingValue(null)).toEqual({
      valueType: 'immediate',
      value: null,
    });
    expect(normalizeMappingValue({ plain: 'object' })).toEqual({
      valueType: 'immediate',
      value: { plain: 'object' },
    });
  });

  it('does not copy a default onto non-reference entries', () => {
    expect(
      normalizeMappingValue({
        valueType: 'immediate',
        value: 'x',
        default: 'ignored',
      })
    ).toEqual({ valueType: 'immediate', value: 'x' });
  });

  it('recursively normalizes nested composite objects and arrays', () => {
    expect(
      normalizeMappingValue({
        valueType: 'composite',
        value: {
          ref: { valueType: 'reference', value: 'data.x', type: 'integer' },
          list: {
            valueType: 'composite',
            value: [{ valueType: 'immediate', value: 1 }],
          },
        },
      })
    ).toEqual({
      valueType: 'composite',
      value: {
        ref: { valueType: 'reference', value: 'data.x', typeHint: 'integer' },
        list: {
          valueType: 'composite',
          value: [{ valueType: 'immediate', value: 1 }],
        },
      },
    });
  });

  it('seeds an empty object for non-object composite payloads', () => {
    expect(
      normalizeMappingValue({ valueType: 'composite', value: '' })
    ).toEqual({ valueType: 'composite', value: {} });
  });

  it('rejects unknown valueType discriminants', () => {
    expect(normalizeMappingValue({ valueType: 'exotic', value: 1 })).toBeNull();
    expect(
      normalizeMappingValue({
        valueType: 'composite',
        value: { bad: { valueType: 'exotic', value: 1 } },
      })
    ).toBeNull();
  });
});

describe('normalizeMappingObject', () => {
  it('normalizes empty-ish form values to an empty mapping', () => {
    expect(normalizeMappingObject('')).toEqual({});
    expect(normalizeMappingObject(null)).toEqual({});
    expect(normalizeMappingObject(undefined)).toEqual({});
  });

  it('returns null for non-object values (JSON-only editing)', () => {
    expect(normalizeMappingObject('not json')).toBeNull();
    expect(normalizeMappingObject([1, 2])).toBeNull();
    expect(normalizeMappingObject(42)).toBeNull();
  });

  it('returns null when any entry is unrepresentable', () => {
    expect(
      normalizeMappingObject({
        ok: { valueType: 'immediate', value: 1 },
        bad: { valueType: 'exotic', value: 1 },
      })
    ).toBeNull();
  });

  it('normalizes a mixed mapping object preserving key order', () => {
    const result = normalizeMappingObject({
      caseId: { valueType: 'reference', value: 'data.caseId', type: 'string' },
      summary: { valueType: 'template', value: 'Case {{ data.caseId }}' },
      flag: true,
    });
    expect(result).toEqual({
      caseId: {
        valueType: 'reference',
        value: 'data.caseId',
        typeHint: 'string',
      },
      summary: { valueType: 'template', value: 'Case {{ data.caseId }}' },
      flag: { valueType: 'immediate', value: true },
    });
    expect(Object.keys(result!)).toEqual(['caseId', 'summary', 'flag']);
  });
});

describe('parseMappingObjectJson / formatMappingObjectJson', () => {
  it('parses blank input to {} (clears the field on save)', () => {
    expect(parseMappingObjectJson('')).toEqual({});
    expect(parseMappingObjectJson('   ')).toEqual({});
  });

  it('parses valid JSON and keeps invalid JSON as the raw string', () => {
    expect(parseMappingObjectJson('{"a": 1}')).toEqual({ a: 1 });
    expect(parseMappingObjectJson('{"a": ')).toBe('{"a": ');
  });

  it('formats falsy values as empty text and objects as pretty JSON', () => {
    expect(formatMappingObjectJson('')).toBe('');
    expect(formatMappingObjectJson(undefined)).toBe('');
    expect(formatMappingObjectJson('raw string')).toBe('raw string');
    expect(formatMappingObjectJson({ a: 1 })).toBe('{\n  "a": 1\n}');
  });

  it('round-trips structured edits through the JSON view', () => {
    const obj = {
      caseId: {
        valueType: 'reference',
        value: 'data.caseId',
        typeHint: 'string',
        defaultValue: 'unknown',
      },
    };
    expect(parseMappingObjectJson(formatMappingObjectJson(obj))).toEqual(obj);
  });
});
