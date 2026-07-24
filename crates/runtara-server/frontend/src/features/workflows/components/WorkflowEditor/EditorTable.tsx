import { type HTMLAttributes, type ThHTMLAttributes } from 'react';
import { cn } from '@/lib/utils';

/**
 * Compact table primitives for workflow-editor sidebars/forms (variables,
 * schema fields, switch cases…). The header-cell and row recipes live here
 * once — don't retype `p-2 text-left text-sm font-medium text-muted-foreground`
 * per editor.
 */

export function EditorTh({
  className,
  ...props
}: ThHTMLAttributes<HTMLTableCellElement>) {
  return (
    <th
      className={cn(
        'p-2 text-left text-sm font-medium text-muted-foreground',
        className
      )}
      {...props}
    />
  );
}

export function EditorRow({
  className,
  ...props
}: HTMLAttributes<HTMLTableRowElement>) {
  return (
    <tr className={cn('border-b hover:bg-muted/30', className)} {...props} />
  );
}
