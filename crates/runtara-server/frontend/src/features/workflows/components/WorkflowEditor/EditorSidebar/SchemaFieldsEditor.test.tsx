import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

import { SchemaFieldsEditor } from './SchemaFieldsEditor';
import { validateSchemaFieldsWithRust } from '@/features/workflows/utils/rust-workflow-validation';

vi.mock('@/features/workflows/utils/rust-workflow-validation', () => ({
  validateSchemaFieldsWithRust: vi.fn(),
}));

describe('SchemaFieldsEditor', () => {
  it('shows validation errors from the shared WASM validator', async () => {
    vi.mocked(validateSchemaFieldsWithRust).mockResolvedValue({
      success: true,
      valid: false,
      status: 'invalid',
      errors: [
        "[E008] Input schema field name 'order_id' is duplicated. Field names must be unique.",
      ],
      warnings: [],
      message: 'Schema field validation failed with 1 error(s)',
      wasmAvailable: true,
      schemaErrors: [
        {
          code: 'E008',
          message:
            "[E008] Input schema field name 'order_id' is duplicated. Field names must be unique.",
          fieldName: 'order_id',
          rowIndices: [0, 1],
        },
      ],
    });

    render(
      <SchemaFieldsEditor
        label="Input Schema Fields"
        fields={[
          {
            name: 'order_id',
            type: 'string',
            required: true,
            description: '',
          },
          {
            name: ' order_id ',
            type: 'number',
            required: false,
            description: '',
          },
        ]}
        onChange={vi.fn()}
      />
    );

    expect(
      await screen.findAllByText('Field name must be unique.')
    ).toHaveLength(2);
    const nameInputs = screen.getAllByPlaceholderText('fieldName');
    expect(nameInputs).toHaveLength(2);
    for (const input of nameInputs) {
      expect(input).toHaveAttribute('aria-invalid', 'true');
    }
  });
});
