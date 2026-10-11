import { useManagedInputSubmissions } from '@/features/workflows/hooks/useManagedInputSubmissions';
import {
  ManagedInputScope,
  InputRetryPanel,
} from '@/features/workflows/components/ManagedInputSubmissions';
import React, { useState, useEffect, useLayoutEffect } from 'react';
import {
  Play,
  Pause,
  Clock,
  ChevronRight,
  ChevronDown,
  Hand,
} from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import { useHierarchicalTimeline } from '@/features/workflows/hooks/useHierarchicalTimeline';
import {
  HierarchicalStep,
  barLabelPlacement,
  estimateBarLabelPx,
  timelineBarPosition,
} from '@/features/workflows/types/timeline';
import { stepStatusDisplay } from '@/features/workflows/utils/step-status';
import { PayloadExplorer } from '@/shared/components/PayloadExplorer';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  getWorkflowInstance,
  getPendingInput,
  type PendingInput,
} from '@/features/workflows/queries';
import { HumanInputCard } from '@/features/workflows/components/ExecutionPanel/HumanInputCard';
import { isActiveStatus } from '@/shared/utils/status-display';
import { StepStatusBadge } from '@/features/workflows/components/StepStatusBadge';
import { getRunEventsEmptyState } from '@/features/workflows/utils/run-empty-state';
import { Spinner } from '@/shared/components/ui/spinner';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { stepTypeConfig } from './step-type-style';

interface ExecutionTimelineProps {
  workflowId: string;
  instanceId: string;
  showInputs?: boolean;
  /** Receives time spent per step type whenever the visible steps change. */
  onStatsChange?: (stats: ExecutionStats) => void;
}

/** Time spent per step type across the visible steps, out of `total`. */
export interface ExecutionStats {
  total: number;
  byType: Record<string, number>;
  /** Top-level steps the run executed. */
  stepCount: number;
}

// Status text colors via theme tokens (dark mode + theming for free).
const statusTextClasses: Record<string, string> = {
  completed: 'text-success',
  running: 'text-info',
  failed: 'text-destructive',
  pending: 'text-muted-foreground',
  waiting: 'text-warning',
  // Unfinished while its run is suspended (parked wait, Delay, signal).
  suspended: 'text-warning',
};

const formatTime = (ms: number | null | undefined): string => {
  if (ms === null || ms === undefined) return '-';
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
};

const formatTimestamp = (ts: number): string => {
  const date = new Date(ts);
  return date.toISOString().split('T')[1].replace('Z', '');
};

// Indentation per depth level
const INDENT_PX = 24;

export function ExecutionTimeline(props: ExecutionTimelineProps) {
  return (
    <ManagedInputScope>
      <ExecutionTimelineContent {...props} />
    </ManagedInputScope>
  );
}

