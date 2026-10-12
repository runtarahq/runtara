import { Icons } from '@/shared/components/icons';
import { cn } from '@/lib/utils';

/**
 * The value cell of an object or list mapping whose editor opens in the row
 * below. One control for every such value: Headers (a typed object) and Body
 * (a string field holding a built object) used to render as two different
 * widgets — a summary button and a green "configure below" banner — for the
 * same kind of value and the same editor.
 *
 * Without `onToggle` it is a static indicator, for callers whose editor is
 * always shown.
 */
export function StructuredValueSummary({
  kind,
  summary,
  expanded,
  onToggle,
  className,
}: {
  kind: 'object' | 'array';
  summary: string;
  expanded?: boolean;
  onToggle?: () => void;
  className?: string;
}) {
  const KindIcon = kind === 'array' ? Icons.list : Icons.braces;
  const Chevron = expanded ? Icons.chevronDown : Icons.chevronRight;
  const classes = cn(
    'flex h-9 w-full min-w-0 items-center gap-2 rounded-md border bg-muted/30 px-3 text-left text-sm',
    onToggle && 'transition-colors hover:bg-muted/50',
    className
  );
  const content = (
    <>
      <KindIcon
        className="size-4 shrink-0 text-muted-foreground"
        aria-hidden="true"
      />
      <span className="min-w-0 flex-1 truncate">{summary}</span>
      {onToggle && (
        <Chevron
          className="size-4 shrink-0 text-muted-foreground"
          aria-hidden="true"
        />
      )}
    </>
  );

  if (!onToggle) return <div className={classes}>{content}</div>;
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={expanded}
      className={classes}
    >
      {content}
    </button>
  );
}
