import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReportWorkflowActionConfig } from '../../types';
import {
  resolveWorkflowActionContext,
  useReportWorkflowAction,
} from './useReportWorkflowAction';

const runReportWorkflow = vi.hoisted(() => vi.fn());
const getReportWorkflowInstanceStatus = vi.hoisted(() => vi.fn());
const executeReportWorkflowAction = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

vi.mock('sonner', () => ({
  toast: { success: toastSuccess, error: toastError },
}));

vi.mock('../../queries', () => ({
  executeReportWorkflowAction: (...args: unknown[]) =>
    executeReportWorkflowAction(...args),
  runReportWorkflow: (...args: unknown[]) => runReportWorkflow(...args),
  getReportWorkflowInstanceStatus: (...args: unknown[]) =>
    getReportWorkflowInstanceStatus(...args),
}));

beforeEach(() => {
  runReportWorkflow.mockReset();
  getReportWorkflowInstanceStatus.mockReset();
  executeReportWorkflowAction.mockReset();
  toastSuccess.mockReset();
  toastError.mockReset();
});

afterEach(() => vi.useRealTimers());

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
    expect(toastSuccess).toHaveBeenCalledAfter(onCompleted);
  });

  it('observes a queued workflow after 100ms instead of waiting 1.5 seconds', async () => {
    vi.useFakeTimers();
    runReportWorkflow.mockResolvedValue({
      instanceId: 'instance-1',
      status: 'queued',
    });
    getReportWorkflowInstanceStatus.mockResolvedValue({
      id: 'instance-1',
      status: 'completed',
      outputs: { nextStage: 'review' },
    });
    const { result } = renderHook(() => useReportWorkflowAction());

    let runPromise: Promise<unknown>;
    act(() => {
      runPromise = result.current.run({
        key: 'advance',
        action: { workflowId: 'advance-case' },
      });
    });
    await act(async () => vi.advanceTimersByTimeAsync(99));
    expect(getReportWorkflowInstanceStatus).not.toHaveBeenCalled();

    await act(async () => vi.advanceTimersByTimeAsync(1));
    await act(async () => runPromise);

    expect(getReportWorkflowInstanceStatus).toHaveBeenCalledOnce();
    expect(toastSuccess).toHaveBeenCalledOnce();
  });

  it('backs off status checks and exposes running then refreshing phases', async () => {
    vi.useFakeTimers();
    let finishRefresh: (() => void) | undefined;
    const onCompleted = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finishRefresh = resolve;
        })
    );
    runReportWorkflow.mockResolvedValue({
      instanceId: 'instance-1',
      status: 'queued',
    });
    getReportWorkflowInstanceStatus
      .mockResolvedValueOnce({ id: 'instance-1', status: 'running' })
      .mockResolvedValueOnce({ id: 'instance-1', status: 'completed' });
    const { result } = renderHook(() =>
      useReportWorkflowAction({ onCompleted })
    );

    let runPromise: Promise<unknown>;
    act(() => {
      runPromise = result.current.run({
        key: 'advance',
        action: { workflowId: 'advance-case' },
      });
    });
    await act(async () => Promise.resolve());
    expect(result.current.phase('advance')).toBe('running');

    await act(async () => vi.advanceTimersByTimeAsync(100));
    expect(getReportWorkflowInstanceStatus).toHaveBeenCalledOnce();
    await act(async () => vi.advanceTimersByTimeAsync(199));
    expect(getReportWorkflowInstanceStatus).toHaveBeenCalledOnce();
    await act(async () => vi.advanceTimersByTimeAsync(1));

    expect(result.current.phase('advance')).toBe('refreshing');
    expect(toastSuccess).not.toHaveBeenCalled();
    await act(async () => finishRefresh?.());
    await act(async () => runPromise);
    expect(toastSuccess).toHaveBeenCalledOnce();
    expect(result.current.phase('advance')).toBeUndefined();
  });

  it('cancels observation without showing an error when the component unmounts', async () => {
    vi.useFakeTimers();
    runReportWorkflow.mockResolvedValue({
      instanceId: 'instance-1',
      status: 'queued',
    });
    const { result, unmount } = renderHook(() => useReportWorkflowAction());

    act(() => {
      void result.current.run({
        key: 'advance',
        action: { workflowId: 'advance-case' },
      });
    });
    await act(async () => Promise.resolve());
    unmount();
    await vi.runAllTimersAsync();

    expect(getReportWorkflowInstanceStatus).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
  });

  it('uses the report endpoint render without client polling or generic execution', async () => {
    const onCompleted = vi.fn();
    const render = {
      success: true,
      report: { id: 'report-1', definitionVersion: 1 },
      resolvedFilters: {},
      blocks: {},
      navigation: {
        requestedViewId: 'intake',
        activeViewId: 'review',
      },
      errors: [],
    };
    executeReportWorkflowAction.mockResolvedValue({
      completedWithinWait: true,
      execution: {
        workflowId: 'advance-case',
        version: 2,
        instanceId: 'instance-1',
        status: 'completed',
        durationMs: 45,
      },
      canonicalViewId: 'review',
      render,
    });
    const { result } = renderHook(() =>
      useReportWorkflowAction({
        onCompleted,
        report: {
          reportId: 'report-1',
          blockId: 'case-card',
          viewId: 'intake',
          filters: {},
        },
      })
    );

    await act(async () => {
      await result.current.run({
        key: 'advance',
        action: {
          id: 'advance-stage',
          workflowId: 'advance-case',
        },
        row: { id: 'case-1', stage: 'intake' },
        value: 'intake',
        fallbackField: 'stage',
      });
    });

    expect(executeReportWorkflowAction).toHaveBeenCalledWith(
      'test-token',
      expect.objectContaining({
        reportId: 'report-1',
        blockId: 'case-card',
        actionId: 'advance-stage',
        body: expect.objectContaining({
          trigger: expect.objectContaining({
            row: { id: 'case-1', stage: 'intake' },
          }),
        }),
      })
    );
    expect(runReportWorkflow).not.toHaveBeenCalled();
    expect(getReportWorkflowInstanceStatus).not.toHaveBeenCalled();
    expect(onCompleted).toHaveBeenCalledWith(
      expect.objectContaining({ render, canonicalViewId: 'review' }),
      expect.objectContaining({ id: 'advance-stage' })
    );
    expect(toastSuccess).toHaveBeenCalledAfter(onCompleted);
  });

  it('falls back to adaptive status observation when the report endpoint returns 202', async () => {
    vi.useFakeTimers();
    const onCompleted = vi.fn();
    executeReportWorkflowAction.mockResolvedValue({
      completedWithinWait: false,
      execution: {
        workflowId: 'advance-case',
        version: 2,
        instanceId: 'instance-1',
        status: 'running',
      },
      retryAfterMs: 500,
    });
    getReportWorkflowInstanceStatus.mockResolvedValue({
      id: 'instance-1',
      status: 'completed',
    });
    const { result } = renderHook(() =>
      useReportWorkflowAction({
        onCompleted,
        report: {
          reportId: 'report-1',
          blockId: 'case-card',
          viewId: 'intake',
          filters: {},
        },
      })
    );

    let runPromise: Promise<unknown>;
    act(() => {
      runPromise = result.current.run({
        key: 'advance',
        action: { id: 'advance-stage', workflowId: 'advance-case' },
        fallbackField: 'stage',
      });
    });
    await act(async () => vi.advanceTimersByTimeAsync(100));
    await act(async () => runPromise);

    expect(getReportWorkflowInstanceStatus).toHaveBeenCalledOnce();
    expect(onCompleted).toHaveBeenCalledOnce();
  });
});
