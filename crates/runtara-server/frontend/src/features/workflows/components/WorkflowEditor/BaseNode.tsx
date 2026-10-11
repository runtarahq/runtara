import { forwardRef, HTMLAttributes, MouseEvent, ReactNode } from 'react';
import { Badge } from '@/shared/components/ui/badge';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/shared/components/ui/tooltip';
import {
  stepBadgeVariant,
  stepDescription,
  stepTypeLabel,
} from './step-summary';
import { stepIdLabel } from './step-label';
import { cardDetailLayout, type StepDetailRow } from './step-details';
import { useCanvasDisplay } from './CanvasDisplayContext';
import { COMPACT_CARD_HEIGHT } from './CustomNodes/node-size';
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

export const BaseNode = forwardRef<
  HTMLDivElement,
  HTMLAttributes<HTMLDivElement> & {
    selected?: boolean;
    id?: string;
    name?: string;
    stepType?: string;
    agentId?: string;
    agentName?: string;
    /** Agent capability, shown with the agent as the step summary. */
    capabilityId?: string;
    /** The capability's display name, used in the step summary. */
    capabilityName?: string;
    /** The step's own description, shown instead of the type summary. */
    description?: string;
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
    /**
     * The step's configured inputs. Listed on the card when the canvas is in
     * detailed mode, and in full in the hover preview.
     */
    detailRows?: StepDetailRow[];
  }
