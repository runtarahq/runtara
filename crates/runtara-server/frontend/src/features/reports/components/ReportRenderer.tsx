import { CSSProperties, Fragment, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {
  ReportDefinition,
  ReportLayoutNode,
  ReportRenderResponse,
} from '../types';
import { getBlockById } from '../utils';
import { ReportBlockHost } from './ReportBlockHost';

type ReportRendererProps = {
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (updates: Record<string, unknown>) => void;
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
  onFilterChange,
  onFiltersChange,
}: ReportRendererProps) {
  const hasStructuredLayout = (definition.layout?.length ?? 0) > 0;
  const segments = useMemo(
    () => splitMarkdown(definition.markdown),
    [definition.markdown]
  );

  return (
    <div className="w-full">
      {hasStructuredLayout ? (
        <LayoutNodes
          nodes={definition.layout ?? []}
          reportId={reportId}
          definition={definition}
          renderResponse={renderResponse}
          filters={filters}
          onFilterChange={onFilterChange}
          onFiltersChange={onFiltersChange}
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
            />
          );
        })
      )}
      {!hasStructuredLayout &&
        segments.length === 0 &&
        definition.blocks.map((block) => (
          <Fragment key={block.id}>
            <ReportBlockHost
              reportId={reportId}
              block={block}
              initialResult={renderResponse?.blocks[block.id]}
              filters={filters}
              onFilterChange={onFilterChange}
              onFiltersChange={onFiltersChange}
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
}: {
  nodes: ReportLayoutNode[];
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (updates: Record<string, unknown>) => void;
}) {
  return (
    <>
      {nodes.map((node) => (
        <LayoutNode
          key={node.id}
          node={node}
          reportId={reportId}
          definition={definition}
          renderResponse={renderResponse}
          filters={filters}
          onFilterChange={onFilterChange}
          onFiltersChange={onFiltersChange}
        />
      ))}
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
}: {
  node: ReportLayoutNode;
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (updates: Record<string, unknown>) => void;
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
}: {
  blockId: string;
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  className?: string;
  onFilterChange?: (filterId: string, value: unknown) => void;
  onFiltersChange?: (updates: Record<string, unknown>) => void;
}) {
  const block = getBlockById(definition, blockId);
  if (!block) {
    return (
      <div className="my-5 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive">
        Unknown report block: {blockId}
      </div>
    );
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
    />
  );
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
