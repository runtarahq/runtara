import type { ExecutionHistoryFilters, ExecutionHistoryItem } from '../types';

/** Detail page of one run. */
export function runDetailPath(workflowId: string, instanceId: string): string {
  return `/workflows/${encodeURIComponent(workflowId)}/history/${encodeURIComponent(instanceId)}`;
}

/** Invocation History filtered to the runs a parent run started. */
export function childRunsListPath(parentInstanceId: string): string {
  return `/invocation-history?${new URLSearchParams({ parentInstanceId })}`;
}

/**
 * Executions-list query that finds one run by id. The list search matches
 * instance ids, so the run is on the first page; `findRun` then picks the
 * exact match out of any looser label/metadata hits.
 */
export function runLookupQueryParams(instanceId: string): {
  pageIndex: number;
  pageSize: number;
  filters: ExecutionHistoryFilters;
} {
  return { pageIndex: 0, pageSize: 20, filters: { search: instanceId } };
}

export function findRun(
  runs: ExecutionHistoryItem[] | undefined,
  instanceId: string
): ExecutionHistoryItem | undefined {
  return runs?.find((run) => run.instanceId === instanceId);
}

export const CHILD_RUNS_PAGE_SIZE = 10;

/** First page (newest first) of the runs `parentInstanceId` started. */
export function childRunsQueryParams(parentInstanceId: string): {
  pageIndex: number;
  pageSize: number;
  filters: ExecutionHistoryFilters;
} {
  return {
    pageIndex: 0,
    pageSize: CHILD_RUNS_PAGE_SIZE,
    filters: { parentInstanceId, sortBy: 'createdAt', sortOrder: 'desc' },
  };
}
