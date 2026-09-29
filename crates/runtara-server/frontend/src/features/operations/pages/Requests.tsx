import { Link, useSearchParams } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { TablePagination } from '@/shared/components/console';
import { attentionQueues, queryAttentionRequests } from '../attention-requests';
import { useOperations } from '../queries';
import { RequestRows } from '../components/RequestRows';
import { OperationHeader, OperationSection, RefreshControls } from './shared';

export function RequestsPage() {
  const [params, setParams] = useSearchParams();
  const overdue = params.get('filter') === 'overdue';
  const rawPage = Number(params.get('page') ?? '1');
  const page = Number.isSafeInteger(rawPage) && rawPage > 0 ? rawPage - 1 : 0;
  const { queues, views } = useOperations({ includeProcesses: false });
  const tenant = useAuthStore((s) => s.orgId);
  const client = useQueryClient();
  const scopes = attentionQueues(queues.data ?? [], views.data ?? []);
  const query = useCustomQuery({
    queryKey: ['operations', tenant, 'request-list', scopes, overdue, page],
    queryFn: (token: string) =>
      queryAttentionRequests(token, scopes, overdue, page),
    enabled: Boolean(queues.data && views.data),
    placeholderData: undefined,
    refetchInterval: 30_000,
    refetchIntervalInBackground: false,
  });
  const error = queues.error || views.error || query.error;
  const rows = query.data?.content ?? [];
  return (
    <div className="mx-auto min-h-full w-full max-w-[1600px] bg-background p-5 lg:px-10 lg:py-7">
      <OperationHeader
        title={overdue ? 'Overdue requests' : 'Waiting requests'}
        description={
          overdue
            ? 'Open requests past the due time configured in their shared view.'
            : 'Open requests requiring an answer across all workflows.'
        }
        actions={
          <RefreshControls
            busy={query.isFetching || queues.isFetching || views.isFetching}
            updatedAt={query.dataUpdatedAt}
            onRefresh={() =>
              void client.invalidateQueries({
                queryKey: ['operations', tenant],
              })
            }
          />
        }
      />
      <main className="space-y-4">
        <nav
          aria-label="Request filters"
          className="flex flex-wrap items-center gap-4 border-b text-sm"
        >
          <Link
            className={`border-b-2 px-1 py-2 ${!overdue ? 'border-primary text-primary-text' : 'border-transparent text-muted-foreground'}`}
            aria-current={!overdue ? 'page' : undefined}
            to="/operations/requests"
          >
            Waiting
          </Link>
          <Link
            className={`border-b-2 px-1 py-2 ${overdue ? 'border-primary text-primary-text' : 'border-transparent text-muted-foreground'}`}
            aria-current={overdue ? 'page' : undefined}
            to="/operations/requests?filter=overdue"
          >
            Overdue
          </Link>
          <Link
            className="ml-auto text-xs text-primary-text"
            to="/operations/queues"
          >
            Browse queues
          </Link>
        </nav>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            Could not refresh requests.
            {rows.length ? ' Showing the last loaded requests.' : ''} Try
            Refresh.
          </p>
        )}
        <OperationSection
          title={
            query.data
              ? `${query.data.totalElements.toLocaleString()} ${overdue ? 'overdue' : 'waiting'} requests`
              : 'Requests'
          }
        >
          <div
            aria-hidden="true"
            className="hidden grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_minmax(0,1.5fr)_minmax(0,1fr)_6.5rem] gap-x-4 border-b px-4 py-2 text-xs font-medium text-muted-foreground lg:grid"
          >
            {['Run', 'Request', 'Workflow', 'Due', 'Actions'].map((label) => (
              <span
                key={label}
                className={label === 'Actions' ? 'text-right' : ''}
              >
                {label}
              </span>
            ))}
          </div>
          {rows.length ? (
            <RequestRows requests={rows} />
          ) : (
            <p className="p-5 text-sm text-muted-foreground">
              {error
                ? 'Requests unavailable.'
                : query.isPending
                  ? 'Loading requests…'
                  : overdue
                    ? 'No overdue requests.'
                    : 'No requests are waiting for an answer.'}
            </p>
          )}
          {query.data && (
            <div className="flex flex-wrap items-center justify-between gap-3 border-t px-4 py-3 text-xs text-muted-foreground">
              <span>{rows.length} shown · grouped by workflow</span>
              <TablePagination
                pageIndex={query.data.number}
                pageSize={query.data.size}
                pageCount={query.data.totalPages}
                onPageChange={(next) => {
                  const nextParams = new URLSearchParams(params);
                  if (next) nextParams.set('page', String(next + 1));
                  else nextParams.delete('page');
                  setParams(nextParams);
                }}
              />
            </div>
          )}
        </OperationSection>
      </main>
    </div>
  );
}
