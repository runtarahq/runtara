import {
  ChevronFirst,
  ChevronLast,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { PAGE_SIZE_OPTIONS } from './page-size-options';
import { PageSizeSelect } from './PageSizeSelect';

export interface TablePaginationProps {
  pageIndex: number;
  pageSize: number;
  pageCount: number;
  onPageChange?: (page: number) => void;
  onPageSizeChange?: (pageSize: number) => void;
  pageSizeOptions?: number[];
  className?: string;
}

// Disabled is a surface of its own rather than a fade, so a dead button still
// reads as a button that is off, not as one that failed to render.
const navBtn =
  'grid size-8 place-items-center rounded-md border bg-background text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:bg-muted/40 disabled:text-muted-foreground/40';

/** Compact pagination control sized for the console status footer. */
export function TablePagination({
  pageIndex,
  pageSize,
  pageCount,
  onPageChange,
  onPageSizeChange,
  pageSizeOptions = PAGE_SIZE_OPTIONS,
  className,
}: TablePaginationProps) {
  const totalPages = Math.max(pageCount, 1);
  const canPrev = pageIndex > 0;
  const canNext = pageIndex < totalPages - 1;

  return (
    <div className={cn('flex items-center gap-3.5', className)}>
      {onPageSizeChange && (
        <PageSizeSelect
          value={pageSize}
          onChange={onPageSizeChange}
          options={pageSizeOptions}
        />
      )}
      <span className="tabular-nums">
        Page {pageIndex + 1} of {totalPages.toLocaleString()}
      </span>
      <div className="flex items-center gap-1">
        <button
          type="button"
          className={navBtn}
          disabled={!canPrev}
          onClick={() => onPageChange?.(0)}
          aria-label="First page"
        >
          <ChevronFirst className="size-4" />
        </button>
        <button
          type="button"
          className={navBtn}
          disabled={!canPrev}
          onClick={() => onPageChange?.(pageIndex - 1)}
          aria-label="Previous page"
        >
          <ChevronLeft className="size-4" />
        </button>
        <button
          type="button"
          className={navBtn}
          disabled={!canNext}
          onClick={() => onPageChange?.(pageIndex + 1)}
          aria-label="Next page"
        >
          <ChevronRight className="size-4" />
        </button>
        <button
          type="button"
          className={navBtn}
          disabled={!canNext}
          onClick={() => onPageChange?.(totalPages - 1)}
          aria-label="Last page"
        >
          <ChevronLast className="size-4" />
        </button>
      </div>
    </div>
  );
}
