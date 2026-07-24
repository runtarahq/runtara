import { type ReactNode } from 'react';
import { cn } from '@/lib/utils';
import { Icons } from '@/shared/components/icons.tsx';
import { Skeleton } from '@/shared/components/ui/skeleton';

/**
 * Shared loading / error / empty bodies for console list pages
 * (TriggersGrid, WorkflowsGrid, ObjectSchemasTable, ExistingConnections,
 * ReportsListPage, Settings…). These used to be copy-pasted ~50-line blocks
 * per page; keep the styling here so list states stay uniform.
 */

export interface TableSkeletonRowsProps {
  rows?: number;
  /** Width classes for the skeleton bars of one row, in column order.
      Prefix the last with `ml-auto` to right-align it like a actions column. */
  widths?: string[];
  className?: string;
}

export function TableSkeletonRows({
  rows = 8,
  widths = ['w-40', 'w-16', 'w-16', 'ml-auto w-48'],
  className,
}: TableSkeletonRowsProps) {
  return (
    <div className={cn('divide-y divide-border/50', className)}>
      {[...Array(rows)].map((_, i) => (
        <div key={i} className="flex items-center gap-4 px-5 py-3.5">
          {widths.map((w, j) => (
            <Skeleton key={j} className={cn('h-4', w)} />
          ))}
        </div>
      ))}
    </div>
  );
}

export interface ConsoleErrorStateProps {
  error: unknown;
  /** Plural entity name for the fallback copy, e.g. "triggers". */
  entityLabel: string;
  className?: string;
}

/** Network-vs-application error body, with the DEV-only detail box. */
export function ConsoleErrorState({
  error,
  entityLabel,
  className,
}: ConsoleErrorStateProps) {
  const err = error as
    { message?: string; code?: string; response?: unknown } | undefined;
  const isNetworkError =
    err?.message?.includes('fetch') ||
    err?.code === 'ERR_NETWORK' ||
    !err?.response;
  return (
    <div
      className={cn(
        'flex h-full flex-col items-center justify-center px-6 py-10 text-center',
        className
      )}
    >
      <Icons.warning className="mb-4 h-10 w-10 text-destructive" />
      <p className="text-base font-semibold text-foreground">
        {isNetworkError ? 'Unable to connect to backend' : 'An error occurred'}
      </p>
      <p className="mt-1 text-sm text-muted-foreground">
        {isNetworkError
          ? 'Please check that the backend service is running and try again.'
          : `There was a problem loading ${entityLabel}. Please try again.`}
      </p>
      {import.meta.env.DEV && err ? (
        <div className="mt-4 max-w-md rounded-lg bg-destructive/10 p-3 text-left">
          <p className="break-words font-mono text-xs text-destructive">
            {err.message || 'Unknown error'}
          </p>
        </div>
      ) : null}
    </div>
  );
}

export interface ConsoleEmptyStateProps {
  /** Icon component (defaults to the inbox icon). */
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  /** Optional call-to-action rendered under the description. */
  action?: ReactNode;
  className?: string;
}

export function ConsoleEmptyState({
  icon,
  title,
  description,
  action,
  className,
}: ConsoleEmptyStateProps) {
  return (
    <div
      className={cn(
        'flex h-full flex-col items-center justify-center px-6 py-10 text-center',
        className
      )}
    >
      {icon ?? <Icons.inbox className="mb-4 h-10 w-10 text-muted-foreground" />}
      <p className="text-base font-semibold text-foreground">{title}</p>
      {description ? (
        <p className="mt-1 text-sm text-muted-foreground">{description}</p>
      ) : null}
      {action ? <div className="mt-4">{action}</div> : null}
    </div>
  );
}
