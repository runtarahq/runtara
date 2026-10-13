import type { ReactNode } from 'react';
import { Link } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { ArrowRight } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useCustomQuery } from '@/shared/hooks/api';
import { Breadcrumb, ConsoleToolbar } from '@/shared/components/console';
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
import { RefreshControls, RunRows } from './shared';
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
        size: 1,
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
  const recent = useCustomQuery({
    queryKey: ['operations', tenant, 'invocations-24h'],
    queryFn: (token: string) =>
      queryRunSummary(token, {
        createdFrom: new Date(Date.now() - 86_400_000).toISOString(),
      }),
    refetchInterval: 30_000,
    placeholderData: undefined,
  });
  const latest = useCustomQuery({
    queryKey: ['operations', tenant, 'recent-runs'],
    queryFn: (token: string) =>
      queryRuns(token, { size: 5, sortBy: 'createdAt', sortOrder: 'desc' }),
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
  const latestRuns = latest.data?.content ?? [];
  return (
    <div className="flex min-h-full flex-col bg-background">
      <ConsoleToolbar
        left={
          <>
            <Breadcrumb
              items={[
                { label: 'Operations', to: '/operations' },
                { label: 'Overview' },
              ]}
            />
            <span className="ml-1 hidden truncate text-[13px] text-muted-foreground lg:inline">
              Requests to answer and how runs are going.
            </span>
          </>
        }
        actions={
          <div className="flex items-center gap-4">
            <RefreshControls
              updatedAt={queues.dataUpdatedAt}
              busy={queues.isFetching || attention.isFetching}
              onRefresh={() =>
                void client.invalidateQueries({
                  queryKey: ['operations', tenant],
                })
              }
            />
          </div>
        }
      />
      <main className="space-y-5 px-4 pb-7 pt-5 md:px-7">
        {queues.error ||
        views.error ||
        failures.error ||
        attention.error ||
        running.error ||
        recent.error ||
        latest.error ? (
          <p
            role="alert"
            className="rounded-md border p-3 text-sm text-destructive"
          >
            Some Operations data could not be refreshed. Try Refresh.
          </p>
        ) : null}
        {/* One strip rather than five cards: the numbers read as a single
            status line, and the one that isn't set up yet says so. */}
        <div className="flex flex-wrap overflow-hidden rounded-[10px] border bg-background">
          <Metric
            title="Waiting for a decision"
            value={waiting}
            note={`Across ${queues.data ? new Set(queues.data.filter((q) => q.count > 0).map((q) => q.workflowId)).size : '…'} workflows`}
            to="/operations/requests"
          />
          <Metric
            title="Failed in the last 24 h"
            value={failures.error ? undefined : failures.data?.totalElements}
            note="Runs that need attention"
            to="/operations/runs?status=failed,timeout&range=24h&dateBasis=completed&sortBy=completedAt&sortOrder=desc"
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
            note="Across all workflows"
            to="/operations/runs?status=running"
          />
          <Metric
            title="Invocations in the last 24 h"
            value={recent.error ? undefined : recent.data?.total}
            note="Runs started across all workflows"
            to="/operations/runs?range=24h"
          />
          {overdueKnown ? (
            <Metric
              title="Overdue"
              value={overdue}
              note="Past their configured due time"
              to="/operations/requests?filter=overdue"
              warning
            />
          ) : (
            <Link
              to="/operations/queues"
              className={cn(metricClass, 'bg-table-head')}
            >
              <span className="text-[12.5px] font-medium text-muted-foreground">
                Overdue
              </span>
              <span className="text-sm font-medium leading-7 text-muted-foreground">
                Not tracked yet
              </span>
              <span className="text-[12.5px] font-medium text-primary-text">
                Choose a due field in a queue →
              </span>
            </Link>
          )}
        </div>
        <OverviewAttention
          requests={items}
          requestCount={queues.error ? undefined : waiting}
          requestsPending={attention.isPending}
          requestsError={!!attention.error || !!queues.error || !!views.error}
        />
        <Panel title="Recent runs" link="View runs" to="/operations/runs">
          {latestRuns.length ? (
            <RunRows rows={latestRuns} showDuration />
          ) : (
            <p className="px-5 py-5 text-sm text-muted-foreground">
              {latest.error
                ? 'Could not load recent runs. Try Refresh.'
                : latest.isPending
                  ? 'Loading…'
                  : 'No runs yet.'}
            </p>
          )}
        </Panel>
      </main>
    </div>
  );
}

function Panel({
  title,
  link,
  to,
  children,
}: {
  title: string;
  link: string;
  to: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className="overflow-hidden rounded-[10px] border bg-background"
    >
      <div className="flex items-center justify-between gap-3 border-b px-5 py-3">
        <h2 className="text-[14.5px] font-semibold">{title}</h2>
        <Link
          className="inline-flex items-center gap-1 text-[13px] font-medium text-primary-text hover:underline"
          to={to}
        >
          {link}
          <ArrowRight aria-hidden="true" className="size-3.5" />
        </Link>
      </div>
      {children}
    </section>
  );
}

const metricClass =
  'flex min-w-[150px] flex-1 flex-col gap-1.5 border-l px-5 py-4 transition-colors first:border-l-0 hover:bg-muted/40';

function Metric({
  title,
  value,
  note,
  to,
  warning,
  danger,
}: {
  title: string;
  value?: number;
  note: string;
  to: string;
  warning?: boolean;
  danger?: boolean;
}) {
  return (
    <Link to={to} className={metricClass}>
      <span className="text-[12.5px] font-medium text-muted-foreground">
        {title}
      </span>
      <span
        className={cn(
          'text-[28px] font-semibold tabular-nums leading-none tracking-tight text-foreground',
          value && danger && 'text-destructive',
          value && warning && 'text-warning-text'
        )}
      >
        {value?.toLocaleString() ?? '—'}
      </span>
      <span className="text-[12.5px] text-muted-foreground">{note}</span>
    </Link>
  );
}
