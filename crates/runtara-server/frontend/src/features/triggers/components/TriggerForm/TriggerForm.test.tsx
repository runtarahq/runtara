import { render, screen, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router';
import { beforeAll, describe, expect, it, vi } from 'vitest';

import { TriggerForm, type TriggerSchemaType } from '.';
import { initialValues } from './TriggerItem';

// The default HTTP trigger type renders WebhookConnectionField, which would
// otherwise reach for connections over the network.
vi.mock('@/features/connections/hooks/useConnections', () => ({
  useConnections: () => ({ data: [], isLoading: false }),
}));

const WORKFLOWS = [
  { id: 'wf-1', name: 'Orders sync' },
  { id: 'wf-2', name: 'Inventory reconcile' },
];

function renderForm(initValues?: TriggerSchemaType) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });

  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <TriggerForm
          title="Create trigger"
          fieldProps={{ workflows: WORKFLOWS }}
          initValues={initValues}
          onSubmit={vi.fn()}
        />
      </MemoryRouter>
    </QueryClientProvider>
  );
}

// SelectInput leaves its Radix trigger without an id, so the label is not
// programmatically associated with it and the combobox has no accessible
// name. Scope the lookup to the field that holds the "Workflow" label.
function workflowCombobox() {
  const field = screen.getByText('Workflow').closest('div');
  return within(field as HTMLElement).getByRole('combobox');
}

describe('TriggerForm workflow field', () => {
  beforeAll(() => {
    // Radix's Select trigger measures itself; jsdom has no ResizeObserver.
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      }
    );
  });

  it('prompts for a selection while no workflow is chosen', () => {
    renderForm();

    expect(workflowCombobox()).toHaveTextContent('Select a workflow');
  });

  it('shows the bound workflow name once one is chosen', () => {
    renderForm({ ...initialValues, workflowId: 'wf-2' } as TriggerSchemaType);

    expect(workflowCombobox()).toHaveTextContent('Inventory reconcile');
  });
});
