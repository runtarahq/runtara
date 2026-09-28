import { describe, expect, it } from 'vitest';
import {
  childRunsListPath,
  childRunsQueryParams,
  findRun,
  runDetailPath,
  runLookupQueryParams,
} from './run-links';
import type { ExecutionHistoryItem } from '../types';

const run = (instanceId: string, workflowId = 'wf'): ExecutionHistoryItem => ({
  instanceId,
  workflowId,
  createdAt: '2026-09-27',
  status: 'completed',
  version: 1,
});

describe('run links', () => {
  it('builds detail and child-list paths', () => {
    expect(runDetailPath('wf-1', 'run-1')).toBe(
      '/workflows/wf-1/history/run-1'
    );
    expect(childRunsListPath('run-1')).toBe(
      '/invocation-history?parentInstanceId=run-1'
    );
  });

  it('looks a parent up by id and picks the exact match only', () => {
    expect(runLookupQueryParams('p-1').filters).toEqual({ search: 'p-1' });
    const runs = [run('p-10', 'other'), run('p-1', 'parent-wf')];
    expect(findRun(runs, 'p-1')?.workflowId).toBe('parent-wf');
    expect(findRun(runs, 'p-2')).toBeUndefined();
    expect(findRun(undefined, 'p-1')).toBeUndefined();
  });

  it('queries the first page of children newest first', () => {
    expect(childRunsQueryParams('p-1')).toEqual({
      pageIndex: 0,
      pageSize: 10,
      filters: {
        parentInstanceId: 'p-1',
        sortBy: 'createdAt',
        sortOrder: 'desc',
      },
    });
  });
});
