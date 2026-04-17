import { describe, it, expect } from 'vitest';
import { schema } from './NodeFormItem';

// Minimal valid base form data for testing
const baseFormData = {
  name: 'Test Step',
  stepType: 'Error',
  inputMapping: [],
  executionTimeout: 120,
  maxRetries: 1,
  retryDelay: 1000,
  retryStrategy: 'Linear' as const,
};

const testSchema = schema({ agents: [] });

describe('NodeFormItem schema — Error step validation', () => {
  it('should reject Error step with empty code and message', () => {
    const data = {
      ...baseFormData,
      inputMapping: [
        {
          type: 'code',
          value: '',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'message',
          value: '',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'category',
          value: 'permanent',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'severity',
          value: 'error',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    };

    const result = testSchema.safeParse(data);
    expect(result.success).toBe(false);
    if (!result.success) {
      const messages = result.error.issues.map((i) => i.message);
      expect(messages).toContain('Error Code is required.');
      expect(messages).toContain('Error Message is required.');
    }
  });

  it('should reject Error step with missing code entry', () => {
    const data = {
      ...baseFormData,
      inputMapping: [
        {
          type: 'message',
          value: 'Something went wrong',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    };

    const result = testSchema.safeParse(data);
    expect(result.success).toBe(false);
    if (!result.success) {
      const messages = result.error.issues.map((i) => i.message);
      expect(messages).toContain('Error Code is required.');
    }
  });

  it('should reject Error step with missing message entry', () => {
    const data = {
      ...baseFormData,
      inputMapping: [
        {
          type: 'code',
          value: 'INVALID_ACCOUNT',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    };

    const result = testSchema.safeParse(data);
    expect(result.success).toBe(false);
    if (!result.success) {
      const messages = result.error.issues.map((i) => i.message);
      expect(messages).toContain('Error Message is required.');
    }
  });

  it('should reject Error step with whitespace-only code', () => {
    const data = {
      ...baseFormData,
      inputMapping: [
        {
          type: 'code',
          value: '   ',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'message',
          value: 'Some message',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    };

    const result = testSchema.safeParse(data);
    expect(result.success).toBe(false);
    if (!result.success) {
      const messages = result.error.issues.map((i) => i.message);
      expect(messages).toContain('Error Code is required.');
    }
  });

  it('should accept Error step with valid code and message', () => {
    const data = {
      ...baseFormData,
      inputMapping: [
        {
          type: 'code',
          value: 'CREDIT_LIMIT_EXCEEDED',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'message',
          value: 'Credit limit exceeded',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'category',
          value: 'permanent',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
        {
          type: 'severity',
          value: 'error',
          typeHint: 'string',
          valueType: 'immediate' as const,
        },
      ],
    };

    const result = testSchema.safeParse(data);
    expect(result.success).toBe(true);
  });

  it('should accept Error step with reference valueType (no immediate value check)', () => {
    const data = {
      ...baseFormData,
      inputMapping: [
        {
          type: 'code',
          value: 'steps.step1.error_code',
          typeHint: 'string',
          valueType: 'reference' as const,
        },
        {
          type: 'message',
          value: 'steps.step1.error_message',
          typeHint: 'string',
          valueType: 'reference' as const,
        },
      ],
    };

    const result = testSchema.safeParse(data);
    expect(result.success).toBe(true);
  });

  it('should not apply Error validation to non-Error step types', () => {
    const data = {
      ...baseFormData,
      stepType: 'Agent',
      agentId: 'some-agent',
      capabilityId: 'some-cap',
      inputMapping: [],
    };

    const result = testSchema.safeParse(data);
    expect(result.success).toBe(true);
  });
});
