import type { ReactNode } from 'react';
import { Link } from 'react-router';
import { ArrowRight, Check } from 'lucide-react';
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
  /** Column labels; omitted when there are no rows to label. */
  columns?: string[];
  children: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className="overflow-hidden rounded-[10px] border bg-background"
    >
      <div className="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-3">
        <h2 className="flex items-center text-[14.5px] font-semibold">
          {title}
          <span className="ml-2 rounded-full bg-muted px-[7px] py-px text-[11.5px] font-semibold tabular-nums text-muted-foreground">
            {count == null ? '—' : count.toLocaleString()}
          </span>
        </h2>
        <Link
          className="inline-flex items-center gap-1 text-[13px] font-medium text-primary-text hover:underline"
          to={to}
        >
          {link}
          <ArrowRight aria-hidden="true" className="size-3.5" />
        </Link>
      </div>
      {columns ? (
        <div
          aria-hidden="true"
          className={cn(
            rowClass,
            'hidden min-h-0 border-b bg-table-head py-2 text-[11.5px] font-semibold uppercase tracking-wider text-muted-foreground lg:grid'
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
      ) : null}
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
  if (failed || loading)
    return (
      <p className="px-5 py-5 text-sm text-muted-foreground">
        {failed ? 'Could not load this group. Try Refresh.' : 'Loading…'}
      </p>
    );
  // Nothing waiting is the good outcome, so it reads as one rather than as a
  // blank table.
  return (
    <div className="flex items-center gap-3 p-5">
      <span className="inline-flex size-9 shrink-0 items-center justify-center rounded-full bg-success/10 text-success-text">
        <Check className="size-4" strokeWidth={2} />
      </span>
      <div>
        <p className="text-sm font-medium">{empty}</p>
        <p className="mt-0.5 text-[12.5px] text-muted-foreground">
          When a run pauses for a decision, it appears here with its workflow
          and due date.
        </p>
      </div>
    </div>
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
      columns={
        requests.length
          ? ['Run', 'Request', 'Workflow', 'Due', 'Actions']
          : undefined
      }
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
