import { useState, type ReactNode } from 'react';
import { Check, Copy, XCircle } from 'lucide-react';
import { formatDate, formatRelativeTime } from '@/lib/utils.ts';
import { Badge } from '@/shared/components/ui/badge.tsx';
import { Button } from '@/shared/components/ui/button';
import { Card, CardContent } from '@/shared/components/ui/card';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { StructuredErrorDisplay } from '@/shared/components/StructuredErrorDisplay';
import {
  getStatusDisplay,
  getTerminationTypeDisplay,
  isActiveStatus,
} from '@/shared/utils/status-display';
import { Spinner } from '@/shared/components/ui/spinner';
import { suspendedStatusLabel } from '@/features/workflows/utils/suspension';
import { ParentRunLink } from '@/features/invocation-history/components/RunLinks';
import type { ExecutionStats } from '@/features/workflows/components/ExecutionTimeline';
import { getStepTypeStyle } from '@/features/workflows/components/ExecutionTimeline/step-type-style';
import type { WorkflowInstanceWithMetadata } from '@/features/workflows/queries';

interface RunOverviewCardProps {
  run: WorkflowInstanceWithMetadata;
  /** Per-step-type timing reported by the Events timeline, once loaded. */
  executionStats: ExecutionStats | null;
  /** Run controls (replay, or pause/resume/stop while active). */
  actions?: ReactNode;
}

/**
 * A compact summary of a run: what happened and when, why it failed, its
 * key figures, then two bars showing queued vs. running time and time by
 * step type.
 */
export function RunOverviewCard({
  run,
  executionStats,
  actions,
}: RunOverviewCardProps) {
  const status = getStatusDisplay(run.status);
  const active = isActiveStatus(run.status);
  const startedAt = run.metadata?.startedAt;
  const completedAt = run.metadata?.completedAt;
  const lastEvent = completedAt ?? startedAt ?? run.created;
  // The status badge says what happened; the headline says when.
  const headline = active
    ? startedAt
      ? `Started ${formatRelativeTime(startedAt)}`
      : `Created ${formatRelativeTime(run.created)}, waiting to start`
    : `Finished ${formatRelativeTime(lastEvent)}`;
  const suspendedLabel = suspendedStatusLabel(run);
  const termination = getTerminationTypeDisplay(run.terminationType);

  const retryCount = run.metadata?.retryCount;
  const maxRetries = run.metadata?.maxRetries;
  const errorMessage = run.metadata?.errorMessage;
  const workerId = run.metadata?.workerId;
  const overhead = run.processingOverheadSeconds;
  const tags = run.tags ?? [];
  // A running run reports no duration yet; show how long it has run so far.
  const executionSeconds =
    run.executionDurationSeconds ??
    secondsBetween(startedAt, completedAt) ??
    (active
      ? secondsBetween(startedAt, new Date().toISOString())
      : undefined) ??
    null;
  const hasStepTypes =
    !!executionStats && Object.keys(executionStats.byType).length > 0;

  return (
    <Card>
      <CardContent className="space-y-4 p-4">
        {/* Identity, status and controls */}
        <div className="flex flex-wrap items-start gap-x-4 gap-y-3">
          <div className="min-w-0 space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="truncate text-base font-semibold">
                {run.runLabel || 'Run'}
              </h2>
              <span className="flex items-center gap-1 font-mono text-xs text-muted-foreground">
                {run.id}
                <CopyButton value={run.id} label="Copy run ID" />
              </span>
            </div>
            <p className="text-sm text-muted-foreground">
              {headline}
              {run.parentInstanceId && (
                <span
                  className="inline-flex items-center gap-1"
                  data-testid="started-by"
                >
                  <span className="px-1">·</span>
                  Started by
                  <ParentRunLink parentInstanceId={run.parentInstanceId} />
                </span>
              )}
            </p>
          </div>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            {tags.map((tag) => (
              <Badge key={tag} variant="secondary" className="px-2 py-0">
                {tag}
              </Badge>
            ))}
            {run.usedVersion !== undefined && (
              <WithTooltip label="Workflow version this run executed">
                <Badge variant="secondary">v{run.usedVersion}</Badge>
              </WithTooltip>
            )}
            <Badge variant={status.variant} className="gap-1">
              {status.showSpinner && <Spinner className="size-3" />}
              {status.text}
            </Badge>
            {suspendedLabel && (
              <Badge variant="warning" title="Why this run is suspended">
                {suspendedLabel}
              </Badge>
            )}
            {termination && (
              <WithTooltip label="Why this run ended">
                <Badge variant={termination.variant}>{termination.text}</Badge>
              </WithTooltip>
            )}
            {actions}
          </div>
        </div>

        {errorMessage && (
          <div>
            <div className="mb-1.5 flex items-center gap-1.5 text-xs font-medium text-destructive">
              <XCircle className="size-3.5" />
              Why it failed
            </div>
            <StructuredErrorDisplay
              error={errorMessage}
              mode="expanded"
              showGuidance
            />
          </div>
        )}

        {/* Key figures */}
        <dl className="flex flex-wrap gap-x-8 gap-y-3">
          <Figure
            label="Duration"
            hint="Time spent running the workflow, excluding time waiting in the queue."
            value={
              executionSeconds !== null ? formatSeconds(executionSeconds) : '—'
            }
            note={
              active && executionSeconds !== null
                ? 'so far'
                : run.queueDurationSeconds && run.queueDurationSeconds >= 0.01
                  ? `+ ${formatSeconds(run.queueDurationSeconds)} queued`
                  : undefined
            }
          />
          <Figure
            label="Steps"
            hint="Top-level steps this run executed. Loops and branches count once."
            value={executionStats ? String(executionStats.stepCount) : '—'}
          />
          {retryCount !== undefined && maxRetries !== undefined && (
            <Figure
              label="Retries"
              hint="Automatic retries after a failed attempt, out of the maximum allowed."
              value={
                retryCount === 0 ? 'None' : `${retryCount} of ${maxRetries}`
              }
              note={retryCount === 0 ? `of ${maxRetries} allowed` : undefined}
              tone={retryCount > 0 ? 'warning' : undefined}
            />
          )}
          {run.maxMemoryMb !== undefined && run.maxMemoryMb !== null && (
            <Figure
              label="Peak memory"
              hint="The most memory the run used at any one time."
              value={`${run.maxMemoryMb.toFixed(1)} MB`}
            />
          )}
          {overhead !== undefined && overhead !== null && (
            <Figure
              label="Overhead"
              hint="Setup and coordination before and after the run, outside its duration."
              value={formatSeconds(overhead)}
              muted
            />
          )}
          {workerId && (
            <Figure
              label="Worker"
              hint="The server that ran this workflow. Useful when tracing a problem to a host."
              value={
                <span className="flex items-center gap-1 font-mono text-sm font-normal">
                  {workerId}
                  <CopyButton value={workerId} label="Copy worker ID" />
                </span>
              }
              muted
            />
          )}
        </dl>

        {/* Where the time went */}
        <div
          className={`grid gap-x-8 gap-y-4 ${hasStepTypes ? 'lg:grid-cols-2' : ''}`}
        >
          <RunPhases
            status={run.status}
            active={active}
            created={run.created}
            startedAt={startedAt}
            completedAt={completedAt}
          />
          {executionStats && hasStepTypes && (
            <TimeByStepType stats={executionStats} />
          )}
        </div>
      </CardContent>
    </Card>
  );
}

