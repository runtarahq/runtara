import { describe, expect, it } from 'vitest';

import {
  initialWorkflowFormValues,
  workflowSchemaToFormDefinition,
} from './form-schema-adapter';

describe('workflow schema form adapter', () => {
  it('normalizes legacy visibleWhen without changing workflow schema storage', () => {
    const definition = workflowSchemaToFormDefinition([
      { name: 'mode', type: 'string', defaultValue: 'manual' },
      {
        name: 'reason',
        type: 'string',
        required: true,
        visibleWhen: { field: 'mode', equals: 'manual' },
      },
    ]);

    expect(definition.fields.reason.conditions?.visible).toEqual({
      type: 'operation',
      op: 'EQ',
      arguments: [
        { type: 'value', valueType: 'reference', value: 'mode' },
        { type: 'value', valueType: 'immediate', value: 'manual' },
      ],
    });
    expect(initialWorkflowFormValues(definition)).toEqual({
      mode: 'manual',
      reason: '',
    });
  });

  it('normalizes nested object and array fields', () => {
    const definition = workflowSchemaToFormDefinition([
      {
        name: 'profile',
        type: 'object',
        properties: [{ name: 'email', type: 'string', format: 'email' }],
      },
      { name: 'tags', type: 'array', items: { type: 'string' } },
    ]);

    expect(definition.fields.profile.properties?.email.format).toBe('email');
    expect(definition.fields.tags.items?.type).toBe('string');
  });
});
