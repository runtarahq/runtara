import { formatRunDuration } from '../utils/run-duration';
import { useMemo, useCallback, useEffect, useState } from 'react';
import { SortingState, Row } from '@tanstack/react-table';
import { DataTable } from '@/shared/components/table';
import {
  Breadcrumb,
  ConsoleTableShell,
  ConsoleToolbar,
  FilterPopover,
  TablePagination,
  TableStatusFooter,
  ToolbarSearch,
} from '@/shared/components/console';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { usePagination } from '@/shared/hooks/usePagination';
import { queryKeys } from '@/shared/queries/query-keys';
import { getAllExecutions } from '../queries';
import { queryRunSummary } from '@/features/operations/queries';
import { resolveRunFilters, summaryFilters } from '../utils/run-filters';
import { RunControls } from './RunControls';
import {
  operationsRunColumns,
  type RunExtraColumn,
} from './OperationsRunColumns';
import {
  RunIdentity,
  RunContext,
  RunActions,
  RunDetails,
  RunDetailsToggle,
  RunTime,
} from './RunRow';
import { ReplayButton } from '@/features/operations/pages/shared';
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from '@/shared/components/ui/dialog';
import { ExecutionHistoryFilters, ExecutionHistoryItem } from '../types';
import {
  InvocationHistoryFilters,
  countActiveInvocationFilters,
} from './InvocationHistoryFilters';

// Map column IDs to API sort field names
// Note: Backend only supports sorting by createdAt and completedAt
const SORT_FIELD_MAP: Record<string, ExecutionHistoryFilters['sortBy']> = {
  createdAt: 'createdAt',
  completedAt: 'completedAt',
};

interface InvocationHistoryTableProps {
  filters: ExecutionHistoryFilters;
  onFiltersChange: (filters: ExecutionHistoryFilters) => void;
}

