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
      className={cn('w-full min-h-screen bg-background', className)}
    >
      {/* Header */}
      <header
        data-tiles-page-header
        className="sticky top-0 z-10 bg-background/80 backdrop-blur-sm border-b"
      >
        <div className="px-4 md:px-8 py-4">
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div className="min-w-0 flex-1 space-y-1">
              {kicker && (
                <p className="text-xs font-semibold uppercase tracking-wider text-primary">
                  {kicker}
                </p>
              )}
              <h1 className="text-xl font-semibold text-foreground">
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
          className="px-4 md:px-8 py-3 border-b"
        >
          {toolbar}
        </div>
      )}

      {/* Content */}
      <div
        data-tiles-page-content
        className={cn('px-4 md:px-8 py-5', contentClassName)}
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