function ExecutionTimelineContent({
  workflowId,
  instanceId,
  showInputs = true,
  onStatsChange,
}: ExecutionTimelineProps) {
  const [selectedStep, setSelectedStep] = useState<string | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const [playhead, setPlayhead] = useState(0);
  const [hoveredStep, setHoveredStep] = useState<string | null>(null);

  const submissions = useManagedInputSubmissions();

  const {
    visibleSteps,
    totalDuration,
    totalRootCount,
    stats,
    isLoadingRoot,
    hasMoreRootSteps,
    loadMoreRoot,
    toggleExpand,
  } = useHierarchicalTimeline(workflowId, instanceId);

  useEffect(() => {
    onStatsChange?.({
      total: stats.total,
      byType: stats.byType,
      stepCount: Math.max(totalRootCount, stats.rootStepCount),
    });
  }, [stats, totalRootCount, onStatsChange]);

  // Poll instance data for status
  const { data: instanceData } = useCustomQuery({
    queryKey: queryKeys.workflows.instance(workflowId, instanceId),
    queryFn: (token: string) =>
      getWorkflowInstance(token, workflowId, instanceId),
    enabled: !!workflowId && !!instanceId,
    refetchInterval: (data: any) => {
      return isActiveStatus(data?.status) ? 5000 : false;
    },
  });

  // Poll pending input while instance is active
  const { data: pendingInputData, error: pendingInputError } = useCustomQuery<
    PendingInput[]
  >({
    queryKey: queryKeys.workflows.pendingInput(workflowId, instanceId),
    queryFn: (token: string) => getPendingInput(token, workflowId, instanceId),
    enabled: !!workflowId && !!instanceId,
    placeholderData: undefined,
    refetchInterval: () => {
      return isActiveStatus(instanceData?.status) ? 3000 : false;
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
  const pendingInputs = (pendingInputData ?? []).filter(
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

  // Reset local state when workflow or instance changes
  useEffect(() => {
    setSelectedStep(null);
    setPlayhead(0);
    setIsPlaying(false);
    setHoveredStep(null);
  }, [workflowId, instanceId]);

  useEffect(() => {
    if (!isPlaying) return;
    const interval = setInterval(() => {
      setPlayhead((prev) => {
        if (prev >= totalDuration) {
          setIsPlaying(false);
          return totalDuration;
        }
        return prev + totalDuration / 100;
      });
    }, 50);
    return () => clearInterval(interval);
  }, [isPlaying, totalDuration]);

  // Playing from the end starts the replay over.
  const togglePlay = () => {
    if (!isPlaying && playhead >= totalDuration) setPlayhead(0);
    setIsPlaying(!isPlaying);
  };

  const seekToPointer = (e: React.PointerEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    if (rect.width <= 0) return;
    const frac = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    setPlayhead(frac * totalDuration);
  };

  const onScaleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const step = totalDuration / 100;
    const next =
      e.key === 'ArrowLeft'
        ? playhead - step
        : e.key === 'ArrowRight'
          ? playhead + step
          : e.key === 'Home'
            ? 0
            : e.key === 'End'
              ? totalDuration
              : null;
    if (next === null) return;
    e.preventDefault();
    setIsPlaying(false);
    setPlayhead(Math.min(totalDuration, Math.max(0, next)));
  };

  const getStepPosition = (step: HierarchicalStep) =>
    timelineBarPosition(step, totalDuration);

  // Track width, to tell when a bar is too narrow for its label and badge.
  const [trackEl, setTrackEl] = useState<HTMLDivElement | null>(null);
  const [trackPx, setTrackPx] = useState(0);
  useLayoutEffect(() => {
    if (!trackEl) return;
    const measure = () => setTrackPx(trackEl.clientWidth);
    measure();
    window.addEventListener('resize', measure);
    const observer =
      typeof ResizeObserver !== 'undefined'
        ? new ResizeObserver(measure)
        : null;
    observer?.observe(trackEl);
    return () => {
      window.removeEventListener('resize', measure);
      observer?.disconnect();
    };
  }, [trackEl]);

  const inputCards = showInputs && (
    <>
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
        <div className="space-y-3">
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
    </>
  );

  if (isLoadingRoot) {
    return (
      <>
        {inputCards}
        <div className="px-6 py-16 text-center">
          <div className="mb-4 inline-flex size-16 items-center justify-center rounded-full bg-purple-500/10">
            <Spinner className="size-8 text-purple-600" />
          </div>
          <h3 className="mb-2 text-lg font-semibold">Loading Timeline...</h3>
          <p className="mx-auto max-w-md text-sm text-muted-foreground">
            Fetching execution data for visualization.
          </p>
        </div>
      </>
    );
  }

  if (visibleSteps.length === 0) {
    const emptyState = getRunEventsEmptyState(
      instanceData?.status,
      'Timeline Events'
    );
    return (
      <>
        {inputCards}
        <div className="px-6 py-16 text-center">
          <div className="mb-4 inline-flex size-16 items-center justify-center rounded-full bg-purple-500/10">
            <Clock className="size-8 text-purple-600" />
          </div>
          <h3 className="mb-2 text-lg font-semibold">{emptyState.title}</h3>
          <p className="mx-auto max-w-md text-sm text-muted-foreground">
            {emptyState.description}
          </p>
        </div>
      </>
    );
  }

  // Renders flush inside its host card: the waterfall runs edge to edge and
  // the selected step's details follow as a section, not a nested card.
  return (
    <div>
      {inputCards && <div className="space-y-4 p-6 pb-0">{inputCards}</div>}

      {/* Timeline */}
      <div className="overflow-hidden">
        <div>
          {/* Time scale doubles as the playback control: play/pause and the
              playhead time on the left, click or drag the scale to seek. */}
          <div className="relative flex h-10 items-center border-b bg-muted/50">
            <div className="flex w-48 shrink-0 items-center gap-2 px-3">
              <Button
                variant="secondary"
                bordered
                size="icon-sm"
                onClick={togglePlay}
                className="rounded-full"
                aria-label={isPlaying ? 'Pause replay' : 'Replay run'}
              >
                {isPlaying ? (
                  <Pause className="size-3.5" />
                ) : (
                  <Play className="size-3.5" />
                )}
              </Button>
              <span className="font-mono text-xs tabular-nums text-muted-foreground">
                {formatTime(Math.round(playhead))}
              </span>
            </div>
            <div
              ref={setTrackEl}
              role="slider"
              tabIndex={0}
              aria-label="Replay position"
              aria-valuemin={0}
              aria-valuemax={Math.round(totalDuration)}
              aria-valuenow={Math.round(playhead)}
              aria-valuetext={formatTime(Math.round(playhead))}
              className="relative h-full flex-1 cursor-ew-resize touch-none select-none focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              onPointerDown={(e) => {
                e.currentTarget.setPointerCapture(e.pointerId);
                setIsPlaying(false);
                seekToPointer(e);
              }}
              onPointerMove={(e) => {
                if (e.currentTarget.hasPointerCapture(e.pointerId)) {
                  seekToPointer(e);
                }
              }}
              onKeyDown={onScaleKeyDown}
            >
              {[0, 25, 50, 75, 100].map((pct) => (
                <div
                  key={pct}
                  className={`pointer-events-none absolute top-1/2 -translate-y-1/2 px-1 text-xs text-muted-foreground ${
                    pct === 0
                      ? ''
                      : pct === 100
                        ? '-translate-x-full'
                        : '-translate-x-1/2'
                  }`}
                  style={{ left: `${pct}%` }}
                >
                  {formatTime(Math.round((pct / 100) * totalDuration))}
                </div>
              ))}
              {/* Playhead handle; the line continues down through the rows */}
              <div
                className="pointer-events-none absolute -bottom-1.5 z-30 size-3 -translate-x-1/2 rounded-full border-2 border-background bg-red-500"
                style={{ left: `${(playhead / totalDuration) * 100}%` }}
              />
            </div>
          </div>

          {/* Steps */}
          {visibleSteps.map((step) => {
            const pos = getStepPosition(step);
            const config =
              stepTypeConfig[step.stepType] || stepTypeConfig.Default;
            const Icon = config.icon;
            const isActive =
              playhead >= step.startMs &&
              playhead <= step.startMs + step.spanMs;
            const isCompleted =
              !step.isOpenEnded && playhead > step.startMs + step.spanMs;
            const isHovered = hoveredStep === step.stepId;
            const isSelected = selectedStep === step.stepId;

            // Check if this step (or its AI agent parent) has pending input
            const isWaitingForInput =
              step.status === 'running' &&
              pendingInputs.some(
                (pi) =>
                  pi.aiAgentStepId === step.stepId ||
                  (!!pi.toolName && step.stepId.includes(pi.toolName))
              );
            const displayStatus = isWaitingForInput ? 'waiting' : step.status;

            // Duration + status badge, inside the bar when they fit, else
            // just outside it (right, or left at the track end).
            const durationText = formatTime(
              step.isOpenEnded ? step.spanMs : step.durationMs
            );
            const statusDisplay = stepStatusDisplay(step.status);
            const labelPlacement = barLabelPlacement(
              pos,
              trackPx,
              displayStatus === 'waiting'
                ? estimateBarLabelPx(durationText, 'waiting for input', true)
                : estimateBarLabelPx(
                    durationText,
                    statusDisplay.label,
                    statusDisplay.spin
                  )
            );
            const selectStep = () =>
              setSelectedStep(
                step.stepId === selectedStep ? null : step.stepId
              );
            const barLabel = (
              <>
                <span
                  className={`whitespace-nowrap text-xs font-medium ${config.text}`}
                  title={
                    step.isOpenEnded
                      ? `Unfinished — ${formatTime(step.spanMs)} so far`
                      : undefined
                  }
                >
                  {durationText}
                </span>
                {displayStatus === 'waiting' ? (
                  <Badge
                    variant="outline"
                    className="border-warning px-1.5 py-0 text-xs text-warning"
                  >
                    <Hand className="mr-1 size-3" />
                    waiting for input
                  </Badge>
                ) : (
                  <StepStatusBadge
                    status={step.status}
                    className="px-1.5 py-0 text-xs"
                  />
                )}
              </>
            );

            // Calculate indentation based on depth
            const indentStyle = {
              paddingLeft: `${16 + step.depth * INDENT_PX}px`,
            };

            return (
              <div
                key={step.stepId}
                className="flex items-stretch border-b transition-colors last:border-b-0 hover:bg-muted/30"
              >
                {/* Row Label */}
                <div
                  className="flex w-48 shrink-0 items-center gap-2 border-r bg-muted/30 py-3"
                  style={indentStyle}
                >
                  {/* Expand/Collapse Button */}
                  {step.hasChildren ? (
                    <button
                      onClick={() => toggleExpand(step)}
                      className="flex size-5 items-center justify-center rounded transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      disabled={step.isLoadingChildren}
                    >
                      {step.isLoadingChildren ? (
                        <Spinner className="size-3 text-muted-foreground" />
                      ) : step.isExpanded ? (
                        <ChevronDown className="size-4 text-muted-foreground" />
                      ) : (
                        <ChevronRight className="size-4 text-muted-foreground" />
                      )}
                    </button>
                  ) : (
                    <div className="w-5" /> // Spacer for alignment
                  )}

                  {/* Step Icon — its tooltip names the step type */}
                  <WithTooltip label={config.label}>
                    <div
                      className={`flex size-6 shrink-0 items-center justify-center rounded ${config.bg}`}
                      aria-label={config.label}
                      role="img"
                    >
                      <Icon size={14} className={config.text} />
                    </div>
                  </WithTooltip>

                  {/* Step Name */}
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm text-foreground">
                      {step.stepName || step.stepId}
                    </div>
                    <div className="text-xs text-muted-foreground">
                      {step.stepType}
                      {step.hasChildren &&
                        step.childrenTotalCount !== undefined && (
                          <span className="ml-1">
                            ({step.childrenTotalCount} items)
                          </span>
                        )}
                    </div>
                  </div>
                </div>

                {/* Timeline Area */}
                <div className="relative h-14 flex-1">
                  {/* Grid lines */}
                  {[25, 50, 75].map((pct) => (
                    <div
                      key={pct}
                      className="absolute bottom-0 top-0 w-px bg-border"
                      style={{ left: `${pct}%` }}
                    />
                  ))}

                  {/* Playhead */}
                  <div
                    className="pointer-events-none absolute bottom-0 top-0 z-20 w-0.5 bg-red-500"
                    style={{ left: `${(playhead / totalDuration) * 100}%` }}
                  />

                  {/* Step Bar. An unfinished (running or parked) step runs
                      up to now and ends open: dashed right edge, no rounding. */}
                  <div
                    data-timeline-bar={step.stepId}
                    data-open-ended={step.isOpenEnded || undefined}
                    className={`absolute bottom-2 top-2 flex min-w-[60px] cursor-pointer items-center gap-2 overflow-hidden rounded border-l-[3px] px-2 transition-all duration-150 ${config.bg} ${config.border} ${
                      step.isOpenEnded
                        ? 'rounded-r-none border-r-2 [border-right-style:dashed]'
                        : ''
                    } ${
                      isActive ? 'z-10 ring-2 ring-foreground/50' : ''
                    } ${isHovered || isSelected ? 'z-10 ring-2 ring-purple-500/50' : ''}`}
                    style={{
                      left: pos.left,
                      width: pos.width,
                      opacity: isCompleted ? 0.7 : 1,
                    }}
                    onClick={selectStep}
                    onMouseEnter={() => setHoveredStep(step.stepId)}
                    onMouseLeave={() => setHoveredStep(null)}
                  >
                    {labelPlacement.side === 'inside' && barLabel}
                  </div>
                  {labelPlacement.side !== 'inside' && (
                    <div
                      data-timeline-label={step.stepId}
                      data-label-side={labelPlacement.side}
                      className="absolute bottom-2 top-2 z-10 flex cursor-pointer items-center gap-2 whitespace-nowrap"
                      style={
                        labelPlacement.side === 'right'
                          ? { left: `${labelPlacement.leftPx}px` }
                          : { right: `${labelPlacement.rightPx}px` }
                      }
                      onClick={selectStep}
                      onMouseEnter={() => setHoveredStep(step.stepId)}
                      onMouseLeave={() => setHoveredStep(null)}
                    >
                      {barLabel}
                    </div>
                  )}
                </div>
              </div>
            );
          })}

          {/* Load More Button */}
          {hasMoreRootSteps && (
            <div className="border-t bg-muted/20 p-4 text-center">
              <Button
                variant="secondary"
                bordered
                size="sm"
                onClick={loadMoreRoot}
              >
                Load More Steps
              </Button>
            </div>
          )}
        </div>
      </div>

      {/* Selected Step Details */}
      {selectedStep && (
        <div className="border-t">
          <div className="p-6">
            {(() => {
              const step = visibleSteps.find((s) => s.stepId === selectedStep);
              if (!step) return null;
              const config =
                stepTypeConfig[step.stepType] || stepTypeConfig.Default;
              const Icon = config.icon;

              const stepPendingInput = pendingInputs.find(
                (pi) =>
                  pi.aiAgentStepId === step.stepId ||
                  (!!pi.toolName && step.stepId.includes(pi.toolName))
              );
              const stepDisplayStatus =
                step.status === 'running' && stepPendingInput
                  ? 'waiting for input'
                  : step.status;

              let parsedInputs = null;
              let parsedOutputs = null;
              try {
                parsedInputs =
                  typeof step.inputs === 'string'
                    ? JSON.parse(step.inputs)
                    : step.inputs;
              } catch {
                parsedInputs = step.inputs;
              }
              try {
                parsedOutputs =
                  typeof step.outputs === 'string'
                    ? JSON.parse(step.outputs)
                    : step.outputs;
              } catch {
                parsedOutputs = step.outputs;
              }

              return (
                <div>
                  <div className="mb-4 flex items-start gap-4">
                    <div
                      className={`flex size-12 shrink-0 items-center justify-center rounded-lg ${config.bg}`}
                    >
                      <Icon size={24} className={config.text} />
                    </div>
                    <div className="min-w-0 flex-1">
                      <h3 className="text-lg font-semibold">
                        {step.stepName || step.stepId}
                      </h3>
                      <div className="mt-1 flex flex-wrap gap-x-6 gap-y-1 text-sm">
                        <div>
                          <span className="text-muted-foreground">Type:</span>{' '}
                          <span className={config.text}>{step.stepType}</span>
                        </div>
                        <div>
                          <span className="text-muted-foreground">Status:</span>{' '}
                          <span
                            className={
                              statusTextClasses[
                                stepPendingInput ? 'waiting' : step.status
                              ] ?? 'text-foreground'
                            }
                          >
                            {stepDisplayStatus}
                          </span>
                        </div>
                        <div>
                          <span className="text-muted-foreground">Start:</span>{' '}
                          <span className="font-mono">
                            +{formatTime(step.startMs)}
                          </span>
                        </div>
                        <div>
                          <span className="text-muted-foreground">
                            Duration:
                          </span>{' '}
                          <span className="font-mono">
                            {formatTime(step.durationMs)}
                          </span>
                        </div>
                        <div>
                          <span className="text-muted-foreground">
                            Timestamp:
                          </span>{' '}
                          <span className="font-mono text-xs">
                            {formatTimestamp(step.absoluteStartMs)}
                          </span>
                        </div>
                        {step.depth > 0 && (
                          <div>
                            <span className="text-muted-foreground">
                              Depth:
                            </span>{' '}
                            <span>{step.depth}</span>
                          </div>
                        )}
                      </div>
                    </div>
                  </div>

                  {/* Inputs/Outputs */}
                  <PayloadExplorer
                    key={step.stepId}
                    compact
                    ariaLabel={`${step.stepName || step.stepId} data`}
                    defaultSectionId={
                      parsedOutputs !== null && parsedOutputs !== undefined
                        ? 'output'
                        : 'input'
                    }
                    sections={[
                      {
                        id: 'input',
                        label: 'Inputs',
                        value: parsedInputs ?? null,
                        emptyState: { title: 'No inputs' },
                      },
                      {
                        id: 'output',
                        label: 'Outputs',
                        value: parsedOutputs ?? null,
                        emptyState: { title: 'No outputs' },
                      },
                    ]}
                  />

                  {/* Error if present */}
                  {step.error && (
                    <div className="mt-4">
                      <div className="mb-1 text-xs text-destructive">Error</div>
                      <pre className="max-h-32 overflow-auto rounded bg-destructive/10 p-3 font-mono text-xs text-destructive">
                        {typeof step.error === 'string'
                          ? step.error
                          : JSON.stringify(step.error, null, 2)}
                      </pre>
                    </div>
                  )}
                </div>
              );
            })()}
          </div>
        </div>
      )}
    </div>
  );
}
