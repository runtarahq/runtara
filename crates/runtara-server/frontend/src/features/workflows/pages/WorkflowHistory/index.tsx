import { useManagedInputSubmissions } from '@/features/workflows/hooks/useManagedInputSubmissions';
import { RunStateCard } from '@/features/operations/components/RunStateCard';
import {
  ManagedInputScope,
  InputRetryPanel,
} from '@/features/workflows/components/ManagedInputSubmissions';
import { useNavigate, useParams } from 'react-router';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Loader } from '@/shared/components/loader.tsx';
import { formatDate } from '@/lib/utils.ts';
import { Badge } from '@/shared/components/ui/badge.tsx';
import { Button } from '@/shared/components/ui/button';
import {
  ChevronLeft,
  FileText,
  Clock,
  Calendar,
  Timer,
  MemoryStick,
  RotateCw,
  XCircle,
  Tag,
  Server,
  Hash,
  Copy,
  Sparkles,
  Flag,
  Zap,
  ChevronRight,
  Database,
  Info,
} from 'lucide-react';
import {
  getWorkflowInstance,
  getPendingInput,
  type PendingInput,
} from '@/features/workflows/queries';
import { useRef, useState } from 'react';
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
import { Separator } from '@/shared/components/ui/separator';
import { toast } from 'sonner';
import { StructuredErrorDisplay } from '@/shared/components/StructuredErrorDisplay';
import {
  getTerminationTypeDisplay,
  getStatusDisplay,
  isActiveStatus,
  isFinishedStatus,
} from '@/shared/utils/status-display';
import { getRunOutputEmptyState } from '@/features/workflows/utils/run-empty-state';
import {
  ExecutionSummary,
  ExecutionTimeline,
  type ExecutionStats,
} from '@/features/workflows/components/ExecutionTimeline';
import { Spinner } from '@/shared/components/ui/spinner';
import {
  canResume,
  suspendedStatusLabel,
} from '@/features/workflows/utils/suspension';
import {
  ChildRunsCard,
  ParentRunLink,
} from '@/features/invocation-history/components/RunLinks';
import {
  hasElidedPayload,
  mapElidedForDisplay,
} from '@/shared/utils/truncated-payload';

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

  // Briefly swap the copy button's icon to a checkmark for feedback.
  const flashCopied = (btnId: string) => {
    const btn = document.getElementById(btnId);
    if (!btn) return;
    btn.innerHTML =
      '<svg class="size-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"></path></svg>';
    setTimeout(() => {
      btn.innerHTML =
        '<svg class="size-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"></path></svg>';
    }, 2000);
  };

  // Copy the input/output payload. When the value was elided by the server,
  // re-fetch the instance with `?full=true` so the clipboard gets the real,
  // byte-complete value (e.g. a base64 file) rather than the preview stub.
  const copyPayload = async (
    which: 'inputs' | 'outputs',
    current: unknown,
    btnId: string
  ) => {
    let value = current;
    if (hasElidedPayload(current) && workflowId && instanceId) {
      try {
        const full = await getWorkflowInstance(token, workflowId, instanceId, {
          full: true,
        });
        value = which === 'inputs' ? full.inputs : full.outputs;
      } catch {
        toast.error(
          'Could not load the full value; copied the preview instead'
        );
      }
    }
    const text =
      typeof value === 'string' ? value : JSON.stringify(value, null, 2);
    try {
      await navigator.clipboard.writeText(text);
      flashCopied(btnId);
    } catch {
      toast.error('Copy failed');
    }
  };

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
        <div className="mb-4 flex items-center gap-3">
          <Button
            variant="secondary"
            size="icon"
            className="rounded-full hover:bg-muted"
            onClick={handleBack}
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
          <div className="flex gap-2">
            {data && data.id && (
              <>
                <Button
                  variant="secondary"
                  bordered
                  size="sm"
                  onClick={() =>
                    navigate(`/workflows/${workflowId}/history/${data.id}/logs`)
                  }
                  className="flex items-center gap-2"
                >
                  <FileText className="size-4" />
                  View Logs
                </Button>
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
            )}
          </div>
        </div>

        {/* Status Overview Bar */}
        {data && (
          <div className="flex items-center gap-4 rounded-lg border bg-muted/50 p-4">
            <div
              className="flex cursor-help items-center gap-2"
              title="Execution Status - Shows the current state of your workflow execution"
            >
              {(() => {
                const statusInfo = getStatusDisplay(data.status);
                return (
                  <>
                    {statusInfo.showSpinner && <Spinner className="size-4" />}
                    <Badge variant={statusInfo.variant}>
                      {statusInfo.text}
                    </Badge>
                    {suspendedStatusLabel(data) && (
                      <Badge
                        variant="warning"
                        title="Why this run is suspended"
                      >
                        {suspendedStatusLabel(data)}
                      </Badge>
                    )}
                  </>
                );
              })()}
            </div>
            {(() => {
              const terminationInfo = getTerminationTypeDisplay(
                data.terminationType
              );

              return (
                terminationInfo && (
                  <>
                    <Separator orientation="vertical" className="h-6" />
                    <div
                      className="flex cursor-help items-center gap-2"
                      title="Termination Type - Provides context for why this execution terminated"
                    >
                      <Info className="size-4 text-muted-foreground" />
                      <Badge variant={terminationInfo.variant}>
                        {terminationInfo.text}
                      </Badge>
                    </div>
                  </>
                )
              );
            })()}
            {data.id && (
              <>
                <Separator orientation="vertical" className="h-6" />
                <div
                  className="flex cursor-help items-center gap-2 text-sm text-muted-foreground"
                  title="Unique identifier for this workflow execution. Use this ID when reporting issues or tracking specific runs."
                >
                  <Hash className="size-4" />
                  <span className="font-mono">{data.id}</span>
                </div>
              </>
            )}
            {data.parentInstanceId && (
              <>
                <Separator orientation="vertical" className="h-6" />
                <div
                  className="flex min-w-0 items-center gap-2 text-sm text-muted-foreground"
                  data-testid="started-by"
                >
                  <span>Started by</span>
                  <ParentRunLink parentInstanceId={data.parentInstanceId} />
                </div>
              </>
            )}
          </div>
        )}
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
        {/* Metrics and Timeline & Details share one row: the metric cards
            stack two high so their column matches the details card. */}
        <div className="flex flex-col gap-4 lg:flex-row">
          <div className="grid shrink-0 grid-cols-2 gap-4 lg:auto-cols-[11rem] lg:grid-flow-col lg:grid-cols-none lg:grid-rows-2">
            {/* Execution Duration Card */}
            {data.executionDurationSeconds !== undefined &&
              data.executionDurationSeconds !== null && (
                <Card>
                  <CardHeader className="pb-3">
                    <CardTitle
                      className="flex cursor-help items-center gap-2 text-sm font-medium text-muted-foreground"
                      title="The total time spent actively running your workflow, from start to finish. This doesn't include time waiting in queue."
                    >
                      <Timer className="size-4" />
                      Execution Time
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="text-2xl font-bold">
                      {data.executionDurationSeconds.toFixed(2)}s
                    </div>
                  </CardContent>
                </Card>
              )}

            {/* Queue Duration Card */}
            {data.queueDurationSeconds !== undefined &&
              data.queueDurationSeconds !== null && (
                <Card>
                  <CardHeader className="pb-3">
                    <CardTitle
                      className="flex cursor-help items-center gap-2 text-sm font-medium text-muted-foreground"
                      title="The time your workflow spent waiting before it could start running. During busy periods, workflows may need to wait for available resources."
                    >
                      <Clock className="size-4" />
                      Queue Time
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="text-2xl font-bold">
                      {data.queueDurationSeconds.toFixed(2)}s
                    </div>
                  </CardContent>
                </Card>
              )}

            {/* Memory Usage Card */}
            {data.maxMemoryMb !== undefined && data.maxMemoryMb !== null && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle
                    className="flex cursor-help items-center gap-2 text-sm font-medium text-muted-foreground"
                    title="The maximum amount of memory (RAM) used while running this workflow. Higher values indicate more data was being processed at once."
                  >
                    <MemoryStick className="size-4" />
                    Max Memory
                  </CardTitle>
                </CardHeader>
                <CardContent>
                  <div className="text-2xl font-bold">
                    {data.maxMemoryMb.toFixed(1)} MB
                  </div>
                </CardContent>
              </Card>
            )}

            {/* Retries Card */}
            {data.metadata?.retryCount !== undefined &&
              data.metadata?.maxRetries !== undefined && (
                <Card>
                  <CardHeader className="pb-3">
                    <CardTitle
                      className="flex cursor-help items-center gap-2 text-sm font-medium text-muted-foreground"
                      title="Shows how many times this workflow was retried after encountering issues (current / maximum allowed). Automatic retries help ensure your workflows complete successfully."
                    >
                      <RotateCw className="size-4" />
                      Retries
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <div className="text-2xl font-bold">
                      {data.metadata.retryCount} / {data.metadata.maxRetries}
                    </div>
                  </CardContent>
                </Card>
              )}
          </div>

          {/* Instance Details Card */}
          <Card className="min-w-0 flex-1">
            <CardHeader className="pb-4">
              <CardTitle className="flex items-center gap-2">
                <Calendar className="size-5" />
                Timeline & Details
                {data.usedVersion !== undefined && (
                  <Badge variant="secondary" className="ml-auto">
                    v{data.usedVersion}
                  </Badge>
                )}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex flex-wrap gap-x-8 gap-y-4">
                <div className="flex items-center gap-3">
                  <div className="rounded-full bg-muted p-2">
                    <Calendar className="size-4 text-muted-foreground" />
                  </div>
                  <div>
                    <p className="text-xs text-muted-foreground">Created</p>
                    <p className="text-sm font-medium">
                      {data.created
                        ? formatDate(
                            new Date(data.created),
                            'yyyy-MM-dd HH:mm:ss'
                          )
                        : 'N/A'}
                    </p>
                  </div>
                </div>

                {data.metadata?.startedAt && (
                  <div className="flex items-center gap-3">
                    <div className="rounded-full bg-info/10 p-2">
                      <Zap className="size-4 text-info" />
                    </div>
                    <div>
                      <p className="text-xs text-muted-foreground">Started</p>
                      <p className="text-sm font-medium">
                        {formatDate(
                          new Date(data.metadata.startedAt),
                          'yyyy-MM-dd HH:mm:ss'
                        )}
                      </p>
                    </div>
                  </div>
                )}

                {data.metadata?.completedAt && (
                  <div className="flex items-center gap-3">
                    <div className="rounded-full bg-success/10 p-2">
                      <Flag className="size-4 text-success" />
                    </div>
                    <div>
                      <p className="text-xs text-muted-foreground">Completed</p>
                      <p className="text-sm font-medium">
                        {formatDate(
                          new Date(data.metadata.completedAt),
                          'yyyy-MM-dd HH:mm:ss'
                        )}
                      </p>
                    </div>
                  </div>
                )}

                {executionStats && (
                  <div>
                    <p className="text-xs text-muted-foreground">Steps</p>
                    <p className="text-sm font-medium">
                      {executionStats.stepCount}
                    </p>
                  </div>
                )}

                {data.processingOverheadSeconds !== undefined &&
                  data.processingOverheadSeconds !== null && (
                    <div
                      className="cursor-help"
                      title="The time spent on setup and coordination tasks before and after running your workflow. This is separate from the actual execution time."
                    >
                      <p className="text-xs text-muted-foreground">
                        Processing Overhead
                      </p>
                      <p className="text-sm font-medium">
                        {data.processingOverheadSeconds.toFixed(2)}s
                      </p>
                    </div>
                  )}

                {data.metadata?.workerId && (
                  <div
                    className="min-w-0 cursor-help"
                    title="A unique identifier for the server that processed this workflow. Useful for troubleshooting and tracking which server handled your request."
                  >
                    <p className="flex items-center gap-1 text-xs text-muted-foreground">
                      <Server className="size-3" />
                      Worker ID
                    </p>
                    <p className="truncate font-mono text-sm">
                      {data.metadata.workerId}
                    </p>
                  </div>
                )}
              </div>

              {/* Time per step type, from the Events timeline */}
              {executionStats &&
                Object.keys(executionStats.byType).length > 0 && (
                  <div className="mt-6">
                    <p className="mb-3 text-sm font-medium">
                      Execution Summary
                    </p>
                    <ExecutionSummary stats={executionStats} />
                  </div>
                )}

              {/* Error Message */}
              {data.metadata?.errorMessage && (
                <div className="mt-6">
                  <div className="mb-2 flex items-start gap-2">
                    <XCircle className="mt-0.5 size-5 text-destructive" />
                    <p className="font-semibold text-destructive">
                      Error Message
                    </p>
                  </div>
                  <StructuredErrorDisplay
                    error={data.metadata.errorMessage}
                    mode="expanded"
                    showGuidance
                  />
                </div>
              )}

              {/* Tags */}
              {data.tags && data.tags.length > 0 && (
                <div className="mt-6">
                  <div className="mb-3 flex items-center gap-2">
                    <Tag className="size-4 text-muted-foreground" />
                    <span className="text-sm font-medium">Tags</span>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {data.tags.map((tag: string, index: number) => (
                      <Badge
                        key={index}
                        variant="secondary"
                        className="px-3 py-1"
                      >
                        {tag}
                      </Badge>
                    ))}
                  </div>
                </div>
              )}
            </CardContent>
          </Card>
        </div>

        {/* Runs this run started through control:start */}
        {data.id && <ChildRunsCard instanceId={data.id} />}

        {/* Inputs & Outputs */}
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
          <Card className="overflow-hidden">
            <CardHeader className="border-b bg-gradient-to-r from-info/10 to-info/5">
              <div className="flex items-center justify-between">
                <CardTitle className="flex items-center gap-2 text-lg">
                  <div className="rounded-lg bg-info/10 p-2">
                    <Database className="size-5 text-info" />
                  </div>
                  Input Data
                </CardTitle>
                {data.inputs && (
                  <Button
                    variant="secondary"
                    size="sm"
                    className="h-8 gap-2"
                    onClick={() =>
                      void copyPayload('inputs', data.inputs, 'copy-inputs')
                    }
                  >
                    <span id="copy-inputs">
                      <Copy className="size-4" />
                    </span>
                    Copy
                  </Button>
                )}
              </div>
            </CardHeader>
            <CardContent className="bg-muted/30 p-0">
              {data.inputs ? (
                <div className="relative">
                  <pre className="max-h-[500px] overflow-auto p-6 font-mono text-xs">
                    <code className="text-foreground">
                      {JSON.stringify(
                        mapElidedForDisplay(data.inputs),
                        null,
                        2
                      )}
                    </code>
                  </pre>
                  {hasElidedPayload(data.inputs) && (
                    <div className="px-6 pb-3">
                      <Badge
                        variant="secondary"
                        className="text-3xs text-muted-foreground"
                      >
                        Large values elided — use Copy for the full value
                      </Badge>
                    </div>
                  )}
                </div>
              ) : (
                <div className="p-12 text-center">
                  <div className="mb-3 inline-flex size-12 items-center justify-center rounded-full bg-muted">
                    <Database className="size-6 text-muted-foreground" />
                  </div>
                  <p className="mb-1 text-sm text-muted-foreground">
                    No input data
                  </p>
                  <p className="text-xs text-muted-foreground">
                    This workflow was executed without input parameters
                  </p>
                </div>
              )}
            </CardContent>
          </Card>

          <Card className="overflow-hidden">
            <CardHeader className="border-b bg-gradient-to-r from-success/10 to-success/5">
              <div className="flex items-center justify-between">
                <CardTitle className="flex items-center gap-2 text-lg">
                  <div className="rounded-lg bg-success/10 p-2">
                    <Sparkles className="size-5 text-success" />
                  </div>
                  Output Data
                </CardTitle>
                {data.outputs && (
                  <Button
                    variant="secondary"
                    size="sm"
                    className="h-8 gap-2"
                    onClick={() =>
                      void copyPayload('outputs', data.outputs, 'copy-outputs')
                    }
                  >
                    <span id="copy-outputs">
                      <Copy className="size-4" />
                    </span>
                    Copy
                  </Button>
                )}
              </div>
            </CardHeader>
            <CardContent className="p-0">
              {data.outputs ? (
                <div className="relative bg-muted/30">
                  <pre className="max-h-[500px] overflow-auto p-6 font-mono text-xs">
                    <code className="text-foreground">
                      {(() => {
                        try {
                          // If outputs is already an object, stringify it
                          // (eliding any large values for display).
                          if (typeof data.outputs === 'object') {
                            return JSON.stringify(
                              mapElidedForDisplay(data.outputs),
                              null,
                              2
                            );
                          }
                          // If outputs is a string, try to parse and re-stringify
                          return JSON.stringify(
                            JSON.parse(data.outputs),
                            null,
                            2
                          );
                        } catch {
                          // If all else fails, convert to string
                          return String(data.outputs);
                        }
                      })()}
                    </code>
                  </pre>
                  {hasElidedPayload(data.outputs) && (
                    <div className="px-6 pb-3">
                      <Badge
                        variant="secondary"
                        className="text-3xs text-muted-foreground"
                      >
                        Large values elided — use Copy for the full value
                      </Badge>
                    </div>
                  )}
                </div>
              ) : (
                <div className="p-12 text-center">
                  <div className="mb-3 inline-flex size-12 items-center justify-center rounded-full bg-muted">
                    <Sparkles className="size-6 text-muted-foreground" />
                  </div>
                  <p className="mb-1 text-sm text-muted-foreground">
                    {outputEmptyState.title}
                  </p>
                  <p className="text-xs text-muted-foreground">
                    {outputEmptyState.description}
                  </p>
                </div>
              )}
            </CardContent>
          </Card>
        </div>

        <RunStateCard
          workflowId={workflowId!}
          version={data.usedVersion}
          state={data.state}
          updatedAt={data.stateUpdatedAt}
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
          <CardContent className="p-6">
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
