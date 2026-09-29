import { Link } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { AlertTriangle, ArrowRight } from 'lucide-react';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { Button } from '@/shared/components/ui/button';
import {
  useOperations,
  queryRuns,
  queryRunSummary,
  operationsRequest,
  defaultView,
  selectedFields,
} from '../queries';
import { StateValue } from '../components/StateValue';
import {
  OperationHeader,
  OperationSection,
  RefreshControls,
  FailureRows,
} from './shared';
import type { OperationRequestPage } from '@/generated/RuntaraRuntimeApi';

export function OverviewPage() {
  const { queues, views } = useOperations({ includeProcesses: false });
  const tenant = useAuthStore((s) => s.orgId);
  const client = useQueryClient();
  const failures = useCustomQuery({
    queryKey: ['operations', tenant, 'overview-failures'],
    queryFn: (token: string) =>
      queryRuns(token, {
        status: 'failed,timeout',
        completedFrom: new Date(Date.now() - 86_400_000).toISOString(),
        size: 3,
      }),
    refetchInterval: 30_000,
    placeholderData: undefined,
  });
  const running = useCustomQuery({
    queryKey: ['operations', tenant, 'running-count'],
    queryFn: (token: string) => queryRunSummary(token, {}),
    refetchInterval: 30_000,
    placeholderData: undefined,
  });
  const queueViews = (queues.data ?? [])
    .filter((q) => q.count > 0)
    .map((q) => ({
      queue: q,
      view:
        views.data?.find(
          (v) =>
            v.configuration.workflow === q.workflowId &&
            v.configuration.where?.openRequest === q.actionKey &&
            v.configuration.roles?.due
        )?.configuration ?? defaultView(q),
    }));
  const attention = useCustomQuery({
    queryKey: ['operations', tenant, 'attention', queueViews],
    queryFn: async (token: string) =>
      Promise.all(
        queueViews.map(async ({ queue, view }) => {
          // One unfiltered query per queue; a saved view supplies presentation roles.
          const due = view.roles?.due;
          const query = {
            stateFields: selectedFields(view),
            stateSort: due ? { field: due, descending: false } : undefined,
            size: 3,
          };
          const page = await operationsRequest<OperationRequestPage>(
            token,
            'operations/requests/query',
            'POST',
            { workflowId: queue.workflowId, actionKey: queue.actionKey, query }
          );
          const overdue = due
            ? (
                await operationsRequest<OperationRequestPage>(
                  token,
                  'operations/requests/query',
                  'POST',
                  {
                    workflowId: queue.workflowId,
                    actionKey: queue.actionKey,
                    query: {
                      size: 1,
                      state: [
                        {
                          field: due,
                          op: 'lt',
                          value: new Date().toISOString(),
                        },
                      ],
                    },
                  }
                )
              ).totalElements
            : undefined;
          return { queue, view, page, overdue };
        })
      ),
    enabled: Boolean(queues.data && views.data),
    refetchInterval: 30_000,
    placeholderData: undefined,
  });
  const waiting = queues.data?.reduce((sum, q) => sum + q.count, 0);
  const overdueKnown = attention.data?.some((q) => q.overdue !== undefined);
  const overdue = overdueKnown
    ? attention.data!.reduce((sum, q) => sum + (q.overdue ?? 0), 0)
    : undefined;
  const items =
    attention.data
      ?.flatMap((group) => group.page.content.map((row) => ({ ...group, row })))
      .sort((a, b) => a.row.requestedAt.localeCompare(b.row.requestedAt))
      .slice(0, 6) ?? [];
  return (
    <div className="mx-auto min-h-full w-full max-w-[1600px] bg-background p-5 lg:px-10 lg:py-7">
      <OperationHeader
        title="Overview"
        description="Requests to answer and recent failures to review."
        actions={
          <RefreshControls
            updatedAt={queues.dataUpdatedAt}
            busy={queues.isFetching || attention.isFetching}
            onRefresh={() =>
              void client.invalidateQueries({
                queryKey: ['operations', tenant],
              })
            }
          />
        }
      />
      <main className="space-y-6">
        {queues.error ||
        views.error ||
        failures.error ||
        attention.error ||
        running.error ? (
          <p
            role="alert"
            className="rounded-md border p-3 text-sm text-destructive"
          >
            Some Operations data could not be refreshed. Try Refresh.
          </p>
        ) : null}
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
          <Metric
            title="Waiting for a decision"
            value={waiting}
            note={`Across ${queues.data?.filter((q) => q.count > 0).length ?? '…'} queues`}
            to="/operations/queues"
            action="Open queues"
          />
          <Metric
            title="Overdue"
            value={overdue}
            note={
              overdueKnown
                ? 'Past their configured due time'
                : 'Choose a due field in a shared view'
            }
            to="/operations/queues"
            action="Review"
            warning
          />
          <Metric
            title="Failed in the last 24 h"
            value={failures.error ? undefined : failures.data?.totalElements}
            note="Runs that need attention"
            to="/operations/runs?status=failed,timeout&range=24h&dateBasis=completed&sortBy=completedAt&sortOrder=desc"
            action="View runs"
            danger
          />
          <Metric
            title="Running now"
            value={
              running.error
                ? undefined
                : (running.data?.counts.running ??
                  (running.data ? 0 : undefined))
            }
            note="Across all workflows · all time"
            to="/operations/runs?status=running"
            action="View runs"
          />
        </div>
        <div className="space-y-4">
          <OperationSection
            title="Needs attention"
            aside={
              <Link
                className="font-medium text-primary-text"
                to="/operations/queues"
              >
                See all {waiting ?? ''}
              </Link>
            }
          >
            {attention.isPending ? (
              <p className="p-5 text-sm text-muted-foreground">
                Loading requests…
              </p>
            ) : (
              <ul className="divide-y">
                {items.map(({ row, queue, view }) => {
                  const due = view.roles?.due
                    ? row.state?.[view.roles.due]
                    : undefined;
                  const isOverdue =
                    typeof due === 'string' && Date.parse(due) < Date.now();
                  return (
                    <li
                      key={`${row.instanceId}/${row.requestId}`}
                      className="flex flex-wrap items-center gap-3 px-4 py-4 sm:flex-nowrap"
                    >
                      <span
                        className={`rounded-full px-2 py-1 text-xs ${isOverdue ? 'bg-warning/10 text-warning' : 'bg-muted text-muted-foreground'}`}
                      >
                        {isOverdue ? (
                          <span className="flex items-center gap-1 whitespace-nowrap">
                            <AlertTriangle className="size-3" />
                            Overdue
                          </span>
                        ) : (
                          'Waiting'
                        )}
                      </span>
                      <div className="min-w-0 flex-1 basis-44">
                        <p className="break-words text-sm font-semibold">
                          {row.label} ·{' '}
                          {row.runLabel ?? row.instanceId.slice(0, 8)}
                        </p>
                        <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">
                          {row.message || queue.workflowName}
                          {typeof due === 'string' ? (
                            <>
                              {' '}
                              · Due{' '}
                              <StateValue
                                value={due}
                                display={{ kind: 'relative' }}
                              />
                            </>
                          ) : null}
                        </p>
                      </div>
                      <Button asChild>
                        <Link
                          to={`/operations/runs/${row.workflowId}/${row.instanceId}`}
                        >
                          Review
                        </Link>
                      </Button>
                    </li>
                  );
                })}
              </ul>
            )}
            <FailureRows rows={failures.data?.content ?? []} />
            {!attention.isPending &&
            !failures.isPending &&
            !attention.error &&
            !failures.error &&
            items.length === 0 &&
            !failures.data?.totalElements ? (
              <p className="p-5 text-sm text-muted-foreground">
                Nothing needs attention right now.
              </p>
            ) : null}
          </OperationSection>
        </div>
      </main>
    </div>
  );
}
function Metric({
  title,
  value,
  note,
  to,
  action,
  warning,
  danger,
}: {
  title: string;
  value?: number;
  note: string;
  to?: string;
  action?: string;
  warning?: boolean;
  danger?: boolean;
}) {
  return (
    <div className="min-w-0 rounded-lg border p-4">
      <p className="text-xs font-medium text-muted-foreground">{title}</p>
      <p
        className={`my-2 text-3xl font-semibold tabular-nums ${value && danger ? 'text-destructive' : value && warning ? 'text-warning' : ''}`}
      >
        {value?.toLocaleString() ?? '—'}
      </p>
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
        <span className="text-muted-foreground">{note}</span>
        {to ? (
          <Link
            className="flex items-center gap-1 whitespace-nowrap font-medium text-primary-text"
            to={to}
          >
            {action}
            <ArrowRight className="size-3" />
          </Link>
        ) : null}
      </div>
    </div>
  );
}
