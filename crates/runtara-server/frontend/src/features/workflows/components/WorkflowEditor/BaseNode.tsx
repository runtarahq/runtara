import { forwardRef, HTMLAttributes, MouseEvent } from 'react';
import { useNavigate } from 'react-router';
import { cn } from '@/lib/utils.ts';
import { StepTypeIcon } from '@/features/workflows/components/StepTypeIcon';
import {
  CheckCircle2,
  XCircle,
  AlertCircle,
  AlertTriangle,
  Pause,
  Circle,
} from 'lucide-react';
import {
  useExecutionStore,
  type NodeExecutionStatus,
} from '@/features/workflows/stores/executionStore';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { parseStructuredError } from '@/shared/utils/structured-error';
import { Spinner } from '@/shared/components/ui/spinner';
import type {
  ReplayIterationCounts,
  ReplayNodeState,
} from '@/features/workflows/components/Replay/types';

/** Border/ring/glow treatment per replay state — mirrors the tutorial palette. */
function getReplayNodeClass(state: ReplayNodeState): string {
  switch (state) {
    case 'running':
      return 'border-info ring-2 ring-info/40 animate-glow-pulse border-2';
    case 'done':
      return 'border-success ring-1 ring-success/30';
    case 'failed':
      return 'border-destructive ring-2 ring-destructive/30 border-2';
    case 'suspended':
      return 'border-warning ring-2 ring-warning/40 animate-parked-pulse border-2';
    case 'skipped':
      return 'border-dashed border-muted-foreground/40 opacity-50';
    case 'idle':
    default:
      return 'border-border opacity-40';
  }
}

export const BaseNode = forwardRef<
  HTMLDivElement,
  HTMLAttributes<HTMLDivElement> & {
    selected?: boolean;
    id?: string;
    name?: string;
    stepType?: string;
    agentId?: string;
    agentName?: string;
    inputMapping?: Array<{
      type: string;
      value?: string | number | boolean | null | any[] | object;
      typeHint?: string;
    }>;
    executionStatus?: NodeExecutionStatus;
    hasUnsavedChanges?: boolean;
    hasValidationError?: boolean;
    hasValidationWarning?: boolean;
    validationMessage?: string | null;
    /** True when this step's `agentId` is not in the current entitlement
     *  allowlist. Surfaces an "Agent disabled" badge on the canvas — the
     *  management-plane allowlist already blocks save. */
    hasStaleAgent?: boolean;
    isExecutionReadOnly?: boolean;
    subtitle?: string | null;
    /** Reserved width on the right side for additional content (e.g., case labels in SwitchNode) */
    rightReservedWidth?: number;
    breakpoint?: boolean;
    onToggleBreakpoint?: () => void;
    /** Graph Replay: per-frame visual state (drives the ring/glow + corner badge). */
    replayState?: ReplayNodeState;
    /** Graph Replay: iteration counts for composite (Split/While) nodes. */
    replayIteration?: ReplayIterationCounts;
  }
