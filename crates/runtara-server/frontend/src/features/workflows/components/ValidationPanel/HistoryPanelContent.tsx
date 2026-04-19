import { useEffect, useMemo, useState } from 'react';
import { Link } from 'react-router';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery, useTableQuery } from '@/shared/hooks/api';
import { getAllExecutions } from '@/features/invocation-history/queries';
import { getStepSummaries } from '@/features/workflows/queries';
import {
  ExecutionHistoryFilters,
  ExecutionHistoryItem,
} from '@/features/invocation-history/types';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { formatDate } from '@/lib/utils';
import { cn } from '@/lib/utils';
import { Icons } from '@/shared/components/icons';
import { Badge } from '@/shared/components/ui/badge';
import { Button } from '@/shared/components/ui/button';
import {
  ExternalLink,
  Loader2,
  Zap,
  ChevronRight,
  ChevronDown,
  Copy,
  Database,
  Sparkles,
  Bug,
} from 'lucide-react';
import {
  getStatusDisplay,
  isActiveStatus,
} from '@/shared/utils/status-display';
import { toast } from 'sonner';
import { StructuredErrorDisplay } from '@/shared/components/StructuredErrorDisplay';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { resolvePayloadForCopy } from '@/shared/utils/truncated-payload';
import { PayloadPreBlock } from '@/shared/components/PayloadPreBlock';

interface HistoryPanelContentProps {
  workflowId: string;
}

const MAX_INSTANCES = 5;

interface StepSummary {
  stepId: string;
  stepName?: string | null;
  stepType?: string;
  status: string;
  durationMs?: number | null;
  inputs?: unknown;
  outputs?: unknown;
  error?: unknown;
  startedAt?: string;
}

/**
 * History tab content for the bottom panel.
 * Shows recent execution instances on the left, and selected instance's events on the right.
 */
