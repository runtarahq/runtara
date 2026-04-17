import { Skeleton } from '@/shared/components/ui/skeleton.tsx';
import { range } from '@/lib/utils.ts';

export function SkeletonTable() {
  return (
    <div className="flex flex-col space-y-3">
      <Skeleton className="w-full h-10 rounded-sm" />
      {range(5).map((item) => (
        <Skeleton key={item} className="bg-card w-full h-12 rounded-sm" />
      ))}
    </div>
  );
}
