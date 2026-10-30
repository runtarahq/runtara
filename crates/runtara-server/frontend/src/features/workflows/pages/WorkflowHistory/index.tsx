import { useManagedInputSubmissions } from '@/features/workflows/hooks/useManagedInputSubmissions';
import { useVersionStateSchema } from '@/features/operations/state-schema';
import { stateLabel } from '@/features/operations/state-label';
import { PayloadExplorer } from '@/shared/components/PayloadExplorer';
import { formatRelativeTime } from '@/lib/utils';
import {
  ManagedInputScope,
  InputRetryPanel,
} from '@/features/workflows/components/ManagedInputSubmissions';
import { useNavigate, useParams } from 'react-router';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Loader } from '@/shared/components/loader.tsx';
import { Button } from '@/shared/components/ui/button';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import {
  getWorkflowInstance,
  getPendingInput,
  type PendingInput,
} from '@/features/workflows/queries';
import { useCallback, useMemo, useRef, useState } from 'react';
import { useToken } from '@/shared/hooks';
import { HumanInputCard } from '@/features/workflows/components/ExecutionPanel/HumanInputCard';
import { PauseButton } from '@/features/workflows/components/PauseButton';
import { ReplayButton } from '@/features/workflows/components/ReplayButton';
import { ResumeButton } from '@/features/workflows/components/ResumeButton';
import { StopButton } from '@/features/workflows/components/StopButton';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import {
  isActiveStatus,
  isFinishedStatus,
} from '@/shared/utils/status-display';
import { getRunOutputEmptyState } from '@/features/workflows/utils/run-empty-state';
import {
  ExecutionTimeline,
  type ExecutionStats,
} from '@/features/workflows/components/ExecutionTimeline';
import { RunOverviewCard } from './RunOverviewCard';
import { GuestOutputCard } from './GuestOutputCard';
import { canResume } from '@/features/workflows/utils/suspension';
import { ChildRunsCard } from '@/features/invocation-history/components/RunLinks';

/** Outputs may be stored as a JSON string; show them parsed when they are. */
function parseRunOutputs(outputs: unknown): unknown {
  if (typeof outputs !== 'string') return outputs ?? null;
  try {
    return JSON.parse(outputs);
  } catch {
    return outputs;
  }
}

export function WorkflowHistory() {
  return (
    <ManagedInputScope>
      <WorkflowHistoryContent />
    </ManagedInputScope>
  );
}

