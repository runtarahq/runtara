import {
  ListAllExecutionsResponse,
  ScenarioInstanceDto,
} from '@/generated/RuntaraRuntimeApi';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { ExecutionHistoryItem, ExecutionHistoryFilters } from '../types';

// Extended type for instances that may include additional fields from the API
type ScenarioInstanceWithTimestamps = ScenarioInstanceDto & {
  started?: string | null;
  finished?: string | null;
};

interface ExecutionsQueryParams {
  pageIndex?: number;
  pageSize?: number;
  filters?: ExecutionHistoryFilters;
}

export async function getAllExecutions(
  token: string,
  context: { queryKey: readonly unknown[] }
) {
  // Params are the last element of the hierarchical key
  const params = context.queryKey[context.queryKey.length - 1] as
    | ExecutionsQueryParams
    | undefined;
  const pageIndex = params?.pageIndex ?? 0;
  const pageSize = params?.pageSize ?? 10;
  const filters = params?.filters ?? ({} as ExecutionHistoryFilters);

  const result = await RuntimeREST.api.listAllExecutionsHandler(
    {
      page: pageIndex,
      size: pageSize,
      scenarioId: filters.scenarioId || undefined,
      status: filters.status || undefined,
      createdFrom: filters.createdFrom || undefined,
      createdTo: filters.createdTo || undefined,
      completedFrom: filters.completedFrom || undefined,
      completedTo: filters.completedTo || undefined,
      sortBy: filters.sortBy || 'createdAt',
      sortOrder: filters.sortOrder || 'desc',
    },
    createAuthHeaders(token)
  );

  const responseData = result.data as ListAllExecutionsResponse;
  const pageData = responseData.data;

  const instances = pageData.content;

  // Transform the API format to match our ExecutionHistoryItem format
  const executionHistory: ExecutionHistoryItem[] = instances.map(
    (instance: ScenarioInstanceWithTimestamps) => ({
      instanceId: instance.id,
      scenarioId: instance.scenarioId,
      scenarioName: instance.scenarioName ?? undefined,
      createdAt: instance.created,
      startedAt: instance.started || null,
      completedAt: instance.finished || instance.updated || null,
      status: instance.status || 'unknown',
      terminationType: instance.terminationType || null,
      version: instance.usedVersion,
      executionDurationSeconds: instance.executionDurationSeconds ?? null,
      queueDurationSeconds: instance.queueDurationSeconds ?? null,
      maxMemoryMb: instance.maxMemoryMb ?? null,
      tags: instance.tags || [],
      hasPendingInput: instance.hasPendingInput ?? false,
    })
  );

  return {
    content: executionHistory,
    number: pageData.number,
    size: pageData.size,
    totalElements: pageData.totalElements,
    totalPages: pageData.totalPages,
  };
}
