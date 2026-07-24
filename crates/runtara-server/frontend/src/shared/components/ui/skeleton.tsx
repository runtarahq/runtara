import { cn } from '@/lib/utils.ts';

function Skeleton({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  // bg-muted/60 + rounded matches the de-facto skeleton look the console list
  // pages established; keep every skeleton on this one recipe.
  return (
    <div
      className={cn('animate-pulse rounded bg-muted/60', className)}
      {...props}
    />
  );
}

export { Skeleton };
