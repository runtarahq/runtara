import type { ExecutionHistoryFilters } from '../types';
import type { ExecutionSummary } from '@/generated/RuntaraRuntimeApi';
import { runStatusFilters, selectedRunStatus } from '../utils/run-filters';
import { RefreshControls } from '@/features/operations/pages/shared';

export function RunControls({
  filters,
  onChange,
  summary,
  countsUnavailable,
  refresh,
  setRefresh,
  busy,
  updatedAt,
  onRefresh,
}: {
  filters: ExecutionHistoryFilters;
  onChange: (filters: ExecutionHistoryFilters) => void;
  summary?: ExecutionSummary;
  countsUnavailable: boolean;
  refresh: boolean;
  setRefresh: (value: boolean) => void;
  busy: boolean;
  updatedAt?: number;
  onRefresh: () => void;
}) {
  const selectClass =
    'h-8 min-w-0 rounded-md border bg-background px-2 text-sm';
  const current = selectedRunStatus(filters.status);
  const range =
    filters.range ??
    (filters.createdFrom ||
    filters.createdTo ||
    filters.completedFrom ||
    filters.completedTo
      ? 'custom'
      : 'all');
  const setRange = (value: ExecutionHistoryFilters['range']) =>
    onChange({
      ...filters,
      range: value,
      ...(value !== 'custom'
        ? {
            createdFrom: undefined,
            createdTo: undefined,
            completedFrom: undefined,
            completedTo: undefined,
          }
        : {}),
    });
  return (
    <div className="space-y-3 border-b px-4 py-3">
      <div
        className="flex flex-wrap items-center gap-2"
        aria-label="Run status filters"
      >
        {runStatusFilters.map((item) => (
          <button
            key={item.label}
            type="button"
            aria-pressed={current === item.label}
            onClick={() => onChange({ ...filters, status: item.status })}
            className={`rounded-md border px-3 py-1.5 text-sm ${current === item.label ? 'border-primary bg-primary/10 font-medium text-primary-text' : 'hover:bg-muted'}`}
          >
            {item.label}{' '}
            <span className="ml-1 tabular-nums text-muted-foreground">
              {!summary || countsUnavailable
                ? '—'
                : (item.label === 'All'
                    ? summary.total
                    : item.keys.reduce(
                        (n, key) => n + (summary.counts[key] ?? 0),
                        0
                      )
                  ).toLocaleString()}
            </span>
          </button>
        ))}
        {!current && <span className="text-sm">Status: {filters.status}</span>}
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <select
          className={selectClass}
          aria-label="Date basis"
          value={filters.dateBasis ?? 'started'}
          onChange={(e) =>
            onChange({
              ...filters,
              dateBasis: e.target.value as 'started' | 'completed',
            })
          }
        >
          <option value="started">Started</option>
          <option value="completed">Completed</option>
        </select>
        <select
          className={selectClass}
          aria-label="Run time range"
          value={range}
          onChange={(e) =>
            setRange(e.target.value as ExecutionHistoryFilters['range'])
          }
        >
          <option value="all">All time</option>
          <option value="24h">Last 24 hours</option>
          <option value="7d">Last 7 days</option>
          <option value="custom">Custom dates</option>
        </select>
        <select
          className={selectClass}
          aria-label="Run order"
          value={`${filters.sortBy ?? 'createdAt'}:${filters.sortOrder ?? 'desc'}`}
          onChange={(e) => {
            const [sortBy, sortOrder] = e.target.value.split(':');
            onChange({
              ...filters,
              sortBy: sortBy as ExecutionHistoryFilters['sortBy'],
              sortOrder: sortOrder as 'asc' | 'desc',
            });
          }}
        >
          <option value="createdAt:desc">Newest started first</option>
          <option value="createdAt:asc">Oldest started first</option>
          <option value="completedAt:desc">Newest completed first</option>
          <option value="completedAt:asc">Oldest completed first</option>
          {filters.sortBy &&
            !['createdAt', 'completedAt'].includes(filters.sortBy) && (
              <option value={`${filters.sortBy}:${filters.sortOrder}`}>
                Custom order
              </option>
            )}
        </select>
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            role="switch"
            checked={refresh}
            onChange={(e) => setRefresh(e.target.checked)}
          />
          Refresh every 30 s
        </label>
        <RefreshControls
          busy={busy}
          updatedAt={updatedAt}
          onRefresh={onRefresh}
        />
      </div>
      {range === 'custom' && (
        <p className="text-xs text-muted-foreground">
          Set custom dates in Filters. Both started and completed bounds apply
          when supplied.
        </p>
      )}
      {range !== 'all' && (
        <p className="text-xs text-muted-foreground">
          {range === 'custom'
            ? 'Custom date filters'
            : `${filters.dateBasis === 'completed' ? 'Completed' : 'Started (run created)'} in the ${range === '24h' ? 'last 24 hours' : 'last 7 days'}`}
        </p>
      )}
    </div>
  );
}
