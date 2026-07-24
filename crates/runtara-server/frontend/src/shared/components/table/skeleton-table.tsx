import { Skeleton } from '@/shared/components/ui/skeleton.tsx';
import { range } from '@/lib/utils.ts';

export function SkeletonTable() {
  return (
    <div className="flex flex-col space-y-3">
      <Skeleton className="h-10 w-full rounded-sm" />
      {range(5).map((item) => (
        <Skeleton key={item} className="h-12 w-full rounded-sm bg-card" />
      ))}
    </div>
  );
}