function WorkflowHistoryContent() {
  const { workflowId, instanceId } = useParams();
  const navigate = useNavigate();
  const isInitialLoadRef = useRef(true);
  // Reported by the Events timeline; summarized under Timeline & Details.
  const [executionStats, setExecutionStats] = useState<ExecutionStats | null>(
    null
  );

  const token = useToken();
  const submissions = useManagedInputSubmissions();

  const { data, isLoading, isError } = useCustomQuery({
    queryKey: queryKeys.workflows.instance(workflowId ?? '', instanceId ?? ''),
    queryFn: (token: string) =>
      getWorkflowInstance(token, workflowId!, instanceId!),
    enabled: !!workflowId && !!instanceId,
    refetchInterval: (query): number | false =>
      isActiveStatus(query.state.data?.status) ? 10000 : false,
  });

  // Poll pending input while instance is Running
  const { data: pendingInputData, error: pendingInputError } = useCustomQuery<
    PendingInput[]
  >({
    queryKey: queryKeys.workflows.pendingInput(
      workflowId ?? '',
      instanceId ?? ''
    ),
    queryFn: (token: string) =>
      getPendingInput(token, workflowId!, instanceId!),
    enabled: !!workflowId && !!instanceId,
    placeholderData: undefined,
    refetchInterval: () => {
      return isActiveStatus(data?.status) ? 3000 : false;
    },
  });

  const forTarget = (requestId: string) =>
    submissions.inputs.filter(
      (input) =>
        input.request.kind === 'execution' &&
        input.request.workflowId === workflowId &&
        input.request.instanceId === instanceId &&
        input.request.requestId === requestId
    );
  // Pending-input polling stops once the run ends, so its last result can be
  // stale: a finished run has no actionable inputs, whatever was fetched last.
  const pendingInputs = isFinishedStatus(data?.status)
    ? []
    : (pendingInputData ?? []).filter(
        (input) =>
          !forTarget(input.requestId).some(
            (submission) => submission.state === 'accepted'
          )
      );
  const handleSignalSubmit = (
    requestId: string,
    payload: Record<string, unknown>
  ) => {
    if (!instanceId || !workflowId) return;
    void submissions.submit(
      { kind: 'execution', workflowId, instanceId, requestId, payload },
      pendingInputs.find((input) => input.requestId === requestId)?.message ||
        'Workflow response'
    );
  };

  // Run data for the explorer. Outputs can arrive as a JSON string; parse it
  // once per fetch so the explorer's per-value caches stay warm.
  const outputValue = useMemo(
    () => parseRunOutputs(data?.outputs),
    [data?.outputs]
  );
  const stateValue = useMemo(
    () =>
      data?.state && Object.keys(data.state).length > 0 ? data.state : null,
    [data?.state]
  );
  const { schema: stateSchema } = useVersionStateSchema(
    workflowId ?? '',
    data?.usedVersion ?? 0
  );
  const stateFieldLabels = useMemo(
    () =>
      Object.fromEntries(
        Object.entries(stateSchema).map(([key, field]) => [
          key,
          { label: stateLabel(key, field), description: field.description },
        ])
      ),
    [stateSchema]
  );
  // Elided placeholders (large uploads) are re-fetched in full on request.
  const loadFullRunValue = useCallback(
    async (sectionId: string) => {
      const full = await getWorkflowInstance(token, workflowId!, instanceId!, {
        full: true,
      });
      if (sectionId === 'input') return full.inputs;
      if (sectionId === 'output') return parseRunOutputs(full.outputs);
      return full.state;
    },
    [token, workflowId, instanceId]
  );

  // Set page title with workflow name from metadata if available
  usePageTitle(
    data?.metadata?.workflowName
      ? `Workflow History - ${data.metadata.workflowName}`
      : 'Workflow History'
  );

  // Update isInitialLoadRef after the first successful data load
  if (data && isInitialLoadRef.current) {
    isInitialLoadRef.current = false;
  }

  const handleBack = () => {
    // Check if there's history to go back to
    if (window.history.state && window.history.state.idx > 0) {
      // There is history, go back
      navigate(-1);
    } else {
      // No history, go to home page
      navigate('/workflows');
    }
  };

  // Only show loader on initial load, not during refetches
  if ((isLoading || !data) && isInitialLoadRef.current) {
    return <Loader />;
  }

  // Show error state if fetch failed
  if (isError) {
    return (
      <div className="flex flex-col items-center justify-center py-20 text-center">
        <p className="text-base font-semibold text-foreground">
          Failed to load execution details
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Please try again or go back to workflows.
        </p>
      </div>
    );
  }

  // Guard against undefined data
  if (!data) {
    return <Loader />;
  }

  // Empty-state wording depends on whether the run can still produce something.
  const outputEmptyState = getRunOutputEmptyState(data.status);

  return (
    <div className="mx-auto max-w-7xl px-4 py-8 sm:px-6 lg:px-8">
      {/* Header Section */}
      <div className="mb-8">
        <div className="flex items-center gap-3">
          {/* -ml-2 cancels the icon button's inner padding so the arrow
              lines up with the cards' left edge. */}
          <Button
            variant="secondary"
            size="icon"
            className="-ml-2 rounded-full hover:bg-muted"
            onClick={handleBack}
            aria-label="Back"
          >
            <ChevronLeft className="size-5" />
          </Button>
          <div className="flex-1">
            <h1 className="text-3xl font-bold tracking-tight">
              Workflow Execution Details
            </h1>
            {data?.metadata?.workflowName && (
              <p className="mt-1 text-muted-foreground">
                {data.metadata.workflowName}
                {data.metadata.workflowDescription && (
                  <span className="ml-2 text-sm">
                    • {data.metadata.workflowDescription}
                  </span>
                )}
              </p>
            )}
          </div>
        </div>
      </div>

      <InputRetryPanel
        matches={(request) =>
          request.kind === 'execution' &&
          request.instanceId === instanceId &&
          request.workflowId === workflowId
        }
      />
      {/* Pending Human Input Cards */}
      {pendingInputError && (
        <p role="alert" className="text-sm text-destructive">
          Pending inputs are unavailable: {pendingInputError.message}
        </p>
      )}
      {pendingInputs.length > 0 && (
        <div className="mb-6 space-y-3">
          {pendingInputs.map((pi) => (
            <HumanInputCard
              key={pi.requestId}
              pendingInput={pi}
              onSubmit={handleSignalSubmit}
              isSubmitting={forTarget(pi.requestId).some(
                (input) => input.state === 'submitting'
              )}
            />
          ))}
        </div>
      )}

      <div className="space-y-6">
        <RunOverviewCard
          run={data}
          executionStats={executionStats}
          actions={
            data.id ? (
              <>
                {isActiveStatus(data.status) ? (
                  <>
                    {/* Only a paused run needs a resume; a waiting run wakes
                      on its own. */}
                    {canResume(data) && (
                      <ResumeButton
                        instanceId={data.id}
                        variant="secondary"
                        size="sm"
                      />
                    )}
                    {/* The pause handler only accepts instances in the
                      'running' state (suspended is a no-op, anything else
                      is a 400), so gate on the exact status. */}
                    {data.status?.toLowerCase() === 'running' && (
                      <PauseButton
                        instanceId={data.id}
                        variant="secondary"
                        size="sm"
                      />
                    )}
                    <StopButton
                      instanceId={data.id}
                      variant="secondary"
                      size="sm"
                    />
                  </>
                ) : (
                  <ReplayButton
                    instanceId={data.id}
                    error={data.metadata?.errorMessage}
                    variant="secondary"
                    size="sm"
                  />
                )}
              </>
            ) : null
          }
        />

        {/* Runs this run started through control:start */}
        {data.id && <ChildRunsCard instanceId={data.id} />}

        <GuestOutputCard output={data.guestOutput} />

        <PayloadExplorer
          ariaLabel="Run data"
          defaultSectionId="output"
          loadFullValue={loadFullRunValue}
          sections={[
            {
              id: 'input',
              label: 'Input',
              value: data.inputs ?? null,
              emptyState: {
                title: 'No input data',
                description:
                  'This workflow was executed without input parameters',
              },
            },
            {
              id: 'output',
              label: 'Output',
              value: outputValue,
              emptyState: outputEmptyState,
            },
            {
              id: 'state',
              label: 'State',
              value: stateValue,
              meta: data.stateUpdatedAt
                ? `Updated ${formatRelativeTime(data.stateUpdatedAt)}`
                : undefined,
              emptyState: { title: 'No state published yet' },
              fieldLabels: stateFieldLabels,
            },
          ]}
        />

        {/* Events Section */}
        <Card className="overflow-hidden">
          <CardHeader className="border-b bg-gradient-to-r from-purple-500/10 to-purple-500/5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <div className="rounded-lg bg-purple-500/10 p-2">
                <ChevronRight className="size-5 text-purple-600" />
              </div>
              Events
            </CardTitle>
            <p className="mt-2 text-sm text-muted-foreground">
              Step execution events showing inputs, outputs, and status for each
              step in your workflow
            </p>
          </CardHeader>
          <CardContent className="p-0">
            <ExecutionTimeline
              showInputs={false}
              onStatsChange={setExecutionStats}
              workflowId={workflowId!}
              instanceId={instanceId!}
            />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
