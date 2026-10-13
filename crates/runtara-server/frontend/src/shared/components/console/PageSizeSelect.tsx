import { ChevronDown } from 'lucide-react';
import { cn } from '@/lib/utils';
import { PAGE_SIZE_OPTIONS } from './page-size-options';

export interface PageSizeSelectProps {
  value: number;
  onChange: (pageSize: number) => void;
  options?: number[];
  className?: string;
}

/**
 * Native page-size select with its own chevron. The browser's arrow ignores
 * padding and sits against the right border, so it is hidden and drawn here
 * with the same inset as the left.
 */
export function PageSizeSelect({
  value,
  onChange,
  options = PAGE_SIZE_OPTIONS,
  className,
}: PageSizeSelectProps) {
  return (
    <span className="relative inline-flex">
      <select
        className={cn(
          'h-8 appearance-none rounded-md border bg-background pl-2.5 pr-8 text-[13px] text-foreground',
          className
        )}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        aria-label="Rows per page"
      >
        {options.map((size) => (
          <option key={size} value={size}>
            {size} / page
          </option>
        ))}
      </select>
      <ChevronDown
        aria-hidden="true"
        className="pointer-events-none absolute right-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
      />
    </span>
  );
}
