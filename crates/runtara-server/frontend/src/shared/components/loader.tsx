import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils.ts';

export function Loader() {
  return (
    <div className="flex h-screen items-center justify-center">
      <Icons.spinner className="h-8 w-8 animate-spin text-primary" />
    </div>
  );
}

export function Loader2({ className }: { className?: string }) {
  return (
    <div className="flex items-center justify-center">
      <Icons.spinner
        className={cn('my-28 h-8 w-8 animate-spin text-primary', className)}
      />
    </div>
  );
}
