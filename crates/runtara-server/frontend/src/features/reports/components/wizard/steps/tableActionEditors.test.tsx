import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { WorkflowActionEditor } from './tableActionEditors';
import type { ReportWorkflowActionConfig } from '../../../types';

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

function renderEditor(
  action: ReportWorkflowActionConfig,
  fields: string[] = []
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <WorkflowActionEditor
        action={action}
        fields={fields}
        onChange={() => {}}
      />
    </QueryClientProvider>
  );
}

describe('WorkflowActionEditor visibleWhen/disabledWhen field picker', () => {
  it('renders a Select populated by row fields when fields are provided', () => {
    renderEditor(
      {
        workflowId: 'workflow_x',
        visibleWhen: { op: 'EQ', arguments: ['status', 'ready'] },
      },
      ['status', 'owner', 'priority']
    );

    // Both Visible/Disabled when sections render a Field combobox.
    const comboboxes = screen.getAllByRole('combobox');
    // workflow, context mode, visible-when field, disabled-when field, etc.
    // Just assert at least 2 combobox-style triggers exist for the row fields.
    expect(comboboxes.length).toBeGreaterThanOrEqual(2);
    // The selected visibleWhen field surfaces as "status" inside the Select trigger.
    expect(screen.getAllByText('status').length).toBeGreaterThan(0);
  });

  it('falls back to a free-form text input when no fields are provided', () => {
    renderEditor(
      {
        workflowId: 'workflow_x',
        visibleWhen: { op: 'EQ', arguments: ['custom_field', 'ready'] },
      },
      []
    );

    // With no fields known, the field input is an <Input placeholder="field">.
    const fieldInputs = screen.getAllByPlaceholderText('field');
    expect(fieldInputs.length).toBeGreaterThanOrEqual(1);
  });
});
