import { type ThHTMLAttributes } from 'react';
import { cn } from '@/lib/utils';

/**
 * Compact table header for workflow-editor sidebars/forms (variables,
 * schema fields, switch cases…). The header-cell recipe lives here
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
