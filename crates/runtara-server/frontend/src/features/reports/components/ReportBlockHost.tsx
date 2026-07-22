import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router';
import { Compass, RefreshCw } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Button } from '@/shared/components/ui/button';
import {
  ReportBlockDefinition,
  ReportBlockRenderResult,
  ReportBlockResult,
  ReportInteractionAction,
  ReportInteractionOptions,
  ReportOrderBy,
  ReportWorkflowActionConfig,
} from '../types';
import { useReportBlockData } from '../hooks/useReports';
import { getFilterDefaultValue } from '../utils';
import { ChartBlock } from './blocks/ChartBlock';
import { MetricBlock } from './blocks/MetricBlock';
import { TableBlock } from './blocks/TableBlock';
import { ActionsBlock } from './blocks/ActionsBlock';
import { CardBlock } from './blocks/CardBlock';
import { FileUploadBlock } from './blocks/FileUploadBlock';
import { ReportFilterBar } from './ReportFilterBar';
import { encodeFilterValue } from '../utils';
import type { ReportWorkflowActionResult } from './blocks/useReportWorkflowAction';

type ReportBlockHostProps = {
  reportId: string;
  activeViewId?: string | null;
  block: ReportBlockDefinition;
  initialResult?: ReportBlockResult | ReportBlockRenderResult;
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
  onReportRefresh?: (
    result?: ReportWorkflowActionResult,
    action?: ReportWorkflowActionConfig
  ) => void | Promise<unknown>;
};

