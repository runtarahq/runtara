import * as React from 'react';

import { cn } from '@/lib/utils.ts';

type TableVariant = 'default' | 'nested' | 'console';

/**
 * Propagates the table variant to sub-components so a single
 * `<Table variant="console">` restyles its header/rows/cells without each
 * caller having to pass classes. "console" is the flat, borderless look from
 * the console mockup: no outer card/shadow, subtle row separators, a sticky
 * opaque header and airier padding.
 */
const TableVariantContext = React.createContext<TableVariant>('default');

const Table = React.forwardRef<
  HTMLTableElement,
  React.HTMLAttributes<HTMLTableElement> & { variant?: TableVariant }
>(({ className, variant = 'default', ...props }, ref) => {
  if (variant === 'console') {
    // No overflow wrapper: the nearest scroll ancestor (e.g. a
    // ConsoleTableShell body) is the scrollport, so a sticky <thead> sticks to
    // it. Sub-components read `variant` from context to apply console styling.
    // w-full by default so a console table fits its container and cells can
    // truncate. Wide tables that need horizontal scroll opt back in by passing
    // `className="min-w-max"`; fixed-layout tables pass `className="table-fixed"`.
    return (
      <TableVariantContext.Provider value="console">
        <table
          ref={ref}
          className={cn('w-full caption-bottom text-sm', className)}
          {...props}
        />
      </TableVariantContext.Provider>
    );
  }

  if (variant === 'nested') {
    return (
      <TableVariantContext.Provider value="nested">
        <div className="w-full">
          <div className="overflow-x-auto rounded-lg border shadow-sm">
            <table
              ref={ref}
              className={cn(
                'w-full min-w-max caption-bottom text-sm',
                className
              )}
              {...props}
            />
          </div>
        </div>
      </TableVariantContext.Provider>
    );
  }

  return (
    <div className="relative w-full overflow-auto">
      <table
        ref={ref}
        className={cn('w-full min-w-max caption-bottom text-sm', className)}
        {...props}
      />
    </div>
  );
});
Table.displayName = 'Table';

const TableHeader = React.forwardRef<
  HTMLTableSectionElement,
  React.HTMLAttributes<HTMLTableSectionElement>
>(({ className, ...props }, ref) => {
  const variant = React.useContext(TableVariantContext);
  return (
    <thead
      ref={ref}
      className={cn(
        variant === 'console' ? 'bg-transparent' : '[&_tr]:border-b bg-muted/50',
        className
      )}
      {...props}
    />
  );
});
TableHeader.displayName = 'TableHeader';

const TableBody = React.forwardRef<
  HTMLTableSectionElement,
  React.HTMLAttributes<HTMLTableSectionElement>
>(({ className, ...props }, ref) => (
  <tbody
    ref={ref}
    className={cn('[&_tr:last-child]:border-0', className)}
    {...props}
  />
));
TableBody.displayName = 'TableBody';

const TableFooter = React.forwardRef<
  HTMLTableSectionElement,
  React.HTMLAttributes<HTMLTableSectionElement>
>(({ className, ...props }, ref) => (
  <tfoot
    ref={ref}
    className={cn(
      'border-t bg-muted/50 font-medium [&>tr]:last:border-b-0',
      className
    )}
    {...props}
  />
));
TableFooter.displayName = 'TableFooter';

const TableRow = React.forwardRef<
  HTMLTableRowElement,
  React.HTMLAttributes<HTMLTableRowElement>
>(({ className, ...props }, ref) => {
  const isConsole = React.useContext(TableVariantContext) === 'console';
  return (
    <tr
      ref={ref}
      className={cn(
        'transition-colors',
        isConsole
          ? 'border-b border-border/50 hover:bg-muted/40 data-[state=selected]:bg-primary/10'
          : 'border-b hover:bg-muted/50 data-[state=selected]:bg-muted',
        className
      )}
      {...props}
    />
  );
});
TableRow.displayName = 'TableRow';

const TableHead = React.forwardRef<
  HTMLTableCellElement,
  React.ThHTMLAttributes<HTMLTableCellElement>
>(({ className, ...props }, ref) => {
  const isConsole = React.useContext(TableVariantContext) === 'console';
  return (
    <th
      ref={ref}
      className={cn(
        'text-left align-middle text-xs font-semibold uppercase tracking-wider text-muted-foreground',
        isConsole
          ? 'sticky top-0 z-20 border-b border-border bg-background px-5 py-3'
          : 'h-9 px-3',
        '[&:has([role=checkbox])]:pr-0 [&>[role=checkbox]]:translate-y-[2px]',
        className
      )}
      {...props}
    />
  );
});
TableHead.displayName = 'TableHead';

const TableCell = React.forwardRef<
  HTMLTableCellElement,
  React.TdHTMLAttributes<HTMLTableCellElement>
>(({ className, ...props }, ref) => {
  const isConsole = React.useContext(TableVariantContext) === 'console';
  return (
    <td
      ref={ref}
      className={cn(
        'align-middle [&:has([role=checkbox])]:pr-0 [&>[role=checkbox]]:translate-y-[2px]',
        isConsole ? 'px-5 py-3' : 'px-3 py-2',
        className
      )}
      {...props}
    />
  );
});
TableCell.displayName = 'TableCell';

const TableCaption = React.forwardRef<
  HTMLTableCaptionElement,
  React.HTMLAttributes<HTMLTableCaptionElement>
>(({ className, ...props }, ref) => (
  <caption
    ref={ref}
    className={cn('mt-4 text-sm text-muted-foreground', className)}
    {...props}
  />
));
TableCaption.displayName = 'TableCaption';

export {
  Table,
  TableHeader,
  TableBody,
  TableFooter,
  TableHead,
  TableRow,
  TableCell,
  TableCaption,
};
