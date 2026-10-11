import { useState, type ElementType, type ReactNode } from 'react';
import {
  Check,
  Copy,
  Layers,
  MemoryStick,
  RotateCw,
  Tag,
  Timer,
  XCircle,
} from 'lucide-react';
import { formatDate, formatRelativeTime } from '@/lib/utils.ts';
import { Badge } from '@/shared/components/ui/badge.tsx';
import { Button } from '@/shared/components/ui/button';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { StructuredErrorDisplay } from '@/shared/components/StructuredErrorDisplay';
import {
  getStatusDisplay,
  isActiveStatus,
} from '@/shared/utils/status-display';
import type { ExecutionStats } from '@/features/workflows/components/ExecutionTimeline';
import { getStepTypeStyle } from '@/features/workflows/components/ExecutionTimeline/step-type-style';
import type { WorkflowInstanceWithMetadata } from '@/features/workflows/queries';

interface RunOverviewCardProps {
  run: WorkflowInstanceWithMetadata;
  /** Per-step-type timing reported by the Events timeline, once loaded. */
  executionStats: ExecutionStats | null;
}

/**
 * Everything a reader checks first about a run, in one card: what happened
 * and when (headline + error), how long it took and where that time went
 * (key figures, the queued/ran phases, time by step type), then the
 * debugging details (worker, overhead, tags).
 */
export function RunOverviewCard({ run, executionStats }: RunOverviewCardProps) {
  const status = getStatusDisplay(run.status);
  const active = isActiveStatus(run.status);
  const startedAt = run.metadata?.startedAt;
  const completedAt = run.metadata?.completedAt;
  const lastEvent = completedAt ?? startedAt ?? run.created;
  const headline = active
    ? startedAt
      ? `Started ${formatRelativeTime(startedAt)}, still running`
      : `Waiting to start, created ${formatRelativeTime(run.created)}`
    : `${status.text} ${formatRelativeTime(lastEvent)}`;

  const retryCount = run.metadata?.retryCount;
  const maxRetries = run.metadata?.maxRetries;
  const errorMessage = run.metadata?.errorMessage;
  // A running run reports no duration yet; show how long it has run so far.
  const executionSeconds =
    run.executionDurationSeconds ??
    secondsBetween(startedAt, completedAt) ??
    (active
      ? secondsBetween(startedAt, new Date().toISOString())
      : undefined) ??
    null;
  const typeCount = executionStats
    ? Object.keys(executionStats.byType).length
    : 0;

  const figures: FigureProps[] = [
    {
      icon: Timer,
      label: 'Duration',
      hint: 'Time spent running the workflow, excluding time waiting in the queue.',
      value: executionSeconds !== null ? formatSeconds(executionSeconds) : '—',
      note:
        active && executionSeconds !== null
          ? 'so far'
          : run.queueDurationSeconds && run.queueDurationSeconds >= 0.01
            ? `+ ${formatSeconds(run.queueDurationSeconds)} in queue`
            : undefined,
    },
    {
      icon: Layers,
      label: 'Steps',
      hint: 'Top-level steps this run executed. Loops and branches count once.',
      value: executionStats ? String(executionStats.stepCount) : '—',
      note:
        executionStats && typeCount < executionStats.stepCount
          ? pluralize(typeCount, 'step type')
          : undefined,
    },
  ];
  if (retryCount !== undefined && maxRetries !== undefined) {
    figures.push({
      icon: RotateCw,
      label: 'Retries',
      hint: 'Automatic retries after a failed attempt, out of the maximum allowed.',
      value: retryCount === 0 ? 'None' : `${retryCount} of ${maxRetries}`,
      note:
        retryCount === 0 ? `up to ${maxRetries} allowed` : 'after a failure',
      tone: retryCount > 0 ? 'warning' : undefined,
    });
  }
  if (run.maxMemoryMb !== undefined && run.maxMemoryMb !== null) {
    figures.push({
      icon: MemoryStick,
      label: 'Peak memory',
      hint: 'The most memory the run used at any one time.',
      value: `${run.maxMemoryMb.toFixed(1)} MB`,
    });
  }

  return (
    <Card>
      <CardHeader className="flex-row items-start justify-between gap-4 space-y-0 pb-4">
        <div className="space-y-1">
          <CardTitle className="text-lg">Run overview</CardTitle>
          <p className="text-sm text-muted-foreground">{headline}</p>
        </div>
        {run.usedVersion !== undefined && (
          <WithTooltip label="Workflow version this run executed">
            <Badge variant="secondary" className="shrink-0">
              v{run.usedVersion}
            </Badge>
          </WithTooltip>
        )}
      </CardHeader>

      <CardContent className="space-y-6">
        {errorMessage && (
          <div>
            <div className="mb-2 flex items-center gap-2 text-sm font-medium text-destructive">
              <XCircle className="size-4" />
              Why it failed
            </div>
            <StructuredErrorDisplay
              error={errorMessage}
              mode="expanded"
              showGuidance
            />
          </div>
        )}

        <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border sm:grid-cols-[repeat(auto-fit,minmax(9rem,1fr))]">
          {figures.map((figure) => (
            <Figure key={figure.label} {...figure} />
          ))}
        </dl>

        <RunPhases
          status={run.status}
          active={active}
          created={run.created}
          startedAt={startedAt}
          completedAt={completedAt}
        />

        {executionStats && Object.keys(executionStats.byType).length > 0 && (
          <TimeByStepType stats={executionStats} />
        )}

        <TechnicalDetails run={run} />
      </CardContent>
    </Card>
  );
}

