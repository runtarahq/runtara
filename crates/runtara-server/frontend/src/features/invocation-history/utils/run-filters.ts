import type { ExecutionHistoryFilters } from '../types';

const keys = [
  'workflowId',
  'status',
  'search',
  'runLabel',
  'parentInstanceId',
  'createdFrom',
  'createdTo',
  'completedFrom',
  'completedTo',
  'sortBy',
  'sortOrder',
  'range',
  'dateBasis',
] as const;

export function readRunFilters(
  params: URLSearchParams
): ExecutionHistoryFilters {
  const result = Object.fromEntries(
    keys.flatMap((key) => (params.get(key) ? [[key, params.get(key)!]] : []))
  ) as ExecutionHistoryFilters;
  if (
    !['createdAt', 'completedAt', 'status', 'workflowId'].includes(
      result.sortBy ?? ''
    )
  )
    result.sortBy = 'createdAt';
  if (!['asc', 'desc'].includes(result.sortOrder ?? ''))
    result.sortOrder = 'desc';
  if (!['24h', '7d', 'all', 'custom'].includes(result.range ?? ''))
    result.range = undefined;
  if (!['started', 'completed'].includes(result.dateBasis ?? ''))
    result.dateBasis = undefined;
  return result;
}

export function writeRunFilters(
  filters: ExecutionHistoryFilters,
  previous = new URLSearchParams()
) {
  const params = new URLSearchParams(previous);
  for (const key of keys) {
    const value = filters[key];
    if (value) params.set(key, value);
    else params.delete(key);
  }
  return params;
}

export function resolveRunFilters(
  filters: ExecutionHistoryFilters,
  now: number
): ExecutionHistoryFilters {
  const { range, dateBasis, ...query } = filters;
  if (range === '24h' || range === '7d') {
    const from = new Date(
      now - (range === '24h' ? 24 : 168) * 3_600_000
    ).toISOString();
    return {
      ...query,
      createdFrom: dateBasis === 'completed' ? undefined : from,
      createdTo:
        dateBasis === 'completed' ? undefined : new Date(now).toISOString(),
      completedFrom: dateBasis === 'completed' ? from : undefined,
      completedTo:
        dateBasis === 'completed' ? new Date(now).toISOString() : undefined,
    };
  }
  return query;
}

export function summaryFilters(filters: ExecutionHistoryFilters) {
  const {
    search,
    workflowId,
    runLabel,
    parentInstanceId,
    createdFrom,
    createdTo,
    completedFrom,
    completedTo,
  } = filters;
  return {
    search,
    workflowId,
    runLabel,
    parentInstanceId,
    createdFrom,
    createdTo,
    completedFrom,
    completedTo,
  };
}

export const runStatusFilters = [
  { label: 'All', status: undefined, keys: [] },
  { label: 'Running', status: 'running', keys: ['running'] },
  { label: 'Waiting', status: 'suspended', keys: ['suspended'] },
  { label: 'Failed', status: 'failed,timeout', keys: ['failed', 'timeout'] },
  { label: 'Completed', status: 'completed', keys: ['completed'] },
];
export function selectedRunStatus(status?: string) {
  const normalized =
    status
      ?.split(',')
      .map((s) => s.trim())
      .filter(Boolean)
      .sort()
      .join(',') || undefined;
  return runStatusFilters.find(
    (item) =>
      item.status === normalized ||
      (item.label === 'Failed' && normalized === 'failed')
  )?.label;
}
