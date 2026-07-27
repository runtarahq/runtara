/* eslint-disable react-refresh/only-export-components */
// Component library pattern: exporting variant helpers with components
// for developer convenience (shadcn/ui standard pattern)
import * as React from 'react';
import { Slot } from '@radix-ui/react-slot';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '@/lib/utils.ts';

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        default: 'bg-primary text-primary-foreground hover:bg-primary/90',
        destructive:
          'bg-destructive text-destructive-foreground hover:bg-destructive/90',
        // Non-primary actions label themselves in the brand colour rather than
        // the default foreground. text-primary-text, not text-primary: the
        // latter is 3.64:1 on white, under the AA bar for text this size.
        outline:
          'border border-input bg-background text-primary-text hover:bg-accent hover:text-accent-foreground',
        secondary:
          'bg-secondary text-secondary-foreground hover:bg-secondary/80',
        ghost: 'text-primary-text hover:bg-accent hover:text-accent-foreground',
        link: 'text-primary-text underline-offset-4 hover:underline',
        // Low-emphasis chrome — row actions, toolbar icons, dismissals. Named
        // rather than spelled out at each call site: 34 of them had written
        // this pair by hand, which is how they drifted out of step with the
        // variants in the first place.
        quiet: 'text-muted-foreground hover:bg-accent hover:text-foreground',
        // The bordered form of `quiet` — the timeline's dashed add-controls,
        // filter and range pickers. Deliberately not brand-coloured: these
        // repeat many times per screen and are scaffolding, not the action.
        quietOutline:
          'border border-input bg-background text-muted-foreground hover:bg-accent hover:text-foreground',
        // Quiet until you reach for it, then it says what it will do. The
        // usual shape for a delete icon in a dense table.
        quietDestructive:
          'text-muted-foreground hover:bg-destructive/10 hover:text-destructive',
        // A destructive action that does not warrant the solid fill. Note the
        // explicit hover text: call sites that wrote `text-destructive` on a
        // ghost button kept ghost's `hover:text-accent-foreground`, so they
        // turned cyan on hover.
        destructiveGhost:
          'text-destructive hover:bg-destructive/10 hover:text-destructive',
      },
      size: {
        default: 'h-8 px-3 py-1',
        sm: 'h-7 rounded-md px-2 text-xs',
        lg: 'h-9 rounded-md px-4',
        icon: 'size-8',
        'icon-sm': 'size-7',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  }
);

interface ButtonProps
  extends
    React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : 'button';
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, className }))}
        ref={ref}
        {...props}
      />
    );
  }
);
Button.displayName = 'Button';

export { Button, buttonVariants };
