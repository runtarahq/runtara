import { CSSProperties, Fragment, Suspense, useMemo } from 'react';
import {
  ReportDefinition,
  ReportInteractionOptions,
  ReportLayoutNode,
  ReportRenderResponse,
  ReportViewBreadcrumb,
  ReportViewDefinition,
} from '../types';
import {
  getActiveReportLayout,
  getActiveReportView,
  getBlockById,
  getReportViewBreadcrumbs,
  isVisibleByShowWhen,
} from '../utils';
import { useReportDsl } from '../hooks/useReportDsl';
import { ReportBlockHost } from './ReportBlockHost';
import { ReportViewNavigation } from './ReportViewNavigation';

type ReportRendererProps = {
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  activeViewId?: string | null;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (
    updates: Record<string, unknown>,
    options?: ReportInteractionOptions
  ) => void;
  onNavigateView?: (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => void;
  onRefresh?: () => void | Promise<unknown>;
};

export function ReportRenderer(props: ReportRendererProps) {
  return (
    <Suspense fallback={<ReportRendererSkeleton />}>
      <ReportRendererInner {...props} />
    </Suspense>
  );
}

function ReportRendererSkeleton() {
  return <div className="h-32 w-full animate-pulse rounded-lg bg-muted/30" />;
}

function ReportRendererInner({
  reportId,
  definition,
  renderResponse,
  filters,
  activeViewId,
  onFilterChange,
  onFiltersChange,
  onNavigateView,
  onRefresh,
}: ReportRendererProps) {
  // Suspends until the WASM bundle loads; cached for the rest of the
  // session afterward. Cell renderers inside the tree call
  // `getReportDsl()` synchronously.
  useReportDsl();
  const activeView = useMemo(
    () => getActiveReportView(definition, activeViewId),
    [activeViewId, definition]
  );
  const layout = useMemo(
    () => getActiveReportLayout(definition, activeViewId),
    [activeViewId, definition]
  );
  const hasStructuredLayout = (layout.items ?? []).length > 0;
  const resolvedViewId = activeView?.id ?? activeViewId ?? null;
  const showEmptyState =
    !hasStructuredLayout &&
    Boolean(activeView || definition.blocks.length === 0);

  return (
    <div className="w-full">
      <ReportViewNavigation
        definition={definition}
        navigation={renderResponse?.navigation}
        activeViewId={resolvedViewId}
        onNavigateView={onNavigateView}
      />
      {activeView && (
        <ReportViewHeader
          view={activeView}
          definition={definition}
          renderResponse={renderResponse}
          filters={filters}
          onNavigateView={onNavigateView}
        />
      )}
      {showEmptyState ? (
        <div className="grid place-items-center gap-2 rounded-lg border border-dashed bg-muted/10 px-6 py-12 text-center">
          <p className="text-sm font-medium text-foreground">
            {activeView
              ? 'This report view has no content yet'
              : 'This report has no content yet'}
          </p>
          <p className="max-w-prose text-xs text-muted-foreground">
            Switch to edit mode to add a markdown section, metric, chart, table,
            or card.
          </p>
        </div>
      ) : hasStructuredLayout ? (
        <LayoutNodes
          nodes={(layout.items ?? []).map((item) => item.child)}
          reportId={reportId}
          activeViewId={resolvedViewId}
          definition={definition}
          renderResponse={renderResponse}
          filters={filters}
          onFilterChange={onFilterChange}
          onFiltersChange={onFiltersChange}
          onNavigateView={onNavigateView}
          onRefresh={onRefresh}
        />
      ) : (
        definition.blocks
          .filter((block) => isVisibleByShowWhen(block.showWhen, filters))
          .map((block) => (
            <Fragment key={block.id}>
              <ReportBlockHost
                reportId={reportId}
                activeViewId={resolvedViewId}
                block={block}
                initialResult={renderResponse?.blocks[block.id]}
                filters={filters}
                onFilterChange={onFilterChange}
                onFiltersChange={onFiltersChange}
                onNavigateView={onNavigateView}
                onReportRefresh={onRefresh}
              />
            </Fragment>
          ))
      )}
    </div>
  );
}

function LayoutNodes({
  nodes,
  reportId,
  activeViewId,
  definition,
  renderResponse,
  filters,
  inGrid = false,
  onFilterChange,
  onFiltersChange,
  onNavigateView,
  onRefresh,
}: {
  nodes: ReportLayoutNode[];
  reportId: string;
  activeViewId: string | null;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  /** True when these nodes render inside a grid cell: the grid's gap owns
   *  the spacing, so cell content drops its own vertical margins and
   *  stretches to equalize row heights. */
  inGrid?: boolean;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (
    updates: Record<string, unknown>,
    options?: ReportInteractionOptions
  ) => void;
  onNavigateView?: (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => void;
  onRefresh?: () => void | Promise<unknown>;
}) {
  return (
    <>
      {nodes.map((node) =>
        isVisibleByShowWhen(node.showWhen, filters) ? (
          <LayoutNode
            key={node.id}
            node={node}
            reportId={reportId}
            activeViewId={activeViewId}
            definition={definition}
            renderResponse={renderResponse}
            filters={filters}
            inGrid={inGrid}
            onFilterChange={onFilterChange}
            onFiltersChange={onFiltersChange}
            onNavigateView={onNavigateView}
            onRefresh={onRefresh}
          />
        ) : null
      )}
    </>
  );
}

function LayoutNode({
  node,
  reportId,
  activeViewId,
  definition,
  renderResponse,
  filters,
  inGrid = false,
  onFilterChange,
  onFiltersChange,
  onNavigateView,
  onRefresh,
}: {
  node: ReportLayoutNode;
  reportId: string;
  activeViewId: string | null;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  inGrid?: boolean;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (
    updates: Record<string, unknown>,
    options?: ReportInteractionOptions
  ) => void;
  onNavigateView?: (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => void;
  onRefresh?: () => void | Promise<unknown>;
}) {
  if (node.type === 'block') {
    return (
      <ReportBlockById
        blockId={node.blockId}
        reportId={reportId}
        activeViewId={activeViewId}
        definition={definition}
        renderResponse={renderResponse}
        filters={filters}
        className={inGrid ? 'h-full' : undefined}
        onFilterChange={onFilterChange}
        onFiltersChange={onFiltersChange}
        onNavigateView={onNavigateView}
        onRefresh={onRefresh}
      />
    );
  }

  if (node.type === 'grid') {
    const columns = Math.max(node.columns ?? 1, 1);
    const widths = node.columnWidths;
    const template =
      widths && widths.length === columns
        ? widths.map((w) => `${Math.max(w, 0.0001)}fr`).join(' ')
        : `repeat(${columns}, minmax(0, 1fr))`;
    return (
      <section className={inGrid ? 'w-full' : 'my-6 w-full'}>
        {(node.title || node.description) && (
          <div className="mb-3">
            {node.title && (
              <h2 className="text-lg font-semibold text-foreground">
                {node.title}
              </h2>
            )}
            {node.description && (
              <p className="mt-1 text-sm text-muted-foreground">
                {node.description}
              </p>
            )}
          </div>
        )}
        <div
          className="grid w-full gap-4 lg:[grid-template-columns:var(--report-grid-columns)]"
          style={
            {
              '--report-grid-columns': template,
            } as CSSProperties
          }
        >
          {node.items.map((item) => {
            const colSpan =
              item.colSpan && item.colSpan > 1
                ? Math.min(item.colSpan, columns)
                : undefined;
            const rowSpan =
              item.rowSpan && item.rowSpan > 1 ? item.rowSpan : undefined;
            // Phase 11: explicit cell pinning. When an item has col/row,
            // CSS pins it to that cell instead of auto-flowing.
            const colCss =
              item.col != null
                ? `${item.col} / span ${colSpan ?? 1}`
                : colSpan
                  ? `span ${colSpan} / span ${colSpan}`
                  : 'auto';
            const rowCss =
              item.row != null
                ? `${item.row} / span ${rowSpan ?? 1}`
                : rowSpan
                  ? `span ${rowSpan} / span ${rowSpan}`
                  : 'auto';
            return (
              <div
                key={item.id}
                className="min-w-0 lg:[grid-column:var(--report-grid-column)] lg:[grid-row:var(--report-grid-row)]"
                style={
                  {
                    '--report-grid-column': colCss,
                    '--report-grid-row': rowCss,
                  } as CSSProperties
                }
              >
                <LayoutNodes
                  nodes={[item.child]}
                  reportId={reportId}
                  activeViewId={activeViewId}
                  definition={definition}
                  renderResponse={renderResponse}
                  filters={filters}
                  inGrid
                  onFilterChange={onFilterChange}
                  onFiltersChange={onFiltersChange}
                  onNavigateView={onNavigateView}
                  onRefresh={onRefresh}
                />
              </div>
            );
          })}
        </div>
      </section>
    );
  }

  return null;
}

function ReportBlockById({
  blockId,
  reportId,
  activeViewId,
  definition,
  renderResponse,
  filters,
  className,
  onFilterChange,
  onFiltersChange,
  onNavigateView,
  onRefresh,
}: {
  blockId: string;
  reportId: string;
  activeViewId: string | null;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
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
  onRefresh?: () => void | Promise<unknown>;
}) {
  const block = getBlockById(definition, blockId);
  if (!block) {
    return (
      <div className="my-6 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive">
        Unknown report block: {blockId}
      </div>
    );
  }

  if (!isVisibleByShowWhen(block.showWhen, filters)) {
    return null;
  }

  return (
    <ReportBlockHost
      reportId={reportId}
      activeViewId={activeViewId}
      block={block}
      initialResult={renderResponse?.blocks[blockId]}
      filters={filters}
      className={className}
      onFilterChange={onFilterChange}
      onFiltersChange={onFiltersChange}
      onNavigateView={onNavigateView}
      onReportRefresh={onRefresh}
    />
  );
}

function ReportViewHeader({
  view,
  definition,
  renderResponse,
  filters,
  onNavigateView,
}: {
  view: ReportViewDefinition;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  onNavigateView?: (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => void;
}) {
  const title = resolveViewTitle(view, definition, renderResponse, filters);
  const breadcrumbs = getReportViewBreadcrumbs(definition, view, (candidate) =>
    resolveViewTitle(candidate, definition, renderResponse, filters)
  );
  if (!title && breadcrumbs.length === 0) return null;

  // Ancestors render as a small breadcrumb trail; the current view always
  // gets a real heading underneath so detail views don't demote their title
  // to a breadcrumb fragment.
  return (
    <div className="report-print-hidden mb-6 flex flex-col gap-1">
      {breadcrumbs.length > 0 && (
        <nav
          aria-label="Report view breadcrumb"
          className="text-sm text-muted-foreground"
        >
          <ol className="flex flex-wrap items-center gap-1.5">
            {breadcrumbs.map((breadcrumb, index) => (
              <Fragment key={`${breadcrumb.label}-${index}`}>
                <li>
                  <BreadcrumbButton
                    breadcrumb={breadcrumb}
                    onNavigateView={onNavigateView}
                  />
                </li>
                {index < breadcrumbs.length - 1 && (
                  <li aria-hidden="true" className="text-muted-foreground/60">
                    /
                  </li>
                )}
              </Fragment>
            ))}
          </ol>
        </nav>
      )}
      {title && (
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">
          {title}
        </h1>
      )}
    </div>
  );
}

function BreadcrumbButton({
  breadcrumb,
  onNavigateView,
}: {
  breadcrumb: ReportViewBreadcrumb;
  onNavigateView?: (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => void;
}) {
  if (!breadcrumb.viewId) {
    return <span>{breadcrumb.label}</span>;
  }

  return (
    <button
      type="button"
      className="font-medium text-primary underline-offset-4 hover:underline"
      onClick={() =>
        onNavigateView?.(breadcrumb.viewId ?? null, {
          clearFilters: breadcrumb.clearFilters ?? [],
        })
      }
    >
      {breadcrumb.label}
    </button>
  );
}

function resolveViewTitle(
  view: ReportViewDefinition,
  definition: ReportDefinition,
  renderResponse: ReportRenderResponse | null | undefined,
  filters: Record<string, unknown>
): string | null {
  if (view.titleFromBlock) {
    const value = resolveTitleFromBlock(
      view.titleFromBlock,
      definition,
      renderResponse
    );
    if (value !== undefined && value !== null && value !== '') {
      return String(value);
    }
  }
  if (view.titleFrom) {
    const value = resolveTitlePath(view.titleFrom, filters);
    if (value !== undefined && value !== null && value !== '') {
      return String(value);
    }
  }
  return view.title ?? null;
}

function resolveTitleFromBlock(
  ref: NonNullable<ReportViewDefinition['titleFromBlock']>,
  definition: ReportDefinition,
  renderResponse: ReportRenderResponse | null | undefined
): unknown {
  const block = definition.blocks.find(
    (candidate) => candidate.id === ref.block
  );
  const result = renderResponse?.blocks?.[ref.block];
  const data = result?.data as
    | {
        rows?: Array<Record<string, unknown> | unknown[]>;
        columns?: Array<string | { key: string }>;
      }
    | undefined;
  const firstRow = data?.rows?.[0];
  if (!firstRow) return undefined;

  const field =
    ref.field ??
    block?.table?.columns?.find((column) => column.descriptive)?.field;
  if (!field) return undefined;

  if (Array.isArray(firstRow)) {
    const dataColumns = data?.columns ?? [];
    const idx = dataColumns.findIndex((column) =>
      typeof column === 'string' ? column === field : column.key === field
    );
    return idx >= 0 ? firstRow[idx] : undefined;
  }
  return firstRow[field];
}

function resolveTitlePath(path: string, filters: Record<string, unknown>) {
  const normalizedPath = path.startsWith('filters.')
    ? path.slice('filters.'.length)
    : path;
  return normalizedPath.split('.').reduce<unknown>((current, part) => {
    if (current && typeof current === 'object' && part in current) {
      return (current as Record<string, unknown>)[part];
    }
    return undefined;
  }, filters);
}
