/* eslint-disable react-refresh/only-export-components */
// Component library pattern: exporting variant helpers with components
// for developer convenience (shadcn/ui standard pattern)
import * as React from 'react';
import { Slot } from '@radix-ui/react-slot';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '@/lib/utils.ts';

/**
 * Four action types, and nothing else:
 *
 *   primary               the one action the screen is for
 *   destructive           primary, and it destroys something
 *   secondary             any other action
 *   secondaryDestructive  a non-primary action that destroys something
 *
 * Whether a button carries a border is a separate question from what kind of
 * action it is — a table-row delete icon and a dialog's Cancel are both
 * secondary, but only one of them wants an outline. So `bordered` is its own
 * axis rather than a second family of variants.
 */
const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0',
  {
    variants: {
      variant: {
        // Filled actions get a little depth: a soft drop shadow and a top
        // highlight, so they read as pressable against flat secondary buttons.
        primary:
          'bg-primary text-primary-foreground shadow-[0_1px_2px_rgba(16,24,40,0.12),inset_0_1px_0_rgba(255,255,255,0.14)] hover:bg-primary/90',
        destructive:
          'bg-destructive text-destructive-foreground shadow-[0_1px_2px_rgba(16,24,40,0.12),inset_0_1px_0_rgba(255,255,255,0.14)] hover:bg-destructive/90',
        // Neutral, not blue: blue is kept for the primary action and for
        // links, so a screen's one primary button is the only blue control.
        secondary: 'text-foreground hover:bg-muted',
        secondaryDestructive:
          'text-destructive hover:bg-destructive/10 hover:text-destructive',
      },
      /** Outline the button. Only meaningful on the secondary types. */
      bordered: {
        true: 'border bg-background',
        false: '',
      },
      size: {
        default: 'h-8 px-3 py-1',
        sm: 'h-7 rounded-md px-2 text-xs',
        lg: 'h-9 rounded-md px-4',
        icon: 'size-8',
        'icon-sm': 'size-7',
      },
    },
    compoundVariants: [
      {
        variant: 'secondary',
        bordered: true,
        class: 'border-input shadow-[0_1px_1px_rgba(16,24,40,0.04)]',
      },
      // Icon-only secondaries are row and toolbar glyphs: quieter than a
      // labelled button until hovered.
      {
        variant: 'secondary',
        size: ['icon', 'icon-sm'],
        class: 'text-muted-foreground hover:text-foreground',
      },
      {
        variant: 'secondaryDestructive',
        bordered: true,
        class: 'border-destructive/40',
      },
    ],
    defaultVariants: {
      variant: 'primary',
      size: 'default',
      bordered: false,
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
  ({ className, variant, size, bordered, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : 'button';
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, bordered, className }))}
        ref={ref}
        {...props}
      />
    );
  }
);
Button.displayName = 'Button';

export { Button, buttonVariants };
