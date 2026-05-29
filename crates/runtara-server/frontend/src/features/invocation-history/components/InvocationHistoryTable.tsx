import { useMemo, useCallback, useEffect } from 'react';
import { SortingState } from '@tanstack/react-table';
import { DataTable } from '@/shared/components/table';
import { useTableQuery } from '@/shared/hooks/api';
import { usePagination } from '@/shared/hooks/usePagination';
import { queryKeys } from '@/shared/queries/query-keys';
import { getAllExecutions } from '../queries';
import { ExecutionHistoryFilters } from '../types';
import { invocationHistoryColumns } from './InvocationHistoryColumns';

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

  // Convert filters to table sorting state
  const sorting = useMemo<SortingState>(() => {
    if (!filters.sortBy) return [];
    return [{ id: filters.sortBy, desc: filters.sortOrder === 'desc' }];
  }, [filters.sortBy, filters.sortOrder]);

  const { data, totalPages, isFetching } = useTableQuery({
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

  return (
    <div className="rounded-lg border shadow-sm overflow-hidden">
      <DataTable
        columns={invocationHistoryColumns}
        data={data}
        pagination={{
          ...pagination,
          onPageChange: (page) =>
            setPagination((prev) => ({ ...prev, pageIndex: page })),
          onPageSizeChange: (size) =>
            setPagination({ pageIndex: 0, pageSize: size }),
        }}
        totalPages={totalPages}
        setPagination={setPagination}
        isFetching={isFetching}
        sorting={sorting}
        onSortingChange={handleSortingChange}
        manualSorting
        getRowClassName={() => 'group'}
      />
    </div>
  );
}
