import { type ReactNode } from 'react';
import { cn } from '@/lib/utils';

export interface TableStatusFooterProps {
  /** Left slot — typically a row count summary. */
  left?: ReactNode;
  /** Right slot — context label or pagination control. */
  right?: ReactNode;
  className?: string;
}

/** Slim status footer pinned below the table. Mirrors the mockup `.footer`. */
export function TableStatusFooter({
  left,
  right,
  className,
}: TableStatusFooterProps) {
  return (
    <div
      className={cn(
        'flex h-12 shrink-0 items-center gap-4 border-t bg-table-head px-4 text-[13px] tabular-nums text-muted-foreground md:px-7',
        className
      )}
    >
      <div className="flex min-w-0 items-center gap-3 truncate">{left}</div>
      <div className="flex-1" />
      <div className="flex shrink-0 items-center gap-3.5">{right}</div>
    </div>
  );
}
