import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import type {
  OperationProcess,
  OperationQueue,
  OperationRequestPage,
  OperationViewConfig,
  SavedOperationView,
  QueryExecutionsRequest,
  PageWorkflowInstanceHistoryDto,
} from '@/generated/RuntaraRuntimeApi';

export async function operationsRequest<T>(
  token: string,
  path: string,
  method = 'GET',
  data?: unknown
): Promise<T> {
  const response = await RuntimeREST.instance.request<{ data: T }>({
    url: `/api/runtime/${path}`,
    method,
    data,
    ...createAuthHeaders(token),
  });
  return response.data.data;
}
export function useOperations() {
  const tenant = useAuthStore((s) => s.orgId);
  const queues = useCustomQuery({
    queryKey: ['operations', tenant, 'queues'],
    queryFn: (token: string) =>
      operationsRequest<OperationQueue[]>(token, 'operations/queues'),
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  const views = useCustomQuery({
    queryKey: ['operations', tenant, 'views'],
    queryFn: (token: string) =>
      operationsRequest<SavedOperationView[]>(token, 'operations/views'),
    placeholderData: undefined,
  });
  const processes = useCustomQuery({
    queryKey: ['operations', tenant, 'processes'],
    queryFn: (token: string) =>
      operationsRequest<OperationProcess[]>(token, 'operations/processes'),
    placeholderData: undefined,
  });
  return { queues, views, processes };
}
export function defaultView(queue: OperationQueue): OperationViewConfig {
  return {
    name: queue.name,
    workflow: queue.workflowId,
    columns: Object.keys(queue.stateSchema ?? {}).slice(0, 29),
    where: { openRequest: queue.actionKey, state: [] },
    roles: {},
    answers: { bulk: true },
    formats: {},
    labels: {},
  };
}
export function selectedFields(view: OperationViewConfig): string[] {
  return [
    ...new Set(
      [
        ...(view.columns ?? []),
        view.roles?.key,
        view.roles?.stage,
        view.roles?.due,
      ].filter((s): s is string => Boolean(s))
    ),
  ];
}
/** Re-evaluate relative time on every poll, without changing saved filters. */
export function resolveQuery(
  view: OperationViewConfig,
  now = Date.now()
): QueryExecutionsRequest {
  return {
    workflowId: view.workflow,
    stateFields: selectedFields(view),
    stateSort: view.sort,
    status: view.where?.status,
    state: (view.where?.state ?? []).map((f) => {
      const value = f.value as unknown;
      if (
        value &&
        typeof value === 'object' &&
        'relative' in value &&
        value.relative === 'now'
      ) {
        const offset =
          'offsetSeconds' in value ? Number(value.offsetSeconds) : 0;
        return {
          ...f,
          value: new Date(
            now + offset * 1000
          ).toISOString() as unknown as typeof f.value,
        };
      }
      return f;
    }),
  };
}
export function queryRequests(
  token: string,
  view: OperationViewConfig,
  page: number,
  search: string
) {
  return operationsRequest<OperationRequestPage>(
    token,
    'operations/requests/query',
    'POST',
    {
      workflowId: view.workflow,
      actionKey: view.where?.openRequest,
      query: {
        ...resolveQuery(view),
        page,
        size: 25,
        search: search || undefined,
      },
    }
  );
}
export function queryRuns(token: string, query: QueryExecutionsRequest) {
  return operationsRequest<PageWorkflowInstanceHistoryDto>(
    token,
    'executions/query',
    'POST',
    query
  );
}
export function message(error: unknown): string {
  const e = error as {
    response?: { data?: { message?: string } };
    message?: string;
  };
  return (
    e.response?.data?.message ?? e.message ?? 'The request failed. Try again.'
  );
}

export function failureText(
  summary:
    | import('@/generated/RuntaraRuntimeApi').OperationErrorSummary
    | null
    | undefined,
  fallback?: string | null
): string {
  if (!summary) return fallback ?? 'No error details recorded';
  if (
    !summary.code ||
    !['transient', 'permanent'].includes(summary.category ?? '')
  )
    return summary.message;
  return JSON.stringify({
    ...summary,
    severity: summary.severity ?? 'error',
    attributes: {},
  });
}
