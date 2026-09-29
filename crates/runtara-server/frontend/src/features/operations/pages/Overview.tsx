import { stateLabel } from '../state-label';
import { Link } from 'react-router';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import type {
  OperationProcess,
  OperationViewConfig,
} from '@/generated/RuntaraRuntimeApi';
import { useOperations, queryRuns } from '../queries';
import { type StateField } from '../components/StateValue';
import { OperationHeader } from './shared';

export function OverviewPage() {
  const { queues, views, processes } = useOperations();
  const tenant = useAuthStore((s) => s.orgId);
  const failures = useCustomQuery({
    queryKey: ['operations', tenant, 'failure-count'],
    queryFn: (token: string) =>
      queryRuns(token, { status: 'failed,timeout', size: 1 }),
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  return (
    <div className="min-h-full bg-background">
      <OperationHeader title="Operations" />
      <main className="space-y-8 p-6">
        <p className="text-muted-foreground">
          Answer waiting requests, follow progress, and review failures.
        </p>
        {queues.error || processes.error || views.error || failures.error ? (
          <p role="alert" className="text-destructive">
            Some Operations data could not be loaded. Refresh to try again.
          </p>
        ) : null}
        <div className="grid gap-4 md:grid-cols-3">
          <div className="rounded-lg border p-5">
            <p className="text-sm text-muted-foreground">Waiting requests</p>
            <p className="mt-2 text-3xl font-semibold">
              {queues.isPending
                ? '…'
                : (queues.data?.reduce((sum, q) => sum + q.count, 0) ?? 0)}
            </p>
          </div>
          <Link
            to="/operations/monitor"
            className="rounded-lg border p-5 hover:bg-muted/30"
          >
            <p className="text-sm text-muted-foreground">Failed runs</p>
            <p className="mt-2 text-3xl font-semibold">
              {failures.isPending ? '…' : (failures.data?.totalElements ?? 0)}
            </p>
          </Link>
          <div className="rounded-lg border p-5">
            <p className="text-sm text-muted-foreground">Shared views</p>
            <p className="mt-2 text-3xl font-semibold">
              {views.data?.length ?? 0}
            </p>
          </div>
        </div>
        <section>
          <h2 className="mb-3 text-lg font-semibold">Queues</h2>
          {queues.isPending ? (
            <p>Loading queues…</p>
          ) : queues.data?.length ? (
            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
              {queues.data.map((queue) => (
                <Link
                  key={`${queue.workflowId}/${queue.actionKey}`}
                  to={`/operations/queues/${encodeURIComponent(queue.workflowId)}/${encodeURIComponent(queue.actionKey)}`}
                  className="flex items-center justify-between rounded-lg border p-4 hover:bg-muted/30"
                >
                  <div>
                    <h3 className="font-medium">{queue.name}</h3>
                    <p className="text-sm text-muted-foreground">
                      {queue.workflowName}
                    </p>
                  </div>
                  <span className="rounded-full bg-muted px-3 py-1 text-sm">
                    {queue.count} requests
                  </span>
                </Link>
              ))}
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">
              No queues yet. Workflows with action requests appear here.
            </p>
          )}
        </section>
        {views.data?.length ? (
          <section>
            <h2 className="mb-3 text-lg font-semibold">Shared views</h2>
            <div className="flex flex-wrap gap-3">
              {views.data.map((view) => (
                <Link
                  key={view.id}
                  to={`/operations/views/${view.id}`}
                  className="rounded-lg border px-4 py-3 hover:bg-muted/30"
                >
                  {view.configuration.name}
                </Link>
              ))}
            </div>
          </section>
        ) : null}
        <section>
          <h2 className="mb-3 text-lg font-semibold">Processes</h2>
          <div className="grid gap-4 md:grid-cols-2">
            {processes.data?.map((process) => (
              <ProcessCard
                key={process.workflowId}
                process={process}
                view={
                  views.data?.find(
                    (view) =>
                      view.configuration.workflow === process.workflowId &&
                      view.configuration.roles?.stage
                  )?.configuration
                }
              />
            ))}
          </div>
        </section>
      </main>
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
    <div className="rounded-lg border p-5">
      <Link
        to={`/operations/processes/${process.workflowId}`}
        className="font-medium text-primary"
      >
        {process.name}
      </Link>
      {stage ? (
        <>
          <p className="mt-2 text-xs text-muted-foreground">
            {stateLabel(stage, schema[stage])} · {total} runs
          </p>
          {counts.error ? (
            <p role="alert">Stage counts unavailable.</p>
          ) : (
            <>
              <div className="my-3 flex h-3 overflow-hidden rounded bg-muted">
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
