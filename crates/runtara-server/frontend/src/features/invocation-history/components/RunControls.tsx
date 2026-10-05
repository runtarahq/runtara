import type { ComponentProps } from 'react';
import { CalendarDays, Columns3, ChevronDown } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import type { RunExtraColumn } from './OperationsRunColumns';
import type { ExecutionHistoryFilters } from '../types';
import type { ExecutionSummary } from '@/generated/RuntaraRuntimeApi';
import { runStatusFilters, selectedRunStatus } from '../utils/run-filters';
import { RefreshControls } from '@/features/operations/pages/shared';

export function RunControls({
  filters,
  onChange,
  extraColumns,
  onExtraColumnsChange,
  summary,
  countsUnavailable,
  refresh,
  setRefresh,
  busy,
  updatedAt,
  onRefresh,
}: {
  extraColumns: ReadonlySet<RunExtraColumn>;
  onExtraColumnsChange: (columns: ReadonlySet<RunExtraColumn>) => void;
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
    <div className="space-y-3 border-b px-4 pb-3">
      <div
        className="flex flex-wrap items-center gap-1 border-b"
        aria-label="Run status filters"
      >
        {runStatusFilters.map((item) => (
          <button
            key={item.label}
            type="button"
            aria-pressed={current === item.label}
            onClick={() => onChange({ ...filters, status: item.status })}
            className={`border-b-2 px-3 py-2.5 text-sm ${current === item.label ? 'border-primary font-medium text-primary-text' : 'border-transparent text-muted-foreground hover:text-foreground'}`}
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
        <Popover>
          <PopoverTrigger asChild>
            <Button variant="secondary" bordered aria-label="Time range">
              <CalendarDays className="size-4" />
              {range === 'all'
                ? 'All time'
                : range === 'custom'
                  ? 'Custom dates'
                  : `${filters.dateBasis === 'completed' ? 'Completed' : 'Started'} · ${range === '24h' ? 'Last 24 hours' : 'Last 7 days'}`}
              <ChevronDown className="size-4 text-muted-foreground" />
            </Button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-72 space-y-3">
            <p className="text-sm font-medium">Time range</p>
            <label className="flex items-center justify-between gap-3 text-xs">
              Based on
              <RunSelect
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
              </RunSelect>
            </label>
            <label className="flex items-center justify-between gap-3 text-xs">
              Period
              <RunSelect
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
              </RunSelect>
            </label>
            {range === 'custom' && (
              <p className="text-xs text-muted-foreground">
                Set custom dates in Filters. Both started and completed bounds
                apply when supplied.
              </p>
            )}
          </PopoverContent>
        </Popover>
        <div className="hidden lg:block">
          <Popover>
            <PopoverTrigger asChild>
              <Button variant="secondary" bordered>
                <Columns3 className="size-4" />
                Columns
              </Button>
            </PopoverTrigger>
            <PopoverContent align="start" className="w-48 space-y-3">
              <p className="text-xs text-muted-foreground">
                Additional columns
              </p>
              {(
                [
                  ['completedAt', 'Completed'],
                  ['parentInstanceId', 'Parent'],
                  ['version', 'Version'],
                ] as const
              ).map(([key, label]) => (
                <label key={key} className="flex items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={extraColumns.has(key)}
                    onChange={(e) => {
                      const next = new Set(extraColumns);
                      if (e.target.checked) next.add(key);
                      else next.delete(key);
                      onExtraColumnsChange(next);
                      if (!e.target.checked && filters.sortBy === key)
                        onChange({
                          ...filters,
                          sortBy: 'createdAt',
                          sortOrder: 'desc',
                        });
                    }}
                  />
                  {label}
                </label>
              ))}
            </PopoverContent>
          </Popover>
        </div>
        <div className="lg:hidden">
          <RunSelect
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
          </RunSelect>
        </div>
        <div className="flex flex-wrap items-center gap-3 lg:ml-auto">
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
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
      </div>
    </div>
  );
}

function RunSelect(props: ComponentProps<'select'>) {
  return (
    <span className="relative inline-flex min-w-0 max-w-full">
      <select
        {...props}
        className="h-8 w-full min-w-0 appearance-none rounded-md border bg-background pl-3 pr-9 text-sm"
      />
      <ChevronDown
        aria-hidden="true"
        className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
      />
    </span>
  );
}
