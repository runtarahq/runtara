import { Fragment, useMemo } from 'react';
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
}: ReportRendererProps) {
  const hasStructuredLayout = (definition.layout?.length ?? 0) > 0;
  const segments = useMemo(
    () => splitMarkdown(definition.markdown),
    [definition.markdown]
  );

  return (
    <div className="mx-auto max-w-7xl">
      {hasStructuredLayout ? (
        <LayoutNodes
          nodes={definition.layout ?? []}
          reportId={reportId}
          definition={definition}
          renderResponse={renderResponse}
          filters={filters}
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
}: {
  nodes: ReportLayoutNode[];
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
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
}: {
  node: ReportLayoutNode;
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
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
      />
    );
  }

  if (node.type === 'metric_row') {
    return (
      <section className="my-5">
        {node.title && (
          <h2 className="mb-2 text-base font-semibold text-foreground">
            {node.title}
          </h2>
        )}
        <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(220px,1fr))]">
          {node.blocks.map((blockId) => (
            <ReportBlockById
              key={blockId}
              blockId={blockId}
              reportId={reportId}
              definition={definition}
              renderResponse={renderResponse}
              filters={filters}
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
        className="my-5 grid gap-4"
        style={{
          gridTemplateColumns: `repeat(auto-fit, minmax(min(100%, 280px), 1fr))`,
        }}
      >
        <div
          className="contents lg:grid lg:gap-4"
          style={{ gridTemplateColumns: template }}
        >
          {node.columns.map((column) => (
            <div key={column.id} className="min-w-0">
              <LayoutNodes
                nodes={column.children ?? []}
                reportId={reportId}
                definition={definition}
                renderResponse={renderResponse}
                filters={filters}
              />
            </div>
          ))}
        </div>
      </section>
    );
  }

  if (node.type === 'grid') {
    const columns = Math.max(node.columns ?? 12, 1);
    return (
      <section
        className="my-5 grid gap-4"
        style={{
          gridTemplateColumns:
            'repeat(auto-fit, minmax(min(100%, 260px), 1fr))',
        }}
      >
        <div
          className="contents xl:grid xl:gap-4"
          style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
        >
          {node.items.map((item, index) => (
            <div
              key={item.id ?? `${item.blockId}-${index}`}
              className="min-w-0"
              style={{
                gridColumn:
                  item.colSpan && item.colSpan > 1
                    ? `span ${Math.min(item.colSpan, columns)} / span ${Math.min(
                        item.colSpan,
                        columns
                      )}`
                    : undefined,
                gridRow:
                  item.rowSpan && item.rowSpan > 1
                    ? `span ${item.rowSpan} / span ${item.rowSpan}`
                    : undefined,
              }}
            >
              <ReportBlockById
                blockId={item.blockId}
                reportId={reportId}
                definition={definition}
                renderResponse={renderResponse}
                filters={filters}
                className="my-0"
              />
            </div>
          ))}
        </div>
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
}: {
  blockId: string;
  reportId: string;
  definition: ReportDefinition;
  renderResponse?: ReportRenderResponse | null;
  filters: Record<string, unknown>;
  className?: string;
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
