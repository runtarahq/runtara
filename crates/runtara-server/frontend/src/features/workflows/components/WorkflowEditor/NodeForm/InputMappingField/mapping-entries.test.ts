import { describe, expect, it } from 'vitest';
import {
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