>(
  (
    {
      className,
      children,
      selected,
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      id: _id,
      name,
      stepType,
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      agentId: _agentId,
      agentName,
      inputMapping,
      executionStatus,
      hasUnsavedChanges,
      hasValidationError,
      hasValidationWarning,
      validationMessage,
      hasStaleAgent,
      isExecutionReadOnly,
      subtitle,
      rightReservedWidth,
      breakpoint,
      onToggleBreakpoint,
      replayState,
      replayIteration,
      onClick,
      ...props
    },
    ref
  ) => {
    const navigate = useNavigate();
    const isSuspendedExecution = useExecutionStore((s) => s.isSuspended);

    const getExecutionBorderClass = (status: ExecutionStatus) => {
      switch (status) {
        case 'running':
        case 'compiling':
          return 'border-info';
        case 'completed':
          return 'border-success';
        case 'failed':
        case 'timeout':
          return 'border-destructive';
        case 'queued':
          return 'border-warning';
        case 'suspended':
          return 'border-info/70';
        case 'cancelled':
          return 'border-muted-foreground/60';
        default:
          return '';
      }
    };

    const getIconTintClass = () => {
      if (hasValidationError) return 'bg-destructive/10';
      if (executionStatus) {
        switch (executionStatus.status) {
          case 'running':
          case 'compiling':
            return 'bg-info/10';
          case 'completed':
            return 'bg-success/10';
          case 'failed':
          case 'timeout':
            return 'bg-destructive/10';
          case 'queued':
            return 'bg-warning/10';
          case 'suspended':
            return 'bg-muted';
          default:
            return 'bg-muted/30';
        }
      }
      return 'bg-muted/30';
    };

    const getStatusPillIcon = (status: ExecutionStatus) => {
      switch (status) {
        case 'running':
        case 'compiling':
          return <Spinner className="h-2 w-2" />;
        case 'completed':
          return <CheckCircle2 className="h-2 w-2" />;
        case 'failed':
          return <XCircle className="h-2 w-2" />;
        case 'timeout':
          return <AlertCircle className="h-2 w-2" />;
        case 'queued':
          return <Pause className="h-2 w-2" />;
        case 'suspended':
          return <Pause className="h-2 w-2" />;
        case 'cancelled':
          return <XCircle className="h-2 w-2" />;
        default:
          return null;
      }
    };

    const getStatusPillClasses = (status: ExecutionStatus) => {
      switch (status) {
        case 'running':
        case 'compiling':
          return 'bg-info/10 text-info';
        case 'completed':
          return 'bg-success/10 text-success';
        case 'failed':
        case 'timeout':
          return 'bg-destructive/10 text-destructive';
        case 'queued':
          return 'bg-warning/10 text-warning';
        case 'suspended':
          return 'bg-muted text-muted-foreground';
        case 'cancelled':
          return 'bg-muted text-muted-foreground';
        default:
          return '';
      }
    };

    const formatExecutionTime = (ms?: number) => {
      if (!ms) return '';
      if (ms < 1000) return `${ms}ms`;
      return `${(ms / 1000).toFixed(2)}s`;
    };

    // Get the subtitle text based on priority:
    // validation error > execution error > agent name > custom subtitle
    const getSubtitleContent = () => {
      if (validationMessage) {
        return { text: validationMessage, className: 'text-destructive' };
      }
      if (executionStatus?.error) {
        const structured = parseStructuredError(executionStatus.error);
        const msg = structured?.message || executionStatus.error;
        return { text: msg, className: 'text-destructive' };
      }
      if (agentName) {
        return { text: agentName, className: 'text-muted-foreground' };
      }
      if (subtitle) {
        return { text: subtitle, className: 'text-muted-foreground' };
      }
      return null;
    };

    const subtitleContent = getSubtitleContent();
    const showStatusPill = !!executionStatus;

    // Graph Replay corner badge (top-right) for terminal/active replay states.
    const replayBadge = (() => {
      switch (replayState) {
        case 'running':
          return {
            icon: <Spinner className="h-2.5 w-2.5" />,
            cls: 'bg-info/10 text-info',
          };
        case 'done':
          return {
            icon: <CheckCircle2 className="h-2.5 w-2.5" />,
            cls: 'bg-success/10 text-success',
          };
        case 'failed':
          return {
            icon: <XCircle className="h-2.5 w-2.5" />,
            cls: 'bg-destructive/10 text-destructive',
          };
        case 'suspended':
          return {
            icon: <Pause className="h-2.5 w-2.5" />,
            cls: 'bg-warning/10 text-warning',
          };
        default:
          return null;
      }
    })();

    const handleClick = (e: MouseEvent<HTMLDivElement>) => {
      // Check if Ctrl (Windows) or Command (Mac) key is pressed
      if ((e.ctrlKey || e.metaKey) && stepType === 'EmbedWorkflow') {
        // Navigate to the workflow editing page
        // The workflow ID is stored as "workflowId" in input mapping
        if (inputMapping && inputMapping.length > 0) {
          const workflowIdMapping = inputMapping.find(
            (item) => item.type === 'workflowId'
          );
          if (
            workflowIdMapping &&
            workflowIdMapping.value &&
            typeof workflowIdMapping.value === 'string'
          ) {
            const workflowId = JSON.parse(workflowIdMapping.value);
            navigate(`/workflows/${workflowId}`);
          }
        }
      }

      if (onClick) {
        onClick(e);
      }
    };

    return (
      <div
        ref={ref}
        className={cn(
          'group relative h-full w-full',
          'rounded-md bg-card text-muted-foreground',
          'border shadow-sm transition-all duration-200 hover:shadow-md',
          // Replay mode owns the visual entirely (it is a distinct 'scrub a past
          // run' surface, never shown alongside live execution). Priority
          // otherwise: validation error > warning > execution > selected > unsaved.
          replayState
            ? getReplayNodeClass(replayState)
            : hasValidationError
              ? 'border-2 border-destructive ring-2 ring-destructive/30'
              : hasValidationWarning
                ? 'border-2 border-warning ring-2 ring-warning/30'
                : executionStatus
                  ? getExecutionBorderClass(executionStatus.status)
                  : selected
                    ? 'border-primary shadow-md ring-1 ring-primary/20'
                    : hasUnsavedChanges
                      ? 'border-dashed border-warning ring-1 ring-warning/20'
                      : 'border-border',
          // Subtle glow for suspended (breakpoint hit) nodes
          executionStatus?.status === 'suspended' &&
            'animate-glow-pulse border-2',
          // Dim unreached nodes during execution
          isExecutionReadOnly && !executionStatus && 'opacity-40',
          // Extra dim for queued (not-yet-reached) nodes when paused at breakpoint
          isSuspendedExecution &&
            executionStatus?.status === 'queued' &&
            'pointer-events-none opacity-25',
          className
        )}
        tabIndex={0}
        onClick={handleClick}
        data-testid="workflow-canvas-node"
        data-step-name={name}
        data-step-type={stepType}
        data-replay-state={replayState}
        {...props}
      >
        {/* Breakpoint indicator - red dot on left edge */}
        {breakpoint && (
          <button
            type="button"
            className="absolute -left-1.5 top-1/2 z-10 flex h-3 w-3 -translate-y-1/2 cursor-pointer items-center justify-center rounded-full border border-destructive bg-destructive transition-colors hover:bg-destructive/80"
            onClick={(e) => {
              e.stopPropagation();
              onToggleBreakpoint?.();
            }}
            title="Remove breakpoint"
          >
            <Circle className="h-1.5 w-1.5 fill-destructive-foreground/80 text-destructive-foreground/80" />
          </button>
        )}

        {/* Breakpoint gutter - appears on hover when no breakpoint is set */}
        {!breakpoint && onToggleBreakpoint && !isExecutionReadOnly && (
          <button
            type="button"
            className="absolute -left-1.5 top-1/2 z-10 flex h-3 w-3 -translate-y-1/2 cursor-pointer items-center justify-center rounded-full border border-destructive/50 bg-destructive/70 opacity-0 transition-all hover:bg-destructive hover:!opacity-100 group-hover:opacity-40"
            onClick={(e) => {
              e.stopPropagation();
              onToggleBreakpoint?.();
            }}
            title="Set breakpoint"
          >
            <Circle className="h-1.5 w-1.5 fill-destructive-foreground/80 text-destructive-foreground/80" />
          </button>
        )}

        {/* Graph Replay state badge + iteration counter (top-right) */}
        {replayBadge && (
          <div
            className="absolute -right-1.5 -top-1.5 z-10 flex items-center gap-0.5"
            data-testid="replay-node-badge"
          >
            {replayIteration && replayIteration.total > 0 && (
              <span
                className="rounded-full border bg-background px-1 text-[8px] font-medium leading-none text-muted-foreground shadow-sm"
                title={`${replayIteration.active} running · ${replayIteration.completed} done · ${replayIteration.total} iterations`}
              >
                {replayIteration.completed + replayIteration.active}/
                {replayIteration.total}
              </span>
            )}
            <span
              className={cn(
                'flex h-3.5 w-3.5 items-center justify-center rounded-full shadow-sm',
                replayBadge.cls
              )}
            >
              {replayBadge.icon}
            </span>
          </div>
        )}

        {/* Unsaved changes corner dot */}
        {hasUnsavedChanges && !hasValidationError && !hasValidationWarning && (
          <div className="absolute right-0.5 top-0.5 z-10 h-1 w-1 rounded-full bg-warning" />
        )}

        {/* Horizontal pill layout: icon left, name + status center/right */}
        {(stepType !== undefined || name) && (
          <div
            className="flex h-full w-full items-center gap-1.5 px-1.5"
            style={
              rightReservedWidth
                ? { paddingRight: rightReservedWidth }
                : undefined
            }
          >
            {/* Left: Icon */}
            {stepType && (
              <div
                className={cn(
                  'flex h-4 w-4 flex-shrink-0 items-center justify-center rounded-sm [&_svg]:h-2.5 [&_svg]:w-2.5',
                  getIconTintClass()
                )}
              >
                <StepTypeIcon type={stepType} />
              </div>
            )}

            {/* Center: Step name, subtitle, and inline status pill */}
            <div
              className="flex min-w-0 flex-1 flex-col justify-center"
              title={name}
            >
              {/* Row 1: Name + status pill */}
              <div className="flex min-w-0 items-center gap-0.5">
                {name ? (
                  <span className="flex-1 truncate text-2xs font-normal leading-tight text-foreground">
                    {name}
                  </span>
                ) : (
                  <span className="flex-1 text-2xs font-normal italic leading-tight text-muted-foreground">
                    Unnamed step
                  </span>
                )}
                {/* Stale-agent badge — entitlement allowlist excludes this
                    step's agent. Workflow can't be saved/run until either the
                    entitlement is restored or the step is swapped. */}
                {hasStaleAgent && (
                  <span
                    title="Agent disabled — workflow can't be saved"
                    aria-label="Agent disabled"
                    data-testid="stale-agent-badge"
                    className="flex shrink-0 items-center gap-0.5 rounded bg-warning/10 px-1 py-0.5 text-[9px] font-medium leading-none text-warning"
                  >
                    <AlertTriangle className="h-2.5 w-2.5" />
                    <span className="hidden md:inline">Agent disabled</span>
                  </span>
                )}
                {/* Inline status pill */}
                {showStatusPill && (
                  <span
                    className={cn(
                      'inline-flex flex-shrink-0 items-center gap-0.5 whitespace-nowrap rounded-full px-0.5 text-[8px] font-medium leading-none',
                      getStatusPillClasses(executionStatus.status)
                    )}
                  >
                    {getStatusPillIcon(executionStatus.status)}
                    {executionStatus.status === 'completed' &&
                    executionStatus.executionTime !== undefined
                      ? formatExecutionTime(executionStatus.executionTime)
                      : null}
                  </span>
                )}
              </div>

              {/* Row 2: Subtitle (validation message, error, agent name, or custom) */}
              {subtitleContent && (
                <span
                  className={cn(
                    'block truncate text-[9px] leading-tight',
                    subtitleContent.className
                  )}
                  title={subtitleContent.text}
                >
                  {subtitleContent.text}
                </span>
              )}
            </div>
          </div>
        )}
        {children}
      </div>
    );
  }
);
