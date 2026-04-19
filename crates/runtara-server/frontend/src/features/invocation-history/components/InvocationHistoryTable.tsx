import { useState, useMemo, useCallback } from 'react';
import { useSearchParams } from 'react-router';
import { SortingState } from '@tanstack/react-table';
import { DataTable } from '@/shared/components/table';
import { useTableQuery } from '@/shared/hooks/api';
import { usePagination } from '@/shared/hooks/usePagination';
import { queryKeys } from '@/shared/queries/query-keys';
import { getAllExecutions } from '../queries';
import { ExecutionHistoryFilters } from '../types';
import { invocationHistoryColumns } from './InvocationHistoryColumns';
import { InvocationHistoryFilters } from './InvocationHistoryFilters';

// Map column IDs to API sort field names
// Note: Backend only supports sorting by createdAt and completedAt
const SORT_FIELD_MAP: Record<string, ExecutionHistoryFilters['sortBy']> = {
  createdAt: 'createdAt',
  completedAt: 'completedAt',
};

export function InvocationHistoryTable() {
  const [searchParams, setSearchParams] = useSearchParams();
  const { pagination, setPagination } = usePagination();

  // Initialize filters from URL params
  const [filters, setFilters] = useState<ExecutionHistoryFilters>(() => ({
    sortBy: 'createdAt',
    sortOrder: 'desc',
    workflowId: searchParams.get('workflowId') || undefined,
    status: searchParams.get('status') || undefined,
  }));

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

  const handleFiltersChange = (newFilters: ExecutionHistoryFilters) => {
    setFilters(newFilters);
    // Reset to first page when filters change
    setPagination((prev) => ({ ...prev, pageIndex: 0 }));

    // Update URL params for workflowId and status
    const newParams = new URLSearchParams(searchParams);
    if (newFilters.workflowId) {
      newParams.set('workflowId', newFilters.workflowId);
    } else {
      newParams.delete('workflowId');
    }
    if (newFilters.status) {
      newParams.set('status', newFilters.status);
    } else {
      newParams.delete('status');
    }
    setSearchParams(newParams, { replace: true });
  };

  const handleSortingChange = useCallback(
    (updater: SortingState | ((old: SortingState) => SortingState)) => {
      setFilters((prev) => {
        const currentSorting: SortingState = prev.sortBy
          ? [{ id: prev.sortBy, desc: prev.sortOrder === 'desc' }]
          : [];

        const newSorting =
          typeof updater === 'function' ? updater(currentSorting) : updater;

        if (newSorting.length === 0) {
          return {
            ...prev,
            sortBy: 'createdAt',
            sortOrder: 'desc',
          };
        } else {
          const { id, desc } = newSorting[0];
          const sortBy = SORT_FIELD_MAP[id] || 'createdAt';
          return {
            ...prev,
            sortBy,
            sortOrder: desc ? 'desc' : 'asc',
          };
        }
      });
      setPagination((prev) => ({ ...prev, pageIndex: 0 }));
    },
    [setPagination]
  );

  return (
    <div>
      <InvocationHistoryFilters
        filters={filters}
        onFiltersChange={handleFiltersChange}
      />
      <div className="bg-white rounded-xl border border-slate-200/80 shadow-sm overflow-hidden dark:bg-card dark:border-slate-700/50">
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
    </div>
  );
}
