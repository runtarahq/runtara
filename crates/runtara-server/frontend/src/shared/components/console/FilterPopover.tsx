import { type ReactNode, useState } from 'react';
import { Filter } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';

export interface FilterPopoverProps {
  /** Number of active filters — drives the badge + active styling. */
  activeCount?: number;
  /** Clears all filters (shown as "Clear all" in the header when active). */
  onClear?: () => void;
  /** Popover header label. */
  title?: string;
  /** The filter controls. */
  children: ReactNode;
  align?: 'start' | 'center' | 'end';
  /** Width/util classes for the popover body. */
  contentClassName?: string;
}

/**
 * Linear-style filter affordance: an icon-only trigger that opens a popover
 * holding the page's filter controls, with a count badge when filters are
 * active. Keeps verbose filter rows out of the toolbar. Drop into the
 * `filter` slot of <ConsoleToolbar />.
 */
export function FilterPopover({
  activeCount = 0,
  onClear,
  title = 'Filters',
  children,
  align = 'end',
  contentClassName,
}: FilterPopoverProps) {
  const [open, setOpen] = useState(false);
  const active = activeCount > 0;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          size="icon"
          aria-label={`${title}${active ? ` (${activeCount} active)` : ''}`}
          className={cn(
            'relative h-9 w-9 shrink-0',
            active && 'border-primary/40 text-primary'
          )}
        >
          <Filter className="h-4 w-4" />
          {active && (
            <span className="absolute -right-1.5 -top-1.5 flex h-4 min-w-[16px] items-center justify-center rounded-full bg-primary px-1 text-[10px] font-semibold leading-none text-primary-foreground">
              {activeCount}
            </span>
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align={align}
        className={cn('w-80 p-0', contentClassName)}
        onInteractOutside={(e) => {
          // Keep the popover open when interacting with a nested popper
          // (a Radix Select/Dropdown dropdown portals outside this content).
          const target = e.target as HTMLElement | null;
          if (
            target?.closest('[data-radix-popper-content-wrapper]') ||
            target?.closest('[role="listbox"]')
          ) {
            e.preventDefault();
          }
        }}
      >
        <div className="flex items-center justify-between border-b px-3 py-2">
          <span className="text-sm font-medium">{title}</span>
          {active && onClear && (
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-muted-foreground hover:text-foreground"
              onClick={onClear}
            >
              Clear all
            </Button>
          )}
        </div>
        <div className="max-h-[min(70vh,520px)] overflow-y-auto p-3">
          {children}
        </div>
      </PopoverContent>
    </Popover>
  );
}
