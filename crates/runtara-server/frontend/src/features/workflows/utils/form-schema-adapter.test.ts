import { describe, expect, it, vi } from 'vitest';

import {
  initialWorkflowFormValues,
  normalizeWorkflowFormDefinition,
  workflowSchemaWireMap,
} from './form-schema-adapter';

const normalizeSchemaFieldsFormJson = vi.fn(() =>
  JSON.stringify({
    success: true,
    definition: {
      schemaVersion: 1,
      fields: {
        mode: { type: 'string', default: 'manual', access: 'read_write' },
      },
      sections: [],
      allowUnknownFields: false,
    },
  })
);

vi.mock('@/shared/lib/rust-validation-wasm', () => ({
  ensureRustValidationInitialized: vi.fn().mockResolvedValue(undefined),
  normalizeSchemaFieldsFormJson: (...args: unknown[]) =>
    normalizeSchemaFieldsFormJson(...args),
}));

describe('workflow schema form adapter boundary', () => {
  it('keeps only wire-envelope normalization in TypeScript', () => {
    expect(
      workflowSchemaWireMap({
        properties: {
          profile: {
            type: 'object',
            properties: [{ name: 'email', type: 'string', format: 'email' }],
          },
        },
        required: ['profile'],
      })
    ).toEqual({
      profile: {
        type: 'object',
        required: true,
        properties: { email: { type: 'string', format: 'email' } },
      },
    });
  });

  it('delegates semantic normalization to the Rust WASM engine', async () => {
    const definition = await normalizeWorkflowFormDefinition({
      mode: { type: 'string', default: 'manual' },
    });

    expect(normalizeSchemaFieldsFormJson).toHaveBeenCalledWith(
      JSON.stringify({ mode: { type: 'string', default: 'manual' } })
    );
    expect(initialWorkflowFormValues(definition)).toEqual({ mode: 'manual' });
  });
});
