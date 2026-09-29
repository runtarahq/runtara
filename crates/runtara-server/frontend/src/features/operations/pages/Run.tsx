import { stateLabel } from '../state-label';
import { Link, useParams } from 'react-router';
import { useCustomQuery } from '@/shared/hooks/api';
import { useAuthStore } from '@/shared/stores/authStore';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { Can } from '@/shared/components/Can';
import { Button } from '@/shared/components/ui/button';
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
import { OperationHeader, ReplayButton } from './shared';
import { useOperations } from '../queries';

export function RunPage() {
  const { workflowId = '', instanceId = '' } = useParams();
  const tenant = useAuthStore((s) => s.orgId);
  const submissions = useManagedInputSubmissions();
  const { views } = useOperations();
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
    <div>
      <OperationHeader
        title={data.runLabel ?? data.id.slice(0, 8)}
        actions={
          <>
            <Button
              variant="secondary"
              onClick={() => {
                void run.refetch();
                void pending.refetch();
              }}
            >
              Refresh
            </Button>
            <Link
              className="rounded border px-3 py-2 text-sm"
              to={`/workflows/${workflowId}/history/${instanceId}`}
            >
              Steps and debugging
            </Link>
          </>
        }
      />
      <main className="space-y-6 p-6">
        <div className="flex flex-wrap items-center gap-4">
          <RunStatusPill
            status={data.status}
            suspensionReason={data.suspensionReason}
          />
          <span className="text-sm text-muted-foreground">
            Started{' '}
            <StateValue value={data.created} display={{ kind: 'relative' }} />
          </span>
          <button
            className="text-xs text-muted-foreground"
            title={instanceId}
            onClick={() => void navigator.clipboard.writeText(instanceId)}
          >
            Copy run ID
          </button>
        </div>
        {stages?.length ? (
          <ol aria-label="Stages" className="flex flex-wrap gap-2">
            {stages.map((value, index) => (
              <li
                key={JSON.stringify(value)}
                aria-current={value === current ? 'step' : undefined}
                className={`rounded-full border px-4 py-2 text-sm ${value === current ? 'border-primary bg-primary/10 font-semibold' : 'text-muted-foreground'}`}
              >
                {index + 1}. {stateLabel(String(value))}
                {value === current ? ' · Current' : ''}
              </li>
            ))}
          </ol>
        ) : null}
        <StructuredErrorDisplay error={data.error} mode="expanded" />
        {data.errorSummary?.category === 'transient' ? (
          <ReplayButton run={data} />
        ) : null}
        <StatePanel
          state={data.state}
          schema={fields}
          updatedAt={data.stateUpdatedAt}
        />
        <section className="space-y-3">
          <h2 className="text-lg font-semibold">Waiting for an answer</h2>
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
                  className="max-w-3xl space-y-3 rounded-lg border p-5"
                >
                  <h3 className="font-medium">
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
            <p className="text-sm text-muted-foreground">
              No requests waiting for an answer.
            </p>
          )}
          <InputRetryPanel matches={(r) => r.instanceId === instanceId} />
        </section>
        <section>
          <h2 className="mb-3 text-lg font-semibold">Recent activity</h2>
          {steps.error ? (
            <p role="alert">Could not load activity.</p>
          ) : (
            <ol className="space-y-2">
              {steps.data?.data?.steps?.map((step) => (
                <li
                  key={`${step.stepId}/${step.scopeId ?? ''}/${step.startedAt}`}
                  className="flex items-center justify-between rounded border p-3 text-sm"
                >
                  <span>{step.stepName ?? step.stepId}</span>
                  <RunStatusPill status={step.status} />
                </li>
              ))}
            </ol>
          )}
        </section>
      </main>
    </div>
  );
}
