import type { ReactNode } from 'react';
import { Copy, Link2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  INSPECTOR_TEXT_LIMIT,
  describeValue,
  formatCount,
  formatSize,
  kindOf,
  plural,
  truncateText,
  type PathSegment,
  type TreeRow,
} from './model';

/** Strings longer than this get a readable preview below the path. */
const PREVIEW_MIN_CHARS = 80;

/**
 * Strip under the tree naming the selected field: its path as clickable
 * segments, its type and size, copy actions, and a preview of long text.
 */
export function PayloadInspector({
  compact = false,
  row,
  sectionLabel,
  onSelectPath,
  onCopyPath,
  onCopyValue,
}: {
  /** One line: path, type and icon copy buttons; no hint, no preview. */
  compact?: boolean;
  row: TreeRow | null;
  sectionLabel: string;
  onSelectPath: (segments: readonly PathSegment[]) => void;
  onCopyPath: (row: TreeRow) => void;
  onCopyValue: (row: TreeRow) => void;
}) {
  if (!row) {
    if (compact) return null;
    return (
      <p className="text-xs text-muted-foreground">
        Select a field to see its path. <Kbd>↑</Kbd>
        <Kbd>↓</Kbd> move, <Kbd>←</Kbd>
        <Kbd>→</Kbd> fold, <Kbd>C</Kbd> copy path, <Kbd>V</Kbd> copy value.
      </p>
    );
  }

  const crumbs: { label: string; segments: readonly PathSegment[] }[] = [];
  row.segments.forEach((segment, i) => {
    crumbs.push({
      label: typeof segment === 'number' ? `[${segment}]` : segment,
      segments: row.segments.slice(0, i + 1),
    });
  });
  const type =
    row.type === 'group' && row.range
      ? `${plural(row.childCount, 'item')} · ${formatCount(row.range[0])}–${formatCount(row.range[1] - 1)}`
      : describeValue(row.value);
  const truncatedStub =
    row.type === 'node' && kindOf(row.value) === 'truncated'
      ? (row.value as { _preview: string; _original_size: number })
      : null;
  const longText = truncatedStub
    ? truncatedStub._preview
    : row.type === 'node' &&
        typeof row.value === 'string' &&
        row.value.length > PREVIEW_MIN_CHARS
      ? row.value
      : null;
  const preview = longText
    ? truncateText(longText, INSPECTOR_TEXT_LIMIT)
    : null;

  return (
    <div className="grid gap-2">
      <div
        className={
          compact
            ? 'flex items-center gap-2'
            : 'flex flex-wrap items-center gap-x-3 gap-y-2'
        }
      >
        <nav
          aria-label="Selected path"
          className={
            compact
              ? 'flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto whitespace-nowrap font-mono text-xs [scrollbar-width:none]'
              : 'flex min-w-0 flex-1 flex-wrap items-center gap-0.5 font-mono text-xs'
          }
        >
          <CrumbButton onClick={() => onSelectPath([])}>
            {sectionLabel}
          </CrumbButton>
          {crumbs.map((crumb, i) => (
            <span key={i} className="flex items-center gap-0.5">
              <span aria-hidden className="text-muted-foreground/60">
                ›
              </span>
              <CrumbButton
                current={i === crumbs.length - 1 && row.type === 'node'}
                onClick={() => onSelectPath(crumb.segments)}
              >
                {truncateText(crumb.label, 40).text}
              </CrumbButton>
            </span>
          ))}
          {row.type === 'group' && row.range && (
            <span className="flex items-center gap-0.5">
              <span aria-hidden className="text-muted-foreground/60">
                ›
              </span>
              <span className="px-1 font-medium">
                [{formatCount(row.range[0])} … {formatCount(row.range[1] - 1)}]
              </span>
            </span>
          )}
        </nav>
        {!compact && (
          <span className="whitespace-nowrap rounded-full border px-2 py-0.5 text-[11px] text-muted-foreground">
            {type}
          </span>
        )}
        <div className="ml-auto flex shrink-0 gap-1.5">
          {compact ? (
            <>
              {row.path !== '' && (
                <CopyIcon label="Copy path" onClick={() => onCopyPath(row)}>
                  <Link2 className="size-3.5" />
                </CopyIcon>
              )}
              <CopyIcon label="Copy value" onClick={() => onCopyValue(row)}>
                <Copy className="size-3.5" />
              </CopyIcon>
            </>
          ) : (
            <>
              {row.path !== '' && (
                <Button
                  variant="secondary"
                  bordered
                  size="sm"
                  onClick={() => onCopyPath(row)}
                >
                  <Link2 className="size-3.5" />
                  Copy path
                </Button>
              )}
              <Button
                variant="secondary"
                bordered
                size="sm"
                onClick={() => onCopyValue(row)}
              >
                <Copy className="size-3.5" />
                Copy value
              </Button>
            </>
          )}
        </div>
      </div>
      {preview && !compact && (
        <div className="grid gap-1">
          <pre
            aria-label="Value preview"
            className="max-h-32 overflow-auto whitespace-pre-wrap break-all rounded-md bg-muted/50 p-2 font-mono text-xs"
          >
            {preview.text}
          </pre>
          {truncatedStub && (
            <p className="text-[11px] text-muted-foreground">
              The server kept only the first{' '}
              {formatCount(truncatedStub._preview.length)} characters of this
              value; it was {formatSize(truncatedStub._original_size)}.
            </p>
          )}
          {preview.truncated && (
            <p className="text-[11px] text-muted-foreground">
              Showing the first {formatCount(INSPECTOR_TEXT_LIMIT)} of{' '}
              {formatCount(longText!.length)} characters. Copy value gets all of
              it.
            </p>
          )}
        </div>
      )}
    </div>
  );
}

function CrumbButton({
  current = false,
  onClick,
  children,
}: {
  current?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={current ? 'location' : undefined}
      className={
        current
          ? 'rounded px-1 font-medium text-foreground hover:bg-primary/10'
          : 'rounded px-1 text-muted-foreground hover:bg-primary/10 hover:text-primary-text'
      }
    >
      {children}
    </button>
  );
}

function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="mx-0.5 rounded border border-b-2 px-1 font-mono text-[10px] text-muted-foreground">
      {children}
    </kbd>
  );
}

function CopyIcon({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="grid size-6 place-items-center rounded text-muted-foreground hover:bg-primary/10 hover:text-primary-text"
    >
      {children}
    </button>
  );
}