export function HistoryPanelContent({ workflowId }: HistoryPanelContentProps) {
  // Get the selected invocation from the execution store (set by "View Details" button)
  const storeSelectedInvocationId = useExecutionStore(
    (s) => s.selectedInvocationId
  );
  const setStoreSelectedInvocationId = useExecutionStore(
    (s) => s.setSelectedInvocationId
  );

  const [selectedInstanceId, setSelectedInstanceId] = useState<string | null>(
    storeSelectedInvocationId
  );

  // Sync from store when it changes (e.g., when "View Details" is clicked)
  useEffect(() => {
    if (storeSelectedInvocationId) {
      setSelectedInstanceId(storeSelectedInvocationId);
      // Clear it from store after consuming
      setStoreSelectedInvocationId(null);
    }
  }, [storeSelectedInvocationId, setStoreSelectedInvocationId]);

  const filters: ExecutionHistoryFilters = useMemo(
    () => ({
      workflowId,
      sortBy: 'createdAt',
      sortOrder: 'desc',
    }),
    [workflowId]
  );

  const { data, isFetching } = useTableQuery({
    queryKey: queryKeys.executions.list({
      pageIndex: 0,
      pageSize: MAX_INSTANCES,
      filters,
    }),
    queryFn: getAllExecutions,
    enabled: Boolean(workflowId),
    staleTime: 0,
    refetchOnMount: 'always',
    refetchInterval: 10000, // Poll every 10 seconds
  });

  // Auto-select the first instance when data loads (only if nothing is selected)
  useEffect(() => {
    if (data && data.length > 0 && !selectedInstanceId) {
      setSelectedInstanceId(data[0].instanceId);
    }
  }, [data, selectedInstanceId]);

  // Fetch step summaries for the selected instance
  const stepFilters = useMemo(
    () => ({
      limit: 100,
      offset: 0,
      sortOrder: 'asc' as const,
    }),
    []
  );

  const { data: stepSummariesData, isLoading: stepsLoading } = useCustomQuery({
    queryKey: queryKeys.workflows.stepSummaries(
      workflowId,
      selectedInstanceId,
      stepFilters
    ),
    queryFn: (token: string) =>
      getStepSummaries(token, workflowId, selectedInstanceId!, stepFilters),
    enabled: !!selectedInstanceId,
    refetchInterval: () => {
      const selectedInstance = data?.find(
        (inst: ExecutionHistoryItem) => inst.instanceId === selectedInstanceId
      );
      return selectedInstance && isActiveStatus(selectedInstance.status)
        ? 2000
        : false;
    },
  });

  const stepSummaries = (stepSummariesData?.data?.steps || []) as StepSummary[];

  if (isFetching && (!data || data.length === 0)) {
    return (
      <div className="flex-1 overflow-hidden p-3">
        <div className="space-y-2">
          {[...Array(3)].map((_, i) => (
            <div
              key={i}
              className="flex items-center gap-3 py-2 px-2 rounded bg-muted/30"
            >
              <div className="h-6 w-20 rounded bg-muted/60 animate-pulse" />
              <div className="h-4 flex-1 rounded bg-muted/60 animate-pulse" />
              <div className="h-4 w-16 rounded bg-muted/60 animate-pulse" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (!data || data.length === 0) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center text-muted-foreground">
        <Icons.inbox className="h-8 w-8 mb-2 opacity-60" />
        <p className="text-sm font-medium">No executions yet</p>
        <p className="text-xs">Run the workflow to see history here</p>
      </div>
    );
  }

  const selectedInstance = data.find(
    (inst: ExecutionHistoryItem) => inst.instanceId === selectedInstanceId
  );
  const isSelectedActive = selectedInstance
    ? isActiveStatus(selectedInstance.status)
    : false;

  return (
    <div className="flex flex-1 min-h-0 overflow-hidden">
      {/* Left panel - Invocations list */}
      <div className="w-72 border-r flex-shrink-0 flex flex-col overflow-hidden">
        <div className="flex items-center justify-between px-3 py-1.5 border-b bg-muted/20">
          <span className="text-xs font-medium text-muted-foreground uppercase tracking-wider">
            Recent Executions
          </span>
          <Link
            to={`/invocation-history?workflowId=${workflowId}`}
            className="flex items-center gap-1 text-xs text-primary hover:underline"
          >
            View all
            <ExternalLink className="h-3 w-3" />
          </Link>
        </div>
        <div className="flex-1 overflow-y-auto">
          {data.map((instance: ExecutionHistoryItem) => {
            const isSelected = selectedInstanceId === instance.instanceId;
            const statusInfo = getStatusDisplay(instance.status);
            const isActive = isActiveStatus(instance.status);

            return (
              <div
                key={instance.instanceId}
                className={cn(
                  'grid grid-cols-[1fr_auto] items-center gap-2 px-3 py-2 border-b cursor-pointer transition-colors',
                  'hover:bg-muted/50',
                  isSelected && 'bg-accent border-l-2 border-l-primary'
                )}
                onClick={() => setSelectedInstanceId(instance.instanceId)}
              >
                <div className="min-w-0">
                  <div className="text-xs font-medium truncate">
                    {formatDate(instance.createdAt)}
                  </div>
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground mt-0.5">
                    <Badge
                      variant="outline"
                      className="text-[10px] px-1 py-0 h-4"
                    >
                      v{instance.version}
                    </Badge>
                    {instance.executionDurationSeconds !== null &&
                      instance.executionDurationSeconds !== undefined && (
                        <span>
                          • {instance.executionDurationSeconds.toFixed(1)}s
                        </span>
                      )}
                  </div>
                </div>

                <div className="flex items-center gap-1 flex-shrink-0">
                  {instance.status === ExecutionStatus.Suspended && (
                    <button
                      type="button"
                      className="inline-flex h-5 w-5 items-center justify-center rounded text-orange-600 hover:bg-orange-100 hover:text-orange-700"
                      title="Reattach — resume debugging"
                      onClick={(e) => {
                        e.stopPropagation();
                        const store = useExecutionStore.getState();
                        if (!store.executingInstanceId) {
                          store.startExecution(
                            instance.instanceId,
                            workflowId!,
                            true,
                            true
                          );
                        }
                      }}
                    >
                      <Bug className="h-3 w-3" />
                    </button>
                  )}
                  <Badge variant={statusInfo.variant} className="text-[10px]">
                    {isActive && statusInfo.showSpinner && (
                      <Loader2 className="h-2.5 w-2.5 mr-1 animate-spin" />
                    )}
                    {statusInfo.text}
                  </Badge>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Right panel - Events history */}
      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {/* Events header */}
        <div className="flex items-center justify-between px-3 py-1.5 border-b bg-muted/20">
          <div className="flex items-center gap-2">
            <div className="rounded bg-amber-500/10 p-1">
              <Zap className="h-3.5 w-3.5 text-amber-600" />
            </div>
            <span className="text-xs font-medium">Events</span>
            {stepSummaries.length > 0 && (
              <span className="text-[10px] text-muted-foreground">
                ({stepSummaries.length} steps)
              </span>
            )}
          </div>
        </div>

        {/* Loading bar for active executions */}
        {isSelectedActive && (
          <div className="h-0.5 bg-muted relative overflow-hidden flex-shrink-0">
            <div className="absolute inset-0 w-1/2 bg-gradient-to-r from-transparent via-primary to-transparent animate-[loading_1.5s_ease-in-out_infinite]" />
          </div>
        )}

        {/* Events table */}
        <div className="flex-1 overflow-y-auto">
          {stepsLoading && stepSummaries.length === 0 ? (
            <div className="flex items-center justify-center h-full">
              <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
            </div>
          ) : stepSummaries.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full text-center">
              <ChevronRight className="h-6 w-6 text-muted-foreground/30 mb-1" />
              <p className="text-xs text-muted-foreground">No events yet</p>
            </div>
          ) : (
            <table className="w-full text-xs">
              <thead className="bg-background sticky top-0 z-10 shadow-sm">
                <tr className="border-b">
                  <th className="text-left font-medium text-muted-foreground px-3 py-1.5 w-10 bg-background">
                    #
                  </th>
                  <th className="text-left font-medium text-muted-foreground px-3 py-1.5 bg-background">
                    Step Name
                  </th>
                  <th className="text-right font-medium text-muted-foreground px-3 py-1.5 w-20 bg-background">
                    Duration
                  </th>
                  <th className="text-right font-medium text-muted-foreground px-3 py-1.5 w-24 bg-background">
                    Status
                  </th>
                </tr>
              </thead>
              <tbody>
                {stepSummaries.map((step, index) => (
                  <EventRow
                    key={step.stepId || index}
                    step={step}
                    sequence={index + 1}
                  />
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </div>
  );
}

// Table row component for events
function EventRow({ step, sequence }: { step: StepSummary; sequence: number }) {
  const [isExpanded, setIsExpanded] = useState(false);

  const isRunning = step.status === 'running';
  const isCompleted = step.status === 'completed';
  const isFailed = step.status === 'failed';

  const hasError = step.error != null;
  const errorString = hasError
    ? typeof step.error === 'string'
      ? step.error
      : JSON.stringify(step.error)
    : null;
  const hasInputs =
    step.inputs != null &&
    typeof step.inputs === 'object' &&
    Object.keys(step.inputs).length > 0;
  const hasOutputs = step.outputs != null;

  const getBadgeVariant = ():
    | 'default'
    | 'secondary'
    | 'destructive'
    | 'outline' => {
    if (isFailed) return 'destructive';
    if (isCompleted) return 'default';
    if (isRunning) return 'secondary';
    return 'outline';
  };

  const getRowClass = () => {
    if (isFailed) return 'bg-destructive/5';
    if (isRunning) return 'bg-blue-500/5';
    return '';
  };

  const getStatusLabel = () => {
    if (isRunning) return 'Running';
    if (isCompleted) return 'Completed';
    if (isFailed) return 'Failed';
    return step.status;
  };

  const handleCopy = (data: unknown, label: string) => {
    navigator.clipboard.writeText(resolvePayloadForCopy(data));
    toast.success(`${label} copied`);
  };

  return (
    <>
      <tr
        className={cn(
          'border-b cursor-pointer hover:bg-muted/50 transition-colors',
          getRowClass()
        )}
        onClick={() => setIsExpanded(!isExpanded)}
      >
        <td className="px-3 py-1.5 text-muted-foreground">
          <div className="flex items-center gap-1">
            {isExpanded ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronRight className="h-3 w-3" />
            )}
            {sequence}
          </div>
        </td>
        <td className="px-3 py-1.5">
          <span className="truncate block" title={step.stepName || step.stepId}>
            {step.stepName || step.stepId}
          </span>
        </td>
        <td className="px-3 py-1.5 text-right text-muted-foreground">
          {step.durationMs !== undefined && step.durationMs !== null
            ? `${step.durationMs}ms`
            : '-'}
        </td>
        <td className="px-3 py-1.5 text-right">
          <Badge
            variant={getBadgeVariant()}
            className="text-[10px] px-1.5 py-0"
          >
            {isRunning && (
              <Loader2 className="h-2.5 w-2.5 mr-0.5 animate-spin" />
            )}
            {getStatusLabel()}
          </Badge>
        </td>
      </tr>

      {/* Expandable details row */}
      {isExpanded && (
        <tr className="bg-muted/20">
          <td colSpan={4} className="px-3 py-2">
            <div className="space-y-2 text-xs">
              {hasError && errorString && (
                <StructuredErrorDisplay error={errorString} mode="compact" />
              )}

              {hasInputs && (
                <div>
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-[10px] font-semibold text-muted-foreground flex items-center gap-1">
                      <Database className="h-2.5 w-2.5" />
                      Inputs
                    </span>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-5 text-[10px] px-1"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleCopy(step.inputs, 'Inputs');
                      }}
                    >
                      <Copy className="h-2.5 w-2.5 mr-0.5" />
                      Copy
                    </Button>
                  </div>
                  <PayloadPreBlock
                    data={step.inputs}
                    className="max-h-24"
                    textClassName="text-[10px]"
                  />
                </div>
              )}

              {hasOutputs && (
                <div>
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-[10px] font-semibold text-muted-foreground flex items-center gap-1">
                      <Sparkles className="h-2.5 w-2.5" />
                      Outputs
                    </span>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-5 text-[10px] px-1"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleCopy(step.outputs, 'Outputs');
                      }}
                    >
                      <Copy className="h-2.5 w-2.5 mr-0.5" />
                      Copy
                    </Button>
                  </div>
                  <PayloadPreBlock
                    data={step.outputs}
                    className="max-h-24"
                    textClassName="text-[10px]"
                  />
                </div>
              )}

              {!hasInputs && !hasOutputs && !hasError && (
                <p className="text-[10px] text-muted-foreground">
                  No input/output data available.
                </p>
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
