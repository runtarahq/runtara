import { ArrowDown } from 'lucide-react';
import { cn } from '@/lib/utils';
import {
  DEFAULT_STUCK_AFTER_MS,
  formatAge,
  formatCount,
  formatRate,
  isNotDraining,
  severityOf,
  sparklinePath,
  utilisation,
  type PipelineStage,
} from '../../utils/pipeline';

interface PipelineStageRowProps {
  stage: PipelineStage;
  /// Occupancy over the last minute, oldest first; `null` where unread.
  history: (number | null)[];
  /// Rate flowing into this stage, or `null` before the first window closes.
  inflow: number | null;
  /// Is the pipeline being offered any work at all?
  ///
  /// Zero throughput is only a symptom when there is something upstream to
  /// have moved. On an idle deployment every stage is legitimately at zero, and
  /// reddening all six would cry wolf on a system doing exactly what it should.
  pipelineActive: boolean;
  isChokepoint: boolean;
  /** Server policy carried by the snapshot; falls back during rolling deploys. */
  stuckAfterMs?: number;
}

const SEVERITY_STROKE: Record<string, string> = {
  ok: 'stroke-primary',
  warn: 'stroke-amber-500',
  bad: 'stroke-destructive',
  unknown: 'stroke-muted-foreground',
};

const SEVERITY_FILL: Record<string, string> = {
  ok: 'fill-primary/15',
  warn: 'fill-amber-500/20',
  bad: 'fill-destructive/15',
  unknown: 'fill-transparent',
};

const SEVERITY_BAR: Record<string, string> = {
  ok: 'bg-primary',
  warn: 'bg-amber-500',
  bad: 'bg-destructive',
  unknown: 'bg-muted-foreground/40',
};

export function PipelineStageRow({
  stage,
  history,
  inflow,
  pipelineActive,
  isChokepoint,
  stuckAfterMs = DEFAULT_STUCK_AFTER_MS,
}: PipelineStageRowProps) {
  const pct = utilisation(stage);
  const severity = severityOf(stage);
  const bounded = stage.limit !== null;
  const path = sparklinePath(history, stage.limit);
  const stuck = isNotDraining(stage, stuckAfterMs);
  const age = formatAge(stage.oldestAgeMs);
  const capacityRejections = stage.capacityRejections ?? null;
  const reapingPrecompileChildren = stage.reapingPrecompileChildren ?? null;
  const topWorkflows = stage.topWorkflows ?? [];
  const workflowAttribution = topWorkflows
    .map(
      (workflow) => `${workflow.workflowId} (${formatCount(workflow.count)})`
    )
    .join(', ');

  return (
    <div
      className={cn(
        'grid items-center gap-x-4 rounded-lg border bg-card px-3 py-2.5',
        'grid-cols-[64px_minmax(0,1fr)] md:grid-cols-[80px_180px_minmax(0,1fr)_150px_130px]',
        isChokepoint
          ? 'border-destructive/60 ring-1 ring-destructive/30'
          : 'border-border/40'
      )}
      data-testid={`pipeline-stage-${stage.key}`}
      data-chokepoint={isChokepoint ? 'true' : 'false'}
    >
      {/* Inflow. Read top to bottom, this column is where throughput dies —
          and the row it dies on is the one at fault. */}
      <div className="flex flex-col items-center gap-0.5 text-muted-foreground">
        <ArrowDown className="size-3 opacity-50" aria-hidden="true" />
        <span
          className={cn(
            'text-sm font-semibold tabular-nums',
            inflow === 0 && pipelineActive
              ? 'text-destructive'
              : 'text-foreground'
          )}
        >
          {formatRate(inflow)}
          <span className="ml-0.5 text-[10px] font-medium text-muted-foreground">
            /s
          </span>
        </span>
      </div>

      <div className="min-w-0">
        <h3 className="truncate text-sm font-semibold text-foreground">
          {stage.label}
        </h3>
        {stage.knob && (
          <div className="truncate font-mono text-[10.5px] leading-tight text-muted-foreground">
            {stage.knob}
          </div>
        )}
      </div>

      {/* Occupancy over the last minute. A line pinned at the top is not by
          itself a fault: textured means turning work over as fast as the host
          allows, ruled-flat means holding work that never leaves. */}
      <div className="hidden h-10 md:block">
        {path ? (
          <svg
            viewBox="0 0 100 100"
            preserveAspectRatio="none"
            className="h-full w-full"
            aria-hidden="true"
          >
            {bounded && (
              <path
                d={path.area}
                className={SEVERITY_FILL[severity]}
                stroke="none"
              />
            )}
            <path
              d={path.line}
              fill="none"
              strokeWidth={1.5}
              vectorEffect="non-scaling-stroke"
              strokeLinejoin="round"
              className={SEVERITY_STROKE[severity]}
            />
          </svg>
        ) : (
          <div className="flex h-full items-center text-[11px] text-muted-foreground">
            collecting…
          </div>
        )}
      </div>

      <div className="hidden md:block">
        <div className="text-base font-semibold tabular-nums text-foreground">
          {formatCount(stage.used)}
          <span className="ml-1 text-xs font-medium text-muted-foreground">
            {bounded ? `/ ${formatCount(stage.limit)}` : 'no limit'}
          </span>
        </div>
        {bounded && (
          <>
            <div className="mt-1 h-1.5 w-full overflow-hidden rounded-full bg-muted">
              <div
                className={cn(
                  'h-full rounded-full transition-[width] duration-700 ease-linear',
                  SEVERITY_BAR[severity]
                )}
                style={{ width: `${pct ?? 0}%` }}
              />
            </div>
            <div className="mt-0.5 text-[11px] tabular-nums text-muted-foreground">
              {/* Unread is a dash, never 0% — an unobserved bound is not an
                  empty one. */}
              {pct === null ? (
                <span>not measured</span>
              ) : (
                <>
                  <span className="font-medium text-foreground">
                    {pct.toFixed(0)}%
                  </span>{' '}
                  in use
                </>
              )}
            </div>
          </>
        )}
      </div>

      <div className="hidden text-[11.5px] md:block">
        {stuck && (
          <span className="inline-block rounded bg-destructive/10 px-2 py-1 font-medium text-destructive">
            not draining
          </span>
        )}
        {age && (
          <span className="mt-1 block tabular-nums text-muted-foreground">
            {age} oldest
          </span>
        )}
        {capacityRejections !== null && capacityRejections > 0 && (
          <span
            className="mt-1 block tabular-nums text-amber-600 dark:text-amber-400"
            data-testid={`pipeline-capacity-rejections-${stage.key}`}
          >
            {formatCount(capacityRejections)} capacity{' '}
            {capacityRejections === 1 ? 'retry' : 'retries'}
          </span>
        )}
        {reapingPrecompileChildren !== null &&
          reapingPrecompileChildren > 0 && (
            <span
              className="mt-1 block tabular-nums text-amber-600 dark:text-amber-400"
              data-testid={`pipeline-reaping-precompile-children-${stage.key}`}
            >
              {formatCount(reapingPrecompileChildren)} child{' '}
              {reapingPrecompileChildren === 1 ? 'reaping' : 'children reaping'}
            </span>
          )}
        {workflowAttribution && (
          <span
            className="mt-1 block truncate text-muted-foreground"
            title={`Top workflows: ${workflowAttribution}`}
            data-testid={`pipeline-workflow-attribution-${stage.key}`}
          >
            top: {workflowAttribution}
          </span>
        )}
      </div>
    </div>
  );
}
