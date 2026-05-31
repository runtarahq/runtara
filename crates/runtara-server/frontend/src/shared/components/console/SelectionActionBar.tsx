import { type ReactNode } from 'react';
import { X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';

export interface SelectionActionBarProps {
  /** Number of selected rows. The bar slides in when greater than zero. */
  count: number;
  onClear: () => void;
  /** Bulk action controls (buttons) rendered after the count. */
  children?: ReactNode;
  className?: string;
}

/**
 * Bottom bulk-action bar that animates into view when rows are selected.
 * Mirrors the mockup `.actionbar`.
 */
export function SelectionActionBar({
  count,
  onClear,
  children,
  className,
}: SelectionActionBarProps) {
  const open = count > 0;
  return (
    <div
      aria-hidden={!open}
      className={cn(
        'flex shrink-0 items-center gap-3 overflow-hidden border-t bg-muted/50 px-4 transition-all duration-200 ease-out',
        open ? 'h-[52px] opacity-100' : 'pointer-events-none h-0 opacity-0',
        className
      )}
    >
      <span className="whitespace-nowrap text-sm text-muted-foreground">
        <b className="font-semibold text-foreground">{count}</b> selected
      </span>
      <div className="h-5 w-px shrink-0 bg-border" />
      <div className="flex items-center gap-2">{children}</div>
      <div className="flex-1" />
      <Button
        variant="ghost"
        size="sm"
        onClick={onClear}
        className="gap-1.5 text-muted-foreground hover:text-foreground"
      >
        <X className="h-4 w-4" />
        Clear
      </Button>
    </div>
  );
}
