import {
  ChevronFirst,
  ChevronLast,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import { cn } from '@/lib/utils';

const PAGE_SIZE_OPTIONS = [10, 20, 50, 100];

export interface TablePaginationProps {
  pageIndex: number;
  pageSize: number;
  pageCount: number;
  onPageChange?: (page: number) => void;
  onPageSizeChange?: (pageSize: number) => void;
  pageSizeOptions?: number[];
  className?: string;
}

const navBtn =
  'grid h-6 w-6 place-items-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-foreground';

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
    <div className={cn('flex items-center gap-3', className)}>
      {onPageSizeChange && (
        <select
          className="h-6 rounded border bg-background px-1.5 text-xs text-foreground"
          value={pageSize}
          onChange={(e) => onPageSizeChange(Number(e.target.value))}
          aria-label="Rows per page"
        >
          {pageSizeOptions.map((size) => (
            <option key={size} value={size}>
              {size} / page
            </option>
          ))}
        </select>
      )}
      <span className="tabular-nums">
        Page {pageIndex + 1} of {totalPages.toLocaleString()}
      </span>
      <div className="flex items-center gap-0.5">
        <button
          type="button"
          className={navBtn}
          disabled={!canPrev}
          onClick={() => onPageChange?.(0)}
          aria-label="First page"
        >
          <ChevronFirst className="h-4 w-4" />
        </button>
        <button
          type="button"
          className={navBtn}
          disabled={!canPrev}
          onClick={() => onPageChange?.(pageIndex - 1)}
          aria-label="Previous page"
        >
          <ChevronLeft className="h-4 w-4" />
        </button>
        <button
          type="button"
          className={navBtn}
          disabled={!canNext}
          onClick={() => onPageChange?.(pageIndex + 1)}
          aria-label="Next page"
        >
          <ChevronRight className="h-4 w-4" />
        </button>
        <button
          type="button"
          className={navBtn}
          disabled={!canNext}
          onClick={() => onPageChange?.(totalPages - 1)}
          aria-label="Last page"
        >
          <ChevronLast className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}
