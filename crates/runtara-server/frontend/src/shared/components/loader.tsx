import { Icons } from '@/shared/components/icons.tsx';
import { cn } from '@/lib/utils.ts';

export function Loader() {
  return (
    <div className="flex justify-center items-center h-screen">
      <Icons.spinner className="h-8 w-8 animate-spin text-primary" />
    </div>
  );
}

export function Loader2({ className }: { className?: string }) {
  return (
    <div className="flex justify-center items-center">
      <Icons.spinner
        className={cn('my-28 h-8 w-8 text-primary animate-spin', className)}
      />
    </div>
  );
}
