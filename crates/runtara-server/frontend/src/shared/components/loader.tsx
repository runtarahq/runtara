import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils.ts';

export function Loader() {
  return (
    <div className="flex h-screen items-center justify-center">
      <Icons.spinner className="h-8 w-8 animate-spin text-primary" />
    </div>
  );
}

/**
 * Centered block loader for page/panel bodies. (Formerly exported as
 * `Loader2`, which collided with the lucide-react icon of the same name —
 * use `Spinner` from ui/spinner for inline spinners.)
 */
export function PageLoader({ className }: { className?: string }) {
  return (
    <div className="flex items-center justify-center">
      <Icons.spinner
        className={cn('my-28 h-8 w-8 animate-spin text-primary', className)}
      />
    </div>
  );
}
