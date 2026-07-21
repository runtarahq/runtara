import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReportWorkflowActionConfig } from '../../types';
import {
  resolveWorkflowActionContext,
  useReportWorkflowAction,
} from './useReportWorkflowAction';

const runReportWorkflow = vi.hoisted(() => vi.fn());
const getReportWorkflowInstanceStatus = vi.hoisted(() => vi.fn());

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

vi.mock('../../queries', () => ({
  runReportWorkflow: (...args: unknown[]) => runReportWorkflow(...args),
  getReportWorkflowInstanceStatus: (...args: unknown[]) =>
    getReportWorkflowInstanceStatus(...args),
}));

beforeEach(() => {
  runReportWorkflow.mockReset();
  getReportWorkflowInstanceStatus.mockReset();
});

describe('resolveWorkflowActionContext', () => {
  it('passes selected rows for table-wide workflow actions', () => {
    const action: ReportWorkflowActionConfig = {
      workflowId: 'process-items',
      context: { mode: 'selection', inputKey: 'items' },
    };
    const selectedRows = [
      { id: 'row-1', status: 'ready' },
      { id: 'row-2', status: 'blocked' },
    ];

    expect(
      resolveWorkflowActionContext(
        action,
        { id: 'ignored-row' },
        'ignored-value',
        'ignored_field',
        selectedRows
      )
    ).toEqual({ items: selectedRows });
  });

  it('notifies the report after every successful workflow, even without reloadBlock', async () => {
    const onCompleted = vi.fn();
    runReportWorkflow.mockResolvedValue({
      instanceId: 'instance-1',
      status: 'completed',
    });
    const { result } = renderHook(() =>
      useReportWorkflowAction({ onCompleted })
    );

    await act(async () => {
      await result.current.run({
        key: 'advance',
        action: { workflowId: 'advance-case', reloadBlock: false },
      });
    });

    expect(onCompleted).toHaveBeenCalledOnce();
  });
});
