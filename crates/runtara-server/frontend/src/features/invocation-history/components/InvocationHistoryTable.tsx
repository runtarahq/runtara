import { useMemo, useCallback, useEffect, useState } from 'react';
import { SortingState } from '@tanstack/react-table';
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
import { useTableQuery } from '@/shared/hooks/api';
import { usePagination } from '@/shared/hooks/usePagination';
import { queryKeys } from '@/shared/queries/query-keys';
import { getAllExecutions } from '../queries';
import { ExecutionHistoryFilters } from '../types';
import { invocationHistoryColumns } from './InvocationHistoryColumns';
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
    if (!filters.sortBy) return [];
    return [{ id: filters.sortBy, desc: filters.sortOrder === 'desc' }];
  }, [filters.sortBy, filters.sortOrder]);

  const { data, totalPages, totalElements, isFetching } = useTableQuery({
    queryKey: queryKeys.executions.list({
      pageIndex: pagination.pageIndex,
      pageSize: pagination.pageSize,
      filters,
    }),
    queryFn: getAllExecutions,
    staleTime: 0,
  });

  // Reset to the first page whenever the active filters change
  useEffect(() => {
    setPagination((prev) => ({ ...prev, pageIndex: 0 }));
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

  const footerLeft = `${totalElements} executions · ${data.length} on this page`;

  const handlePageChange = (page: number) =>
    setPagination((prev) => ({ ...prev, pageIndex: page }));
  const handlePageSizeChange = (size: number) =>
    setPagination({ pageIndex: 0, pageSize: size });

  const activeFilterCount = countActiveInvocationFilters(filters);
  const handleClearFilters = () =>
    onFiltersChange({
      ...filters,
      runLabel: undefined,
      workflowId: undefined,
      status: undefined,
      createdFrom: undefined,
      createdTo: undefined,
      completedFrom: undefined,
      completedTo: undefined,
    });

  return (
    <ConsoleTableShell
      toolbar={
        <ConsoleToolbar
          left={<Breadcrumb items={[{ label: 'Invocation History' }]} />}
          search={
            <ToolbarSearch
              value={search}
              onChange={setSearch}
              placeholder="Search executions…"
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
      <DataTable
        columns={invocationHistoryColumns}
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
        getRowClassName={() => 'group'}
      />
    </ConsoleTableShell>
  );
}
