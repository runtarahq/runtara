import type {
  OperationQueue,
  OperationRequest,
  OperationRequestPage,
  OperationViewConfig,
  SavedOperationView,
  QueryExecutionsRequest,
  StateFilterDto,
} from '@/generated/RuntaraRuntimeApi';
import { defaultView, operationsRequest, selectedFields } from './queries';

export interface AttentionRequest {
  row: OperationRequest;
  workflowName: string;
  due?: string;
  isOverdue: boolean;
}
export interface AttentionQueue {
  queue: OperationQueue;
  view: OperationViewConfig;
}

/** Overview and its drilldowns must use the same due role, without saved-view filters. */
export function attentionQueues(
  queues: OperationQueue[],
  views: SavedOperationView[]
): AttentionQueue[] {
  return queues
    .filter((queue) => queue.count > 0)
    .map((queue) => ({
      queue,
      view:
        views.find(
          (v) =>
            v.configuration.workflow === queue.workflowId &&
            v.configuration.where?.openRequest === queue.actionKey &&
            v.configuration.roles?.due
        )?.configuration ?? defaultView(queue),
    }));
}
export function attentionRequest(
  row: OperationRequest,
  { queue, view }: AttentionQueue,
  now: number
): AttentionRequest {
  const value = view.roles?.due ? row.state?.[view.roles.due] : undefined;
  const due =
    typeof value === 'string' && Number.isFinite(Date.parse(value))
      ? value
      : undefined;
  return {
    row,
    workflowName: queue.workflowName,
    due,
    isOverdue: !!due && Date.parse(due) < now,
  };
}
export function overdueFilters(
  view: OperationViewConfig,
  now: number
): QueryExecutionsRequest['state'] {
  return view.roles?.due
    ? [
        {
          field: view.roles.due,
          op: 'lt',
          value: new Date(
            now
          ).toISOString() as unknown as StateFilterDto['value'],
        },
      ]
    : [];
}

/** Page across queue partitions without loading every request or silently truncating to a preview. */
export async function queryAttentionRequests(
  token: string,
  sources: AttentionQueue[],
  overdue: boolean,
  requestedPage: number,
  size = 25,
  now = Date.now()
) {
  const scopes = sources
    .filter((scope) => !overdue || scope.view.roles?.due)
    .sort(
      (a, b) =>
        a.queue.workflowName.localeCompare(b.queue.workflowName) ||
        a.queue.workflowId.localeCompare(b.queue.workflowId) ||
        a.queue.actionKey.localeCompare(b.queue.actionKey)
    );
  const fetchPage = (scope: AttentionQueue, page: number) =>
    operationsRequest<OperationRequestPage>(
      token,
      'operations/requests/query',
      'POST',
      {
        workflowId: scope.queue.workflowId,
        actionKey: scope.queue.actionKey,
        query: {
          page,
          size,
          stateFields: selectedFields(scope.view),
          state: overdue ? overdueFilters(scope.view, now) : [],
          stateSort:
            overdue && scope.view.roles?.due
              ? { field: scope.view.roles.due, descending: false }
              : undefined,
        },
      }
    );
  // Each partition supplies its total and first page in one call; all must succeed.
  const firstPages = await Promise.all(
    scopes.map((scope) => fetchPage(scope, 0))
  );
  const totalElements = firstPages.reduce(
    (sum, page) => sum + page.totalElements,
    0
  );
  const totalPages = Math.ceil(totalElements / size);
  const number = Math.max(0, Math.min(requestedPage, totalPages - 1));
  let offset = 0;
  const slices = scopes.map((scope, index) => {
    const first = firstPages[index];
    const start = Math.max(0, number * size - offset);
    const end = Math.min(first.totalElements, (number + 1) * size - offset);
    offset += first.totalElements;
    return { scope, first, start, end };
  });
  const parts = await Promise.all(
    slices.map(async ({ scope, first, start, end }) => {
      if (end <= start) return [];
      const firstPage = Math.floor(start / size);
      const lastPage = Math.floor((end - 1) / size);
      const pages = await Promise.all(
        Array.from(
          { length: lastPage - firstPage + 1 },
          (_, i) => firstPage + i
        ).map((page) => (page === 0 ? first : fetchPage(scope, page)))
      );
      return pages
        .flatMap((page) => page.content)
        .slice(start % size, (start % size) + end - start)
        .map((row) => attentionRequest(row, scope, now));
    })
  );
  return { content: parts.flat(), totalElements, totalPages, number, size };
}
