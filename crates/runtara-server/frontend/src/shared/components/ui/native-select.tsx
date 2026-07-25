// Styled native <select> sharing the Input skin (focus-visible ring +
// disabled styles), with a lucide chevron replacing the platform arrow.
import * as React from 'react';
import { ChevronDown } from 'lucide-react';

import { cn } from '@/lib/utils.ts';

const NativeSelect = React.forwardRef<
  HTMLSelectElement,
  React.ComponentProps<'select'>
>(({ className, children, ...props }, ref) => {
  return (
    <div className="relative">
      <select
        className={cn(
          'h-8 w-full appearance-none rounded-md border border-input bg-background px-3 py-1 pr-8 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50',
          className
        )}
        ref={ref}
        {...props}
      >
        {children}
      </select>
      <ChevronDown className="pointer-events-none absolute right-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
    </div>
  );
});
NativeSelect.displayName = 'NativeSelect';

export { NativeSelect };
