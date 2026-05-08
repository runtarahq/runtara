import { CSSProperties, Fragment, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
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
  isVisibleByShowWhen,
} from '../utils';
import { ReportBlockHost } from './ReportBlockHost';

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

type MarkdownSegment =
  | { type: 'markdown'; content: string }
  | { type: 'block'; blockId: string };

const BLOCK_PLACEHOLDER_RE = /\{\{\s*block\.([a-zA-Z0-9_-]+)\s*\}\}/g;

export function ReportRenderer({
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
  const activeView = useMemo(
    () => getActiveReportView(definition, activeViewId),
    [activeViewId, definition]
  );
  const layout = useMemo(
    () => getActiveReportLayout(definition, activeViewId),
    [activeViewId, definition]
  );
  const hasStructuredLayout = layout.length > 0;
  const segments = useMemo(
    () => splitMarkdown(definition.markdown),
    [definition.markdown]
  );

  return (
    <div className="w-full">
      {activeView && (
        <ReportViewHeader
          view={activeView}
          filters={filters}
          onNavigateView={onNavigateView}
        />
      )}
      {hasStructuredLayout ? (
        <LayoutNodes
          nodes={layout}
          reportId={reportId}
          definition={definition}
          renderResponse={renderResponse}
          filters={filters}
          onFilterChange={onFilterChange}
          onFiltersChange={onFiltersChange}
          onNavigateView={onNavigateView}
          onRefresh={onRefresh}
        />
      ) : (
        segments.map((segment, index) => {
          if (segment.type === 'markdown') {
            return <MarkdownContent key={index} content={segment.content} />;
          }

          return (
            <ReportBlockById
              key={`${segment.blockId}-${index}`}
              blockId={segment.blockId}
              reportId={reportId}
              definition={definition}
              renderResponse={renderResponse}
              filters={filters}
              onFilterChange={onFilterChange}
              onFiltersChange={onFiltersChange}
              onNavigateView={onNavigateView}
              onRefresh={onRefresh}
            />
          );
        })
      )}
      {!hasStructuredLayout &&
        segments.length === 0 &&
        definition.blocks
          .filter((block) => isVisibleByShowWhen(block.showWhen, filters))
          .map((block) => (
            <Fragment key={block.id}>
              <ReportBlockHost
                reportId={reportId}
                block={block}
                initialResult={renderResponse?.blocks[block.id]}
                filters={filters}
                onFilterChange={onFilterChange}
                onFiltersChange={onFiltersChange}
                onNavigateView={onNavigateView}
                onReportRefresh={onRefresh}
              />
            </Fragment>
          ))}
    </div>
  );
}

function LayoutNodes({
  nodes,
  reportId,
  definition,
  renderResponse,
  filters,
  onFilterChange,
  onFiltersChange,
  onNavigateView,
  onRefresh,
}: {
  nodes: ReportLayoutNode[];
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
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
            definition={definition}
            renderResponse={renderResponse}
            filters={filters}
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
  definition,
  renderResponse,
  filters,
  onFilterChange,
  onFiltersChange,
  onNavigateView,
  onRefresh,
}: {
  node: ReportLayoutNode;
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
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
  if (node.type === 'markdown') {
    return <MarkdownContent content={node.content} />;
  }

  if (node.type === 'block') {
    return (
      <ReportBlockById
        blockId={node.blockId}
        reportId={reportId}
        definition={definition}
        renderResponse={renderResponse}
        filters={filters}
        onFilterChange={onFilterChange}
        onFiltersChange={onFiltersChange}
        onNavigateView={onNavigateView}
        onRefresh={onRefresh}
      />
    );
  }

  if (node.type === 'metric_row') {
    return (
      <section className="my-5 w-full">
        {node.title && (
          <h2 className="mb-2 text-base font-semibold text-foreground">
            {node.title}
          </h2>
        )}
        <div className="grid w-full gap-3 [grid-template-columns:repeat(auto-fit,minmax(220px,1fr))]">
          {node.blocks.map((blockId) => (
            <ReportBlockById
              key={blockId}
              blockId={blockId}
              reportId={reportId}
              definition={definition}
              renderResponse={renderResponse}
              filters={filters}
              onFilterChange={onFilterChange}
              onFiltersChange={onFiltersChange}
              onNavigateView={onNavigateView}
              onRefresh={onRefresh}
              className="my-0"
            />
          ))}
        </div>
      </section>
    );
  }

  if (node.type === 'section') {
    return (
      <section className="my-8">
        {(node.title || node.description) && (
          <div className="mb-4">
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
        <LayoutNodes
          nodes={node.children ?? []}
          reportId={reportId}
          definition={definition}
          renderResponse={renderResponse}
          filters={filters}
          onFilterChange={onFilterChange}
          onFiltersChange={onFiltersChange}
          onNavigateView={onNavigateView}
          onRefresh={onRefresh}
        />
      </section>
    );
  }

  if (node.type === 'columns') {
    const template = node.columns
      .map((column) => `${Math.max(column.width ?? 1, 1)}fr`)
      .join(' ');
    return (
      <section
        className="my-5 grid w-full gap-4 lg:[grid-template-columns:var(--report-columns)]"
        style={{ '--report-columns': template } as CSSProperties}
      >
        {node.columns.map((column) => (
          <div key={column.id} className="min-w-0">
            <LayoutNodes
              nodes={column.children ?? []}
              reportId={reportId}
              definition={definition}
              renderResponse={renderResponse}
              filters={filters}
              onFilterChange={onFilterChange}
              onFiltersChange={onFiltersChange}
              onNavigateView={onNavigateView}
              onRefresh={onRefresh}
            />
          </div>
        ))}
      </section>
    );
  }

  if (node.type === 'grid') {
    const columns = Math.max(node.columns ?? 12, 1);
    return (
      <section
        className="my-5 grid w-full gap-4 xl:[grid-template-columns:var(--report-grid-columns)]"
        style={
          {
            '--report-grid-columns': `repeat(${columns}, minmax(0, 1fr))`,
          } as CSSProperties
        }
      >
        {node.items.map((item, index) => {
          const colSpan =
            item.colSpan && item.colSpan > 1
              ? Math.min(item.colSpan, columns)
              : undefined;
          const rowSpan =
            item.rowSpan && item.rowSpan > 1 ? item.rowSpan : undefined;

          return (
            <div
              key={item.id ?? `${item.blockId}-${index}`}
              className="min-w-0 xl:[grid-column:var(--report-grid-column)] xl:[grid-row:var(--report-grid-row)]"
              style={
                {
                  '--report-grid-column': colSpan
                    ? `span ${colSpan} / span ${colSpan}`
                    : 'auto',
                  '--report-grid-row': rowSpan
                    ? `span ${rowSpan} / span ${rowSpan}`
                    : 'auto',
                } as CSSProperties
              }
            >
              <ReportBlockById
                blockId={item.blockId}
                reportId={reportId}
                definition={definition}
                renderResponse={renderResponse}
                filters={filters}
                onFilterChange={onFilterChange}
                onFiltersChange={onFiltersChange}
                onNavigateView={onNavigateView}
                onRefresh={onRefresh}
                className="my-0"
              />
            </div>
          );
        })}
      </section>
    );
  }

  return null;
}

function MarkdownContent({ content }: { content: string }) {
  return (
    <div className="prose prose-slate max-w-none dark:prose-invert">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
    </div>
  );
}

function ReportBlockById({
  blockId,
  reportId,
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
      <div className="my-5 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive">
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
  filters,
  onNavigateView,
}: {
  view: ReportViewDefinition;
  filters: Record<string, unknown>;
  onNavigateView?: (
    viewId: string | null,
    options?: Omit<ReportInteractionOptions, 'viewId'>
  ) => void;
}) {
  const title = resolveViewTitle(view, filters);
  const breadcrumbs = view.breadcrumb ?? [];
  if (!title && breadcrumbs.length === 0) return null;

  return (
    <div className="report-print-hidden mb-5 flex flex-col gap-2">
      {breadcrumbs.length > 0 && (
        <nav
          aria-label="Report view breadcrumb"
          className="text-sm text-muted-foreground"
        >
          <ol className="flex flex-wrap items-center gap-2">
            {breadcrumbs.map((breadcrumb, index) => (
              <Fragment key={`${breadcrumb.label}-${index}`}>
                <li>
                  <BreadcrumbButton
                    breadcrumb={breadcrumb}
                    onNavigateView={onNavigateView}
                  />
                </li>
                {(index < breadcrumbs.length - 1 || title) && (
                  <li aria-hidden="true">/</li>
                )}
              </Fragment>
            ))}
            {title && <li className="font-medium text-foreground">{title}</li>}
          </ol>
        </nav>
      )}
      {title && (
        <h1 className="text-xl font-semibold tracking-normal text-foreground">
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
  filters: Record<string, unknown>
): string | null {
  if (view.titleFrom) {
    const value = resolveTitlePath(view.titleFrom, filters);
    if (value !== undefined && value !== null && value !== '') {
      return String(value);
    }
  }
  return view.title ?? null;
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

function splitMarkdown(markdown: string): MarkdownSegment[] {
  const segments: MarkdownSegment[] = [];
  let lastIndex = 0;
  BLOCK_PLACEHOLDER_RE.lastIndex = 0;
  let match = BLOCK_PLACEHOLDER_RE.exec(markdown);

  while (match) {
    if (match.index > lastIndex) {
      segments.push({
        type: 'markdown',
        content: markdown.slice(lastIndex, match.index),
      });
    }
    segments.push({ type: 'block', blockId: match[1] });
    lastIndex = match.index + match[0].length;
    match = BLOCK_PLACEHOLDER_RE.exec(markdown);
  }

  if (lastIndex < markdown.length) {
    segments.push({ type: 'markdown', content: markdown.slice(lastIndex) });
  }

  return segments.filter(
    (segment) => segment.type === 'block' || segment.content.trim().length > 0
  );
}
