import { useMemo } from 'react';
import { useSearchParams } from 'react-router';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { InvocationHistoryTable } from '../../components/InvocationHistoryTable';
import { readRunFilters, writeRunFilters } from '../../utils/run-filters';

export function InvocationHistory() {
  usePageTitle('Runs');
  const [searchParams, setSearchParams] = useSearchParams();
  const filters = useMemo(() => readRunFilters(searchParams), [searchParams]);
  return (
    <InvocationHistoryTable
      filters={filters}
      onFiltersChange={(next) =>
        setSearchParams(writeRunFilters(next, searchParams))
      }
    />
  );
}
