import * as React from 'react';
import { cn } from '@/lib/utils';

export interface ConsoleTableShellProps
  extends React.HTMLAttributes<HTMLDivElement> {
  /** Pinned toolbar row(s) at the top. */
  toolbar?: React.ReactNode;
  /** Bottom selection / bulk-action bar (e.g. <SelectionActionBar />). */
  selectionBar?: React.ReactNode;
  /** Pinned status footer (e.g. <TableStatusFooter />). */
  footer?: React.ReactNode;
  /** Extra classes for the scrolling body that holds the table. */
  bodyClassName?: string;
}

/**
 * Full-height flex shell for a console table page: a pinned toolbar, a single
 * scrolling body (the only scroll container, so a sticky table header sticks to
 * it), then an optional selection bar and status footer pinned to the bottom.
 * Mirrors the mockup `.content` column.
 *
 * Extra props (ref, data-*, onKeyDown, tabIndex…) are forwarded to the root so
 * callers can keep behaviors like click-outside detection or keyboard handlers.
 */
export const ConsoleTableShell = React.forwardRef<
  HTMLDivElement,
  ConsoleTableShellProps
>(function ConsoleTableShell(
  { toolbar, selectionBar, footer, children, className, bodyClassName, ...rest },
  ref
) {
  return (
    <div
      ref={ref}
      className={cn(
        'flex h-dvh max-h-dvh flex-col overflow-hidden bg-background',
        className
      )}
      {...rest}
    >
      {toolbar}
      <div className={cn('min-h-0 flex-1 overflow-auto', bodyClassName)}>
        {children}
      </div>
      {selectionBar}
      {footer}
    </div>
  );
});
