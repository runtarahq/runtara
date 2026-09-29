import { Link } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { AlertTriangle, ArrowRight } from 'lucide-react';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { Button } from '@/shared/components/ui/button';
import type {
  OperationProcess,
  OperationViewConfig,
} from '@/generated/RuntaraRuntimeApi';
import {
  useOperations,
  queryRuns,
  operationsRequest,
  defaultView,
  selectedFields,
} from '../queries';
import { StateValue, type StateField } from '../components/StateValue';
import {
  OperationHeader,
  OperationSection,
  RefreshControls,
  FailureRows,
} from './shared';
import { stateLabel } from '../state-label';
import type { OperationRequestPage } from '@/generated/RuntaraRuntimeApi';

export function OverviewPage() {
  const { queues, views, processes } = useOperations();
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
    queryFn: (token: string) =>
      queryRuns(token, { status: 'running', size: 1 }),
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
        description="Work that needs a person, and how your processes are running."
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
        processes.error ||
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
            value={failures.data?.totalElements}
            note="Runs that need attention"
            to="/operations/monitor"
            action="Monitor"
            danger
          />
          <Metric
            title="Running now"
            value={running.data?.totalElements}
            note="Across all workflows"
          />
        </div>
        <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(280px,1fr)]">
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
            <FailureRows
              rows={(failures.data?.content ?? []).map((run) => ({
                ...run,
                workflowName:
                  processes.data?.find((p) => p.workflowId === run.workflowId)
                    ?.name ?? run.workflowName,
              }))}
            />
            {!attention.isPending &&
            !failures.isPending &&
            items.length === 0 &&
            !failures.data?.totalElements ? (
              <p className="p-5 text-sm text-muted-foreground">
                Nothing needs attention right now.
              </p>
            ) : null}
          </OperationSection>
          <OperationSection title="Processes" aside="All runs">
            <div className="divide-y">
              {processes.data?.map((process) => (
                <ProcessCard
                  key={process.workflowId}
                  process={process}
                  view={
                    views.data?.find(
                      (v) =>
                        v.configuration.workflow === process.workflowId &&
                        v.configuration.roles?.stage
                    )?.configuration
                  }
                />
              ))}
              {processes.isPending ? (
                <p className="p-4 text-sm text-muted-foreground">
                  Loading processes…
                </p>
              ) : !processes.data?.length ? (
                <p className="p-4 text-sm text-muted-foreground">
                  No workflows yet.
                </p>
              ) : null}
            </div>
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
function ProcessCard({
  process,
  view,
}: {
  process: OperationProcess;
  view?: OperationViewConfig;
}) {
  const schema = process.stateSchema as Record<string, StateField>;
  const stage =
    view?.roles?.stage ??
    Object.keys(schema).find((field) => schema[field]?.enum?.length);
  const values = (stage ? (schema[stage]?.enum ?? []) : []).slice(0, 32);
  const tenant = useAuthStore((s) => s.orgId);
  const counts = useCustomQuery({
    queryKey: [
      'operations',
      tenant,
      'stages',
      process.workflowId,
      stage,
      values,
    ],
    queryFn: async (token: string) =>
      Promise.all(
        values.map(async (value) => ({
          value,
          count: (
            await queryRuns(token, {
              workflowId: process.workflowId,
              size: 1,
              state: [{ field: stage!, op: 'eq', value: value as never }],
            })
          ).totalElements,
        }))
      ),
    enabled: values.length > 0,
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  const total = counts.data?.reduce((n, s) => n + s.count, 0) ?? 0;
  return (
    <div className="p-4">
      <Link
        to={`/operations/processes/${process.workflowId}`}
        className="text-sm font-semibold hover:text-primary-text"
      >
        {process.name}
      </Link>
      {stage ? (
        <>
          <p className="mt-2 text-xs text-muted-foreground">
            {stateLabel(stage, schema[stage])} ·{' '}
            {counts.isPending ? '…' : total} {total === 1 ? 'run' : 'runs'}
          </p>
          {counts.isPending ? (
            <p className="py-3 text-xs text-muted-foreground">
              Loading stages…
            </p>
          ) : counts.error ? (
            <p role="alert">Stage counts unavailable.</p>
          ) : (
            <>
              <div className="my-2 flex h-2 overflow-hidden rounded-sm bg-muted">
                {counts.data?.map((s, index) => (
                  <div
                    key={JSON.stringify(s.value)}
                    title={`${String(s.value)}: ${s.count}`}
                    style={{
                      width: `${total ? (s.count / total) * 100 : 0}%`,
                      opacity: 0.35 + (index % 5) * 0.13,
                    }}
                    className="bg-primary"
                  />
                ))}
              </div>
              <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
                {counts.data?.map((s) => (
                  <span key={JSON.stringify(s.value)}>
                    {stateLabel(String(s.value))} <strong>{s.count}</strong>
                  </span>
                ))}
              </div>
            </>
          )}
        </>
      ) : (
        <p className="mt-2 text-sm text-muted-foreground">
          View runs and configure a shared view.
        </p>
      )}
    </div>
  );
}
