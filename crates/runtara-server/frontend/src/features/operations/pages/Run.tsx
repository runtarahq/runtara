import { stateLabel } from '../state-label';
import { Link, useParams } from 'react-router';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { Can } from '@/shared/components/Can';
import { StructuredErrorDisplay } from '@/shared/components/StructuredErrorDisplay';
import {
  getWorkflowInstance,
  getPendingInput,
  getStepSummaries,
} from '@/features/workflows/queries';
import { RunStatusPill } from '@/features/invocation-history/components/RunLinks';
import { ActionForm } from '@/features/workflows/components/ActionForm';
import { InputRetryPanel } from '@/features/workflows/components/ManagedInputSubmissions';
import { useManagedInputSubmissions } from '@/features/workflows/hooks/useManagedInputSubmissions';
import { StatePanel } from '../components/RunStateCard';
import { StateValue, type StateField } from '../components/StateValue';
import {
  OperationHeader,
  OperationSection,
  RefreshControls,
  ReplayButton,
} from './shared';
import { useOperations } from '../queries';

export function RunPage() {
  const { workflowId = '', instanceId = '' } = useParams();
  const tenant = useAuthStore((s) => s.orgId);
  const submissions = useManagedInputSubmissions();
  const { views, processes } = useOperations();
  const run = useCustomQuery({
    queryKey: ['operations', tenant, 'run', workflowId, instanceId],
    queryFn: (token: string) =>
      getWorkflowInstance(token, workflowId, instanceId),
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  const pending = useCustomQuery({
    queryKey: ['operations', tenant, 'pending', workflowId, instanceId],
    queryFn: (token: string) => getPendingInput(token, workflowId, instanceId),
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  const schema = useCustomQuery({
    queryKey: [
      'operations',
      tenant,
      'schema',
      workflowId,
      run.data?.usedVersion,
    ],
    queryFn: async (token: string) =>
      (
        await RuntimeREST.api.getVersionSchemasHandler(
          workflowId,
          run.data!.usedVersion,
          createAuthHeaders(token)
        )
      ).data,
    enabled: Boolean(run.data?.usedVersion),
    placeholderData: undefined,
  });
  const steps = useCustomQuery({
    queryKey: ['operations', tenant, 'activity', workflowId, instanceId],
    queryFn: (token: string) =>
      getStepSummaries(token, workflowId, instanceId, {
        limit: 20,
        sortOrder: 'desc',
      }),
    refetchInterval: 10_000,
    placeholderData: undefined,
  });
  if (run.error)
    return (
      <p role="alert" className="p-6">
        Could not load this run.
      </p>
    );
  if (!run.data) return <p className="p-6">Loading run…</p>;
  const data = run.data;
  const fields = (schema.data?.stateSchema ?? {}) as Record<string, StateField>;
  const role = views.data?.find(
    (v) =>
      v.configuration.workflow === workflowId && v.configuration.roles?.stage
  )?.configuration.roles?.stage;
  const stage =
    role ?? Object.keys(fields).find((field) => fields[field]?.enum?.length);
  const stages = stage ? fields[stage]?.enum : undefined;
  const current = stage ? data.state?.[stage] : undefined;
  return (
    <div className="mx-auto min-h-full w-full max-w-[1600px] bg-background p-5 lg:px-10 lg:py-7">
      <OperationHeader
        title={data.runLabel ?? data.id.slice(0, 8)}
        section="Queues"
        description={
          <span className="inline-flex flex-wrap items-center gap-2">
            {processes.data?.find((p) => p.workflowId === workflowId)?.name ??
              data.workflowName ??
              'Workflow run'}{' '}
            · run{' '}
            <span className="font-mono" title={instanceId}>
              {instanceId.slice(0, 4)}…{instanceId.slice(-4)}
            </span>
            <RunStatusPill
              status={data.status}
              suspensionReason={data.suspensionReason}
            />
          </span>
        }
        actions={
          <>
            <RefreshControls
              updatedAt={run.dataUpdatedAt}
              busy={run.isFetching || pending.isFetching}
              onRefresh={() => {
                void run.refetch();
                void pending.refetch();
                void steps.refetch();
              }}
            />
            <Link
              className="rounded border px-3 py-2 text-sm"
              to={`/workflows/${workflowId}/history/${instanceId}`}
            >
              Open execution
            </Link>
          </>
        }
      />
      <main className="space-y-5">
        {stages?.length ? (
          <ol
            aria-label="Stages"
            className="flex items-center gap-3 overflow-x-auto rounded-lg border px-4 py-4"
          >
            {stages.map((value, index) => (
              <li
                key={JSON.stringify(value)}
                aria-current={value === current ? 'step' : undefined}
                className={`flex min-w-32 flex-1 items-center gap-2 whitespace-nowrap text-xs ${value === current ? 'font-semibold text-warning' : 'text-muted-foreground'}`}
              >
                <span
                  className={`flex size-6 shrink-0 items-center justify-center rounded-full border-2 ${value === current ? 'border-warning bg-warning/10' : 'border-muted-foreground/30'}`}
                >
                  {index + 1}
                </span>
                {stateLabel(String(value))}
                <span className="ml-1 h-px flex-1 bg-border" />
              </li>
            ))}
          </ol>
        ) : null}
        <StructuredErrorDisplay error={data.error} mode="expanded" />
        {data.errorSummary?.category === 'transient' ? (
          <ReplayButton run={data} />
        ) : null}
        <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(280px,1fr)]">
          <section className="space-y-4">
            {pending.error ? (
              <p role="alert">Could not load requests.</p>
            ) : pending.isPending ? (
              <p>Loading requests…</p>
            ) : pending.data?.length ? (
              pending.data.map((request) => {
                const retained = [...submissions.inputs]
                  .reverse()
                  .find(
                    (input) =>
                      input.request.instanceId === instanceId &&
                      input.request.requestId === request.requestId
                  );
                return (
                  <article
                    key={request.requestId}
                    className="space-y-4 rounded-lg border p-4"
                  >
                    <h2 className="-mx-4 -mt-4 border-b px-4 py-3 text-sm font-semibold">
                      Decision needed
                    </h2>
                    <h3 className="text-sm font-semibold">
                      {request.toolName || 'Workflow request'}
                    </h3>
                    {request.message ? (
                      <p className="text-sm">{request.message}</p>
                    ) : null}
                    <Can
                      permission="workflow:execute"
                      fallback={<p>Read only</p>}
                    >
                      {retained ? (
                        <p role="status">
                          {retained.state === 'accepted'
                            ? 'Answered'
                            : retained.state === 'submitting'
                              ? 'Sending…'
                              : (retained.error ??
                                'Acceptance unconfirmed — retry below')}
                        </p>
                      ) : (
                        <ActionForm
                          submitLabel="Send decision"
                          inputSchema={request.responseSchema}
                          onSubmit={(payload) =>
                            void submissions.submit(
                              {
                                kind: 'execution',
                                workflowId,
                                instanceId,
                                requestId: request.requestId,
                                payload,
                              },
                              data.runLabel ?? 'Run'
                            )
                          }
                        />
                      )}
                    </Can>
                  </article>
                );
              })
            ) : (
              <OperationSection title="Decision needed">
                <p className="p-5 text-sm text-muted-foreground">
                  No requests waiting for an answer.
                </p>
              </OperationSection>
            )}
            <InputRetryPanel matches={(r) => r.instanceId === instanceId} />
          </section>
          <aside className="min-w-0 space-y-4">
            <StatePanel
              state={data.state}
              schema={fields}
              updatedAt={data.stateUpdatedAt}
              compact
            />
            <OperationSection
              title="Activity"
              aside={
                <Link
                  className="text-primary-text"
                  to={`/workflows/${workflowId}/history/${instanceId}`}
                >
                  Execution history
                </Link>
              }
            >
              {steps.error ? (
                <p role="alert">Could not load activity.</p>
              ) : (
                <ol className="space-y-4 p-4">
                  {steps.data?.data?.steps?.map((step) => (
                    <li
                      key={`${step.stepId}/${step.scopeId ?? ''}/${step.startedAt}`}
                      className="flex min-w-0 items-start gap-3 text-xs"
                    >
                      <span
                        className={`mt-1 size-2 shrink-0 rounded-full ${step.status === 'failed' ? 'bg-destructive' : 'bg-muted-foreground/50'}`}
                      />
                      <div className="min-w-0">
                        <p className="break-words font-medium">
                          {step.stepName ?? step.stepId}
                        </p>
                        <p className="mt-1 text-muted-foreground">
                          {stateLabel(step.status)}
                          {step.startedAt ? (
                            <>
                              {' '}
                              ·{' '}
                              <StateValue
                                value={step.startedAt}
                                display={{ kind: 'relative' }}
                              />
                            </>
                          ) : null}
                        </p>
                      </div>
                    </li>
                  ))}
                </ol>
              )}
            </OperationSection>
            <p className="px-4 text-xs text-muted-foreground">
              Workflow version {data.usedVersion}
            </p>
          </aside>
        </div>
      </main>
    </div>
  );
}
