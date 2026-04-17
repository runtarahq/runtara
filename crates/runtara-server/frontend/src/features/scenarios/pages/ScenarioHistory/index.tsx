import { useNavigate, useParams } from 'react-router';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Loader } from '@/shared/components/loader.tsx';
import { formatDate } from '@/lib/utils.ts';
import { Badge } from '@/shared/components/ui/badge.tsx';
import { Button } from '@/shared/components/ui/button';
import {
  ChevronLeft,
  ChevronDown,
  FileText,
  Clock,
  Calendar,
  Timer,
  MemoryStick,
  RotateCw,
  XCircle,
  Loader2,
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
  List,
  BarChart3,
  ChevronsLeft,
  ChevronsRight,
} from 'lucide-react';
import {
  getScenarioInstance,
  getStepSummaries,
  getPendingInput,
  deliverSignal,
  type PendingInput,
} from '@/features/scenarios/queries';
import { useRef, useState, useMemo, useCallback } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useCustomMutation } from '@/shared/hooks/api';
import { useToken } from '@/shared/hooks';
import { HumanInputCard } from '@/features/scenarios/components/ExecutionPanel/HumanInputCard';
import { ReplayButton } from '@/features/scenarios/components/ReplayButton';
import { ResumeButton } from '@/features/scenarios/components/ResumeButton';
import { StopButton } from '@/features/scenarios/components/StopButton';
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
} from '@/shared/utils/status-display';
import { ExecutionTimeline } from '@/features/scenarios/components/ExecutionTimeline';
import { Tabs, TabsList, TabsTrigger } from '@/shared/components/ui/tabs';

const LIST_PAGE_SIZE = 20;

