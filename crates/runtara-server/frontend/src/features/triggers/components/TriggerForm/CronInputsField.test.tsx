import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { FormProvider, useForm, useWatch } from 'react-hook-form';
import { describe, expect, it, vi } from 'vitest';

import type { FormDefinition } from '@/shared/forms';

import { CronInputsField } from './CronInputsField';

vi.mock('@/shared/forms/rust-form-validation', () => ({
  analyzeFormWithRust: vi.fn(
    async (definition: FormDefinition, value: Record<string, unknown>) => ({
      success: true,
      valid: true,
      status: 'valid',
      fields: Object.fromEntries(
        Object.entries(definition.fields).map(([name, field]) => [
          name,
          { visible: true, enabled: true, required: Boolean(field.required) },
        ])
      ),
      issues: [],
      message: JSON.stringify(value),
      wasmAvailable: true,
    })
  ),
}));

vi.mock('@/shared/lib/rust-validation-wasm', () => ({
  ensureRustValidationInitialized: vi.fn().mockResolvedValue(undefined),
  normalizeSchemaFieldsFormJson: vi.fn((schemaJson: string) => {
    const fields = JSON.parse(schemaJson) as Record<string, any>;
    return JSON.stringify({
      success: true,
      definition: {
        schemaVersion: 1,
        fields: Object.fromEntries(
          Object.entries(fields).map(([name, field]) => [
            name,
            { ...field, access: 'read_write' },
          ])
        ),
        sections: [],
        allowUnknownFields: false,
      },
    });
  }),
}));

function Harness() {
  const form = useForm({
    defaultValues: {
      triggerType: 'CRON',
      workflowId: 'workflow-1',
      cronInputs:
        '{"data":{"note":"hello","extra":42},"variables":{"region":"eu"}}',
    },
  });
  const cronInputs = useWatch({ control: form.control, name: 'cronInputs' });
  return (
    <FormProvider {...form}>
      <CronInputsField
        label="Inputs"
        workflows={[
          {
            id: 'workflow-1',
            inputSchema: {
              note: { type: 'string', label: 'Note', required: false },
            },
          },
        ]}
      />
      <output data-testid="cron-inputs-value">{cronInputs}</output>
    </FormProvider>
  );
}

describe('CronInputsField', () => {
  it('uses the shared renderer and preserves unrepresented envelope data', async () => {
    render(<Harness />);

    const note = await screen.findByLabelText('Note');
    fireEvent.change(note, { target: { value: 'updated' } });
    await waitFor(() => {
      expect(
        JSON.parse(screen.getByTestId('cron-inputs-value').textContent ?? '{}')
      ).toEqual({
        data: { note: 'updated', extra: 42 },
        variables: { region: 'eu' },
      });
    });

    fireEvent.click(screen.getByRole('button', { name: 'Clear Note' }));
    await waitFor(() => {
      expect(
        JSON.parse(screen.getByTestId('cron-inputs-value').textContent ?? '{}')
      ).toEqual({
        data: { extra: 42 },
        variables: { region: 'eu' },
      });
    });
  });
});
