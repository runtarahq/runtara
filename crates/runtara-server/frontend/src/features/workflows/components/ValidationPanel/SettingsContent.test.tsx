import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import { SettingsContent } from './SettingsContent';
import type { WorkflowData } from '../WorkflowEditor/EditorSidebar';
import { validateSchemaFieldsWithRust } from '@/features/workflows/utils/rust-workflow-validation';
import {
  buildStateSchemaFromFields,
  parseSchema,
} from '@/features/workflows/utils/schema';

vi.mock('@/features/workflows/utils/rust-workflow-validation', () => ({
  validateSchemaFieldsWithRust: vi.fn(),
}));

const stateSchema = {
  stage: {
    type: 'string',
    label: 'Stage',
    enum: ['received', 'approval', 'delivered'],
  },
  amount: { type: 'number', label: 'Amount', format: 'currency' },
};

function workflow(overrides: Partial<WorkflowData> = {}): WorkflowData {
  return {
    id: 'wf-1',
    name: 'Orders',
    // The page loads state fields with parseSchema, as the queries layer does.
    stateSchemaFields: parseSchema(
      stateSchema
    ) as WorkflowData['stateSchemaFields'],
    ...overrides,
  };
}

describe('SettingsContent state section', () => {
  beforeAll(() => {
    // Radix switches and selects measure themselves; jsdom has no ResizeObserver.
    Object.defineProperty(globalThis, 'ResizeObserver', {
      writable: true,
      value: class {
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    });
  });

  beforeEach(() => {
    vi.mocked(validateSchemaFieldsWithRust).mockResolvedValue({
      success: true,
      valid: true,
      status: 'valid',
      errors: [],
      warnings: [],
      message: '',
      wasmAvailable: true,
      schemaErrors: [],
    });
  });

  it('lists the declared state fields without form-only columns', () => {
    render(<SettingsContent workflow={workflow()} onChange={vi.fn()} />);

    fireEvent.click(screen.getByText('State'));

    expect(screen.getByDisplayValue('stage')).toBeInTheDocument();
    expect(screen.getByDisplayValue('amount')).toBeInTheDocument();
    expect(screen.getByDisplayValue('currency')).toBeInTheDocument();
    expect(
      screen.getByDisplayValue('received, approval, delivered')
    ).toBeInTheDocument();
    expect(screen.queryByText('Required')).not.toBeInTheDocument();
    expect(screen.queryByText('Default')).not.toBeInTheDocument();
    expect(validateSchemaFieldsWithRust).toHaveBeenCalledWith(
      'State schema',
      expect.any(Array)
    );
  });

  it('adds a state field that serializes without required', () => {
    const onChange = vi.fn();
    render(
      <SettingsContent
        workflow={workflow({ stateSchemaFields: [] })}
        onChange={onChange}
      />
    );

    fireEvent.click(screen.getByText('State'));
    expect(screen.getByText(/No state fields defined/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Add Field/ }));

    expect(onChange).toHaveBeenCalledTimes(1);
    const { stateSchemaFields } = onChange.mock.calls[0][0];
    expect(stateSchemaFields).toHaveLength(1);
    expect(stateSchemaFields[0].required).toBe(false);
    expect(
      buildStateSchemaFromFields([{ ...stateSchemaFields[0], name: 'stage' }])
    ).toEqual({ stage: { type: 'string' } });
  });

  it('is read-only when the workflow is running', () => {
    render(
      <SettingsContent workflow={workflow()} onChange={vi.fn()} readOnly />
    );

    fireEvent.click(screen.getByText('State'));
    expect(screen.getByDisplayValue('stage')).toBeDisabled();
    expect(
      screen.queryByRole('button', { name: /Add Field/ })
    ).not.toBeInTheDocument();
  });
});
