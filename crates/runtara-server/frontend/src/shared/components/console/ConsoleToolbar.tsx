import { type ReactNode, useEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import { ChevronRight, Search, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';

export interface BreadcrumbItem {
  label: ReactNode;
  /** Navigate via router link. */
  to?: string;
  /** Navigate via a custom handler (e.g. folder param updates). */
  onClick?: () => void;
}

/** Path breadcrumb for the toolbar. The last item renders as the current page. */
export function Breadcrumb({
  items,
  className,
}: {
  items: BreadcrumbItem[];
  className?: string;
}) {
  return (
    <nav
      aria-label="Breadcrumb"
      className={cn(
        'flex min-w-0 items-center gap-1 text-sm text-muted-foreground',
        className
      )}
    >
      {items.map((item, i) => {
        const last = i === items.length - 1;
        return (
          <span key={i} className="flex min-w-0 items-center gap-1">
            {!last && item.to ? (
              <Link
                to={item.to}
                className="truncate rounded px-1 py-0.5 hover:bg-muted hover:text-foreground"
              >
                {item.label}
              </Link>
            ) : !last && item.onClick ? (
              <button
                type="button"
                onClick={item.onClick}
                className="truncate rounded px-1 py-0.5 hover:bg-muted hover:text-foreground"
              >
                {item.label}
              </button>
            ) : (
              <span
                className={cn(
                  'truncate px-1 py-0.5',
                  last && 'font-medium text-foreground'
                )}
              >
                {item.label}
              </span>
            )}
            {!last && (
              <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground/60" />
            )}
          </span>
        );
      })}
    </nav>
  );
}

export interface ToolbarSearchProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  className?: string;
}

/**
 * Search that collapses to an icon-only button and expands into a field on
 * click or ⌘/Ctrl+F. Stays expanded while a query is active so it's visible;
 * collapses back to the icon when cleared.
 */
export function ToolbarSearch({
  value,
  onChange,
  placeholder = 'Search…',
  className,
}: ToolbarSearchProps) {
  const [open, setOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const hasValue = value.trim().length > 0;
  const expanded = open || hasValue;

  const openSearch = () => {
    setOpen(true);
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  // ⌘F / Ctrl+F opens the search instead of the browser's find.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === 'f' || e.key === 'F')) {
        e.preventDefault();
        openSearch();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  if (!expanded) {
    return (
      <Button
        variant="outline"
        size="icon"
        className="h-9 w-9 shrink-0"
        aria-label="Search (⌘F)"
        onClick={openSearch}
      >
        <Search className="h-4 w-4" />
      </Button>
    );
  }

  return (
    <div
      className={cn(
        'flex h-9 items-center gap-2 rounded-md border bg-muted/40 px-2.5',
        className
      )}
    >
      <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            onChange('');
            setOpen(false);
          }
        }}
        onBlur={() => setOpen(false)}
        placeholder={placeholder}
        className="w-full min-w-0 bg-transparent text-sm text-foreground outline-none placeholder:text-muted-foreground"
      />
      <button
        type="button"
        aria-label="Clear search"
        className="shrink-0 text-muted-foreground hover:text-foreground"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => {
          onChange('');
          setOpen(false);
        }}
      >
        <X className="h-4 w-4" />
      </button>
    </div>
  );
}

export interface ConsoleToolbarProps {
  /** Left slot — typically a <Breadcrumb /> or a title. */
  left?: ReactNode;
  /** Optional search control (e.g. <ToolbarSearch />). */
  search?: ReactNode;
  /** Optional filter control(s). */
  filter?: ReactNode;
  /** Primary / trailing actions. */
  actions?: ReactNode;
  className?: string;
}

/**
 * Pinned content-area toolbar: breadcrumb/title on the left, then search,
 * filter and primary actions on the right. Mirrors the mockup `.toolbar`.
 */
export function ConsoleToolbar({
  left,
  search,
  filter,
  actions,
  className,
}: ConsoleToolbarProps) {
  return (
    <div
      className={cn(
        'flex h-14 shrink-0 items-center gap-3 border-b px-4 md:px-5',
        className
      )}
    >
      <div className="flex min-w-0 flex-1 items-center gap-2">{left}</div>
      {search}
      {filter}
      {actions}
    </div>
  );
}