export function ReportBlockHost({
  reportId,
  activeViewId,
  block,
  initialResult,
  filters,
  className = 'my-6',
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
            column.type !== 'workflow_button' &&
            column.type !== 'interaction_buttons' &&
            !column.workflowAction
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
      viewId: activeViewId ?? undefined,
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
      activeViewId,
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
    error: fetchError,
    refetch,
  } = useReportBlockData(reportId, block.id, request, needsBlockFetch);
  const explorePath = useMemo(
    () => buildExplorePath(reportId, block.id, filters),
    [block.id, filters, reportId]
  );
  // Metric cards render their own label inside the card, so the block-host
  // heading would just duplicate it. Group-level headings belong to the
  // grid layout node, not per-metric.
  const showBlockTitle = Boolean(block.title) && block.type !== 'metric';

  const result = needsBlockFetch
    ? (fetchedResult ?? initialResult)
    : initialResult;
  // A failed block fetch leaves `result` undefined. Without this the block
  // renders its loading skeleton forever: the query is settled so nothing
  // refetches, and `refetchOnWindowFocus` is off, so only a remount recovers
  // it. Lazy blocks are the common casualty — the report-level render omits
  // them, so they have no `initialResult` to fall back on. Guarded on
  // `!result` so a block that already has data keeps showing it when a
  // refetch fails, rather than blanking out.
  const showFetchError = Boolean(fetchError) && !result;
  const refreshAfterActionSubmit = async (
    actionResult?: ReportWorkflowActionResult,
    action?: ReportWorkflowActionConfig
  ) => {
    const refreshes: Promise<unknown>[] = [
      Promise.resolve(onReportRefresh?.(actionResult, action)),
    ];
    if ((!action || action.reloadBlock) && needsBlockFetch) {
      refreshes.push(refetch());
    }
    await Promise.allSettled(refreshes);
  };
  const runInteractionActions = (
    actions: ReportInteractionAction[],
    datum: Record<string, unknown>
  ): boolean => {
    let handled = false;
    const updates: Record<string, unknown> = {};
    const clearFilters = new Set<string>();
    let nextViewId: string | null | undefined;
    for (const action of actions) {
      if (action.type === 'set_filter' && action.filterId) {
        const value =
          action.valueFrom != null
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

  const runInteraction = (
    event: string,
    datum: Record<string, unknown>
  ): boolean => {
    const actions: ReportInteractionAction[] = [];
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
      actions.push(...(interaction.actions ?? []));
    }
    return runInteractionActions(actions, datum);
  };

  if (block.hideWhenEmpty && result?.status === 'empty') {
    return null;
  }

  return (
    <div ref={rootRef} className={className}>
      {(showBlockTitle || block.dataset) && (
        <div className="mb-2 flex items-center justify-between gap-3">
          {showBlockTitle ? (
            <h2 className="text-base font-semibold text-foreground">
              {block.title}
            </h2>
          ) : (
            <span />
          )}
          <div className="report-print-hidden flex items-center gap-2">
            {block.dataset && (
              <Link to={explorePath}>
                <Button variant="outline" size="sm">
                  <Compass className="mr-2 h-4 w-4" />
                  Explore this
                </Button>
              </Link>
            )}
          </div>
        </div>
      )}
      {(block.filters?.length ?? 0) > 0 && (
        <div className="report-print-hidden mb-3">
          <ReportFilterBar
            reportId={reportId}
            definition={{
              definitionVersion: 1,
              layout: { id: 'root', columns: 1, items: [] },
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
      {block.type === 'file_upload' ? (
        // File-upload blocks are pure controls: no data source, no fetch, no
        // skeleton — they render straight from the definition.
        <FileUploadBlock
          reportId={reportId}
          activeViewId={activeViewId}
          block={block}
          filters={filters}
          onRefresh={refreshAfterActionSubmit}
        />
      ) : !isVisible || (!result && isFetching) ? (
        <BlockSkeleton block={block} />
      ) : showFetchError ? (
        <BlockError message={fetchError?.message} onRetry={() => refetch()} />
      ) : !result ? (
        <BlockSkeleton block={block} />
      ) : result.status === 'error' ? (
        <BlockError message={result.error?.message} onRetry={() => refetch()} />
      ) : (
        <RenderedBlock
          reportId={reportId}
          activeViewId={activeViewId}
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
          onInteractionButtonClick={runInteractionActions}
          onRefresh={refreshAfterActionSubmit}
        />
      )}
    </div>
  );
}

function RenderedBlock({
  reportId,
  activeViewId,
  block,
  result,
  sort,
  filters,
  blockFilters,
  onPageChange,
  onSortChange,
  onBlockInteraction,
  onInteractionButtonClick,
  onRefresh,
}: {
  reportId: string;
  activeViewId?: string | null;
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
  onInteractionButtonClick: (
    actions: ReportInteractionAction[],
    row: Record<string, unknown>
  ) => boolean;
  onRefresh: (
    result?: ReportWorkflowActionResult,
    action?: ReportWorkflowActionConfig
  ) => void | Promise<void>;
}) {
  if (block.type === 'table') {
    return (
      <TableBlock
        reportId={reportId}
        activeViewId={activeViewId}
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
        onInteractionButtonClick={onInteractionButtonClick}
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
        activeViewId={activeViewId}
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
      : (block.markdown?.content ?? '');
  const source =
    data.source && typeof data.source === 'object'
      ? (data.source as Record<string, unknown>)
      : undefined;
  const rendered = interpolateMarkdownSource(content, source);

  return (
    // prose-sm keeps markdown body copy on the report's 14px scale, and the
    // heading caps keep authored markdown below the view title in hierarchy.
    <div className="prose prose-sm prose-slate max-w-none dark:prose-invert prose-headings:font-semibold prose-headings:tracking-tight prose-h1:text-2xl prose-h2:text-xl prose-h3:text-base">
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
  if (block.type === 'metric') {
    return (
      <div
        className="rounded-lg border bg-card p-4"
        aria-label="Loading report block"
      >
        <div className="h-3 w-20 animate-pulse rounded bg-muted/50" />
        <div className="mt-3 h-7 w-24 animate-pulse rounded bg-muted/50" />
      </div>
    );
  }

  if (block.type === 'table') {
    return (
      <div
        className="overflow-hidden rounded-lg border bg-card"
        aria-label="Loading report block"
      >
        <div className="h-9 animate-pulse bg-muted/40" />
        <div className="divide-y">
          {Array.from({ length: 6 }).map((_, index) => (
            <div key={index} className="flex items-center gap-4 px-3 py-2.5">
              <div className="h-3 w-1/4 animate-pulse rounded bg-muted/40" />
              <div className="h-3 w-1/3 animate-pulse rounded bg-muted/40" />
              <div className="ml-auto h-3 w-16 animate-pulse rounded bg-muted/40" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (block.type === 'chart') {
    const bars = [48, 72, 36, 84, 56, 68, 44, 76, 52, 64];
    return (
      <div
        className="flex h-72 items-end gap-2 rounded-lg border bg-card p-4"
        aria-label="Loading report block"
      >
        {bars.map((height, index) => (
          <div
            key={index}
            className="flex-1 animate-pulse rounded-t bg-muted/40"
            style={{ height: `${height}%` }}
          />
        ))}
      </div>
    );
  }

  // markdown / card / default
  return (
    <div
      className="space-y-3 rounded-lg border bg-card p-4"
      aria-label="Loading report block"
    >
      <div className="h-4 w-1/3 animate-pulse rounded bg-muted/40" />
      <div className="h-3 w-full animate-pulse rounded bg-muted/40" />
      <div className="h-3 w-5/6 animate-pulse rounded bg-muted/40" />
      <div className="h-3 w-2/3 animate-pulse rounded bg-muted/40" />
    </div>
  );
}

function BlockError({
  message,
  onRetry,
}: {
  message?: string;
  onRetry: () => void;
}) {
  return (
    <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-4">
      <p className="text-sm font-semibold text-destructive">
        {message ?? 'This report block could not be rendered.'}
      </p>
      <Button
        className="report-print-hidden mt-3"
        variant="outline"
        size="sm"
        onClick={onRetry}
      >
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
