import { ReactNode } from 'react';
import { cn } from '@/lib/utils';

type TilesPageProps = {
  kicker?: ReactNode;
  title: ReactNode;
  action?: ReactNode;
  toolbar?: ReactNode;
  children: ReactNode;
  className?: string;
  contentClassName?: string;
};

export function TilesPage(props: TilesPageProps) {
  const {
    kicker,
    title,
    action,
    toolbar,
    children,
    className,
    contentClassName,
  } = props;

  return (
    <div
      className={cn(
        'w-full min-h-screen bg-slate-50/50 dark:bg-background',
        className
      )}
    >
      {/* Header */}
      <header
        data-tiles-page-header
        className="sticky top-0 z-10 bg-slate-50/80 backdrop-blur-sm border-b border-slate-200/60 dark:bg-background/80 dark:border-slate-800/60"
      >
        <div className="px-4 md:px-8 py-5">
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div className="space-y-1">
              {kicker && (
                <p className="text-xs font-semibold uppercase tracking-wider text-blue-600 dark:text-blue-400">
                  {kicker}
                </p>
              )}
              <h1 className="text-xl font-semibold text-slate-900 dark:text-slate-100">
                {title}
              </h1>
            </div>
            {action && (
              <div data-tiles-page-action className="contents">
                {action}
              </div>
            )}
          </div>
        </div>
      </header>

      {/* Filters/Toolbar */}
      {toolbar && (
        <div
          data-tiles-page-toolbar
          className="px-4 md:px-8 py-4 border-b border-slate-200/60 bg-white/50 dark:bg-slate-900/50 dark:border-slate-800/60"
        >
          {toolbar}
        </div>
      )}

      {/* Content */}
      <div
        data-tiles-page-content
        className={cn('px-4 md:px-8 py-6', contentClassName)}
      >
        {children}
      </div>
    </div>
  );
}

type TileListProps = {
  children: ReactNode;
  className?: string;
};

export function TileList({ children, className }: TileListProps) {
  return <section className={cn('space-y-3', className)}>{children}</section>;
}
