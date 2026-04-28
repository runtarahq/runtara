import { useEffect, useMemo, useRef, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportOrderBy,
} from '../types';
import { useReportBlockData } from '../hooks/useReports';
import { getFilterDefaultValue } from '../utils';
import { ChartBlock } from './blocks/ChartBlock';
import { MetricBlock } from './blocks/MetricBlock';
import { TableBlock } from './blocks/TableBlock';
import { ReportFilterBar } from './ReportFilterBar';

type ReportBlockHostProps = {
  reportId: string;
  block: ReportBlockDefinition;
  initialResult?: ReportBlockResult;
  filters: Record<string, unknown>;
};

export function ReportBlockHost({
  reportId,
  block,
  initialResult,
  filters,
}: ReportBlockHostProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [isVisible, setIsVisible] = useState(!block.lazy);
  const defaultPageSize = getDefaultPageSize(block);
  const defaultSort = useMemo(
    () => block.table?.defaultSort ?? [],
    [block.table?.defaultSort]
  );
  const [page, setPage] = useState({ offset: 0, size: defaultPageSize });
  const [sort, setSort] = useState<ReportOrderBy[]>(defaultSort);
  const [search, setSearch] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');
  const initialBlockFilters = useMemo(
    () =>
      Object.fromEntries(
        (block.filters ?? []).map((filter) => [
          filter.id,
          getFilterDefaultValue(filter),
        ])
      ),
    [block.filters]
  );
  const [blockFilters, setBlockFilters] =
    useState<Record<string, unknown>>(initialBlockFilters);

  useEffect(() => {
    if (!block.lazy || isVisible) return;
    const element = rootRef.current;
    if (!element) return;

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setIsVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: '200px' }
    );

    observer.observe(element);
    return () => observer.disconnect();
  }, [block.lazy, isVisible]);

  useEffect(() => {
    setPage({ offset: 0, size: defaultPageSize });
    setBlockFilters(initialBlockFilters);
    setSort(defaultSort);
    setSearch('');
    setDebouncedSearch('');
  }, [block.id, defaultPageSize, defaultSort, initialBlockFilters]);

  useEffect(() => {
    setPage({ offset: 0, size: defaultPageSize });
    setBlockFilters(initialBlockFilters);
  }, [defaultPageSize, filters, initialBlockFilters]);

  useEffect(() => {
    const timeout = window.setTimeout(() => setDebouncedSearch(search), 250);
    return () => window.clearTimeout(timeout);
  }, [search]);

  const hasBlockFilters = (block.filters?.length ?? 0) > 0;
  const searchableFields = useMemo(
    () => (block.table?.columns ?? []).map((column) => column.field),
    [block.table?.columns]
  );
  const hasInteractiveTableState =
    block.type === 'table' &&
    (page.offset > 0 ||
      page.size !== defaultPageSize ||
      debouncedSearch.trim().length > 0 ||
      !areSortsEqual(sort, defaultSort));
  const needsBlockFetch =
    isVisible &&
    (block.lazy ||
      hasBlockFilters ||
      !initialResult ||
      hasInteractiveTableState);

  const request = useMemo(
    () => ({
      filters,
      page: block.type === 'table' ? page : undefined,
      sort,
      search:
        block.type === 'table' && debouncedSearch.trim().length > 0
          ? { query: debouncedSearch.trim(), fields: searchableFields }
          : undefined,
      blockFilters,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    }),
    [
      block.type,
      blockFilters,
      debouncedSearch,
      filters,
      page,
      searchableFields,
      sort,
    ]
  );

  const {
    data: fetchedResult,
    isFetching,
    refetch,
  } = useReportBlockData(reportId, block.id, request, needsBlockFetch);

  const result = fetchedResult ?? initialResult;

  return (
    <div ref={rootRef} className="my-5">
      {block.title && (
        <div className="mb-2 flex items-center justify-between gap-3">
          <h2 className="text-base font-semibold text-foreground">
            {block.title}
          </h2>
          {result?.status === 'error' && (
            <Button variant="outline" size="sm" onClick={() => refetch()}>
              <RefreshCw className="mr-2 h-4 w-4" />
              Retry
            </Button>
          )}
        </div>
      )}
      {(block.filters?.length ?? 0) > 0 && (
        <div className="mb-3 rounded-lg border bg-muted/20 p-3">
          <ReportFilterBar
            definition={{
              definitionVersion: 1,
              markdown: '',
              filters: block.filters ?? [],
              blocks: [block],
            }}
            values={blockFilters}
            onChange={(filterId, value) => {
              setBlockFilters((current) => ({
                ...current,
                [filterId]: value,
              }));
              setPage((current) => ({ ...current, offset: 0 }));
            }}
          />
        </div>
      )}
      {!isVisible || isFetching ? (
        <BlockSkeleton block={block} />
      ) : !result ? (
        <BlockSkeleton block={block} />
      ) : result.status === 'error' ? (
        <BlockError result={result} onRetry={() => refetch()} />
      ) : (
        <RenderedBlock
          block={block}
          result={result}
          search={search}
          sort={sort}
          onPageChange={(offset, size) => setPage({ offset, size })}
          onSearchChange={(nextSearch) => {
            setSearch(nextSearch);
            setPage((current) => ({ ...current, offset: 0 }));
          }}
          onSortChange={(nextSort) => {
            setSort(nextSort);
            setPage((current) => ({ ...current, offset: 0 }));
          }}
        />
      )}
    </div>
  );
}

function RenderedBlock({
  block,
  result,
  search,
  sort,
  onPageChange,
  onSearchChange,
  onSortChange,
}: {
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  search: string;
  sort: ReportOrderBy[];
  onPageChange: (offset: number, size: number) => void;
  onSearchChange: (search: string) => void;
  onSortChange: (sort: ReportOrderBy[]) => void;
}) {
  if (block.type === 'table') {
    return (
      <TableBlock
        block={block}
        result={result}
        search={search}
        sort={sort}
        onPageChange={onPageChange}
        onSearchChange={onSearchChange}
        onSortChange={onSortChange}
      />
    );
  }

  if (block.type === 'chart') {
    return <ChartBlock block={block} result={result} />;
  }

  if (block.type === 'metric') {
    return <MetricBlock block={block} result={result} />;
  }

  return null;
}

function BlockSkeleton({ block }: { block: ReportBlockDefinition }) {
  const height = block.type === 'metric' ? 'h-28' : 'h-72';
  return (
    <div
      className={`${height} animate-pulse rounded-lg border bg-muted/30`}
      aria-label="Loading report block"
    />
  );
}

function BlockError({
  result,
  onRetry,
}: {
  result: ReportBlockResult;
  onRetry: () => void;
}) {
  return (
    <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-4">
      <p className="text-sm font-semibold text-destructive">
        {result.error?.message ?? 'This report block could not be rendered.'}
      </p>
      <Button className="mt-3" variant="outline" size="sm" onClick={onRetry}>
        <RefreshCw className="mr-2 h-4 w-4" />
        Retry
      </Button>
    </div>
  );
}

function getDefaultPageSize(block: ReportBlockDefinition) {
  return block.table?.pagination?.defaultPageSize ?? 50;
}

function areSortsEqual(left: ReportOrderBy[], right: ReportOrderBy[]) {
  if (left.length !== right.length) return false;
  return left.every(
    (entry, index) =>
      entry.field === right[index]?.field &&
      normalizeSortDirection(entry.direction) ===
        normalizeSortDirection(right[index]?.direction)
  );
}

function normalizeSortDirection(direction: ReportOrderBy['direction']) {
  return direction?.toLowerCase() === 'desc' ? 'desc' : 'asc';
}
