import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { InvocationHistoryTable } from '../../components/InvocationHistoryTable';
import { ExecutionHistoryFilters } from '../../types';

export function InvocationHistory() {
  usePageTitle('Runs');

  const [searchParams, setSearchParams] = useSearchParams();
  const [filters, setFilters] = useState<ExecutionHistoryFilters>(() => ({
    sortBy: 'createdAt',
    sortOrder: 'desc',
    workflowId: searchParams.get('workflowId') || undefined,
    status: searchParams.get('status') || undefined,
    search: searchParams.get('search') || undefined,
    runLabel: searchParams.get('runLabel') || undefined,
    parentInstanceId: searchParams.get('parentInstanceId') || undefined,
  }));

  useEffect(() => {
    setFilters((previous) => {
      const fromUrl = {
        workflowId: searchParams.get('workflowId') || undefined,
        status: searchParams.get('status') || undefined,
        search: searchParams.get('search') || undefined,
        runLabel: searchParams.get('runLabel') || undefined,
        parentInstanceId: searchParams.get('parentInstanceId') || undefined,
      };
      return (Object.keys(fromUrl) as (keyof typeof fromUrl)[]).every(
        (key) => previous[key] === fromUrl[key]
      )
        ? previous
        : { ...previous, ...fromUrl };
    });
  }, [searchParams]);

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
    for (const key of ['search', 'runLabel', 'parentInstanceId'] as const) {
      if (newFilters[key]) params.set(key, newFilters[key]);
      else params.delete(key);
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
