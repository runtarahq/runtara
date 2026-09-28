import type { QueryClient } from '@tanstack/react-query';
import { queryKeys } from '@/shared/queries/query-keys';

/**
 * Refetch the views that show a run's status after it is stopped, paused or
 * resumed: execution lists (invocation history, child runs) and every
 * workflow's instance queries.
 */
export function refreshRunViews(queryClient: QueryClient) {
  return Promise.all([
    queryClient.invalidateQueries({ queryKey: queryKeys.executions.lists() }),
    queryClient.invalidateQueries({ queryKey: queryKeys.workflows.details() }),
  ]);
}