interface FigureProps {
  icon: ElementType;
  label: string;
  hint: string;
  value: string;
  note?: string;
  tone?: 'warning';
}

function Figure({ icon: Icon, label, hint, value, note, tone }: FigureProps) {
  return (
    <div className="bg-card px-4 py-3">
      <dt className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Icon className="size-3.5" />
        <WithTooltip label={hint}>
          <span className="cursor-help underline decoration-muted-foreground/50 decoration-dotted underline-offset-2">
            {label}
          </span>
        </WithTooltip>
      </dt>
      <dd
        className={`mt-1 text-xl font-semibold tabular-nums ${
          tone === 'warning' ? 'text-warning' : 'text-foreground'
        }`}
      >
        {value}
      </dd>
      {note && <p className="mt-0.5 text-xs text-muted-foreground">{note}</p>}
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
      ? 'Still running'
      : 'Ended';

  return (
    <section>
      <div className="mb-2 flex items-baseline justify-between gap-4">
        <h3 className="text-sm font-medium">Timeline</h3>
        <span className="text-xs text-muted-foreground">
          {formatDate(new Date(created), 'EEE, d MMM yyyy')}
        </span>
      </div>
      <div className="flex h-2.5 gap-0.5 overflow-hidden rounded-full bg-muted">
        {queuedPct > 0 && (
          <div
            className="h-full bg-muted-foreground/30"
            style={{ width: `${queuedPct}%` }}
          />
        )}
        {ranPct > 0 && (
          <div
            className={`h-full ${runColor}`}
            style={{ width: `${ranPct}%` }}
          />
        )}
      </div>
      <ol className="mt-2 grid grid-cols-3 gap-2 text-xs">
        <PhasePoint label="Created" at={created} />
        <PhasePoint
          label="Started"
          at={startedAt}
          note={queuedMs > 0 ? `after ${formatMs(queuedMs)} queued` : undefined}
          align="center"
        />
        <PhasePoint
          label={endLabel}
          at={
            completedAt ?? (active ? new Date(endMs).toISOString() : undefined)
          }
          note={startedMs !== null ? `ran ${formatMs(ranMs)}` : undefined}
          align="right"
        />
      </ol>
    </section>
  );
}

function PhasePoint({
  label,
  at,
  note,
  align = 'left',
}: {
  label: string;
  at?: string;
  note?: string;
  align?: 'left' | 'center' | 'right';
}) {
  const alignClass =
    align === 'center'
      ? 'text-center'
      : align === 'right'
        ? 'text-right'
        : 'text-left';
  return (
    <li className={alignClass}>
      <div className="text-muted-foreground">{label}</div>
      <div
        className="font-medium tabular-nums text-foreground"
        title={at ? formatDate(new Date(at), 'yyyy-MM-dd HH:mm:ss.SSS') : ''}
      >
        {at ? formatDate(new Date(at), 'HH:mm:ss') : '—'}
      </div>
      {note && <div className="text-muted-foreground">{note}</div>}
    </li>
  );
}

/** Share of step time per step type, colored like the Events timeline. */
function TimeByStepType({ stats }: { stats: ExecutionStats }) {
  const entries = Object.entries(stats.byType).sort(([, a], [, b]) => b - a);
  const stepTotal = entries.reduce((sum, [, ms]) => sum + ms, 0);

  return (
    <section>
      <h3 className="mb-2 text-sm font-medium">Where the time went</h3>
      <div className="flex h-2.5 gap-0.5 overflow-hidden rounded-full bg-muted">
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
      <ul className="mt-2 flex flex-wrap gap-x-5 gap-y-1 text-xs">
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

/** Low-priority details people reach for when debugging a specific run. */
function TechnicalDetails({ run }: { run: WorkflowInstanceWithMetadata }) {
  const workerId = run.metadata?.workerId;
  const overhead = run.processingOverheadSeconds;
  const tags = run.tags ?? [];
  if (
    !workerId &&
    (overhead === undefined || overhead === null) &&
    !tags.length
  )
    return null;

  return (
    <div className="flex flex-wrap items-center gap-x-6 gap-y-2 border-t pt-4 text-xs text-muted-foreground">
      {workerId && (
        <DetailItem label="Worker">
          <span className="font-mono text-foreground">{workerId}</span>
          <CopyButton value={workerId} label="Copy worker ID" />
        </DetailItem>
      )}
      {overhead !== undefined && overhead !== null && (
        <DetailItem label="Platform overhead">
          <span
            className="tabular-nums text-foreground"
            title="Setup and coordination before and after the run, outside its execution time."
          >
            {formatSeconds(overhead)}
          </span>
        </DetailItem>
      )}
      {tags.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          <Tag className="size-3.5" />
          {tags.map((tag) => (
            <Badge key={tag} variant="secondary" className="px-2 py-0">
              {tag}
            </Badge>
          ))}
        </div>
      )}
    </div>
  );
}

function DetailItem({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <span>{label}</span>
      {children}
    </div>
  );
}

function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <WithTooltip label={copied ? 'Copied' : label}>
      <Button
        variant="secondary"
        size="icon-sm"
        className="size-6"
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

function pluralize(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? '' : 's'}`;
}
