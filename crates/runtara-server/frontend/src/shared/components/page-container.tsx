import { type HTMLAttributes } from 'react';
import { cn } from '@/lib/utils';

/**
 * Standard page frame for form-style pages (create/edit flows). Keeps the
 * console gutter defined once instead of per page.
 */
export function PageContainer({
  className,
  ...props
}: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn('w-full px-4 py-6 sm:px-6 lg:px-10', className)}
      {...props}
    />
  );
}
