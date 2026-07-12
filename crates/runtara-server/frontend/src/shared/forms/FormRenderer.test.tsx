import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { FormRenderer } from './FormRenderer';
import { analyzeFormWithRust } from './rust-form-validation';
import type { FormAnalysisResult, FormDefinition } from './types';

vi.mock('./rust-form-validation', () => ({
  analyzeFormWithRust: vi.fn(),
}));

const definition: FormDefinition = {
  fields: {
    mode: { type: 'string', label: 'Mode', enum: ['basic', 'advanced'] },
    token: {
      type: 'string',
      label: 'Token',
      access: 'write',
      secret: true,
      control: { kind: 'password' },
    },
    managed_id: { type: 'string', label: 'Managed ID', access: 'read' },
  },
};

function result(patch: Partial<FormAnalysisResult> = {}): FormAnalysisResult {
  return {
    success: true,
    valid: true,
    status: 'valid',
    fields: {
      mode: { visible: true, enabled: true, required: false },
      token: { visible: true, enabled: true, required: true },
      managed_id: { visible: true, enabled: true, required: false },
    },
    issues: [],
    message: 'Form validation passed',
    wasmAvailable: true,
    ...patch,
  };
}

describe('FormRenderer', () => {
  beforeEach(() => {
    vi.mocked(analyzeFormWithRust).mockResolvedValue(result());
  });

  it('renders controlled fields and enforces access state', async () => {
    const onChange = vi.fn();
    render(
      <FormRenderer
        definition={definition}
        value={{ mode: 'basic', managed_id: 'server-value' }}
        onChange={onChange}
      />
    );

    const token = await screen.findByLabelText('Token*');
    expect(token).toHaveAttribute('type', 'password');
    expect(screen.getByLabelText('Managed ID')).toBeDisabled();

    fireEvent.change(token, { target: { value: 'new-secret' } });
    expect(onChange).toHaveBeenCalledWith({
      mode: 'basic',
      managed_id: 'server-value',
      token: 'new-secret',
    });
  });

  it('uses Rust field state and structured issues', async () => {
    vi.mocked(analyzeFormWithRust).mockResolvedValue(
      result({
        valid: false,
        status: 'invalid',
        fields: {
          mode: { visible: true, enabled: true, required: false },
          token: { visible: false, enabled: true, required: false },
          managed_id: { visible: true, enabled: true, required: false },
        },
        issues: [
          {
            code: 'FORM_FIELD_TYPE_MISMATCH',
            path: 'data.managed_id',
            message: 'Expected string',
            severity: 'error',
          },
        ],
      })
    );

    render(
      <FormRenderer definition={definition} value={{}} onChange={vi.fn()} />
    );

    await waitFor(() =>
      expect(screen.queryByLabelText('Token')).not.toBeInTheDocument()
    );
    expect(screen.getByText('Expected string')).toBeInTheDocument();
    expect(screen.getByLabelText('Managed ID')).toHaveAttribute(
      'aria-invalid',
      'true'
    );
  });

  it('blocks rendering when shared WASM is unavailable', async () => {
    vi.mocked(analyzeFormWithRust).mockResolvedValue(
      result({
        success: false,
        valid: false,
        status: 'unavailable',
        wasmAvailable: false,
      })
    );

    render(
      <FormRenderer definition={definition} value={{}} onChange={vi.fn()} />
    );

    expect(await screen.findByText('Form unavailable')).toBeInTheDocument();
    expect(screen.queryByLabelText('Mode')).not.toBeInTheDocument();
  });
});
