import { Fragment, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { ReportDefinition, ReportRenderResponse } from '../types';
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
  const segments = useMemo(
    () => splitMarkdown(definition.markdown),
    [definition.markdown]
  );

  return (
    <div className="mx-auto max-w-7xl">
      {segments.map((segment, index) => {
        if (segment.type === 'markdown') {
          return (
            <div
              key={index}
              className="prose prose-slate max-w-none dark:prose-invert"
            >
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {segment.content}
              </ReactMarkdown>
            </div>
          );
        }

        const block = getBlockById(definition, segment.blockId);
        if (!block) {
          return (
            <div
              key={index}
              className="my-5 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive"
            >
              Unknown report block: {segment.blockId}
            </div>
          );
        }

        return (
          <ReportBlockHost
            key={`${segment.blockId}-${index}`}
            reportId={reportId}
            block={block}
            initialResult={renderResponse?.blocks[segment.blockId]}
            filters={filters}
          />
        );
      })}
      {segments.length === 0 &&
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

function splitMarkdown(markdown: string): MarkdownSegment[] {
  const segments: MarkdownSegment[] = [];
  let lastIndex = 0;
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
