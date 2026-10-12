import type { ReactNode } from 'react';
import { Link } from 'react-router';
import { cn } from '@/lib/utils';
import { RequestRows } from './RequestRows';
import type { AttentionRequest } from '../attention-requests';

const rowClass =
  'grid min-h-16 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 px-4 py-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_minmax(0,1.5fr)_minmax(0,1fr)_6.5rem]';

function AttentionGroup({
  title,
  count,
  link,
  to,
  columns,
  children,
}: {
  title: string;
  count?: number;
  link: string;
  to: string;
  columns: string[];
  children: ReactNode;
}) {
  return (
    <section aria-label={title}>
      <div className="flex flex-wrap items-center justify-between gap-2 bg-muted/20 px-4 py-3">
        <h3 className="text-sm font-medium">
          {title}{' '}
          <span className="ml-1 tabular-nums text-muted-foreground">
            {count == null ? '—' : count.toLocaleString()}
          </span>
        </h3>
        <Link
          className="text-xs font-medium text-primary-text hover:underline"
          to={to}
        >
          {link}
        </Link>
      </div>
      <div
        aria-hidden="true"
        className={cn(
          rowClass,
          'hidden min-h-0 border-y py-2 text-xs font-medium text-muted-foreground lg:grid'
        )}
      >
        {columns.map((column) => (
          <span
            key={column}
            className={column === 'Actions' ? 'text-right' : ''}
          >
            {column}
          </span>
        ))}
      </div>
      {children}
    </section>
  );
}

function EmptyGroup({
  loading,
  failed,
  empty,
}: {
  loading: boolean;
  failed: boolean;
  empty: string;
}) {
  return (
    <p className="px-4 py-5 text-sm text-muted-foreground">
      {failed
        ? 'Could not load this group. Try Refresh.'
        : loading
          ? 'Loading…'
          : empty}
    </p>
  );
}

export function OverviewAttention({
  requests,
  requestCount,
  requestsPending,
  requestsError,
}: {
  requests: AttentionRequest[];
  requestCount?: number;
  requestsPending: boolean;
  requestsError: boolean;
}) {
  return (
    <AttentionGroup
      title="Requests requiring input"
      count={requestCount}
      link="View all requests"
      to="/operations/requests"
      columns={['Run', 'Request', 'Workflow', 'Due', 'Actions']}
    >
      {requests.length ? (
        <RequestRows requests={requests} />
      ) : (
        <EmptyGroup
          loading={requestsPending}
          failed={requestsError}
          empty="No requests need input."
        />
      )}
    </AttentionGroup>
  );
}
