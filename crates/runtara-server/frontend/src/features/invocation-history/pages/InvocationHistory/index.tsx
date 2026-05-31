import { useState } from 'react';
import { useSearchParams } from 'react-router';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { InvocationHistoryTable } from '../../components/InvocationHistoryTable';
import { ExecutionHistoryFilters } from '../../types';

export function InvocationHistory() {
  usePageTitle('Invocation History');

  const [searchParams, setSearchParams] = useSearchParams();
  const [filters, setFilters] = useState<ExecutionHistoryFilters>(() => ({
    sortBy: 'createdAt',
    sortOrder: 'desc',
    workflowId: searchParams.get('workflowId') || undefined,
    status: searchParams.get('status') || undefined,
  }));

  const handleFiltersChange = (newFilters: ExecutionHistoryFilters) => {
    setFilters(newFilters);

    const params = new URLSearchParams(searchParams);
    if (newFilters.workflowId) {
      params.set('workflowId', newFilters.workflowId);
    } else {
      params.delete('workflowId');
    }
    if (newFilters.status) {
      params.set('status', newFilters.status);
    } else {
      params.delete('status');
    }
    setSearchParams(params, { replace: true });
  };

  return (
    <InvocationHistoryTable
      filters={filters}
      onFiltersChange={handleFiltersChange}
    />
  );
}