export function InvocationHistoryTable({
  filters,
  onFiltersChange,
}: InvocationHistoryTableProps) {
  const tenant = useAuthStore((s) => s.orgId);
  const [refresh, setRefresh] = useState(true);
  const [replayRun, setReplayRun] = useState<ExecutionHistoryItem | null>(null);
  const [expandedDetails, setExpandedDetails] = useState<ReadonlySet<string>>(
    new Set()
  );
  const onDetailsChange = useCallback((id: string, open: boolean) => {
    setExpandedDetails((previous) => {
      if (previous.has(id) === open) return previous;
      const next = new Set(previous);
      if (open) next.add(id);
      else next.delete(id);
      return next;
    });
  }, []);
  const [extraColumns, setExtraColumns] = useState<ReadonlySet<RunExtraColumn>>(
    new Set()
  );
  // Show the active completion sort when arriving from Overview or a saved URL.
  const visibleExtraColumns = useMemo(() => {
    const next = new Set(extraColumns);
    if (filters.sortBy === 'completedAt') next.add('completedAt');
    return next;
  }, [extraColumns, filters.sortBy]);
  const expanded = useMemo(
    () => Object.fromEntries([...expandedDetails].map((id) => [id, true])),
    [expandedDetails]
  );
  const columns = useMemo(
    () =>
      operationsRunColumns(
        setReplayRun,
        expandedDetails,
        onDetailsChange,
        visibleExtraColumns
      ),
    [expandedDetails, onDetailsChange, visibleExtraColumns]
  );
  const { pagination, setPagination } = usePagination();
  const [search, setSearch] = useState(filters.search ?? '');
  useEffect(() => {
    setSearch(filters.search ?? '');
  }, [filters.search]);
  useEffect(() => {
    const timer = setTimeout(() => {
      const nextSearch = search.trim() || undefined;
      if (nextSearch !== filters.search) {
        setPagination((prev) => ({ ...prev, pageIndex: 0 }));
        onFiltersChange({ ...filters, search: nextSearch });
      }
    }, 300);
    return () => clearTimeout(timer);
  }, [search, filters, onFiltersChange, setPagination]);

  // Convert filters to table sorting state
  const sorting = useMemo<SortingState>(() => {
    if (!filters.sortBy) return [{ id: 'createdAt', desc: true }];
    return [{ id: filters.sortBy, desc: filters.sortOrder === 'desc' }];
  }, [filters.sortBy, filters.sortOrder]);

  const query = useCustomQuery({
    queryKey: [
      ...queryKeys.executions.lists(),
      tenant,
      { ...pagination, filters },
    ],
    queryFn: async (token: string) => {
      const resolved = resolveRunFilters(filters, Date.now());
      const [page, summary] = await Promise.allSettled([
        getAllExecutions(token, {
          queryKey: [
            {
              pageIndex: pagination.pageIndex,
              pageSize: pagination.pageSize,
              filters: resolved,
            },
          ],
        }),
        queryRunSummary(token, summaryFilters(resolved)),
      ]);
      if (page.status === 'rejected') throw page.reason;
      return {
        ...page.value,
        summary: summary.status === 'fulfilled' ? summary.value : undefined,
        countsUnavailable: summary.status === 'rejected',
      };
    },
    refetchInterval:
      refresh && !replayRun && expandedDetails.size === 0 ? 30_000 : false,
    refetchIntervalInBackground: false,
    placeholderData: undefined,
    staleTime: 0,
    retry: false,
  });
  const data = query.data?.content ?? [];
  const totalPages = query.data?.totalPages ?? 0;
  const totalElements = query.data?.totalElements ?? 0;
  const isFetching = query.isFetching;

  // Reset to the first page whenever the active filters change
  useEffect(() => {
    setPagination((prev) => ({ ...prev, pageIndex: 0 }));
    setExpandedDetails(new Set());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filters]);

  const handleSortingChange = useCallback(
    (updater: SortingState | ((old: SortingState) => SortingState)) => {
      const currentSorting: SortingState = filters.sortBy
        ? [{ id: filters.sortBy, desc: filters.sortOrder === 'desc' }]
        : [];

      const newSorting =
        typeof updater === 'function' ? updater(currentSorting) : updater;

      if (newSorting.length === 0) {
        onFiltersChange({
          ...filters,
          sortBy: 'createdAt',
          sortOrder: 'desc',
        });
      } else {
        const { id, desc } = newSorting[0];
        const sortBy = SORT_FIELD_MAP[id] || 'createdAt';
        onFiltersChange({
          ...filters,
          sortBy,
          sortOrder: desc ? 'desc' : 'asc',
        });
      }
    },
    [filters, onFiltersChange]
  );

  const footerLeft = `${query.data ? totalElements : '—'} runs · ${data.length} on this page`;

  const handlePageChange = (page: number) => {
    setExpandedDetails(new Set());
    setPagination((prev) => ({ ...prev, pageIndex: page }));
  };
  const handlePageSizeChange = (size: number) => {
    setExpandedDetails(new Set());
    setPagination({ pageIndex: 0, pageSize: size });
  };

  const activeFilterCount = countActiveInvocationFilters(filters);
  const handleClearFilters = () =>
    onFiltersChange({
      ...filters,
      runLabel: undefined,
      parentInstanceId: undefined,
      workflowId: undefined,
      status: undefined,
      createdFrom: undefined,
      createdTo: undefined,
      completedFrom: undefined,
      completedTo: undefined,
      range: 'all',
    });

  return (
    <>
      <ConsoleTableShell
        toolbar={
          <>
            <ConsoleToolbar
              left={
                <Breadcrumb
                  items={[
                    { label: 'Operations', to: '/operations' },
                    { label: 'Runs' },
                  ]}
                />
              }
              search={
                <ToolbarSearch
                  value={search}
                  onChange={setSearch}
                  placeholder="Search runs…"
                  className="w-56"
                />
              }
              filter={
                <FilterPopover
                  activeCount={activeFilterCount}
                  onClear={handleClearFilters}
                >
                  <InvocationHistoryFilters
                    filters={filters}
                    onFiltersChange={onFiltersChange}
                  />
                </FilterPopover>
              }
            />
            <RunControls
              extraColumns={visibleExtraColumns}
              onExtraColumnsChange={setExtraColumns}
              filters={filters}
              onChange={onFiltersChange}
              summary={query.data?.summary}
              countsUnavailable={
                !!query.error || !!query.data?.countsUnavailable
              }
              refresh={refresh}
              setRefresh={setRefresh}
              busy={isFetching}
              updatedAt={query.dataUpdatedAt}
              onRefresh={() => {
                setExpandedDetails(new Set());
                void query.refetch();
              }}
            />
            {refresh && expandedDetails.size > 0 && (
              <p
                role="status"
                className="border-b px-4 py-2 text-xs text-muted-foreground"
              >
                Auto-refresh paused while run details are open.
              </p>
            )}
            {(query.error || query.data?.countsUnavailable) && (
              <p
                role="alert"
                className="border-b px-4 py-2 text-sm text-destructive"
              >
                {query.error
                  ? query.data
                    ? 'Could not refresh runs. Showing stale data.'
                    : 'Could not load runs.'
                  : 'Status counts unavailable.'}{' '}
                Try Refresh.
              </p>
            )}
          </>
        }
        footer={
          <TableStatusFooter
            left={footerLeft}
            right={
              <TablePagination
                pageIndex={pagination.pageIndex}
                pageSize={pagination.pageSize}
                pageCount={totalPages ?? 1}
                onPageChange={handlePageChange}
                onPageSizeChange={handlePageSizeChange}
              />
            }
          />
        }
      >
        <div className="hidden lg:block">
          <DataTable
            expanded={expanded}
            getRowCanExpand={() => true}
            SubComponent={RunDetailRow}
            columns={columns}
            data={data}
            pagination={{
              ...pagination,
              onPageChange: handlePageChange,
              onPageSizeChange: handlePageSizeChange,
            }}
            totalPages={totalPages}
            setPagination={setPagination}
            isFetching={isFetching}
            sorting={sorting}
            onSortingChange={handleSortingChange}
            manualSorting
            stickyHeader
            shouldRenderPagination={false}
            getRowId={(row) => row.instanceId}
            getRowClassName={() => 'group'}
          />
        </div>
        <div className="divide-y lg:hidden">
          {data.map((run) => (
            <article
              key={run.instanceId}
              className="space-y-3 p-4"
              aria-label={run.runLabel || run.instanceId}
            >
              <div className="flex items-center gap-2">
                <RunDetailsToggle
                  run={run}
                  open={expandedDetails.has(run.instanceId)}
                  onChange={onDetailsChange}
                />
                <RunIdentity run={run} />
              </div>
              <RunContext run={run} />
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
                  <span>
                    Started <RunTime value={run.createdAt} />
                  </span>
                  <span>
                    Duration {formatRunDuration(run.executionDurationSeconds)}
                  </span>
                </div>
                <RunActions run={run} onReplay={setReplayRun} />
              </div>
              {expandedDetails.has(run.instanceId) && (
                <div className="rounded-md bg-muted/30 p-3">
                  <RunDetails run={run} />
                </div>
              )}
            </article>
          ))}
          {!data.length && (
            <p className="p-4 text-sm text-muted-foreground">
              {isFetching
                ? 'Loading runs…'
                : query.error
                  ? 'Runs unavailable.'
                  : 'No runs match these filters.'}
            </p>
          )}
        </div>
      </ConsoleTableShell>
      <Dialog
        open={!!replayRun}
        onOpenChange={(open) => {
          if (!open) setReplayRun(null);
        }}
      >
        <DialogContent>
          <DialogTitle>
            Replay {replayRun?.runLabel || replayRun?.instanceId.slice(0, 8)}
          </DialogTitle>
          <DialogDescription>
            Start a new run with the original input.
          </DialogDescription>
          {replayRun && (
            <ReplayButton
              run={{
                id: replayRun.instanceId,
                workflowId: replayRun.workflowId,
                runLabel: replayRun.runLabel,
              }}
              initiallyConfirm
              onClose={() => setReplayRun(null)}
            />
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}

function RunDetailRow({ row }: { row: Row<ExecutionHistoryItem> }) {
  return <RunDetails run={row.original} />;
}
