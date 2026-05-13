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
});
