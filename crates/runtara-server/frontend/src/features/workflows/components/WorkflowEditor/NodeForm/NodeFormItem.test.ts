import { describe, expect, it } from 'vitest';
import { schema } from './NodeFormItem';

const baseFormData = {
  name: 'Test Step',
  stepType: 'Error',
  inputMapping: [],
  executionTimeout: 120,
  maxRetries: 1,
  retryDelay: 1000,
  retryStrategy: 'Linear' as const,
};

const testSchema = schema();

describe('NodeFormItem schema', () => {
  it('keeps graph-semantic Error step validation out of the local form schema', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      inputMapping: [
        {
          type: 'code',
          value: '',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    });

    expect(result.success).toBe(true);
  });

  it('keeps graph-semantic Agent capability validation out of the local form schema', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Agent',
      agentId: '',
      capabilityId: '',
      inputMapping: [],
    });

    expect(result.success).toBe(true);
  });

  it('keeps UI-local JSON literal parsing for immediate JSON inputs', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      inputMapping: [
        {
          type: 'payload',
          value: '{bad json',
          typeHint: 'json',
          valueType: 'immediate' as const,
        },
      ],
    });

    expect(result.success).toBe(false);
    if (!result.success) {
      expect(result.error.issues.map((issue) => issue.message)).toContain(
        'Invalid JSON format'
      );
    }
  });

  it('keeps graph-semantic Finish output source validation out of the local form schema', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Finish',
      inputMapping: [
        {
          type: 'orderId',
          value: '',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    });

    expect(result.success).toBe(true);
  });

  it('keeps graph-semantic Finish output name validation out of the local form schema', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Finish',
      inputMapping: [
        {
          type: '',
          value: 'steps.fetch.outputs.orderId',
          typeHint: 'string',
          valueType: 'reference' as const,
        },
      ],
    });

    expect(result.success).toBe(true);
  });

  it('allows a Finish output when both name and source are configured', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Finish',
      inputMapping: [
        {
          type: 'orderId',
          value: 'steps.fetch.outputs.orderId',
          typeHint: 'string',
          valueType: 'reference' as const,
        },
      ],
    });

    expect(result.success).toBe(true);
  });

  it('passes ReferenceValue.default (defaultValue) through the resolver', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      inputMapping: [
        {
          type: 'orderId',
          value: "steps['fetch'].outputs.orderId",
          typeHint: 'string',
          valueType: 'reference' as const,
          defaultValue: 'unknown-order',
        },
        {
          type: 'payload',
          value: "steps['fetch'].outputs.payload",
          typeHint: 'json',
          valueType: 'reference' as const,
          defaultValue: { fallback: true },
        },
      ],
    });

    expect(result.success).toBe(true);
    if (result.success) {
      // zodResolver replaces form data with the parsed output on save —
      // a stripped key here means the JSON-authored default is destroyed.
      expect(result.data.inputMapping[0].defaultValue).toBe('unknown-order');
      expect(result.data.inputMapping[1].defaultValue).toEqual({
        fallback: true,
      });
    }
  });

  it('allows a Finish output with a literal null source', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Finish',
      inputMapping: [
        {
          type: 'optionalPayload',
          value: null,
          typeHint: 'json',
          valueType: 'immediate' as const,
        },
      ],
    });

    expect(result.success).toBe(true);
  });

  it('passes the autoSeeded marker through the resolver', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Agent',
      inputMapping: [
        {
          type: 'separator',
          value: '',
          typeHint: 'string',
          valueType: 'immediate' as const,
          autoSeeded: true,
        },
      ],
    });

    expect(result.success).toBe(true);
    if (result.success) {
      // zodResolver replaces form data with the parsed output on save — a
      // stripped marker would make untouched auto-seeded rows persist as ''.
      expect(result.data.inputMapping[0].autoSeeded).toBe(true);
    }
  });

  it('accepts template Split variables (mode toggle can cycle into template)', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Split',
      splitVariablesFields: [
        {
          name: 'greeting',
          value: 'Hello {{ data.name }}',
          valueType: 'template' as const,
          type: 'string',
        },
      ],
    });

    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.splitVariablesFields?.[0]?.valueType).toBe('template');
    }
  });

  it('accepts template While variables', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'While',
      whileVariablesFields: [
        {
          name: 'greeting',
          value: 'Hello {{ data.name }}',
          valueType: 'template' as const,
          type: 'string',
        },
      ],
    });

    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.whileVariablesFields?.[0]?.valueType).toBe('template');
    }
  });

  it('rejects duplicate Finish output names on every duplicated row', () => {
    const result = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Finish',
      inputMapping: [
        {
          type: 'orderId',
          value: 'first',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'other',
          value: 'kept',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'orderId',
          value: 'second',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    });

    expect(result.success).toBe(false);
    if (!result.success) {
      const duplicatePaths = result.error.issues
        .filter((issue) => issue.message.includes('Duplicate output name'))
        .map((issue) => issue.path.join('.'));
      expect(duplicatePaths).toEqual(
        expect.arrayContaining(['inputMapping.0.type', 'inputMapping.2.type'])
      );
      expect(duplicatePaths).not.toContain('inputMapping.1.type');
    }
  });

  it('does not flag duplicate Finish names for non-Finish steps or empty names', () => {
    const agentResult = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Agent',
      inputMapping: [
        {
          type: 'same',
          value: 'a',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'same',
          value: 'b',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    });
    expect(agentResult.success).toBe(true);

    const emptyNamesResult = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Finish',
      inputMapping: [
        {
          type: '',
          value: 'a',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: '',
          value: 'b',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    });
    expect(emptyNamesResult.success).toBe(true);
  });

  it('validates object/array hinted Finish outputs as JSON before save', () => {
    const invalid = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Finish',
      inputMapping: [
        {
          type: 'payload',
          value: '{not json',
          typeHint: 'object',
          valueType: 'immediate' as const,
        },
      ],
    });
    expect(invalid.success).toBe(false);
    if (!invalid.success) {
      expect(invalid.error.issues.map((issue) => issue.message)).toContain(
        'Invalid JSON format'
      );
    }

    const valid = testSchema.safeParse({
      ...baseFormData,
      stepType: 'Finish',
      inputMapping: [
        {
          type: 'payload',
          value: '{"a": 1}',
          typeHint: 'object',
          valueType: 'immediate' as const,
        },
        {
          type: 'list',
          value: '[1, 2]',
          typeHint: 'array',
          valueType: 'immediate' as const,
        },
        {
          type: 'fromRef',
          value: "steps['fetch'].outputs.payload",
          typeHint: 'object',
          valueType: 'reference' as const,
        },
      ],
    });
    expect(valid.success).toBe(true);
  });
});
