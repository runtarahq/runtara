import { memo, useEffect, type KeyboardEvent, type ReactNode } from 'react';
import { ChevronRight, Copy, Link2, Loader2 } from 'lucide-react';
import { cn, formatRelativeTime } from '@/lib/utils';
import { Highlight } from './Highlight';
import {
  ROW_TEXT_LIMIT,
  branchPreview,
  formatCount,
  formatSize,
  plural,
  truncateText,
  type TreeRow,
} from './model';
import { useVirtualWindow } from './useVirtualWindow';

export const ROW_HEIGHT = 28;
const INDENT_PX = 16;
const ISO_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/;

export interface FieldLabel {
  label?: string;
  description?: string;
}

export interface PayloadTreeProps {
  rows: TreeRow[];
  /** Id prefix for row elements (aria-activedescendant). */
  idPrefix: string;
  ariaLabel: string;
  maxHeight: number;
  selectedId: string | null;
  query: string;
  matchedPaths: ReadonlySet<string>;
  currentMatchPath: string | null;
  /** Labels for top-level keys (e.g. from a state schema). */
  fieldLabels?: Record<string, FieldLabel>;
  canLoadFull: boolean;
  loadingFull: boolean;
  onSelect: (id: string) => void;
  onToggle: (row: TreeRow) => void;
  onCopyPath: (row: TreeRow) => void;
  onCopyValue: (row: TreeRow) => void;
  onLoadFull: () => void;
}

