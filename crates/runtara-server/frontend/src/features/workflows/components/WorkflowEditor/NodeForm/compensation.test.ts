import { describe, expect, it } from 'vitest';
import {
  patchCompensation,
  readCompensationParts,
  serializeCompensationData,
} from './compensation';

describe('patchCompensation', () => {
  it('assembles a full CompensationConfig-shaped object', () => {
    const result = patchCompensation(undefined, {
      compensationStep: 'refund',
    });
    const withTrigger = patchCompensation(result, { trigger: 'manual' });
    const withOrder = patchCompensation(withTrigger, { order: 10 });
    const withData = patchCompensation(withOrder, {
      compensationData: {
        chargeId: { valueType: 'reference', value: 'steps.charge.outputs.id' },
      },
    });

    expect(withData).toEqual({
      compensationStep: 'refund',
      trigger: 'manual',
      order: 10,
      compensationData: {
        chargeId: { valueType: 'reference', value: 'steps.charge.outputs.id' },
      },
    });
  });

  it('omits empty optionals instead of writing empty values', () => {
    const result = patchCompensation(
      { compensationStep: 'refund', trigger: 'manual', order: 10 },
      { trigger: undefined, order: undefined }
    );
    expect(result).toEqual({ compensationStep: 'refund' });
  });

  it('treats an empty compensationData object as cleared', () => {
    const result = patchCompensation(
      {
        compensationStep: 'refund',
        compensationData: { a: { valueType: 'immediate', value: 1 } },
      },
      { compensationData: {} }
    );
    expect(result).toEqual({ compensationStep: 'refund' });
  });

  it('only touches fields present in the patch', () => {
    const result = patchCompensation(
      { compensationStep: 'refund', trigger: 'manual' },
      { order: 2 }
    );
    expect(result).toEqual({
      compensationStep: 'refund',
      trigger: 'manual',
      order: 2,
    });
  });

  it('returns undefined when everything is cleared (drops the compensation key)', () => {
    // Matches the legacy empty-textarea behavior:
    // form.setValue('compensation', undefined).
    const result = patchCompensation(
      { compensationStep: 'refund', trigger: 'manual', order: 1 },
      { compensationStep: '', trigger: '', order: undefined }
    );
    expect(result).toBeUndefined();
  });

  it('returns undefined when patching nothing into nothing', () => {
    expect(patchCompensation(undefined, { compensationStep: '' })).toBeUndefined();
  });

  it('preserves unknown JSON-authored keys on structured edits', () => {
    const result = patchCompensation(
      { compensationStep: 'refund', customKey: { nested: true } },
      { trigger: 'on_any_error' }
    );
    expect(result).toEqual({
      compensationStep: 'refund',
      customKey: { nested: true },
      trigger: 'on_any_error',
    });
  });

  it('keeps the object alive when unknown keys remain after clearing known fields', () => {
    const result = patchCompensation(
      { compensationStep: 'refund', customKey: 1 },
      { compensationStep: undefined }
    );
    expect(result).toEqual({ customKey: 1 });
  });
});

describe('readCompensationParts', () => {
  it('extracts structured parts from a DSL compensation object', () => {
    expect(
      readCompensationParts({
        compensationStep: 'refund',
        trigger: 'on_downstream_error',
        order: 5,
        compensationData: { a: { valueType: 'immediate', value: 1 } },
      })
    ).toEqual({
      compensationStep: 'refund',
      trigger: 'on_downstream_error',
      order: 5,
      compensationData: { a: { valueType: 'immediate', value: 1 } },
    });
  });

  it('reads empty parts from undefined or non-object values', () => {
    const empty = {
      compensationStep: '',
      trigger: '',
      order: '',
      compensationData: undefined,
    };
    expect(readCompensationParts(undefined)).toEqual(empty);
    expect(readCompensationParts('garbage')).toEqual(empty);
    expect(readCompensationParts([1, 2])).toEqual(empty);
  });

  it('ignores wrongly-typed fields instead of crashing', () => {
    expect(
      readCompensationParts({ compensationStep: 42, order: 'high' })
    ).toEqual({
      compensationStep: '',
      trigger: '',
      order: '',
      compensationData: undefined,
    });
  });
});

