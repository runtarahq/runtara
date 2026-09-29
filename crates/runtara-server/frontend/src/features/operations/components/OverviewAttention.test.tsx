import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import { useAuthStore } from '@/shared/stores/authStore';
import type { WorkflowInstanceDto } from '@/generated/RuntaraRuntimeApi';
import { OverviewAttention } from './OverviewAttention';

vi.mock('../pages/shared', () => ({
  ReplayButton: ({ run }: { run: WorkflowInstanceDto }) => (
    <p>Confirm run {run.id}</p>
  ),
}));
const failure = {
  id: 'run-id',
  workflowId: 'orders',
  runLabel: 'ORDER-123',
  workflowName: 'Orders',
  status: 'failed',
  created: '2026-09-29T10:00:00Z',
  updated: '2026-09-29T10:01:00Z',
  usedVersion: 1,
  inputs: {},
  error: JSON.stringify({
    message: 'Try again later',
    code: 'TEMP',
    category: 'transient',
  }),
} as WorkflowInstanceDto;
const props = {
  requests: [],
  requestCount: 0,
  requestsPending: false,
  requestsError: false,
  failures: [failure],
  failureCount: 3,
  failuresPending: false,
  failuresError: false,
};
afterEach(() => act(() => useAuthStore.getState().clearMe()));

describe('Overview attention', () => {
  it('keeps Replay attached to the chosen run when the list refreshes', () => {
    const { rerender } = render(
      <MemoryRouter>
        <OverviewAttention {...props} />
      </MemoryRouter>
    );
    fireEvent.click(screen.getByRole('button', { name: 'Replay' }));
    rerender(
      <MemoryRouter>
        <OverviewAttention {...props} failures={[]} />
      </MemoryRouter>
    );
    expect(screen.getByRole('dialog')).toHaveTextContent('Confirm run run-id');
  });
  it('keeps copy and details available to readers while hiding Replay', () => {
    useAuthStore
      .getState()
      .setMe({
        role: 'viewer',
        permissions: { 'invocation_history:read': true } as never,
      });
    render(
      <MemoryRouter>
        <OverviewAttention {...props} />
      </MemoryRouter>
    );
    expect(
      screen.queryByRole('button', { name: 'Replay' })
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Copy run ID' })
    ).toBeInTheDocument();
    expect(
      screen.getByRole('link', { name: 'Open execution' })
    ).toHaveAttribute('href', '/workflows/orders/history/run-id');
  });
  it('shows a failed prerequisite as an error instead of loading or an empty queue', () => {
    render(
      <MemoryRouter>
        <OverviewAttention {...props} requestsPending requestsError />
      </MemoryRouter>
    );
    expect(
      within(
        screen.getByRole('region', { name: 'Requests requiring input' })
      ).getByText('Could not load this group. Try Refresh.')
    ).toBeInTheDocument();
    expect(
      screen.getByRole('link', { name: 'View all requests' })
    ).toHaveAttribute('href', '/operations/queues');
    expect(
      screen.getByRole('link', { name: 'View failed runs' })
    ).toHaveAttribute('href', expect.stringContaining('dateBasis=completed'));
  });
});
