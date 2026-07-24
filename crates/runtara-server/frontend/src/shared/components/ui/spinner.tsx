import { Loader2 } from 'lucide-react';
import { cn } from '@/lib/utils.ts';

/**
 * Canonical inline loading spinner. Defaults to the 16px icon size; override
 * size/margins via className (cn/twMerge resolves conflicts), e.g.
 * `<Spinner className="mr-2" />` inside a submit button or
 * `<Spinner className="h-8 w-8 text-muted-foreground" />` for a pane loader.
 */
export function Spinner({ className }: { className?: string }) {
  return (
    <Loader2
      aria-hidden="true"
      className={cn('h-4 w-4 animate-spin', className)}
    />
  );
}
