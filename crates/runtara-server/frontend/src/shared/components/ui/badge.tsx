import * as React from 'react';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '@/lib/utils.ts';

const badgeVariants = cva(
  'inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium transition-colors',
  {
    variants: {
      variant: {
        default: 'border border-primary/20 bg-primary/15 text-primary',
        secondary:
          'border border-secondary-foreground/20 bg-secondary text-secondary-foreground',
        destructive:
          'border border-destructive/20 bg-destructive/15 text-destructive',
        outline: 'border text-foreground',
        success: 'border border-success/20 bg-success/15 text-success',
        warning: 'border border-warning/20 bg-warning/15 text-warning',
        muted:
          'border border-muted-foreground/20 bg-muted text-muted-foreground',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  }
);

interface BadgeProps
  extends
    React.HTMLAttributes<HTMLDivElement>,
    VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return (
    <div className={cn(badgeVariants({ variant }), className)} {...props} />
  );
}

export { Badge };
