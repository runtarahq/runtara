import { Link } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { ArrowRight } from 'lucide-react';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import {
  useOperations,
  queryRuns,
  queryRunSummary,
  operationsRequest,
  selectedFields,
} from '../queries';
import {
  attentionQueues,
  attentionRequest,
  overdueFilters,
} from '../attention-requests';
import { OverviewAttention } from '../components/OverviewAttention';
import { OperationHeader, OperationSection, RefreshControls } from './shared';
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
        sortBy: 'completedAt',
        sortOrder: 'desc',
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
  const queueViews = attentionQueues(queues.data ?? [], views.data ?? []);
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
                      state: overdueFilters(view, Date.now()),
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
      .map(({ row, queue, view }) =>
        attentionRequest(row, { queue, view }, Date.now())
      )
      .sort(
        (a, b) =>
          Number(b.isOverdue) - Number(a.isOverdue) ||
          (a.isOverdue && b.isOverdue
            ? Date.parse(a.due!) - Date.parse(b.due!)
            : 0) ||
          a.row.requestedAt.localeCompare(b.row.requestedAt)
      )
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
            to="/operations/requests"
            action="View requests"
          />
          <Metric
            title="Overdue"
            value={overdue}
            note={
              overdueKnown
                ? 'Past their configured due time'
                : 'Choose a due field in a shared view'
            }
            to="/operations/requests?filter=overdue"
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
        <OperationSection title="Needs attention">
          <OverviewAttention
            requests={items}
            requestCount={queues.error ? undefined : waiting}
            requestsPending={attention.isPending}
            requestsError={!!attention.error || !!queues.error || !!views.error}
            failures={failures.data?.content ?? []}
            failureCount={
              failures.error ? undefined : failures.data?.totalElements
            }
            failuresPending={failures.isPending}
            failuresError={!!failures.error}
          />
        </OperationSection>
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
