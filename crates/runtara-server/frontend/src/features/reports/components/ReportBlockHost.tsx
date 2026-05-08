import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router';
import { Compass, RefreshCw } from 'lucide-react';
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
import { ActionsBlock } from './blocks/ActionsBlock';
import { ReportFilterBar } from './ReportFilterBar';
import { encodeFilterValue } from '../utils';

type ReportBlockHostProps = {
  reportId: string;
  block: ReportBlockDefinition;
  initialResult?: ReportBlockResult;
  filters: Record<string, unknown>;
  className?: string;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (updates: Record<string, unknown>) => void;
  onReportRefresh?: () => void | Promise<unknown>;
};

export function ReportBlockHost({
  reportId,
  block,
  initialResult,
  filters,
  className = 'my-5',
  onFilterChange,
  onFiltersChange,
  onReportRefresh,
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
    (block.type === 'actions' ||
      block.lazy ||
      hasBlockFilters ||
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
  const explorePath = useMemo(
    () => buildExplorePath(reportId, block.id, filters),
    [block.id, filters, reportId]
  );

  const result = needsBlockFetch
    ? (fetchedResult ?? initialResult)
    : initialResult;
  const refreshAfterActionSubmit = () => {
    void refetch();
    void onReportRefresh?.();

    window.setTimeout(() => {
      void refetch();
      void onReportRefresh?.();
    }, 1250);
  };
  const runInteraction = (
    event: string,
    datum: Record<string, unknown>
  ): boolean => {
    let handled = false;
    const updates: Record<string, unknown> = {};
    for (const interaction of block.interactions ?? []) {
      if (interaction.trigger.event !== event) continue;
      const triggerField = interaction.trigger.field;
      if (triggerField) {
        if (datum.field !== undefined) {
          if (datum.field !== triggerField) continue;
        } else if (!(triggerField in datum)) {
          continue;
        }
      }
      for (const action of interaction.actions) {
        if (action.type !== 'set_filter' || !action.filterId) continue;
        const value =
          action.valueFrom !== undefined
            ? resolveInteractionValue(action.valueFrom, datum)
            : action.value;
        if (value !== undefined) {
          updates[action.filterId] = value;
          handled = true;
        }
      }
    }
    if (handled) {
      if (onFiltersChange) {
        onFiltersChange(updates);
      } else {
        for (const [filterId, value] of Object.entries(updates)) {
          onFilterChange?.(filterId, value);
        }
      }
    }
    return handled;
  };

  return (
    <div ref={rootRef} className={className}>
      {(block.title || block.dataset) && (
        <div className="mb-2 flex items-center justify-between gap-3">
          {block.title ? (
            <h2 className="text-base font-semibold text-foreground">
              {block.title}
            </h2>
          ) : (
            <span />
          )}
          <div className="flex items-center gap-2">
            {block.dataset && (
              <Link to={explorePath}>
                <Button variant="outline" size="sm">
                  <Compass className="mr-2 h-4 w-4" />
                  Explore this
                </Button>
              </Link>
            )}
            {result?.status === 'error' && (
              <Button variant="outline" size="sm" onClick={() => refetch()}>
                <RefreshCw className="mr-2 h-4 w-4" />
                Retry
              </Button>
            )}
          </div>
        </div>
      )}
      {(block.filters?.length ?? 0) > 0 && (
        <div className="mb-3 rounded-lg border bg-muted/20 p-3">
          <ReportFilterBar
            reportId={reportId}
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
            showChips={false}
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
          reportId={reportId}
          block={block}
          result={result}
          search={search}
          sort={sort}
          filters={filters}
          blockFilters={blockFilters}
          onPageChange={(offset, size) => setPage({ offset, size })}
          onSearchChange={(nextSearch) => {
            setSearch(nextSearch);
            setPage((current) => ({ ...current, offset: 0 }));
          }}
          onSortChange={(nextSort) => {
            setSort(nextSort);
            setPage((current) => ({ ...current, offset: 0 }));
          }}
          onBlockInteraction={runInteraction}
          onRefresh={refreshAfterActionSubmit}
        />
      )}
    </div>
  );
}

function RenderedBlock({
  reportId,
  block,
  result,
  search,
  sort,
  filters,
  blockFilters,
  onPageChange,
  onSearchChange,
  onSortChange,
  onBlockInteraction,
  onRefresh,
}: {
  reportId: string;
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  search: string;
  sort: ReportOrderBy[];
  filters: Record<string, unknown>;
  blockFilters: Record<string, unknown>;
  onPageChange: (offset: number, size: number) => void;
  onSearchChange: (search: string) => void;
  onSortChange: (sort: ReportOrderBy[]) => void;
  onBlockInteraction: (
    event: string,
    datum: Record<string, unknown>
  ) => boolean;
  onRefresh: () => void | Promise<void>;
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
        onRowClick={
          hasBlockInteraction(block, 'row_click')
            ? (row) => onBlockInteraction('row_click', row)
            : undefined
        }
        onCellClick={
          hasBlockInteraction(block, 'cell_click')
            ? (cell) => onBlockInteraction('cell_click', cell)
            : undefined
        }
      />
    );
  }

  if (block.type === 'chart') {
    return (
      <ChartBlock
        block={block}
        result={result}
        onPointClick={
          hasBlockInteraction(block, 'point_click')
            ? (datum) => onBlockInteraction('point_click', datum)
            : undefined
        }
      />
    );
  }

  if (block.type === 'metric') {
    return <MetricBlock block={block} result={result} />;
  }

  if (block.type === 'actions') {
    return (
      <ActionsBlock
        reportId={reportId}
        block={block}
        result={result}
        filters={filters}
        blockFilters={blockFilters}
        onSubmitted={onRefresh}
      />
    );
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

function buildExplorePath(
  reportId: string,
  blockId: string,
  filters: Record<string, unknown>
) {
  const params = new URLSearchParams();
  params.set('block', blockId);
  for (const [filterId, value] of Object.entries(filters)) {
    if (!isEmptyFilterValue(value)) {
      params.set(filterId, encodeFilterValue(value));
    }
  }
  return `/reports/${reportId}/explore?${params.toString()}`;
}

function isEmptyFilterValue(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'string') return value.trim().length === 0;
  if (Array.isArray(value)) return value.length === 0;
  return false;
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

function hasBlockInteraction(block: ReportBlockDefinition, event: string) {
  return (block.interactions ?? []).some(
    (interaction) => interaction.trigger.event === event
  );
}

function normalizeSortDirection(direction: ReportOrderBy['direction']) {
  return direction?.toLowerCase() === 'desc' ? 'desc' : 'asc';
}

function resolveInteractionValue(
  source: string,
  datum: Record<string, unknown>
): unknown {
  const path = source.startsWith('datum.') ? source.slice('datum.'.length) : source;
  return path.split('.').reduce<unknown>((current, part) => {
    if (current && typeof current === 'object' && part in current) {
      return (current as Record<string, unknown>)[part];
    }
    return undefined;
  }, datum);
}