describe('serializeCompensationData', () => {
  it('collapses empty-ish input to undefined (omit the optional key)', () => {
    expect(serializeCompensationData(undefined)).toEqual({
      ok: true,
      data: undefined,
    });
    expect(serializeCompensationData(null)).toEqual({
      ok: true,
      data: undefined,
    });
    expect(serializeCompensationData('')).toEqual({ ok: true, data: undefined });
    expect(serializeCompensationData('   ')).toEqual({
      ok: true,
      data: undefined,
    });
    expect(serializeCompensationData({})).toEqual({ ok: true, data: undefined });
  });

  it('serializes UI-format entries to the DSL MappingValue shape', () => {
    const result = serializeCompensationData({
      chargeId: {
        valueType: 'reference',
        value: "steps['charge'].outputs.chargeId",
        typeHint: 'string',
        defaultValue: 'unknown',
      },
      amount: { valueType: 'immediate', value: '150', typeHint: 'integer' },
      note: { valueType: 'template', value: 'refund {{ data.orderId }}' },
    });

    expect(result).toEqual({
      ok: true,
      data: {
        chargeId: {
          valueType: 'reference',
          value: "steps['charge'].outputs.chargeId",
          type: 'string',
          default: 'unknown',
        },
        amount: { valueType: 'immediate', value: 150 },
        note: { valueType: 'template', value: 'refund {{ data.orderId }}' },
      },
    });
  });

  it('drops form-level type hints that are not backend ValueTypes', () => {
    const result = serializeCompensationData({
      payload: { valueType: 'reference', value: 'data.x', typeHint: 'object' },
    });
    expect(result).toEqual({
      ok: true,
      data: { payload: { valueType: 'reference', value: 'data.x' } },
    });
  });

  it('serializes nested composite entries recursively', () => {
    const result = serializeCompensationData({
      meta: {
        valueType: 'composite',
        value: {
          reason: { valueType: 'immediate', value: 'rollback' },
          source: { valueType: 'reference', value: 'data.source' },
        },
      },
    });
    expect(result).toEqual({
      ok: true,
      data: {
        meta: {
          valueType: 'composite',
          value: {
            reason: { valueType: 'immediate', value: 'rollback' },
            source: { valueType: 'reference', value: 'data.source' },
          },
        },
      },
    });
  });

  it('is idempotent over already-DSL values (loaded data round-trips unchanged)', () => {
    const dsl = {
      chargeId: {
        valueType: 'reference',
        value: "steps['agent'].outputs.chargeId",
        type: 'string',
        default: 'none',
      },
      amount: { valueType: 'immediate', value: 150 },
      flag: { valueType: 'immediate', value: true },
      note: { valueType: 'template', value: '{{ data.x }}' },
    };
    const first = serializeCompensationData(dsl);
    expect(first).toEqual({ ok: true, data: dsl });
    if (first.ok) {
      expect(serializeCompensationData(first.data)).toEqual({
        ok: true,
        data: dsl,
      });
    }
  });

  it('wraps bare literals as immediate values (JSON-textarea shorthand)', () => {
    const result = serializeCompensationData({ amount: 5, label: 'hi' });
    expect(result).toEqual({
      ok: true,
      data: {
        amount: { valueType: 'immediate', value: 5 },
        label: { valueType: 'immediate', value: 'hi' },
      },
    });
  });

  it('passes exotic objects (unknown valueType discriminants) through verbatim', () => {
    const exotic = { weird: { valueType: 'lookup', value: 'x' } };
    expect(serializeCompensationData(exotic)).toEqual({
      ok: true,
      data: exotic,
    });
  });

  it('parses valid JSON strings and rejects invalid ones as not committable', () => {
    expect(
      serializeCompensationData('{"a": {"valueType": "immediate", "value": 1}}')
    ).toEqual({
      ok: true,
      data: { a: { valueType: 'immediate', value: 1 } },
    });
    expect(serializeCompensationData('{not json')).toEqual({ ok: false });
    expect(serializeCompensationData('"a string"')).toEqual({ ok: false });
    expect(serializeCompensationData('[1, 2]')).toEqual({ ok: false });
  });

  it('rejects non-object values as not committable', () => {
    expect(serializeCompensationData([1, 2])).toEqual({ ok: false });
    expect(serializeCompensationData(42)).toEqual({ ok: false });
  });
});

describe('compensation structured-edit end-to-end shape', () => {
  it('produces the exact object the legacy JSON textarea produced', () => {
    // The shape asserted by the cleanNodeData round-trip test
    // (CustomNodes/utils.test.ts "round-trips Agent retry, timeout, and
    // compensation fields") — structured edits must assemble it identically
    // since the save path passes `compensation` through verbatim.
    const dataResult = serializeCompensationData({
      chargeId: {
        valueType: 'reference',
        value: "steps['agent'].outputs.chargeId",
        typeHint: 'string',
      },
    });
    expect(dataResult.ok).toBe(true);

    let compensation: Record<string, unknown> | undefined;
    compensation = patchCompensation(compensation, {
      compensationStep: 'refund',
    });
    compensation = patchCompensation(compensation, {
      compensationData: dataResult.ok ? dataResult.data : undefined,
    });
    compensation = patchCompensation(compensation, {
      trigger: 'on_downstream_error',
    });
    compensation = patchCompensation(compensation, { order: 10 });

    expect(compensation).toEqual({
      compensationStep: 'refund',
      compensationData: {
        chargeId: {
          valueType: 'reference',
          value: "steps['agent'].outputs.chargeId",
          type: 'string',
        },
      },
      trigger: 'on_downstream_error',
      order: 10,
    });

    // Clearing everything removes the compensation key entirely.
    expect(
      patchCompensation(compensation, {
        compensationStep: undefined,
        compensationData: undefined,
        trigger: undefined,
        order: undefined,
      })
    ).toBeUndefined();
  });
});
