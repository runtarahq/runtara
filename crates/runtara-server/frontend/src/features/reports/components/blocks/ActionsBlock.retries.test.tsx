import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  ManagedInputScope,
  InputRetryPanel,
} from '@/features/workflows/components/ManagedInputSubmissions';
import { useAuthStore } from '@/shared/stores/authStore';
import { submitReportWorkflowAction } from '../../queries';
import { ActionsBlock } from './ActionsBlock';
import type {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportWorkflowAction,
} from '../../types';

const state = vi.hoisted(() => ({
  principal: 'viewer',
  invalidate: vi.fn().mockResolvedValue(undefined),
}));
vi.mock('react-oidc-context', () => ({
  useAuth: () => ({
    user: { access_token: 'token', profile: { sub: state.principal } },
  }),
}));
vi.mock('@/shared/hooks', () => ({ useToken: () => 'token' }));
vi.mock('@tanstack/react-query', () => ({
  useQueryClient: () => ({ invalidateQueries: state.invalidate }),
}));
vi.mock('../../queries', () => ({ submitReportWorkflowAction: vi.fn() }));
vi.mock('@/features/workflows/components/ActionForm', () => ({
  ActionForm: ({
    onSubmit,
    disabled,
  }: {
    onSubmit: (payload: Record<string, unknown>) => void;
    disabled: boolean;
  }) => (
    <button disabled={disabled} onClick={() => onSubmit({ answer: true })}>
      Send approval
    </button>
  ),
}));

const block = {
  id: 'approve',
  type: 'actions',
  source: {
    kind: 'workflow_runtime',
    entity: 'actions',
    workflowId: 'workflow',
  },
} as ReportBlockDefinition;
const action = (instanceId: string): ReportWorkflowAction =>
  ({
    actionId: 'same-request-hash',
    instanceId,
    label: `Approve ${instanceId}`,
    message: '',
    actionKind: 'external_input',
    status: 'open',
  }) as ReportWorkflowAction;
const receipt = {
  requestId: 'same-request-hash',
  receiptId: 'receipt',
  acceptedAt: 'now',
};

function Page({
  visible = true,
  instances = ['one'],
  report = 'report',
  region = 'first',
}: {
  visible?: boolean;
  instances?: string[];
  report?: string;
  region?: string;
}) {
  return (
    <ManagedInputScope>
      <InputRetryPanel
        matches={(request) =>
          request.kind === 'report' && request.reportId === report
        }
      />
      {visible && (
        <ActionsBlock
          reportId={report}
          block={block}
          result={
            { data: { actions: instances.map(action) } } as ReportBlockResult
          }
          filters={{ region }}
          blockFilters={{ team: region }}
        />
      )}
    </ManagedInputScope>
  );
}

beforeEach(() => {
  useAuthStore.setState({ orgId: 'tenant' });
  state.principal = 'viewer';
  vi.mocked(submitReportWorkflowAction).mockReset();
  state.invalidate.mockClear();
});
afterEach(cleanup);

describe('report submission owner', () => {
  it('retains target, filters and operation after the action block unmounts and remounts empty', async () => {
    vi.mocked(submitReportWorkflowAction)
      .mockRejectedValueOnce(new Error('Lost acknowledgement'))
      .mockResolvedValueOnce(receipt);
    const view = render(<Page />);
    fireEvent.click(screen.getByRole('button', { name: 'Send approval' }));
    await screen.findByText(/Acceptance is unconfirmed/);
    view.rerender(<Page visible={false} region="changed" />);
    view.rerender(<Page instances={[]} region="changed" />);
    expect(screen.getByText('No open actions.')).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole('button', { name: 'Retry original response' })
    );
    await waitFor(() =>
      expect(
        screen.queryByRole('button', { name: 'Retry original response' })
      ).not.toBeInTheDocument()
    );
    const calls = vi.mocked(submitReportWorkflowAction).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[1]).toEqual(calls[0]);
    expect(calls[1][1]).toMatchObject({
      instanceId: 'one',
      filters: { region: 'first' },
      blockFilters: { team: 'first' },
    });
  });

  it('allocates another operation for changed report filters without discarding the uncertain original', async () => {
    vi.mocked(submitReportWorkflowAction).mockRejectedValue(
      new Error('Offline')
    );
    const view = render(<Page />);
    fireEvent.click(screen.getByRole('button', { name: 'Send approval' }));
    await screen.findByText(/Acceptance is unconfirmed/);
    view.rerender(<Page region="second" />);
    fireEvent.click(screen.getByRole('button', { name: 'Send approval' }));
    await waitFor(() => expect(screen.getAllByRole('alert')).toHaveLength(2));
    const calls = vi.mocked(submitReportWorkflowAction).mock.calls;
    expect(calls[0][1].operationId).not.toBe(calls[1][1].operationId);
    fireEvent.click(
      screen.getAllByRole('button', { name: 'Retry original response' })[0]
    );
    await waitFor(() =>
      expect(submitReportWorkflowAction).toHaveBeenCalledTimes(3)
    );
    expect(vi.mocked(submitReportWorkflowAction).mock.calls[2]).toEqual(
      calls[0]
    );
  });

  it('does not hide another instance with the same request hash on success', async () => {
    vi.mocked(submitReportWorkflowAction).mockResolvedValue(receipt);
    render(<Page instances={['one', 'two']} />);
    fireEvent.click(
      screen.getAllByRole('button', { name: 'Send approval' })[0]
    );
    await waitFor(() =>
      expect(
        screen.getAllByRole('button', { name: 'Send approval' })
      ).toHaveLength(1)
    );
    expect(screen.getByText('Approve two')).toBeInTheDocument();
  });

  it('hides retries on another report and restores the original intent on return', async () => {
    vi.mocked(submitReportWorkflowAction).mockRejectedValue(
      new Error('Offline')
    );
    const view = render(<Page />);
    fireEvent.click(screen.getByRole('button', { name: 'Send approval' }));
    await screen.findByText(/Acceptance is unconfirmed/);
    view.rerender(<Page report="other" instances={[]} />);
    expect(
      screen.queryByRole('button', { name: 'Retry original response' })
    ).not.toBeInTheDocument();
    view.rerender(<Page instances={[]} />);
    expect(
      screen.getByRole('button', { name: 'Retry original response' })
    ).toBeInTheDocument();
  });

  it.each(['tenant', 'principal'])(
    'discards private retry state on a %s change and ignores late completion',
    async (kind) => {
      let resolve!: (value: unknown) => void;
      vi.mocked(submitReportWorkflowAction).mockImplementation(
        () =>
          new Promise((done) => {
            resolve = done;
          })
      );
      const view = render(<Page />);
      fireEvent.click(screen.getByRole('button', { name: 'Send approval' }));
      expect(screen.getByText('Confirming response…')).toBeInTheDocument();
      act(() => {
        if (kind === 'tenant')
          useAuthStore.setState({ orgId: 'another-tenant' });
        else state.principal = 'another-viewer';
      });
      view.rerender(<Page instances={[]} />);
      await act(async () => {
        resolve(receipt);
      });
      expect(
        screen.queryByRole('region', { name: 'Response submissions' })
      ).not.toBeInTheDocument();
      expect(state.invalidate).not.toHaveBeenCalled();
      expect(submitReportWorkflowAction).toHaveBeenCalledTimes(1);
    }
  );
});
