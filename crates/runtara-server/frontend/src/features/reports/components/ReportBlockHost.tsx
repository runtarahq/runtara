import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router';
import { Compass, RefreshCw } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Button } from '@/shared/components/ui/button';
import {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportInteractionOptions,
  ReportOrderBy,
} from '../types';
import { useReportBlockData } from '../hooks/useReports';
import { getFilterDefaultValue } from '../utils';
import { ChartBlock } from './blocks/ChartBlock';
import { MetricBlock } from './blocks/MetricBlock';
import { TableBlock } from './blocks/TableBlock';
import { ActionsBlock } from './blocks/ActionsBlock';
import { CardBlock } from './blocks/CardBlock';
import { ReportFilterBar } from './ReportFilterBar';
import { encodeFilterValue } from '../utils';

type ReportBlockHostProps = {
  reportId: string;
  block: ReportBlockDefinition;
  initialResult?: ReportBlockResult;
  filters: Record<string, unknown>;
  className?: string;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (
    updates: Record<string, unknown>,
    options?: ReportInteractionOptions
  ) => void;
  onNavigateView?: (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => void;
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
  onNavigateView,
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
    () =>
      (block.table?.columns ?? [])
        .filter(
          (column) =>
            column.type !== 'workflow_button' && !column.workflowAction
        )
        .flatMap((column) =>
          column.displayField
            ? [column.field, column.displayField]
            : [column.field]
        ),
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
    const clearFilters = new Set<string>();
    let nextViewId: string | null | undefined;
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
        if (action.type === 'set_filter' && action.filterId) {
          const value =
            action.valueFrom !== undefined
              ? resolveInteractionValue(action.valueFrom, datum)
              : action.value;
          if (value !== undefined) {
            updates[action.filterId] = value;
            handled = true;
          }
          continue;
        }

        if (action.type === 'clear_filter' && action.filterId) {
          clearFilters.add(action.filterId);
          handled = true;
          continue;
        }

        if (action.type === 'clear_filters') {
          for (const filterId of action.filterIds ?? []) {
            clearFilters.add(filterId);
          }
          handled = true;
          continue;
        }

        if (action.type === 'navigate_view' && action.viewId) {
          nextViewId = action.viewId;
          handled = true;
        }
      }
    }
    if (handled) {
      const options: ReportInteractionOptions = {
        clearFilters: Array.from(clearFilters),
        viewId: nextViewId,
        replace: nextViewId ? false : undefined,
      };
      if (onFiltersChange) {
        onFiltersChange(updates, options);
      } else {
        for (const [filterId, value] of Object.entries(updates)) {
          onFilterChange?.(filterId, value);
        }
        if (nextViewId) {
          onNavigateView?.(nextViewId, {
            clearFilters: Array.from(clearFilters),
            replace: false,
          });
        }
      }
    }
    return handled;
  };

  if (block.hideWhenEmpty && result?.status === 'empty') {
    return null;
  }

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
          reportId={reportId}
          block={block}
          result={result}
          sort={sort}
          filters={filters}
          blockFilters={blockFilters}
          onPageChange={(offset, size) => setPage({ offset, size })}
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
  sort,
  filters,
  blockFilters,
  onPageChange,
  onSortChange,
  onBlockInteraction,
  onRefresh,
}: {
  reportId: string;
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  sort: ReportOrderBy[];
  filters: Record<string, unknown>;
  blockFilters: Record<string, unknown>;
  onPageChange: (offset: number, size: number) => void;
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
        reportId={reportId}
        block={block}
        result={result}
        sort={sort}
        filters={filters}
        blockFilters={blockFilters}
        onPageChange={onPageChange}
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
        onRefresh={onRefresh}
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

  if (block.type === 'markdown') {
    return <MarkdownBlock block={block} result={result} />;
  }

  if (block.type === 'card') {
    return (
      <CardBlock
        reportId={reportId}
        block={block}
        result={result}
        filters={filters}
        blockFilters={blockFilters}
        onRefresh={onRefresh}
      />
    );
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

function MarkdownBlock({
  block,
  result,
}: {
  block: ReportBlockDefinition;
  result: ReportBlockResult;
}) {
  const data =
    result.data && typeof result.data === 'object'
      ? (result.data as Record<string, unknown>)
      : {};
  const content =
    typeof data.content === 'string'
      ? data.content
      : block.markdown?.content ?? '';
  const source =
    data.source && typeof data.source === 'object'
      ? (data.source as Record<string, unknown>)
      : undefined;
  const rendered = interpolateMarkdownSource(content, source);

  return (
    <div className="prose prose-slate max-w-none dark:prose-invert">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{rendered}</ReactMarkdown>
    </div>
  );
}

const MARKDOWN_INTERPOLATION_RE = /\{\{\s*([^{}]+?)\s*\}\}/g;
const SOURCE_INTERPOLATION_RE = /^source(?:\[(\d+)])?\.([A-Za-z0-9_.-]+)$/;

function interpolateMarkdownSource(
  content: string,
  source: Record<string, unknown> | undefined
) {
  return content.replace(MARKDOWN_INTERPOLATION_RE, (token, expression) => {
    const match = SOURCE_INTERPOLATION_RE.exec(String(expression).trim());
    if (!match) return token;
    const rowIndex = match[1] ? Number.parseInt(match[1], 10) : 0;
    const fieldPath = match[2];
    const value = resolveMarkdownSourceValue(source, rowIndex, fieldPath);
    return escapeMarkdownValue(value);
  });
}

function resolveMarkdownSourceValue(
  source: Record<string, unknown> | undefined,
  rowIndex: number,
  fieldPath: string
) {
  const rows = Array.isArray(source?.rows) ? source.rows : [];
  const row = rows[rowIndex];
  if (row == null) return undefined;

  if (Array.isArray(row)) {
    const columnKeys = markdownSourceColumnKeys(source?.columns);
    const index = columnKeys.indexOf(fieldPath);
    return index >= 0 ? row[index] : undefined;
  }

  if (typeof row !== 'object') return undefined;
  return resolveObjectPath(row as Record<string, unknown>, fieldPath);
}

function markdownSourceColumnKeys(columns: unknown): string[] {
  if (!Array.isArray(columns)) return [];
  return columns.flatMap((column) => {
    if (typeof column === 'string') return [column];
    if (column && typeof column === 'object') {
      const key = (column as Record<string, unknown>).key;
      return typeof key === 'string' ? [key] : [];
    }
    return [];
  });
}

function resolveObjectPath(row: Record<string, unknown>, fieldPath: string) {
  if (Object.prototype.hasOwnProperty.call(row, fieldPath)) {
    return row[fieldPath];
  }
  return fieldPath.split('.').reduce<unknown>((current, part) => {
    if (current && typeof current === 'object') {
      const object = current as Record<string, unknown>;
      if (Object.prototype.hasOwnProperty.call(object, part)) {
        return object[part];
      }
    }
    return undefined;
  }, row);
}

function escapeMarkdownValue(value: unknown) {
  if (value == null) return '';
  if (typeof value === 'object') {
    return escapeMarkdownText(JSON.stringify(value));
  }
  return escapeMarkdownText(String(value));
}

function escapeMarkdownText(value: string) {
  return value.replace(/([\\`*_{}[\]()#+\-.!|>])/g, '\\$1');
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
  const path = source.startsWith('datum.')
    ? source.slice('datum.'.length)
    : source;
  return path.split('.').reduce<unknown>((current, part) => {
    if (current && typeof current === 'object' && part in current) {
      return (current as Record<string, unknown>)[part];
    }
    return undefined;
  }, datum);
}
