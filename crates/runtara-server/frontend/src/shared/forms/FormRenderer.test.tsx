import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
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
    vi.clearAllMocks();
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

  it('does not reanalyze semantically unchanged inline values', async () => {
    vi.mocked(analyzeFormWithRust).mockImplementation(async () => result());

    function InlineParent() {
      const [, setAnalysis] = useState<FormAnalysisResult | null>(null);
      return (
        <FormRenderer
          definition={{ ...definition }}
          value={{}}
          onChange={vi.fn()}
          onAnalysisChange={setAnalysis}
        />
      );
    }

    render(<InlineParent />);
    expect(await screen.findByLabelText('Mode')).toBeInTheDocument();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(analyzeFormWithRust).toHaveBeenCalledTimes(1);
  });

  it('focuses the first invalid field only after an explicit submit attempt', async () => {
    vi.mocked(analyzeFormWithRust).mockResolvedValue(
      result({
        valid: false,
        status: 'invalid',
        issues: [
          {
            code: 'FORM_FIELD_REQUIRED',
            path: 'data.token',
            message: 'Token is required',
            severity: 'error',
          },
        ],
      })
    );

    const { rerender } = render(
      <FormRenderer
        definition={definition}
        value={{}}
        onChange={vi.fn()}
        submitAttempt={0}
      />
    );
    const mode = await screen.findByLabelText('Mode');
    const token = screen.getByLabelText('Token*');

    mode.focus();
    rerender(
      <FormRenderer
        definition={definition}
        value={{ mode: 'advanced' }}
        onChange={vi.fn()}
        submitAttempt={0}
      />
    );
    await waitFor(() => expect(mode).toHaveFocus());

    rerender(
      <FormRenderer
        definition={definition}
        value={{ mode: 'advanced' }}
        onChange={vi.fn()}
        submitAttempt={1}
      />
    );
    await waitFor(() => expect(token).toHaveFocus());
  });

  it('delegates dynamic option retrieval to the domain frame', async () => {
    const resolveOptions = vi.fn().mockResolvedValue([
      { value: 'invoice', label: 'Invoice' },
      { value: 'payment', label: 'Payment' },
    ]);
    const dynamicDefinition: FormDefinition = {
      fields: {
        company: { type: 'string' },
        resource: {
          type: 'string',
          label: 'Resource',
          control: {
            kind: 'lookup',
            optionResolver: 'object-model.resources',
            optionDependencies: ['company'],
          },
        },
      },
    };
    vi.mocked(analyzeFormWithRust).mockResolvedValue(
      result({
        fields: {
          company: { visible: true, enabled: true, required: false },
          resource: { visible: true, enabled: true, required: false },
        },
      })
    );

    render(
      <FormRenderer
        definition={dynamicDefinition}
        value={{ company: 'acme' }}
        onChange={vi.fn()}
        frame={{ resolveOptions }}
      />
    );

    await waitFor(() => expect(resolveOptions).toHaveBeenCalledTimes(1));
    expect(resolveOptions).toHaveBeenCalledWith(
      expect.objectContaining({
        resolverKey: 'object-model.resources',
        fieldName: 'resource',
        currentData: { company: 'acme' },
      })
    );
    expect(await screen.findByText('Select a value')).toBeInTheDocument();
  });

  it('honors descriptor order and renders author-declared advanced sections collapsed', async () => {
    const ordered: FormDefinition = {
      sections: [
        {
          id: 'advanced',
          label: 'Advanced settings',
          advanced: true,
          order: 200,
        },
      ],
      fields: {
        client_secret: {
          type: 'string',
          label: 'Client Secret',
          order: 1,
        },
        client_id: { type: 'string', label: 'Client ID', order: 0 },
        headers: {
          type: 'object',
          label: 'Extra Headers',
          order: 2,
          section: 'advanced',
          control: { kind: 'key_value' },
        },
      },
    };
    vi.mocked(analyzeFormWithRust).mockResolvedValue(
      result({
        fields: {
          client_id: { visible: true, enabled: true, required: false },
          client_secret: { visible: true, enabled: true, required: false },
          headers: { visible: true, enabled: true, required: false },
        },
      })
    );

    const { container } = render(
      <FormRenderer definition={ordered} value={{}} onChange={vi.fn()} />
    );
    await screen.findByLabelText('Client ID');
    const labels = [...container.querySelectorAll('label')].map(
      (label) => label.textContent
    );
    expect(labels.slice(0, 2)).toEqual(['Client ID', 'Client Secret']);
    const details = screen.getByText('Advanced settings').closest('details');
    expect(details).not.toHaveAttribute('open');
    expect(details?.querySelector('summary')).toHaveTextContent(
      'Advanced settings'
    );
    fireEvent.click(screen.getByText('Advanced settings'));
    expect(screen.getByRole('group', { name: 'Extra Headers' })).toBeVisible();
  });
});
