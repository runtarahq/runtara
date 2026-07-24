import { type ElementType, type HTMLAttributes } from 'react';
import { cn } from '@/lib/utils';

export interface BlockFrameProps extends HTMLAttributes<HTMLElement> {
  as?: ElementType;
}

/**
 * The one card frame for report viewer blocks. Owns the elevation decision
 * (shadow-sm) so adjacent blocks render with the same depth — don't restate
 * `rounded-lg border bg-card …` per block type.
 */
export function BlockFrame({
  as: Tag = 'div',
  className,
  ...props
}: BlockFrameProps) {
  return (
    <Tag
      className={cn(
        'rounded-lg border bg-card text-card-foreground shadow-sm',
        className
      )}
      {...props}
    />
  );
}