/** A virtualized, keyboard-driven tree of payload rows. */
export function PayloadTree({
  rows,
  idPrefix,
  ariaLabel,
  maxHeight,
  selectedId,
  query,
  matchedPaths,
  currentMatchPath,
  fieldLabels,
  canLoadFull,
  loadingFull,
  onSelect,
  onToggle,
  onCopyPath,
  onCopyValue,
  onLoadFull,
}: PayloadTreeProps) {
  const viewportHeight = Math.min(
    maxHeight,
    Math.max(rows.length, 1) * ROW_HEIGHT
  );
  const {
    scrollRef,
    onScroll,
    scrollToIndex,
    start,
    end,
    totalHeight,
    offsetTop,
  } = useVirtualWindow({
    count: rows.length,
    rowHeight: ROW_HEIGHT,
    viewportHeight,
  });

  const selectedIndex =
    selectedId === null ? -1 : rows.findIndex((r) => r.id === selectedId);

  // Keep the selected row in view when it moves (keyboard, search, folding).
  useEffect(() => {
    if (selectedIndex >= 0) scrollToIndex(selectedIndex);
  }, [selectedIndex, scrollToIndex]);

  const select = (index: number) => {
    const row = rows[Math.min(Math.max(index, 0), rows.length - 1)];
    if (row) onSelect(row.id);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!rows.length || event.altKey || event.ctrlKey || event.metaKey) return;
    const row = selectedIndex >= 0 ? rows[selectedIndex] : null;
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        select(selectedIndex + 1);
        break;
      case 'ArrowUp':
        event.preventDefault();
        select(selectedIndex < 0 ? 0 : selectedIndex - 1);
        break;
      case 'Home':
        event.preventDefault();
        select(0);
        break;
      case 'End':
        event.preventDefault();
        select(rows.length - 1);
        break;
      case 'ArrowRight':
        event.preventDefault();
        if (!row) select(0);
        else if (row.expandable && !row.expanded) onToggle(row);
        else if (row.expanded) select(selectedIndex + 1);
        break;
      case 'ArrowLeft': {
        event.preventDefault();
        if (!row) break;
        if (row.expanded) {
          onToggle(row);
          break;
        }
        for (let i = selectedIndex - 1; i >= 0; i--) {
          if (rows[i].depth < row.depth) {
            select(i);
            break;
          }
        }
        break;
      }
      case 'Enter':
      case ' ':
        if (row?.expandable) {
          event.preventDefault();
          onToggle(row);
        }
        break;
      case 'c':
        if (row) onCopyPath(row);
        break;
      case 'v':
        if (row) onCopyValue(row);
        break;
      default:
        break;
    }
  };

  return (
    <div
      ref={scrollRef}
      role="tree"
      aria-label={ariaLabel}
      tabIndex={0}
      aria-activedescendant={
        selectedIndex >= 0 ? `${idPrefix}-${selectedIndex}` : undefined
      }
      onScroll={onScroll}
      onKeyDown={onKeyDown}
      onFocus={() => {
        if (selectedIndex < 0 && rows.length) onSelect(rows[0].id);
      }}
      className="overflow-auto overscroll-contain focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
      style={{ height: viewportHeight }}
    >
      <div style={{ height: totalHeight, position: 'relative' }}>
        <div style={{ transform: `translateY(${offsetTop}px)` }}>
          {rows.slice(start, end).map((row, i) => (
            <PayloadRow
              key={row.id}
              row={row}
              domId={`${idPrefix}-${start + i}`}
              selected={row.id === selectedId}
              query={query}
              isMatch={row.type === 'node' && matchedPaths.has(row.path)}
              isCurrentMatch={
                row.type === 'node' && row.path === currentMatchPath
              }
              fieldLabel={
                row.depth === 0 && typeof row.label === 'string'
                  ? fieldLabels?.[row.label]
                  : undefined
              }
              canLoadFull={canLoadFull}
              loadingFull={loadingFull}
              onSelect={onSelect}
              onToggle={onToggle}
              onCopyPath={onCopyPath}
              onCopyValue={onCopyValue}
              onLoadFull={onLoadFull}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

interface PayloadRowProps {
  row: TreeRow;
  domId: string;
  selected: boolean;
  query: string;
  isMatch: boolean;
  isCurrentMatch: boolean;
  fieldLabel?: FieldLabel;
  canLoadFull: boolean;
  loadingFull: boolean;
  onSelect: (id: string) => void;
  onToggle: (row: TreeRow) => void;
  onCopyPath: (row: TreeRow) => void;
  onCopyValue: (row: TreeRow) => void;
  onLoadFull: () => void;
}

const PayloadRow = memo(function PayloadRow({
  row,
  domId,
  selected,
  query,
  isMatch,
  isCurrentMatch,
  fieldLabel,
  canLoadFull,
  loadingFull,
  onSelect,
  onToggle,
  onCopyPath,
  onCopyValue,
  onLoadFull,
}: PayloadRowProps) {
  const highlightQuery = isMatch ? query : '';
  return (
    <div
      id={domId}
      role="treeitem"
      aria-level={row.depth + 1}
      aria-selected={selected}
      aria-expanded={row.expandable ? row.expanded : undefined}
      data-row-id={row.id}
      className={cn(
        'group/row relative flex cursor-default items-center gap-1 whitespace-nowrap pr-2 font-mono text-xs',
        selected ? 'bg-primary/10' : 'hover:bg-muted/60'
      )}
      style={{ height: ROW_HEIGHT, paddingLeft: 6 + row.depth * INDENT_PX }}
      onClick={() => onSelect(row.id)}
      onDoubleClick={() => row.expandable && onToggle(row)}
    >
      {selected && (
        <span
          aria-hidden
          className="absolute inset-y-0 left-0 w-0.5 bg-primary"
        />
      )}
      {row.expandable ? (
        <button
          type="button"
          tabIndex={-1}
          aria-label={row.expanded ? 'Collapse' : 'Expand'}
          className="grid size-5 shrink-0 place-items-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
          onClick={(event) => {
            event.stopPropagation();
            onSelect(row.id);
            onToggle(row);
          }}
        >
          <ChevronRight
            className={cn(
              'size-3.5 transition-transform motion-reduce:transition-none',
              row.expanded && 'rotate-90'
            )}
          />
        </button>
      ) : (
        <span aria-hidden className="w-5 shrink-0" />
      )}

      <RowLabel row={row} query={highlightQuery} current={isCurrentMatch} />
      {fieldLabel?.label && fieldLabel.label !== row.label && (
        <span
          className="shrink-0 font-sans text-[11px] text-muted-foreground"
          title={fieldLabel.description}
        >
          {fieldLabel.label}
        </span>
      )}

      <span className="min-w-0 truncate">
        <RowValue
          row={row}
          query={highlightQuery}
          current={isCurrentMatch}
          canLoadFull={canLoadFull}
          loadingFull={loadingFull}
          onLoadFull={onLoadFull}
        />
      </span>

      <span
        className={cn(
          'ml-auto flex shrink-0 gap-0.5 pl-3',
          selected ? 'visible' : 'invisible group-hover/row:visible'
        )}
      >
        {row.path !== '' && (
          <RowAction label="Copy path" onClick={() => onCopyPath(row)}>
            <Link2 className="size-3.5" />
          </RowAction>
        )}
        <RowAction label="Copy value" onClick={() => onCopyValue(row)}>
          <Copy className="size-3.5" />
        </RowAction>
      </span>
    </div>
  );
});

function RowAction({
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
      tabIndex={-1}
      aria-label={label}
      title={label}
      className="grid size-6 place-items-center rounded text-muted-foreground hover:bg-primary/10 hover:text-primary-text"
      onClick={(event) => {
        event.stopPropagation();
        onClick();
      }}
    >
      {children}
    </button>
  );
}

function RowLabel({
  row,
  query,
  current,
}: {
  row: TreeRow;
  query: string;
  current: boolean;
}) {
  if (row.type === 'group' && row.range) {
    const [from, to] = row.range;
    return (
      <span className="shrink-0 text-muted-foreground">
        {row.kind === 'array'
          ? `[${formatCount(from)} … ${formatCount(to - 1)}]`
          : `keys ${formatCount(from + 1)} … ${formatCount(to)}`}
      </span>
    );
  }
  if (row.label === null) return null;
  if (typeof row.label === 'number') {
    return (
      <>
        <span className="shrink-0 text-muted-foreground">{row.label}</span>
        <span className="shrink-0 text-muted-foreground/70">:</span>
      </>
    );
  }
  const { text, truncated } = truncateText(row.label, 80);
  return (
    <>
      <span
        className="max-w-[40%] shrink-0 truncate font-medium text-foreground"
        title={truncated ? row.label : undefined}
      >
        <Highlight text={text} query={query} current={current} />
      </span>
      <span className="shrink-0 text-muted-foreground/70">:</span>
    </>
  );
}

function RowValue({
  row,
  query,
  current,
  canLoadFull,
  loadingFull,
  onLoadFull,
}: {
  row: TreeRow;
  query: string;
  current: boolean;
  canLoadFull: boolean;
  loadingFull: boolean;
  onLoadFull: () => void;
}) {
  if (row.type === 'group') {
    return (
      <span className="text-muted-foreground">
        {plural(row.childCount, 'item')}
      </span>
    );
  }
  const { value } = row;
  switch (row.kind) {
    case 'object':
    case 'array': {
      const count =
        row.kind === 'array'
          ? plural(row.childCount, 'item')
          : plural(row.childCount, 'key');
      if (row.childCount === 0) {
        return (
          <span className="text-muted-foreground">
            {row.kind === 'array' ? '[ ]' : '{ }'}
          </span>
        );
      }
      return (
        <span className="text-muted-foreground">
          {!row.expanded && (
            <span className="mr-1.5">{branchPreview(value)}</span>
          )}
          <span className="font-sans text-[11px] text-muted-foreground/80">
            {count}
          </span>
        </span>
      );
    }
    case 'string': {
      const full = value as string;
      const { text, truncated } = truncateText(full, ROW_TEXT_LIMIT);
      const time =
        ISO_TIMESTAMP.test(full) && !Number.isNaN(Date.parse(full))
          ? formatRelativeTime(full)
          : null;
      return (
        <>
          <span className="text-success">
            "<Highlight text={text} query={query} current={current} />
            {truncated ? '…' : '"'}
          </span>
          {truncated && (
            <span className="ml-2 font-sans text-[11px] text-muted-foreground">
              {formatCount(full.length)} chars
            </span>
          )}
          {time && (
            <span className="ml-2 font-sans text-[11px] text-muted-foreground">
              {time}
            </span>
          )}
        </>
      );
    }
    case 'number':
      return (
        <span className="text-info">
          <Highlight text={String(value)} query={query} current={current} />
        </span>
      );
    case 'boolean':
      return (
        <span className="text-purple-600 dark:text-purple-400">
          <Highlight text={String(value)} query={query} current={current} />
        </span>
      );
    case 'null':
      return <span className="italic text-muted-foreground">null</span>;
    case 'elided': {
      const size = (value as { _original_size: number })._original_size;
      return (
        <span className="inline-flex items-center gap-2 rounded-full bg-warning/10 py-0.5 pl-2.5 pr-1 font-sans text-[11px] text-warning">
          Large value · {formatSize(size)}
          {canLoadFull && (
            <button
              type="button"
              className="inline-flex items-center gap-1 rounded-full bg-background px-2 py-0.5 font-medium text-foreground hover:text-primary-text disabled:opacity-60"
              disabled={loadingFull}
              onClick={(event) => {
                event.stopPropagation();
                onLoadFull();
              }}
            >
              {loadingFull && <Loader2 className="size-3 animate-spin" />}
              Load full value
            </button>
          )}
        </span>
      );
    }
    case 'truncated': {
      const stub = value as { _original_size: number; _preview: string };
      const { text } = truncateText(stub._preview, ROW_TEXT_LIMIT);
      return (
        <>
          <span
            className="mr-2 inline-flex items-center rounded-full bg-warning/10 px-2 py-0.5 font-sans text-[11px] text-warning"
            title="The server stored only the beginning of this value"
          >
            Cut off · {formatSize(stub._original_size)}
          </span>
          <span className="text-muted-foreground">
            <Highlight text={text} query={query} current={current} />…
          </span>
        </>
      );
    }
    default:
      return <span className="text-muted-foreground">{String(value)}</span>;
  }
}
