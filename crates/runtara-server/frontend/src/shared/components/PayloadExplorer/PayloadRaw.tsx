import { useMemo } from 'react';
import { FileJson } from 'lucide-react';
import { Highlight } from './Highlight';
import { RAW_VIEW_MAX_BYTES, formatSize, rawLines } from './model';
import { useVirtualWindow } from './useVirtualWindow';

const LINE_HEIGHT = 20;

/**
 * Pretty-printed JSON with line numbers, virtualized by line. Payloads above
 * RAW_VIEW_MAX_BYTES are not serialized at all; the tree handles those.
 */
export function PayloadRaw({
  value,
  query,
  maxHeight,
  label,
}: {
  value: unknown;
  query: string;
  maxHeight: number;
  label: string;
}) {
  const raw = useMemo(() => rawLines(value), [value]);
  const lines = raw.status === 'ok' ? raw.lines : [];
  const viewportHeight = Math.min(
    maxHeight,
    Math.max(lines.length, 1) * LINE_HEIGHT
  );
  const { scrollRef, onScroll, start, end, totalHeight, offsetTop } =
    useVirtualWindow({
      count: lines.length,
      rowHeight: LINE_HEIGHT,
      viewportHeight,
      overscan: 12,
    });

  if (raw.status === 'too-large') {
    const shown = raw.size.complete
      ? `about ${formatSize(raw.size.bytes)}`
      : `more than ${formatSize(raw.size.bytes)}`;
    return (
      <div className="flex flex-col items-center gap-2 px-6 py-12 text-center">
        <FileJson className="size-6 text-muted-foreground" />
        <p className="text-sm font-medium">
          This {label.toLowerCase()} is too large to show as text
        </p>
        <p className="max-w-md text-sm text-muted-foreground">
          It is {shown}; the text view stops at {formatSize(RAW_VIEW_MAX_BYTES)}
          . Use the tree to browse it, or Copy JSON to get the whole value.
        </p>
      </div>
    );
  }

  const gutter = String(lines.length).length;
  return (
    <div
      ref={scrollRef}
      onScroll={onScroll}
      tabIndex={0}
      role="region"
      aria-label={`${label} as JSON`}
      className="overflow-auto overscroll-contain focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
      style={{ height: viewportHeight }}
    >
      <div
        style={{
          height: totalHeight,
          position: 'relative',
          minWidth: 'max-content',
        }}
      >
        <pre
          className="m-0 font-mono text-xs"
          style={{ transform: `translateY(${offsetTop}px)` }}
        >
          {lines.slice(start, end).map((line, i) => (
            <div
              key={start + i}
              className="flex pr-4"
              style={{ height: LINE_HEIGHT, lineHeight: `${LINE_HEIGHT}px` }}
            >
              <span
                aria-hidden
                className="sticky left-0 shrink-0 select-none bg-card pr-3 text-right text-muted-foreground/70"
                style={{ width: `${gutter + 3}ch` }}
              >
                {start + i + 1}
              </span>
              <span className="whitespace-pre">
                <Highlight text={line} query={query} />
              </span>
            </div>
          ))}
        </pre>
      </div>
    </div>
  );
}
