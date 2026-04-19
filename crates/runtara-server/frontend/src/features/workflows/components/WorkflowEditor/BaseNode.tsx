import { forwardRef, HTMLAttributes, MouseEvent } from 'react';
import { useNavigate } from 'react-router';
import { cn } from '@/lib/utils.ts';
import { StepTypeIcon } from '@/features/workflows/components/StepTypeIcon';
import {
  Loader2,
  CheckCircle2,
  XCircle,
  AlertCircle,
  Pause,
  Circle,
} from 'lucide-react';
import {
  useExecutionStore,
  type NodeExecutionStatus,
} from '@/features/workflows/stores/executionStore';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { parseStructuredError } from '@/shared/utils/structured-error';

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
      value?: string | number | boolean | any[] | object;
      typeHint?: string;
    }>;
    executionStatus?: NodeExecutionStatus;
    hasUnsavedChanges?: boolean;
    hasValidationError?: boolean;
    hasValidationWarning?: boolean;
    validationMessage?: string | null;
    isExecutionReadOnly?: boolean;
    subtitle?: string | null;
    /** Reserved width on the right side for additional content (e.g., case labels in SwitchNode) */
    rightReservedWidth?: number;
    breakpoint?: boolean;
    onToggleBreakpoint?: () => void;
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
      isExecutionReadOnly,
      subtitle,
      rightReservedWidth,
      breakpoint,
      onToggleBreakpoint,
      onClick,
      ...props
    },
    ref
  ) => {
    const navigate = useNavigate();
    const isSuspendedExecution = useExecutionStore((s) => s.isSuspended);

    const getExecutionBorderClass = (status: ExecutionStatus) => {
      switch (status) {
        case ExecutionStatus.Running:
        case ExecutionStatus.Compiling:
          return 'border-blue-500';
        case ExecutionStatus.Completed:
          return 'border-green-500';
        case ExecutionStatus.Failed:
        case ExecutionStatus.Timeout:
          return 'border-red-500';
        case ExecutionStatus.Queued:
          return 'border-yellow-500';
        case ExecutionStatus.Suspended:
          return 'border-blue-400';
        case ExecutionStatus.Cancelled:
          return 'border-gray-400';
        default:
          return '';
      }
    };

    const getIconTintClass = () => {
      if (hasValidationError) return 'bg-red-100 dark:bg-red-950';
      if (executionStatus) {
        switch (executionStatus.status) {
          case ExecutionStatus.Running:
          case ExecutionStatus.Compiling:
            return 'bg-blue-100 dark:bg-blue-950';
          case ExecutionStatus.Completed:
            return 'bg-green-100 dark:bg-green-950';
          case ExecutionStatus.Failed:
          case ExecutionStatus.Timeout:
            return 'bg-red-100 dark:bg-red-950';
          case ExecutionStatus.Queued:
            return 'bg-yellow-100 dark:bg-yellow-950';
          case ExecutionStatus.Suspended:
            return 'bg-slate-100 dark:bg-slate-900';
          default:
            return 'bg-muted/30';
        }
      }
      return 'bg-muted/30';
    };

    const getStatusPillIcon = (status: ExecutionStatus) => {
      switch (status) {
        case ExecutionStatus.Running:
        case ExecutionStatus.Compiling:
          return <Loader2 className="h-2 w-2 animate-spin" />;
        case ExecutionStatus.Completed:
          return <CheckCircle2 className="h-2 w-2" />;
        case ExecutionStatus.Failed:
          return <XCircle className="h-2 w-2" />;
        case ExecutionStatus.Timeout:
          return <AlertCircle className="h-2 w-2" />;
        case ExecutionStatus.Queued:
          return <Pause className="h-2 w-2" />;
        case ExecutionStatus.Suspended:
          return <Pause className="h-2 w-2" />;
        case ExecutionStatus.Cancelled:
          return <XCircle className="h-2 w-2" />;
        default:
          return null;
      }
    };

    const getStatusPillClasses = (status: ExecutionStatus) => {
      switch (status) {
        case ExecutionStatus.Running:
        case ExecutionStatus.Compiling:
          return 'bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300';
        case ExecutionStatus.Completed:
          return 'bg-green-100 text-green-700 dark:bg-green-900 dark:text-green-300';
        case ExecutionStatus.Failed:
        case ExecutionStatus.Timeout:
          return 'bg-red-100 text-red-700 dark:bg-red-900 dark:text-red-300';
        case ExecutionStatus.Queued:
          return 'bg-yellow-100 text-yellow-700 dark:bg-yellow-900 dark:text-yellow-300';
        case ExecutionStatus.Suspended:
          return 'bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300';
        case ExecutionStatus.Cancelled:
          return 'bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400';
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
        return { text: validationMessage, className: 'text-red-500' };
      }
      if (executionStatus?.error) {
        const structured = parseStructuredError(executionStatus.error);
        const msg = structured?.message || executionStatus.error;
        return { text: msg, className: 'text-red-500' };
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
          'group relative w-full h-full',
          'bg-card rounded-md text-muted-foreground',
          'border shadow-sm hover:shadow-md transition-all duration-200',
          // Priority: validation error > validation warning > execution > selected > unsaved > default
          hasValidationError
            ? 'border-red-500 ring-2 ring-red-500/30 border-2'
            : hasValidationWarning
              ? 'border-amber-500 ring-2 ring-amber-500/30 border-2'
              : executionStatus
                ? getExecutionBorderClass(executionStatus.status)
                : selected
                  ? 'border-primary ring-1 ring-primary/20 shadow-md'
                  : hasUnsavedChanges
                    ? 'border-dashed border-orange-500 ring-1 ring-orange-500/20'
                    : 'border-border',
          // Subtle glow for suspended (breakpoint hit) nodes
          executionStatus?.status === ExecutionStatus.Suspended &&
            'border-2 animate-glow-pulse',
          // Dim unreached nodes during execution
          isExecutionReadOnly && !executionStatus && 'opacity-40',
          // Extra dim for queued (not-yet-reached) nodes when paused at breakpoint
          isSuspendedExecution &&
            executionStatus?.status === ExecutionStatus.Queued &&
            'opacity-25 pointer-events-none',
          className
        )}
        tabIndex={0}
        onClick={handleClick}
        {...props}
      >
        {/* Breakpoint indicator - red dot on left edge */}
        {breakpoint && (
          <button
            type="button"
            className="absolute -left-1.5 top-1/2 -translate-y-1/2 z-10 flex items-center justify-center w-3 h-3 rounded-full bg-red-500 hover:bg-red-600 transition-colors cursor-pointer border border-red-600"
            onClick={(e) => {
              e.stopPropagation();
              onToggleBreakpoint?.();
            }}
            title="Remove breakpoint"
          >
            <Circle className="w-1.5 h-1.5 fill-red-200 text-red-200" />
          </button>
        )}

        {/* Breakpoint gutter - appears on hover when no breakpoint is set */}
        {!breakpoint && onToggleBreakpoint && !isExecutionReadOnly && (
          <button
            type="button"
            className="absolute -left-1.5 top-1/2 -translate-y-1/2 z-10 flex items-center justify-center w-3 h-3 rounded-full opacity-0 group-hover:opacity-40 hover:!opacity-100 bg-red-400 hover:bg-red-500 transition-all cursor-pointer border border-red-500/50"
            onClick={(e) => {
              e.stopPropagation();
              onToggleBreakpoint?.();
            }}
            title="Set breakpoint"
          >
            <Circle className="w-1.5 h-1.5 fill-red-200 text-red-200" />
          </button>
        )}

        {/* Unsaved changes corner dot */}
        {hasUnsavedChanges && !hasValidationError && !hasValidationWarning && (
          <div className="absolute top-0.5 right-0.5 w-1 h-1 rounded-full bg-orange-500 z-10" />
        )}

        {/* Horizontal pill layout: icon left, name + status center/right */}
        {(stepType !== undefined || name) && (
          <div
            className="flex items-center w-full h-full px-1.5 gap-1.5"
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
                  'flex-shrink-0 w-4 h-4 flex items-center justify-center rounded-sm [&_svg]:w-2.5 [&_svg]:h-2.5',
                  getIconTintClass()
                )}
              >
                <StepTypeIcon type={stepType} />
              </div>
            )}

            {/* Center: Step name, subtitle, and inline status pill */}
            <div
              className="flex-1 min-w-0 flex flex-col justify-center"
              title={name}
            >
              {/* Row 1: Name + status pill */}
              <div className="flex items-center gap-0.5 min-w-0">
                {name ? (
                  <span className="text-[11px] font-normal truncate flex-1 text-foreground leading-tight">
                    {name}
                  </span>
                ) : (
                  <span className="text-[11px] font-normal text-muted-foreground italic flex-1 leading-tight">
                    Unnamed step
                  </span>
                )}
                {/* Inline status pill */}
                {showStatusPill && (
                  <span
                    className={cn(
                      'inline-flex items-center gap-0.5 px-0.5 rounded-full text-[8px] font-medium whitespace-nowrap flex-shrink-0 leading-none',
                      getStatusPillClasses(executionStatus.status)
                    )}
                  >
                    {getStatusPillIcon(executionStatus.status)}
                    {executionStatus.status === ExecutionStatus.Completed &&
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
                    'text-[9px] truncate block leading-tight',
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