function Figure({
  label,
  hint,
  value,
  note,
  tone,
  muted,
}: {
  label: string;
  hint: string;
  value: ReactNode;
  note?: string;
  tone?: 'warning';
  /** Secondary, debugging-only figures read quieter. */
  muted?: boolean;
}) {
  return (
    <div>
      <dt className="text-xs text-muted-foreground">
        <WithTooltip label={hint}>
          <span className="cursor-help underline decoration-muted-foreground/40 decoration-dotted underline-offset-2">
            {label}
          </span>
        </WithTooltip>
      </dt>
      <dd
        className={`flex items-baseline gap-1.5 tabular-nums ${
          muted ? 'text-sm' : 'text-base font-semibold'
        } ${tone === 'warning' ? 'text-warning' : 'text-foreground'}`}
      >
        {value}
        {note && (
          <span className="text-xs font-normal text-muted-foreground">
            {note}
          </span>
        )}
      </dd>
    </div>
  );
}

/** Small section label above each bar. */
function BarLabel({
  children,
  aside,
}: {
  children: ReactNode;
  aside?: string;
}) {
  return (
    <div className="mb-1.5 flex items-baseline justify-between gap-4 text-xs">
      <span className="font-medium text-muted-foreground">{children}</span>
      {aside && <span className="text-muted-foreground">{aside}</span>}
    </div>
  );
}

/**
 * Created → started → finished on one bar, so a long queue wait stands out
 * next to a short run (or the other way round).
 */