export function ScenarioHistory() {
  const { scenarioId, instanceId } = useParams();
  const navigate = useNavigate();
  const isInitialLoadRef = useRef(true);

  const [expandedSteps, setExpandedSteps] = useState<Set<number>>(new Set());
  const [eventsViewMode, setEventsViewMode] = useState<'list' | 'timeline'>(
    'timeline'
  );
  const [listPageIndex, setListPageIndex] = useState(0);

  const token = useToken();
  const queryClient = useQueryClient();

  const { data, isLoading, isError } = useCustomQuery({
    queryKey: queryKeys.scenarios.instance(scenarioId ?? '', instanceId ?? ''),
    queryFn: (token: string) =>
      getScenarioInstance(token, scenarioId!, instanceId!),
    enabled: !!scenarioId && !!instanceId,
    refetchInterval: (query): number | false =>
      isActiveStatus(query.state.data?.status) ? 10000 : false,
  });

  // Poll pending input while instance is Running
  const { data: pendingInputData } = useCustomQuery<PendingInput[]>({
    queryKey: queryKeys.scenarios.pendingInput(
      scenarioId ?? '',
      instanceId ?? ''
    ),
    queryFn: (token: string) =>
      getPendingInput(token, scenarioId!, instanceId!),
    enabled: !!scenarioId && !!instanceId && isActiveStatus(data?.status),
    refetchInterval: () => {
      return isActiveStatus(data?.status) ? 3000 : false;
    },
  });

  const pendingInputs = pendingInputData ?? [];

  // Signal delivery mutation
  const signalMutation = useCustomMutation({
    mutationFn: (
      _token: any,
      payload: { signalId: string; payload: Record<string, any> }
    ) => {
      if (!instanceId) return Promise.reject(new Error('Missing instanceId'));
      return deliverSignal(token, instanceId, payload);
    },
    onSuccess: () => {
      toast.success('Response submitted successfully');
      queryClient.invalidateQueries({
        queryKey: queryKeys.scenarios.pendingInput(
          scenarioId ?? '',
          instanceId ?? ''
        ),
      });
    },
  });

  const handleSignalSubmit = (
    signalId: string,
    payload: Record<string, any>
  ) => {
    signalMutation.mutate({ signalId, payload });
  };

  // Filters for list view with pagination (oldest first)
  const listFilters = useMemo(
    () => ({
      limit: LIST_PAGE_SIZE,
      offset: listPageIndex * LIST_PAGE_SIZE,
      sortOrder: 'asc' as const,
    }),
    [listPageIndex]
  );

  // Fetch step summaries for list view (paginated, paired events)
  const { data: stepSummariesData, isFetching: isListFetching } =
    useCustomQuery({
      queryKey: queryKeys.scenarios.stepSummaries(
        scenarioId ?? '',
        instanceId ?? '',
        listFilters
      ),
      queryFn: (token: string) =>
        getStepSummaries(token, scenarioId!, instanceId!, listFilters),
      enabled: !!scenarioId && !!instanceId && eventsViewMode === 'list',
      refetchInterval: isActiveStatus(data?.status) ? 10000 : false,
    });

  // Step summaries for list view (paired events from API)
  const stepSummaries = stepSummariesData?.data?.steps || [];

  // Pagination info for list view
  const totalCount = stepSummariesData?.data?.totalCount || 0;
  const totalPages = Math.ceil(totalCount / LIST_PAGE_SIZE);

  // Reset page when switching to list view
  const handleViewModeChange = useCallback((mode: 'list' | 'timeline') => {
    setEventsViewMode(mode);
    if (mode === 'list') {
      setListPageIndex(0);
    }
  }, []);

  const toggleStepExpanded = (index: number) => {
    setExpandedSteps((prev) => {
      const next = new Set(prev);
      if (next.has(index)) {
        next.delete(index);
      } else {
        next.add(index);
      }
      return next;
    });
  };

  // Set page title with scenario name from metadata if available
  usePageTitle(
    data?.metadata?.scenarioName
      ? `Scenario History - ${data.metadata.scenarioName}`
      : 'Scenario History'
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
      navigate('/scenarios');
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
          Please try again or go back to scenarios.
        </p>
      </div>
    );
  }

  // Guard against undefined data
  if (!data) {
    return <Loader />;
  }

  return (
    <div className="py-8 px-4 sm:px-6 lg:px-8 max-w-7xl mx-auto">
      {/* Header Section */}
      <div className="mb-8">
        <div className="flex items-center gap-3 mb-4">
          <Button
            variant="ghost"
            size="icon"
            className="rounded-full hover:bg-muted"
            onClick={handleBack}
          >
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <div className="flex-1">
            <h1 className="text-3xl font-bold tracking-tight">
              Scenario Execution Details
            </h1>
            {data?.metadata?.scenarioName && (
              <p className="text-muted-foreground mt-1">
                {data.metadata.scenarioName}
                {data.metadata.scenarioDescription && (
                  <span className="text-sm ml-2">
                    • {data.metadata.scenarioDescription}
                  </span>
                )}
              </p>
            )}
          </div>
          <div className="flex gap-2">
            {data && data.id && (
              <>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    navigate(`/scenarios/${scenarioId}/history/${data.id}/logs`)
                  }
                  className="flex items-center gap-2"
                >
                  <FileText className="h-4 w-4" />
                  View Logs
                </Button>
                {isActiveStatus(data.status) ? (
                  <StopButton
                    instanceId={data.id}
                    variant="outline"
                    size="sm"
                  />
                ) : (
                  <>
                    {(data.status === 'failed' ||
                      data.status === 'cancelled') && (
                      <ResumeButton
                        instanceId={data.id}
                        variant="outline"
                        size="sm"
                      />
                    )}
                    <ReplayButton
                      instanceId={data.id}
                      error={data.metadata?.errorMessage}
                      variant="outline"
                      size="sm"
                    />
                  </>
                )}
              </>
            )}
          </div>
        </div>

        {/* Status Overview Bar */}
        {data && (
          <div className="flex items-center gap-4 p-4 bg-muted/50 rounded-lg border">
            <div
              className="flex items-center gap-2 cursor-help"
              title="Execution Status - Shows the current state of your scenario execution"
            >
              {(() => {
                const statusInfo = getStatusDisplay(data.status);
                return (
                  <>
                    {statusInfo.showSpinner && (
                      <Loader2 className="h-4 w-4 animate-spin" />
                    )}
                    <Badge variant={statusInfo.variant}>
                      {statusInfo.text}
                    </Badge>
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
                      className="flex items-center gap-2 cursor-help"
                      title="Termination Type - Provides context for why this execution terminated"
                    >
                      <Info className="h-4 w-4 text-muted-foreground" />
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
                  className="flex items-center gap-2 text-sm text-muted-foreground cursor-help"
                  title="Unique identifier for this scenario execution. Use this ID when reporting issues or tracking specific runs."
                >
                  <Hash className="h-4 w-4" />
                  <span className="font-mono">{data.id}</span>
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* Pending Human Input Cards */}
      {pendingInputs.length > 0 && (
        <div className="space-y-3 mb-6">
          {pendingInputs.map((pi) => (
            <HumanInputCard
              key={pi.signalId}
              pendingInput={pi}
              onSubmit={handleSignalSubmit}
              isSubmitting={signalMutation.isPending}
            />
          ))}
        </div>
      )}

      <div className="space-y-6">
        {/* Timing and Performance Metrics */}
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          {/* Execution Duration Card */}
          {data.executionDurationSeconds !== undefined &&
            data.executionDurationSeconds !== null && (
              <Card>
                <CardHeader className="pb-3">
                  <CardTitle
                    className="text-sm font-medium flex items-center gap-2 text-muted-foreground cursor-help"
                    title="The total time spent actively running your scenario, from start to finish. This doesn't include time waiting in queue."
                  >
                    <Timer className="h-4 w-4" />
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
                    className="text-sm font-medium flex items-center gap-2 text-muted-foreground cursor-help"
                    title="The time your scenario spent waiting before it could start running. During busy periods, scenarios may need to wait for available resources."
                  >
                    <Clock className="h-4 w-4" />
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
                  className="text-sm font-medium flex items-center gap-2 text-muted-foreground cursor-help"
                  title="The maximum amount of memory (RAM) used while running this scenario. Higher values indicate more data was being processed at once."
                >
                  <MemoryStick className="h-4 w-4" />
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
                    className="text-sm font-medium flex items-center gap-2 text-muted-foreground cursor-help"
                    title="Shows how many times this scenario was retried after encountering issues (current / maximum allowed). Automatic retries help ensure your scenarios complete successfully."
                  >
                    <RotateCw className="h-4 w-4" />
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
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Calendar className="h-5 w-5" />
              Timeline & Details
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
              {/* Timeline */}
              <div className="space-y-4">
                <div className="flex items-start gap-3">
                  <div className="rounded-full bg-slate-500/10 p-2 mt-0.5">
                    <Calendar className="h-4 w-4 text-slate-600" />
                  </div>
                  <div className="flex-1 space-y-1">
                    <p className="text-sm font-medium">Created</p>
                    <p className="text-sm text-muted-foreground">
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
                  <div className="flex items-start gap-3">
                    <div className="rounded-full bg-blue-500/10 p-2 mt-0.5">
                      <Zap className="h-4 w-4 text-blue-600" />
                    </div>
                    <div className="flex-1 space-y-1">
                      <p className="text-sm font-medium">Started Execution</p>
                      <p className="text-sm text-muted-foreground">
                        {formatDate(
                          new Date(data.metadata.startedAt),
                          'yyyy-MM-dd HH:mm:ss'
                        )}
                      </p>
                    </div>
                  </div>
                )}

                {data.metadata?.completedAt && (
                  <div className="flex items-start gap-3">
                    <div className="rounded-full bg-green-500/10 p-2 mt-0.5">
                      <Flag className="h-4 w-4 text-green-600" />
                    </div>
                    <div className="flex-1 space-y-1">
                      <p className="text-sm font-medium">Completed</p>
                      <p className="text-sm text-muted-foreground">
                        {formatDate(
                          new Date(data.metadata.completedAt),
                          'yyyy-MM-dd HH:mm:ss'
                        )}
                      </p>
                    </div>
                  </div>
                )}
              </div>

              {/* Additional Details */}
              <div className="space-y-4">
                {data.usedVersion !== undefined && (
                  <div className="flex justify-between items-center py-2 border-b">
                    <span className="text-sm text-muted-foreground">
                      Version
                    </span>
                    <Badge variant="outline">v{data.usedVersion}</Badge>
                  </div>
                )}

                {data.processingOverheadSeconds !== undefined &&
                  data.processingOverheadSeconds !== null && (
                    <div className="flex justify-between items-center py-2 border-b">
                      <span
                        className="text-sm text-muted-foreground flex items-center gap-1 cursor-help"
                        title="The time spent on setup and coordination tasks before and after running your scenario. This is separate from the actual execution time."
                      >
                        Processing Overhead
                      </span>
                      <span className="text-sm font-medium">
                        {data.processingOverheadSeconds.toFixed(2)}s
                      </span>
                    </div>
                  )}

                {data.metadata?.workerId && (
                  <div className="flex justify-between items-center py-2 border-b">
                    <span
                      className="text-sm text-muted-foreground flex items-center gap-2 cursor-help"
                      title="A unique identifier for the server that processed this scenario. Useful for troubleshooting and tracking which server handled your request."
                    >
                      <Server className="h-4 w-4" />
                      Worker ID
                    </span>
                    <span className="text-sm font-mono text-muted-foreground">
                      {data.metadata.workerId}
                    </span>
                  </div>
                )}
              </div>
            </div>

            {/* Error Message */}
            {data.metadata?.errorMessage && (
              <div className="mt-6">
                <div className="flex items-start gap-2 mb-2">
                  <XCircle className="h-5 w-5 text-destructive mt-0.5" />
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
                <div className="flex items-center gap-2 mb-3">
                  <Tag className="h-4 w-4 text-muted-foreground" />
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

        {/* Inputs & Outputs */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          <Card className="overflow-hidden">
            <CardHeader className="bg-gradient-to-r from-blue-500/10 to-blue-500/5 border-b">
              <div className="flex items-center justify-between">
                <CardTitle className="flex items-center gap-2 text-lg">
                  <div className="rounded-lg bg-blue-500/10 p-2">
                    <Database className="h-5 w-5 text-blue-600" />
                  </div>
                  Input Data
                </CardTitle>
                {data.inputs && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-8 gap-2"
                    onClick={() => {
                      navigator.clipboard.writeText(
                        JSON.stringify(data.inputs, null, 2)
                      );
                      const btn = document.getElementById('copy-inputs');
                      if (btn) {
                        btn.innerHTML =
                          '<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"></path></svg>';
                        setTimeout(() => {
                          btn.innerHTML =
                            '<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"></path></svg>';
                        }, 2000);
                      }
                    }}
                  >
                    <span id="copy-inputs">
                      <Copy className="h-4 w-4" />
                    </span>
                    Copy
                  </Button>
                )}
              </div>
            </CardHeader>
            <CardContent className="p-0 bg-muted/30">
              {data.inputs ? (
                <div className="relative">
                  <pre className="text-xs font-mono p-6 overflow-auto max-h-[500px]">
                    <code className="text-foreground">
                      {JSON.stringify(data.inputs, null, 2)}
                    </code>
                  </pre>
                </div>
              ) : (
                <div className="p-12 text-center">
                  <div className="inline-flex items-center justify-center w-12 h-12 rounded-full bg-muted mb-3">
                    <Database className="h-6 w-6 text-muted-foreground" />
                  </div>
                  <p className="text-sm text-muted-foreground mb-1">
                    No input data
                  </p>
                  <p className="text-xs text-muted-foreground">
                    This scenario was executed without input parameters
                  </p>
                </div>
              )}
            </CardContent>
          </Card>

          <Card className="overflow-hidden">
            <CardHeader className="bg-gradient-to-r from-green-500/10 to-green-500/5 border-b">
              <div className="flex items-center justify-between">
                <CardTitle className="flex items-center gap-2 text-lg">
                  <div className="rounded-lg bg-green-500/10 p-2">
                    <Sparkles className="h-5 w-5 text-green-600" />
                  </div>
                  Output Data
                </CardTitle>
                {data.outputs && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-8 gap-2"
                    onClick={() => {
                      const outputData = (() => {
                        try {
                          // If outputs is already an object, stringify it directly
                          if (typeof data.outputs === 'object') {
                            return JSON.stringify(data.outputs, null, 2);
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
                      })();
                      navigator.clipboard.writeText(outputData);
                      const btn = document.getElementById('copy-outputs');
                      if (btn) {
                        btn.innerHTML =
                          '<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"></path></svg>';
                        setTimeout(() => {
                          btn.innerHTML =
                            '<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"></path></svg>';
                        }, 2000);
                      }
                    }}
                  >
                    <span id="copy-outputs">
                      <Copy className="h-4 w-4" />
                    </span>
                    Copy
                  </Button>
                )}
              </div>
            </CardHeader>
            <CardContent className="p-0">
              {data.outputs ? (
                <div className="relative bg-muted/30">
                  <pre className="text-xs font-mono p-6 overflow-auto max-h-[500px]">
                    <code className="text-foreground">
                      {(() => {
                        try {
                          // If outputs is already an object, stringify it directly
                          if (typeof data.outputs === 'object') {
                            return JSON.stringify(data.outputs, null, 2);
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
                </div>
              ) : (
                <div className="p-12 text-center">
                  <div className="inline-flex items-center justify-center w-12 h-12 rounded-full bg-muted mb-3">
                    <Sparkles className="h-6 w-6 text-muted-foreground" />
                  </div>
                  <p className="text-sm text-muted-foreground mb-1">
                    No output data yet
                  </p>
                  <p className="text-xs text-muted-foreground">
                    Output will be available once the scenario completes
                  </p>
                </div>
              )}
            </CardContent>
          </Card>
        </div>

        {/* Events Section */}
        <Card className="overflow-hidden">
          <CardHeader className="bg-gradient-to-r from-purple-500/10 to-purple-500/5 border-b">
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-lg">
                <div className="rounded-lg bg-purple-500/10 p-2">
                  <ChevronRight className="h-5 w-5 text-purple-600" />
                </div>
                Events
              </CardTitle>
              <Tabs
                value={eventsViewMode}
                onValueChange={(v) =>
                  handleViewModeChange(v as 'list' | 'timeline')
                }
              >
                <TabsList className="grid w-[200px] grid-cols-2">
                  <TabsTrigger
                    value="timeline"
                    className="flex items-center gap-1"
                  >
                    <BarChart3 className="h-4 w-4" />
                    Timeline
                  </TabsTrigger>
                  <TabsTrigger value="list" className="flex items-center gap-1">
                    <List className="h-4 w-4" />
                    List
                  </TabsTrigger>
                </TabsList>
              </Tabs>
            </div>
            <p className="text-sm text-muted-foreground mt-2">
              Step execution events showing inputs, outputs, and status for each
              step in your scenario
            </p>
          </CardHeader>
          <CardContent className="p-6">
            {eventsViewMode === 'timeline' ? (
              <ExecutionTimeline
                scenarioId={scenarioId!}
                instanceId={instanceId!}
              />
            ) : isListFetching ? (
              <div className="flex items-center justify-center py-16">
                <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
              </div>
            ) : stepSummaries.length > 0 ? (
              <div className="space-y-3">
                {stepSummaries.map((step: any, index: number) => {
                  // Calculate global sequence number based on pagination
                  const globalSequence = listPageIndex * LIST_PAGE_SIZE + index;
                  const isExpanded = expandedSteps.has(index);

                  // Extract data from step summary
                  const isRunning = step.status === 'running';
                  const isCompleted = step.status === 'completed';
                  const isFailed = step.status === 'failed';

                  // Get badge variant based on status
                  const getBadgeVariant = () => {
                    if (isFailed) return 'destructive';
                    if (isCompleted) return 'default';
                    if (isRunning) return 'secondary';
                    return 'outline';
                  };

                  // Get border color based on status
                  const getBorderClass = () => {
                    if (isFailed)
                      return 'border-destructive/50 bg-destructive/5';
                    if (isCompleted)
                      return 'border-green-500/50 bg-green-500/5';
                    if (isRunning) return 'border-blue-500/50 bg-blue-500/5';
                    return 'border-border';
                  };

                  // Capitalize status for display
                  const getStatusLabel = () => {
                    if (isRunning) return 'Running';
                    if (isCompleted) return 'Completed';
                    if (isFailed) return 'Failed';
                    return step.status;
                  };

                  return (
                    <div
                      key={step.stepId || index}
                      className={`border rounded-lg p-4 space-y-3 transition-colors ${getBorderClass()}`}
                    >
                      <div className="flex items-center justify-between">
                        <button
                          onClick={() => toggleStepExpanded(index)}
                          className="flex items-center gap-2 font-medium hover:text-purple-600 transition-colors"
                        >
                          {isExpanded ? (
                            <ChevronDown className="h-4 w-4" />
                          ) : (
                            <ChevronRight className="h-4 w-4" />
                          )}
                          <span className="font-semibold">
                            #{globalSequence + 1} -{' '}
                            {step.stepName || step.stepId}
                          </span>
                        </button>
                        <Badge variant={getBadgeVariant()}>
                          {isRunning && (
                            <Loader2 className="h-3 w-3 mr-1 animate-spin" />
                          )}
                          {getStatusLabel()}
                        </Badge>
                      </div>

                      <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
                        <Badge variant="outline" className="font-mono text-xs">
                          {step.stepType}
                        </Badge>
                        {step.startedAt && (
                          <>
                            <span>•</span>
                            <span className="flex items-center gap-1">
                              <Calendar className="h-3 w-3" />
                              {formatDate(step.startedAt)}
                            </span>
                          </>
                        )}
                        {step.durationMs !== undefined &&
                          step.durationMs !== null && (
                            <>
                              <span>•</span>
                              <span className="flex items-center gap-1">
                                <Clock className="h-3 w-3" />
                                {step.durationMs}ms
                              </span>
                            </>
                          )}
                      </div>

                      {step.error && (
                        <StructuredErrorDisplay
                          error={step.error}
                          mode="compact"
                        />
                      )}

                      {/* Expandable Inputs/Outputs */}
                      {isExpanded && (
                        <div className="mt-3 pt-3 border-t space-y-4">
                          {/* Inputs */}
                          {step.inputs && (
                            <div>
                              <div className="flex items-center justify-between mb-2">
                                <span className="text-sm font-semibold text-muted-foreground flex items-center gap-1">
                                  <Database className="h-3 w-3" />
                                  Inputs
                                </span>
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  className="h-6 text-xs"
                                  onClick={() => {
                                    navigator.clipboard.writeText(
                                      JSON.stringify(step.inputs, null, 2)
                                    );
                                    toast.success('Inputs copied');
                                  }}
                                >
                                  <Copy className="h-3 w-3 mr-1" />
                                  Copy
                                </Button>
                              </div>
                              <pre className="text-xs bg-muted p-3 rounded overflow-x-auto max-h-60 overflow-y-auto font-mono">
                                {JSON.stringify(step.inputs, null, 2)}
                              </pre>
                            </div>
                          )}

                          {/* Outputs */}
                          {step.outputs && (
                            <div>
                              <div className="flex items-center justify-between mb-2">
                                <span className="text-sm font-semibold text-muted-foreground flex items-center gap-1">
                                  <Sparkles className="h-3 w-3" />
                                  Outputs
                                </span>
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  className="h-6 text-xs"
                                  onClick={() => {
                                    navigator.clipboard.writeText(
                                      JSON.stringify(step.outputs, null, 2)
                                    );
                                    toast.success('Outputs copied');
                                  }}
                                >
                                  <Copy className="h-3 w-3 mr-1" />
                                  Copy
                                </Button>
                              </div>
                              <pre className="text-xs bg-muted p-3 rounded overflow-x-auto max-h-60 overflow-y-auto font-mono">
                                {JSON.stringify(step.outputs, null, 2)}
                              </pre>
                            </div>
                          )}

                          {!step.inputs && !step.outputs && (
                            <p className="text-sm text-muted-foreground">
                              No input/output data available for this step.
                            </p>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })}

                {/* Pagination Controls */}
                {totalPages > 1 && (
                  <div className="flex items-center justify-between pt-4 border-t mt-4">
                    <div className="text-sm text-muted-foreground">
                      Showing {listPageIndex * LIST_PAGE_SIZE + 1} -{' '}
                      {Math.min(
                        (listPageIndex + 1) * LIST_PAGE_SIZE,
                        totalCount
                      )}{' '}
                      of {totalCount} events
                    </div>
                    <div className="flex items-center gap-2">
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => setListPageIndex(0)}
                        disabled={listPageIndex === 0}
                      >
                        <ChevronsLeft className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() =>
                          setListPageIndex((prev) => Math.max(0, prev - 1))
                        }
                        disabled={listPageIndex === 0}
                      >
                        <ChevronLeft className="h-4 w-4" />
                      </Button>
                      <span className="text-sm text-muted-foreground px-2">
                        Page {listPageIndex + 1} of {totalPages}
                      </span>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() =>
                          setListPageIndex((prev) =>
                            Math.min(totalPages - 1, prev + 1)
                          )
                        }
                        disabled={listPageIndex >= totalPages - 1}
                      >
                        <ChevronRight className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => setListPageIndex(totalPages - 1)}
                        disabled={listPageIndex >= totalPages - 1}
                      >
                        <ChevronsRight className="h-4 w-4" />
                      </Button>
                    </div>
                  </div>
                )}
              </div>
            ) : (
              <div className="py-16 px-6 text-center">
                <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-purple-500/10 mb-4">
                  <ChevronRight className="h-8 w-8 text-purple-600" />
                </div>
                <h3 className="text-lg font-semibold mb-2">No Events Yet</h3>
                <p className="text-sm text-muted-foreground max-w-md mx-auto">
                  Events will appear here as your scenario executes. If your
                  scenario is still running, events may appear soon. Check back
                  or refresh the page to see the latest events.
                </p>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
