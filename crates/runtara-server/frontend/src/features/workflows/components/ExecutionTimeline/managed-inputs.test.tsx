import {
  cleanup,
  render,
  screen,
  fireEvent,
  waitFor,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ExecutionTimeline } from './index';
import { deliverSignal } from '@/features/workflows/queries';
import { ManagedInputScope, InputRetryPanel } from '../ManagedInputSubmissions';

const state = vi.hoisted(() => ({
  error: null as Error | null,
  loading: false,
  empty: false,
}));
vi.mock('@/features/workflows/hooks/useHierarchicalTimeline', () => ({
  useHierarchicalTimeline: () => ({
    visibleSteps: [],
    totalDuration: 0,
    stats: {},
    isLoadingRoot: state.loading,
  }),
}));
vi.mock('@/shared/hooks', () => ({ useToken: () => 'token' }));
vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { profile: { sub: 'viewer' } } }),
}));
vi.mock('@tanstack/react-query', () => ({
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));
vi.mock('@/shared/hooks/api', () => ({
  useCustomQuery: ({ queryKey }: { queryKey: string[] }) =>
    queryKey.includes('pendingInput')
      ? {
          data: state.error
            ? undefined
            : state.empty
              ? []
              : [{ requestId: 'managed-request', message: 'Approve order' }],
          error: state.error,
        }
      : { data: { status: 'suspended' } },
  useCustomMutation: () => ({ mutate: vi.fn() }),
}));
vi.mock('@/features/workflows/queries', () => ({
  getWorkflowInstance: vi.fn(),
  getPendingInput: vi.fn(),
  deliverSignal: vi.fn(),
}));
vi.mock(
  '@/features/workflows/components/ExecutionPanel/HumanInputCard',
  () => ({
    HumanInputCard: ({
      pendingInput,
      onSubmit,
    }: {
      pendingInput: { message: string };
      onSubmit: (id: string, payload: Record<string, unknown>) => void;
    }) => (
      <button onClick={() => onSubmit('managed-request', { answer: true })}>
        {pendingInput.message}
      </button>
    ),
  })
);
afterEach(() => {
  cleanup();
  state.error = null;
  state.loading = false;
  state.empty = false;
  vi.mocked(deliverSignal).mockReset();
});
describe('timeline inputs without debug telemetry', () => {
  it('shares retry state with the page when a timeline tab unmounts', async () => {
    vi.mocked(deliverSignal)
      .mockRejectedValueOnce(new Error('Offline'))
      .mockResolvedValueOnce({
        requestId: 'managed-request',
        receiptId: 'receipt',
        acceptedAt: 'now',
      });
    const view = render(
      <ManagedInputScope>
        <ExecutionTimeline workflowId="workflow" instanceId="instance" />
      </ManagedInputScope>
    );
    fireEvent.click(screen.getByRole('button', { name: 'Approve order' }));
    await screen.findByText(/Acceptance is unconfirmed/);
    view.rerender(
      <ManagedInputScope>
        <InputRetryPanel matches={() => true} />
      </ManagedInputScope>
    );
    fireEvent.click(
      screen.getByRole('button', { name: 'Retry original response' })
    );
    await waitFor(() =>
      expect(
        screen.queryByRole('button', { name: 'Retry original response' })
      ).not.toBeInTheDocument()
    );
    expect(vi.mocked(deliverSignal).mock.calls[1]).toEqual(
      vi.mocked(deliverSignal).mock.calls[0]
    );
  });
  it.each([false, true])(
    'shows managed requests while timeline loading is %s',
    (loading) => {
      state.loading = loading;
      render(<ExecutionTimeline workflowId="workflow" instanceId="instance" />);
      expect(screen.getByText('Approve order')).toBeInTheDocument();
    }
  );
  it('shows discovery failure even with no timeline events', () => {
    state.error = new Error('Service unavailable');
    render(<ExecutionTimeline workflowId="workflow" instanceId="instance" />);
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Pending inputs are unavailable'
    );
  });
  it('retries the original response after refresh removes the action and the execution changes', async () => {
    vi.mocked(deliverSignal)
      .mockRejectedValueOnce(new Error('Acknowledgement lost'))
      .mockResolvedValueOnce({
        requestId: 'managed-request',
        receiptId: 'receipt',
        acceptedAt: 'now',
      });
    const view = render(
      <ExecutionTimeline workflowId="workflow" instanceId="first" />
    );
    fireEvent.click(screen.getByRole('button', { name: 'Approve order' }));
    await screen.findByText(/Acceptance is unconfirmed/);
    state.empty = true;
    view.rerender(
      <ExecutionTimeline workflowId="workflow" instanceId="second" />
    );
    expect(
      screen.queryByRole('button', { name: 'Retry original response' })
    ).not.toBeInTheDocument();
    view.rerender(
      <ExecutionTimeline workflowId="workflow" instanceId="first" />
    );
    fireEvent.click(
      screen.getByRole('button', { name: 'Retry original response' })
    );
    await waitFor(() =>
      expect(
        screen.queryByRole('button', { name: 'Retry original response' })
      ).not.toBeInTheDocument()
    );
    const calls = vi.mocked(deliverSignal).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[1]).toEqual(calls[0]);
    expect(calls[1][1]).toBe('first');
  });
});