function RunPhases({
  status,
  active,
  created,
  startedAt,
  completedAt,
}: {
  status: string;
  active: boolean;
  created: string;
  startedAt?: string;
  completedAt?: string;
}) {
  const createdMs = Date.parse(created);
  const startedMs = startedAt ? Date.parse(startedAt) : null;
  const endMs = completedAt
    ? Date.parse(completedAt)
    : active
      ? Date.now()
      : (startedMs ?? createdMs);
  const span = Math.max(endMs - createdMs, 1);
  const queuedMs = Math.max((startedMs ?? endMs) - createdMs, 0);
  const ranMs = startedMs !== null ? Math.max(endMs - startedMs, 0) : 0;
  // Keep a sliver of each phase that happened visible even when the other
  // one dwarfs it.
  const queuedPct = queuedMs > 0 ? Math.max((queuedMs / span) * 100, 2) : 0;
  const ranPct = startedMs !== null ? Math.max(100 - queuedPct, 2) : 0;

  const normalized = status.toLowerCase();
  const runColor = active
    ? 'bg-info animate-pulse'
    : normalized === 'failed' || normalized === 'error'
      ? 'bg-destructive'
      : normalized === 'completed' || normalized === 'success'
        ? 'bg-success'
        : 'bg-muted-foreground/60';
  const endLabel = completedAt
    ? getStatusDisplay(status).text
    : active
      ? 'Now'
      : 'Ended';

  return (
    <section className="min-w-0">
      <BarLabel aside={formatDate(new Date(created), 'd MMM yyyy')}>
        Queued, then running
      </BarLabel>
      <div className="flex h-1.5 gap-0.5 overflow-hidden rounded-full bg-muted">
        {queuedPct > 0 && (
          <div
            className="h-full bg-muted-foreground/30"
            style={{ width: `${queuedPct}%` }}
            title={`Queued ${formatMs(queuedMs)}`}
          />
        )}
        {ranPct > 0 && (
          <div
            className={`h-full ${runColor}`}
            style={{ width: `${ranPct}%` }}
            title={`Ran ${formatMs(ranMs)}`}
          />
        )}
      </div>
      <ol className="mt-1.5 flex flex-wrap justify-between gap-x-3 gap-y-0.5 text-xs">
        <PhasePoint label="Created" at={created} />
        <PhasePoint label="Started" at={startedAt} />
        <PhasePoint
          label={endLabel}
          at={
            completedAt ?? (active ? new Date(endMs).toISOString() : undefined)
          }
        />
      </ol>
    </section>
  );
}

function PhasePoint({ label, at }: { label: string; at?: string }) {
  return (
    <li
      className="whitespace-nowrap"
      title={at ? formatDate(new Date(at), 'yyyy-MM-dd HH:mm:ss.SSS') : ''}
    >
      <span className="text-muted-foreground">{label} </span>
      <span className="font-medium tabular-nums text-foreground">
        {at ? formatDate(new Date(at), 'HH:mm:ss') : '—'}
      </span>
    </li>
  );
}

/** Share of step time per step type, colored like the Events timeline. */
function TimeByStepType({ stats }: { stats: ExecutionStats }) {
  const entries = Object.entries(stats.byType).sort(([, a], [, b]) => b - a);
  const stepTotal = entries.reduce((sum, [, ms]) => sum + ms, 0);

  return (
    <section className="min-w-0">
      <BarLabel>Time by step type</BarLabel>
      <div className="flex h-1.5 gap-0.5 overflow-hidden rounded-full bg-muted">
        {stepTotal > 0 &&
          entries.map(([type, ms]) =>
            ms > 0 ? (
              <div
                key={type}
                className={`h-full ${getStepTypeStyle(type).dot}`}
                style={{ width: `${(ms / stepTotal) * 100}%` }}
                title={`${getStepTypeStyle(type).label}: ${formatMs(ms)}`}
              />
            ) : null
          )}
      </div>
      <ul className="mt-1.5 flex flex-wrap gap-x-4 gap-y-1 text-xs">
        {entries.map(([type, ms]) => (
          <li key={type} className="flex items-center gap-1.5">
            <span
              className={`size-2 rounded-full ${getStepTypeStyle(type).dot}`}
            />
            <span className="text-foreground">
              {getStepTypeStyle(type).label}
            </span>
            <span className="tabular-nums text-muted-foreground">
              {formatMs(ms)}
              {stepTotal > 0 && ` · ${Math.round((ms / stepTotal) * 100)}%`}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <WithTooltip label={copied ? 'Copied' : label}>
      <Button
        variant="secondary"
        size="icon-sm"
        className="size-5"
        aria-label={label}
        onClick={() => {
          void navigator.clipboard.writeText(value).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          });
        }}
      >
        {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
      </Button>
    </WithTooltip>
  );
}

function secondsBetween(from?: string, to?: string): number | undefined {
  if (!from || !to) return undefined;
  const ms = Date.parse(to) - Date.parse(from);
  return Number.isFinite(ms) && ms >= 0 ? ms / 1000 : undefined;
}

function formatSeconds(seconds: number): string {
  return formatMs(seconds * 1000);
}

function formatMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)}s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  if (minutes < 60) return `${minutes}m ${seconds}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}