>(
  (
    {
      className,
      children,
      selected,
      id,
      name,
      stepType,
      agentId,
      agentName,
      capabilityId,
      capabilityName,
      description,
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
      detailRows,
      onClick,
      ...props
    },
    ref
  ) => {
    const navigate = useNavigate();
    const { detailLevel, stepPreviews, editable } = useCanvasDisplay();
    const cardDetails =
      detailRows && detailLevel === 'detailed'
        ? cardDetailLayout(detailRows)
        : null;
    const showCardDetails = !!cardDetails && cardDetails.lineCount > 0;
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
          return <Spinner className="size-2" />;
        case 'completed':
          return <CheckCircle2 className="size-2" />;
        case 'failed':
          return <XCircle className="size-2" />;
        case 'timeout':
          return <AlertCircle className="size-2" />;
        case 'queued':
          return <Pause className="size-2" />;
        case 'suspended':
          return <Pause className="size-2" />;
        case 'cancelled':
          return <XCircle className="size-2" />;
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
      if (subtitle) {
        return { text: subtitle, className: 'text-muted-foreground' };
      }
      // The step's description, or what the step does.
      const summary = stepDescription(
        { stepType, agentId, capabilityId, description },
        agentName,
        capabilityName
      );
      // Without a summary, the readable step type ("Wait for signal") keeps
      // the card saying what kind of step it is now the type chip is gone.
      const text = summary || (stepType ? stepTypeLabel(stepType) : undefined);
      return text ? { text, className: 'text-muted-foreground' } : null;
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

    const card = (
      <div
        ref={ref}
        className={cn(
          'group relative h-full w-full',
          showCardDetails && 'flex flex-col',
          'rounded-md bg-card text-muted-foreground',
          'border shadow-sm transition-all duration-200 hover:shadow-md',
          // Priority: validation error > warning > execution > selected > unsaved.
          hasValidationError
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
        {...props}
      >
        {/* Breakpoint indicator - red dot on left edge */}
        {breakpoint && (
          <button
            type="button"
            className="absolute -left-1.5 top-1/2 z-10 flex size-3 -translate-y-1/2 cursor-pointer items-center justify-center rounded-full border border-destructive bg-destructive transition-colors hover:bg-destructive/80"
            onClick={(e) => {
              e.stopPropagation();
              onToggleBreakpoint?.();
            }}
            title="Remove breakpoint"
          >
            <Circle className="size-1.5 fill-destructive-foreground/80 text-destructive-foreground/80" />
          </button>
        )}

        {/* Breakpoint gutter - appears on hover when no breakpoint is set */}
        {!breakpoint && onToggleBreakpoint && !isExecutionReadOnly && (
          <button
            type="button"
            className="absolute -left-1.5 top-1/2 z-10 flex size-3 -translate-y-1/2 cursor-pointer items-center justify-center rounded-full border border-destructive/50 bg-destructive/70 opacity-0 transition-all hover:bg-destructive hover:!opacity-100 group-hover:opacity-40"
            onClick={(e) => {
              e.stopPropagation();
              onToggleBreakpoint?.();
            }}
            title="Set breakpoint"
          >
            <Circle className="size-1.5 fill-destructive-foreground/80 text-destructive-foreground/80" />
          </button>
        )}

        {/* Unsaved changes corner dot */}
        {hasUnsavedChanges && !hasValidationError && !hasValidationWarning && (
          <div className="absolute right-0.5 top-0.5 z-10 size-1 rounded-full bg-warning" />
        )}

        {/* Horizontal pill layout: icon left, name + status center/right */}
        {(stepType !== undefined || name) && (
          <div
            className={cn(
              'flex w-full items-center gap-1.5 px-1.5',
              showCardDetails ? 'shrink-0' : 'h-full'
            )}
            style={{
              ...(rightReservedWidth
                ? { paddingRight: rightReservedWidth }
                : {}),
              ...(showCardDetails ? { height: COMPACT_CARD_HEIGHT } : {}),
            }}
          >
            {/* Left: Icon */}
            {stepType && (
              <div
                className={cn(
                  'flex size-4 flex-shrink-0 items-center justify-center rounded-sm [&_svg]:h-2.5 [&_svg]:w-2.5',
                  getIconTintClass()
                )}
              >
                <StepTypeIcon type={stepType} />
              </div>
            )}

            {/* Center: Step name, subtitle, and inline status pill */}
            <div
              className="flex min-w-0 flex-1 flex-col justify-center"
              title={name || stepIdLabel(id)}
            >
              {/* Row 1: Name + status pill */}
              <div className="flex min-w-0 items-center gap-0.5">
                {name ? (
                  <span className="min-w-0 flex-1 truncate text-xs font-medium leading-tight text-foreground">
                    {name}
                  </span>
                ) : stepIdLabel(id) ? (
                  <span className="min-w-0 flex-1 truncate font-mono text-xs leading-tight text-muted-foreground">
                    {stepIdLabel(id)}
                  </span>
                ) : (
                  <span className="min-w-0 flex-1 text-xs italic leading-tight text-muted-foreground">
                    Unnamed step
                  </span>
                )}
                {/* No step-type chip here: the icon and the subtitle already
                    say what the step is, and a chip (plus the stale-agent one)
                    truncated the step name. The hover preview shows the type. */}
                {/* Stale-agent badge — entitlement allowlist excludes this
                    step's agent. Workflow can't be saved/run until either the
                    entitlement is restored or the step is swapped. */}
                {hasStaleAgent && (
                  <span
                    title="Agent disabled — workflow can't be saved"
                    aria-label="Agent disabled"
                    data-testid="stale-agent-badge"
                    className="flex shrink-0 items-center gap-0.5 rounded-full bg-warning/10 px-1 py-0.5 text-[9px] font-medium leading-none text-warning"
                  >
                    <AlertTriangle className="size-2.5" />
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
                    'mt-0.5 block truncate text-[10px] leading-tight',
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
        {showCardDetails && cardDetails && (
          <div
            // A two-column table: shaded labels, a divider, a rule per row.
            className="flex min-h-0 flex-1 flex-col divide-y divide-border/70 overflow-hidden rounded-b-md border-t border-border/70 text-[10px]"
            data-testid="step-card-details"
            role="table"
            aria-label="Step inputs"
          >
            {cardDetails.visible.map((row) => (
              <StepDetailLine key={row.key} row={row} />
            ))}
            {cardDetails.hiddenCount > 0 && (
              <div
                role="row"
                className="flex min-h-0 flex-1 items-center bg-muted/30 px-2 text-muted-foreground"
              >
                <span role="cell">+{cardDetails.hiddenCount} more</span>
              </div>
            )}
          </div>
        )}
        {children}
      </div>
    );

    // The hover preview is for compact cards only: a card that already shows
    // its inputs as a table would just pop the same rows up again.
    if (
      !stepPreviews ||
      detailLevel === 'detailed' ||
      (stepType === undefined && !name)
    ) {
      return card;
    }

    return (
      <TooltipProvider delayDuration={450} skipDelayDuration={150}>
        <Tooltip>
          <TooltipTrigger asChild>{card}</TooltipTrigger>
          <TooltipContent
            side="bottom"
            align="start"
            collisionPadding={12}
            className="w-80 max-w-[min(20rem,calc(100vw-24px))] rounded-md border bg-popover p-3 text-popover-foreground shadow-lg"
            data-testid="step-preview"
          >
            <StepPreview
              id={id}
              name={name}
              stepType={stepType}
              summary={subtitleContent?.text}
              summaryIsError={subtitleContent?.className === 'text-destructive'}
              rows={detailRows ?? []}
              editable={editable}
              hasStaleAgent={hasStaleAgent}
            />
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
    );
  }
);

/** A row's value: references read "Source › field", the field emphasised. */
function DetailValue({ row }: { row: StepDetailRow }) {
  if (row.kind === 'reference' && row.reference) {
    const { source, path } = row.reference;
    if (!source || !path) {
      return (
        <span className="font-medium text-primary-text">{source || path}</span>
      );
    }
    return (
      <>
        <span className="text-muted-foreground">{source}</span>
        <span className="px-0.5 text-muted-foreground/60" aria-hidden="true">
          ›
        </span>
        <span className="sr-only"> field </span>
        <span className="font-medium text-primary-text">{path}</span>
      </>
    );
  }
  return <span className="text-foreground">{row.value}</span>;
}

function StepDetailLine({ row }: { row: StepDetailRow }) {
  return (
    <div
      role="row"
      className="grid min-h-0 flex-1 grid-cols-[minmax(0,40%)_minmax(0,1fr)] items-stretch"
    >
      <span
        role="rowheader"
        className="flex min-w-0 items-center border-r border-border/70 bg-muted/40 px-2 text-muted-foreground"
      >
        <span className="truncate">{row.label}</span>
      </span>
      <span role="cell" className="flex min-w-0 items-center px-2">
        <span className="truncate">
          <DetailValue row={row} />
        </span>
      </span>
    </div>
  );
}

function PreviewRows({
  rows,
  nested = false,
}: {
  rows: StepDetailRow[];
  nested?: boolean;
}) {
  return (
    <>
      {rows.map((row) => (
        <div key={row.key} className="contents">
          <dt
            className={cn(
              'truncate text-muted-foreground',
              nested && 'pl-3 before:mr-1 before:content-["↳"]'
            )}
          >
            {row.label}
          </dt>
          <dd className="line-clamp-3 break-words">
            {row.children && row.children.length > 0 ? (
              <span className="text-muted-foreground">
                {row.children.length} field
                {row.children.length === 1 ? '' : 's'}
              </span>
            ) : (
              <DetailValue row={row} />
            )}
          </dd>
          {row.children && row.children.length > 0 && (
            <PreviewRows rows={row.children} nested />
          )}
        </div>
      ))}
    </>
  );
}

/** Hover preview: everything a step is configured with, untruncated. */
function StepPreview({
  id,
  name,
  stepType,
  summary,
  summaryIsError,
  rows,
  editable,
  hasStaleAgent,
}: {
  id?: string;
  name?: string;
  stepType?: string;
  summary?: string;
  summaryIsError?: boolean;
  rows: StepDetailRow[];
  editable: boolean;
  hasStaleAgent?: boolean;
}): ReactNode {
  return (
    <div className="space-y-2 text-xs">
      <div className="flex min-w-0 items-center gap-2">
        <span className="min-w-0 flex-1 truncate font-medium text-foreground">
          {name || stepIdLabel(id) || 'Unnamed step'}
        </span>
        {stepType && (
          <Badge
            variant={stepBadgeVariant(stepType)}
            className="shrink-0 px-1.5 py-0 text-[10px]"
          >
            {stepType}
          </Badge>
        )}
      </div>
      {summary && (
        <p
          className={cn(
            'break-words leading-snug',
            summaryIsError ? 'text-destructive' : 'text-muted-foreground'
          )}
        >
          {summary}
        </p>
      )}
      {hasStaleAgent && (
        <p className="flex items-center gap-1.5 text-warning">
          <AlertTriangle className="size-3.5 shrink-0" aria-hidden="true" />
          Agent disabled — the workflow can&apos;t be saved or run until
          it&apos;s enabled or this step is replaced.
        </p>
      )}
      {rows.length > 0 && (
        <dl className="grid grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-3 gap-y-1 border-t pt-2">
          <PreviewRows rows={rows} />
        </dl>
      )}
      <div className="flex items-center justify-between gap-2 border-t pt-1.5 text-[10px] text-muted-foreground">
        <span className="truncate font-mono">{id}</span>
        {editable && <span className="shrink-0">Double-click to edit</span>}
      </div>
    </div>
  );
}
