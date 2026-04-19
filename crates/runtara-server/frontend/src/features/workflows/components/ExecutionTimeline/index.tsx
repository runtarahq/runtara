import React, { useState, useEffect } from 'react';
import {
  Play,
  Pause,
  RotateCcw,
  Zap,
  Globe,
  Clock,
  Flag,
  Cpu,
  ArrowRight,
  GitBranch,
  Repeat,
  MessageSquare,
  Split,
  ToggleLeft,
  ChevronRight,
  ChevronDown,
  Loader2,
  Sparkles,
  Wrench,
  Hand,
  Brain,
} from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { useHierarchicalTimeline } from '@/features/workflows/hooks/useHierarchicalTimeline';
import { HierarchicalStep } from '@/features/workflows/types/timeline';
import { PayloadPreBlock } from '@/shared/components/PayloadPreBlock';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';
import { useToken } from '@/shared/hooks';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  getWorkflowInstance,
  getPendingInput,
  deliverSignal,
  type PendingInput,
} from '@/features/workflows/queries';
import { HumanInputCard } from '@/features/workflows/components/ExecutionPanel/HumanInputCard';
import { useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { isActiveStatus } from '@/shared/utils/status-display';

interface ExecutionTimelineProps {
  workflowId: string;
  instanceId: string;
}

const stepTypeConfig: Record<
  string,
  { color: string; bg: string; label: string; icon: React.ElementType }
> = {
  Agent: {
    color: '#3B82F6',
    bg: 'rgba(59, 130, 246, 0.1)',
    label: 'Agent',
    icon: Globe,
  },
  Connection: {
    color: '#8B5CF6',
    bg: 'rgba(139, 92, 246, 0.1)',
    label: 'Connection',
    icon: Zap,
  },
  Finish: {
    color: '#22C55E',
    bg: 'rgba(34, 197, 94, 0.1)',
    label: 'Finish',
    icon: Flag,
  },
  Conditional: {
    color: '#EC4899',
    bg: 'rgba(236, 72, 153, 0.1)',
    label: 'Conditional',
    icon: GitBranch,
  },
  While: {
    color: '#F97316',
    bg: 'rgba(249, 115, 22, 0.1)',
    label: 'While',
    icon: Repeat,
  },
  Log: {
    color: '#6366F1',
    bg: 'rgba(99, 102, 241, 0.1)',
    label: 'Log',
    icon: MessageSquare,
  },
  Split: {
    color: '#14B8A6',
    bg: 'rgba(20, 184, 166, 0.1)',
    label: 'Split',
    icon: Split,
  },
  Switch: {
    color: '#A855F7',
    bg: 'rgba(168, 85, 247, 0.1)',
    label: 'Switch',
    icon: ToggleLeft,
  },
  EmbedWorkflow: {
    color: '#F59E0B',
    bg: 'rgba(245, 158, 11, 0.1)',
    label: 'Start Workflow',
    icon: Zap,
  },
  AiAgent: {
    color: '#8B5CF6',
    bg: 'rgba(139, 92, 246, 0.1)',
    label: 'AI Agent',
    icon: Sparkles,
  },
  AiAgentToolCall: {
    color: '#6366F1',
    bg: 'rgba(99, 102, 241, 0.1)',
    label: 'Tool Call',
    icon: Wrench,
  },
  AiAgentMemoryCompaction: {
    color: '#3B82F6',
    bg: 'rgba(59, 130, 246, 0.1)',
    label: 'Memory Compaction',
    icon: Brain,
  },
  WaitForSignal: {
    color: '#F59E0B',
    bg: 'rgba(245, 158, 11, 0.1)',
    label: 'Wait For Signal',
    icon: Hand,
  },
  Default: {
    color: '#6B7280',
    bg: 'rgba(107, 114, 128, 0.1)',
    label: 'Step',
    icon: Cpu,
  },
};

const statusColors: Record<string, string> = {
  completed: '#22C55E',
  running: '#3B82F6',
  failed: '#EF4444',
  pending: '#6B7280',
  waiting: '#F59E0B',
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

export function ExecutionTimeline({
  workflowId,
  instanceId,
}: ExecutionTimelineProps) {
  const [selectedStep, setSelectedStep] = useState<string | null>(null);
  const [isPlaying, setIsPlaying] = useState(false);
  const [playhead, setPlayhead] = useState(0);
  const [hoveredStep, setHoveredStep] = useState<string | null>(null);

  const token = useToken();
  const queryClient = useQueryClient();

  const {
    visibleSteps,
    totalDuration,
    stats,
    isLoadingRoot,
    hasMoreRootSteps,
    loadMoreRoot,
    toggleExpand,
  } = useHierarchicalTimeline(workflowId, instanceId);

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
  const { data: pendingInputData } = useCustomQuery<PendingInput[]>({
    queryKey: queryKeys.workflows.pendingInput(workflowId, instanceId),
    queryFn: (token: string) => getPendingInput(token, workflowId, instanceId),
    enabled:
      !!workflowId && !!instanceId && isActiveStatus(instanceData?.status),
    refetchInterval: () => {
      return isActiveStatus(instanceData?.status) ? 3000 : false;
    },
  });

  const pendingInputs = pendingInputData ?? [];

  // Signal delivery mutation
  const signalMutation = useCustomMutation({
    mutationFn: (
      _token: any,
      data: { signalId: string; payload: Record<string, any> }
    ) => {
      return deliverSignal(token, instanceId!, data);
    },
    onSuccess: () => {
      toast.success('Response submitted successfully');
      queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.pendingInput(workflowId, instanceId),
      });
    },
  });

  const handleSignalSubmit = (
    signalId: string,
    payload: Record<string, any>
  ) => {
    signalMutation.mutate({ signalId, payload });
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

  const getStepPosition = (step: HierarchicalStep) => {
    if (totalDuration === 0) {
      return { left: '0%', width: '100%' };
    }
    const left = (step.startMs / totalDuration) * 100;
    const width = ((step.durationMs || 1) / totalDuration) * 100;
    return { left: `${left}%`, width: `${Math.max(width, 1)}%` };
  };

  if (isLoadingRoot) {
    return (
      <div className="py-16 px-6 text-center">
        <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-purple-500/10 mb-4">
          <Loader2 className="h-8 w-8 text-purple-600 animate-spin" />
        </div>
        <h3 className="text-lg font-semibold mb-2">Loading Timeline...</h3>
        <p className="text-sm text-muted-foreground max-w-md mx-auto">
          Fetching execution data for visualization.
        </p>
      </div>
    );
  }

  if (visibleSteps.length === 0) {
    return (
      <div className="py-16 px-6 text-center">
        <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-purple-500/10 mb-4">
          <Clock className="h-8 w-8 text-purple-600" />
        </div>
        <h3 className="text-lg font-semibold mb-2">No Timeline Events Yet</h3>
        <p className="text-sm text-muted-foreground max-w-md mx-auto">
          Timeline events will appear here as your workflow executes. If your
          workflow is still running, events may appear soon.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* Stats Bar */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-xs text-muted-foreground">
              Total Duration
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-xl font-bold">{formatTime(stats.total)}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-xs text-muted-foreground">
              Steps
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-xl font-bold">{stats.rootStepCount}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-xs text-muted-foreground">
              Agent Time
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-xl font-bold text-blue-500">
              {formatTime(stats.byType.Agent || 0)}
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-xs text-muted-foreground">
              Connection Time
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-xl font-bold text-purple-500">
              {formatTime(stats.byType.Connection || 0)}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* Pending Human Input Cards */}
      {pendingInputs.length > 0 && (
        <div className="space-y-3">
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

      {/* Playback Controls */}
      <Card>
        <CardContent className="py-4">
          <div className="flex items-center gap-4">
            <Button
              variant="outline"
              size="icon"
              onClick={() => setIsPlaying(!isPlaying)}
              className="rounded-full"
            >
              {isPlaying ? (
                <Pause className="h-4 w-4" />
              ) : (
                <Play className="h-4 w-4" />
              )}
            </Button>
            <Button
              variant="outline"
              size="icon"
              onClick={() => {
                setPlayhead(0);
                setIsPlaying(false);
              }}
              className="rounded-full"
            >
              <RotateCcw className="h-4 w-4" />
            </Button>
            <div className="flex-1 h-2 bg-muted rounded-full overflow-hidden">
              <div
                className="h-full bg-purple-500 transition-all duration-50"
                style={{ width: `${(playhead / totalDuration) * 100}%` }}
              />
            </div>
            <span className="text-muted-foreground font-mono text-sm w-20 text-right">
              {formatTime(Math.round(playhead))}
            </span>
          </div>
        </CardContent>
      </Card>

      {/* Timeline */}
      <Card className="overflow-hidden">
        <CardContent className="p-0">
          {/* Time Scale */}
          <div className="h-8 bg-muted/50 border-b flex items-center relative">
            <div className="w-48 shrink-0 px-4 text-xs text-muted-foreground">
              Time →
            </div>
            <div className="flex-1 relative">
              {[0, 25, 50, 75, 100].map((pct) => (
                <div
                  key={pct}
                  className="absolute text-xs text-muted-foreground transform -translate-x-1/2"
                  style={{ left: `${pct}%` }}
                >
                  {formatTime(Math.round((pct / 100) * totalDuration))}
                </div>
              ))}
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
              playhead <= step.startMs + (step.durationMs || 0);
            const isCompleted =
              playhead > step.startMs + (step.durationMs || 0);
            const isHovered = hoveredStep === step.stepId;
            const isSelected = selectedStep === step.stepId;

            // Check if this step (or its AI agent parent) has pending input
            const isWaitingForInput =
              step.status === 'running' &&
              pendingInputs.some(
                (pi) =>
                  pi.aiAgentStepId === step.stepId ||
                  step.stepId.includes(pi.toolName)
              );
            const displayStatus = isWaitingForInput ? 'waiting' : step.status;

            // Calculate indentation based on depth
            const indentStyle = {
              paddingLeft: `${16 + step.depth * INDENT_PX}px`,
            };

            return (
              <div
                key={step.stepId}
                className="flex items-stretch border-b last:border-b-0 hover:bg-muted/30 transition-colors"
              >
                {/* Row Label */}
                <div
                  className="w-48 shrink-0 py-3 bg-muted/30 border-r flex items-center gap-2"
                  style={indentStyle}
                >
                  {/* Expand/Collapse Button */}
                  {step.hasChildren ? (
                    <button
                      onClick={() => toggleExpand(step)}
                      className="w-5 h-5 flex items-center justify-center hover:bg-muted rounded transition-colors"
                      disabled={step.isLoadingChildren}
                    >
                      {step.isLoadingChildren ? (
                        <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" />
                      ) : step.isExpanded ? (
                        <ChevronDown className="h-4 w-4 text-muted-foreground" />
                      ) : (
                        <ChevronRight className="h-4 w-4 text-muted-foreground" />
                      )}
                    </button>
                  ) : (
                    <div className="w-5" /> // Spacer for alignment
                  )}

                  {/* Step Icon */}
                  <div
                    className="w-6 h-6 rounded flex items-center justify-center shrink-0"
                    style={{ backgroundColor: config.bg }}
                  >
                    <Icon size={14} style={{ color: config.color }} />
                  </div>

                  {/* Step Name */}
                  <div className="min-w-0 flex-1">
                    <div className="text-sm text-foreground truncate">
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
                <div className="flex-1 relative h-14">
                  {/* Grid lines */}
                  {[25, 50, 75].map((pct) => (
                    <div
                      key={pct}
                      className="absolute top-0 bottom-0 w-px bg-border"
                      style={{ left: `${pct}%` }}
                    />
                  ))}

                  {/* Playhead */}
                  <div
                    className="absolute top-0 bottom-0 w-0.5 bg-red-500 z-20 pointer-events-none"
                    style={{ left: `${(playhead / totalDuration) * 100}%` }}
                  />

                  {/* Step Bar */}
                  <div
                    className={`absolute top-2 bottom-2 rounded cursor-pointer transition-all duration-150 flex items-center gap-2 px-2 overflow-hidden ${
                      isActive ? 'ring-2 ring-foreground/50 z-10' : ''
                    } ${isHovered || isSelected ? 'z-10 ring-2 ring-purple-500/50' : ''}`}
                    style={{
                      left: pos.left,
                      width: pos.width,
                      minWidth: '60px',
                      backgroundColor: config.bg,
                      borderLeft: `3px solid ${config.color}`,
                      opacity: isCompleted ? 0.7 : 1,
                    }}
                    onClick={() =>
                      setSelectedStep(
                        step.stepId === selectedStep ? null : step.stepId
                      )
                    }
                    onMouseEnter={() => setHoveredStep(step.stepId)}
                    onMouseLeave={() => setHoveredStep(null)}
                  >
                    <span
                      className="text-xs font-medium whitespace-nowrap"
                      style={{ color: config.color }}
                    >
                      {formatTime(step.durationMs)}
                    </span>
                    <Badge
                      variant={
                        displayStatus === 'completed'
                          ? 'default'
                          : displayStatus === 'waiting'
                            ? 'outline'
                            : displayStatus === 'running'
                              ? 'secondary'
                              : 'destructive'
                      }
                      className={`text-xs px-1.5 py-0 ${displayStatus === 'waiting' ? 'border-amber-500 text-amber-600' : ''}`}
                    >
                      {displayStatus === 'running' && (
                        <Loader2 className="h-3 w-3 mr-1 animate-spin" />
                      )}
                      {displayStatus === 'waiting' && (
                        <Hand className="h-3 w-3 mr-1" />
                      )}
                      {displayStatus === 'waiting'
                        ? 'waiting for input'
                        : displayStatus}
                    </Badge>
                  </div>
                </div>
              </div>
            );
          })}

          {/* Load More Button */}
          {hasMoreRootSteps && (
            <div className="p-4 border-t bg-muted/20 text-center">
              <Button variant="outline" size="sm" onClick={loadMoreRoot}>
                Load More Steps
              </Button>
            </div>
          )}
        </CardContent>
      </Card>

      {/* Legend */}
      <div className="flex flex-wrap gap-4">
        {Object.entries(stepTypeConfig)
          .filter(([k]) => k !== 'Default')
          .map(([key, value]) => {
            const Icon = value.icon;
            return (
              <div key={key} className="flex items-center gap-2 text-sm">
                <div
                  className="w-5 h-5 rounded flex items-center justify-center"
                  style={{
                    backgroundColor: value.bg,
                    border: `1px solid ${value.color}`,
                  }}
                >
                  <Icon size={12} style={{ color: value.color }} />
                </div>
                <span className="text-muted-foreground">{value.label}</span>
              </div>
            );
          })}
      </div>

      {/* Selected Step Details */}
      {selectedStep && (
        <Card>
          <CardContent className="pt-6">
            {(() => {
              const step = visibleSteps.find((s) => s.stepId === selectedStep);
              if (!step) return null;
              const config =
                stepTypeConfig[step.stepType] || stepTypeConfig.Default;
              const Icon = config.icon;

              const stepPendingInput = pendingInputs.find(
                (pi) =>
                  pi.aiAgentStepId === step.stepId ||
                  step.stepId.includes(pi.toolName)
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
                  <div className="flex items-start gap-4 mb-4">
                    <div
                      className="w-12 h-12 rounded-lg flex items-center justify-center shrink-0"
                      style={{ backgroundColor: config.bg }}
                    >
                      <Icon size={24} style={{ color: config.color }} />
                    </div>
                    <div className="flex-1 min-w-0">
                      <h3 className="font-semibold text-lg">
                        {step.stepName || step.stepId}
                      </h3>
                      <div className="flex flex-wrap gap-x-6 gap-y-1 text-sm mt-1">
                        <div>
                          <span className="text-muted-foreground">Type:</span>{' '}
                          <span style={{ color: config.color }}>
                            {step.stepType}
                          </span>
                        </div>
                        <div>
                          <span className="text-muted-foreground">Status:</span>{' '}
                          <span
                            style={{
                              color:
                                statusColors[
                                  stepPendingInput ? 'waiting' : step.status
                                ],
                            }}
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
                  <div className="grid md:grid-cols-2 gap-4">
                    <div>
                      <div className="text-xs text-muted-foreground mb-1 flex items-center gap-1">
                        <ArrowRight size={12} /> Inputs
                      </div>
                      {parsedInputs ? (
                        <PayloadPreBlock
                          data={parsedInputs}
                          className="max-h-32 p-3"
                        />
                      ) : (
                        <pre className="bg-muted rounded p-3 text-xs font-mono text-foreground overflow-auto max-h-32">
                          (none)
                        </pre>
                      )}
                    </div>
                    <div>
                      <div className="text-xs text-muted-foreground mb-1 flex items-center gap-1">
                        <ArrowRight size={12} className="rotate-180" /> Outputs
                      </div>
                      {parsedOutputs ? (
                        <PayloadPreBlock
                          data={parsedOutputs}
                          className="max-h-32 p-3"
                        />
                      ) : (
                        <pre className="bg-muted rounded p-3 text-xs font-mono text-foreground overflow-auto max-h-32">
                          (none)
                        </pre>
                      )}
                    </div>
                  </div>

                  {/* Error if present */}
                  {step.error && (
                    <div className="mt-4">
                      <div className="text-xs text-destructive mb-1">Error</div>
                      <pre className="bg-destructive/10 rounded p-3 text-xs font-mono text-destructive overflow-auto max-h-32">
                        {typeof step.error === 'string'
                          ? step.error
                          : JSON.stringify(step.error, null, 2)}
                      </pre>
                    </div>
                  )}
                </div>
              );
            })()}
          </CardContent>
        </Card>
      )}

      {/* Execution Summary */}
      <Card className="bg-muted/30">
        <CardHeader className="pb-3">
          <CardTitle className="text-sm font-medium">
            Execution Summary
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex flex-wrap gap-6 text-sm">
            {Object.entries(stats.byType).map(([type, duration]) => {
              const config = stepTypeConfig[type] || stepTypeConfig.Default;
              const percentage = stats.total
                ? ((duration / stats.total) * 100).toFixed(1)
                : '0';
              return (
                <div key={type} className="flex items-center gap-2">
                  <div
                    className="w-3 h-3 rounded-sm"
                    style={{ backgroundColor: config.color }}
                  />
                  <span className="text-muted-foreground">{type}:</span>
                  <span className="font-mono">{formatTime(duration)}</span>
                  <span className="text-muted-foreground">({percentage}%)</span>
                </div>
              );
            })}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
