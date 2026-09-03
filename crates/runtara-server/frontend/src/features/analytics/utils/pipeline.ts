/// Classification for the execution-pipeline view.
///
/// The server sends facts and no judgement, so deciding which stage is the
/// constraint happens here — where it can be tested against fixtures without a
/// running server. That matters more than it sounds: a prototype of this view
/// got the choice wrong three separate ways, each of them plausible on
/// inspection and each only visible under a fixture. Those three are pinned by
/// name in `pipeline.test.ts`.

export interface PipelineRates {
  offered: number;
  accepted: number;
  denied: number;
  started: number;
  finished: number;
  /** `null` means no live run could have reported a step — not that none did. */
  steps: number | null;
}

/** A bounded workflow contributor for one durable launch stage. */
export interface PipelineWorkflowAttribution {
  workflowId: string;
  count: number;
  oldestAgeMs: number | null;
}

export interface PipelineStage {
  key: string;
  label: string;
  knob: string | null;
  /** `null` for a stage with no ceiling. */
  limit: number | null;
  /** `null` when the source could not be read, which is not the same as 0. */
  used: number | null;
  oldestAgeMs: number | null;
  inflowKey: string;
  /**
   * Current queued launches whose latest dispatcher result was an exhausted
   * runner. Omitted by older servers and inapplicable stages.
   */
  capacityRejections?: number | null;
  /** Timed-out precompile children still awaiting the bounded reaper. */
  reapingPrecompileChildren?: number | null;
  /** Highest-count workflow contributors, capped server-side. */
  topWorkflows?: PipelineWorkflowAttribution[];
}

export interface PipelineSnapshot {
  capturedAt: string;
  /**
   * Server policy for the "not draining" callout. Optional during a rolling
   * upgrade so a newer console can still render a snapshot from an older
   * server with the documented fallback.
   */
  stuckAfterMs?: number;
  windowMs: number;
  /** `null` on the first tick after start, when there is no window yet. */
  rates: PipelineRates | null;
  stages: PipelineStage[];
}

/** Utilisation at which a stage is worth calling out. */
export const CHOKE_THRESHOLD = 80;

/** How long a full stage may hold its oldest item before that is remarkable. */
export const DEFAULT_STUCK_AFTER_MS = 5 * 60 * 1000;

/** The server policy carried by a snapshot, with a safe rollout fallback. */
export function snapshotStuckAfterMs(snapshot: PipelineSnapshot): number {
  return snapshot.stuckAfterMs ?? DEFAULT_STUCK_AFTER_MS;
}

export type StageSeverity = 'unknown' | 'ok' | 'warn' | 'bad';

/// A stage's utilisation, or `null` when that question does not apply.
///
/// Two distinct reasons for `null`: the stage has no ceiling to be a fraction
/// of, or its occupancy could not be read. Neither is zero percent.
export function utilisation(stage: PipelineStage): number | null {
  if (stage.limit === null || stage.limit === 0 || stage.used === null) {
    return null;
  }
  return Math.min(100, (stage.used / stage.limit) * 100);
}

export function severityOf(stage: PipelineStage): StageSeverity {
  const pct = utilisation(stage);
  if (pct === null) return 'unknown';
  if (pct >= 99) return 'bad';
  if (pct >= CHOKE_THRESHOLD) return 'warn';
  return 'ok';
}

/// Is this stage holding observable work that is not moving?
///
/// A bounded capacity stage needs to be full and old: capacity in use alone is
/// normal. An old durable launch generation is different: it has already been
/// admitted, so even one that is queued for too long is a real blocked
/// condition. The queue borrows the admission limit for occupancy display, but
/// must not borrow its "full" requirement for that diagnosis.
export function isNotDraining(
  stage: PipelineStage,
  stuckAfterMs: number = DEFAULT_STUCK_AFTER_MS
): boolean {
  if (
    stage.used === null ||
    stage.used === 0 ||
    stage.oldestAgeMs === null ||
    stage.oldestAgeMs < stuckAfterMs
  ) {
    return false;
  }
  // These are terminal outcome counters, not work waiting to make progress.
  // Their oldest retained row can legitimately be days old; labelling that
  // history "not draining" would hide the actionable live stages in noise.
  if (stage.key === 'launchExpired' || stage.key === 'launchCancelled') {
    return false;
  }
  if (stage.key === 'launchQueued' || stage.key === 'launchPreparing') return true;
  const pct = utilisation(stage);
  // An unbounded queue cannot be the capacity chokepoint by itself, but an old
  // queued generation is still a real blocked condition and must be called
  // out. `findChokepoint` keeps launchQueued out of candidates explicitly:
  // that row describes the evidence, not necessarily the downstream limit.
  return pct === null || pct >= 99;
}

/// The stage actually constraining the pipeline, or `null` if none is.
///
/// Three rules, each of them a bug this had before:
///
/// 1. **Only bounded stages are candidates.** An unbounded queue is where the
///    evidence piles up, never the constraint. Ranking by raw depth put the
///    trigger queue at fault while the run permits — full, and holding work
///    that had not moved in forty-eight minutes — went unnamed.
/// 2. **Nothing is named below the threshold.** A page that always points at
///    something teaches its reader to ignore where it points.
/// 3. **Not-draining outranks merely-full.** Otherwise whichever full stage
///    came first in the list won, and full stages are common while stuck ones
///    are the emergency.
///
/// A fourth rule lives at the call site rather than here: this is a pure
/// function of one snapshot, so a caller that re-runs it every tick must damp
/// the result, or ordinary jitter across the threshold makes the highlight
/// flicker while somebody is reading it. See `stickyChokepoint`.
export function findChokepoint(
  stages: PipelineStage[],
  stuckAfterMs: number = DEFAULT_STUCK_AFTER_MS
): PipelineStage | null {
  let best: PipelineStage | null = null;
  let bestScore = -1;

  for (const stage of stages) {
    // The launch queue shows what is stuck, but it inherits the admission
    // ceiling solely for occupancy context. It must not be chosen as the
    // capacity constraint just because an old row received the stuck bonus.
    if (stage.key === 'launchQueued' || stage.key === 'launchPreparing') continue;
    const pct = utilisation(stage);
    if (pct === null) continue;
    const score = pct + (isNotDraining(stage, stuckAfterMs) ? 1000 : 0);
    if (score > bestScore) {
      bestScore = score;
      best = stage;
    }
  }

  return bestScore >= CHOKE_THRESHOLD ? best : null;
}

/// Hold a chokepoint choice steady across ticks.
///
/// Occupancy wobbles, and a stage hovering at the threshold would otherwise
/// have its highlight appear and vanish once a second — unreadable, and it
/// makes a real constraint look like noise. Once named, a stage keeps the
/// label until it drops a clear margin below the threshold.
export function stickyChokepoint(
  stages: PipelineStage[],
  previousKey: string | null,
  stuckAfterMs: number = DEFAULT_STUCK_AFTER_MS
): string | null {
  const fresh = findChokepoint(stages, stuckAfterMs);
  if (fresh) return fresh.key;

  if (previousKey) {
    const held = stages.find((s) => s.key === previousKey);
    if (held) {
      const pct = utilisation(held);
      // Hysteresis: five points of margin, so a stage sitting on the line does
      // not toggle, but one that genuinely recovers is released promptly.
      if (pct !== null && pct >= CHOKE_THRESHOLD - 5) return previousKey;
    }
  }

  return null;
}

/// Is the steps-per-second figure a measurement or an absence of one?
///
/// `trackEvents` is compile-time, so a workflow built without it runs perfectly
/// and reports nothing. Rendering that absence as `0/s` would let a reader
/// conclude a healthy deployment had stopped dead.
export function stepsAreMeasured(rates: PipelineRates | null): boolean {
  return rates !== null && rates.steps !== null;
}

/// Format an age for display, coarsening as it grows.
///
/// Sub-second precision matters when permits recycle in milliseconds; at
/// forty-eight minutes nobody needs the seconds.
export function formatAge(ms: number | null): string | null {
  if (ms === null) return null;
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m`;
  if (ms < 86_400_000) return `${Math.round(ms / 3_600_000)}h`;
  return `${Math.round(ms / 86_400_000)}d`;
}

/// Format a count, keeping large stocks legible.
export function formatCount(value: number | null): string {
  if (value === null) return '—';
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`;
  if (value >= 100_000) return `${Math.round(value / 1000)}k`;
  if (value >= 10_000) return `${(value / 1000).toFixed(1)}k`;
  return Math.round(value).toLocaleString();
}

/// Format a per-second rate.
export function formatRate(value: number | null | undefined): string {
  if (value === null || value === undefined) return '—';
  if (value >= 1000) return `${(value / 1000).toFixed(1)}k`;
  if (value >= 100) return Math.round(value).toLocaleString();
  if (value >= 10) return value.toFixed(0);
  return value.toFixed(1);
}

/// Points for a sparkline, scaled from zero.
///
/// Zero-based on purpose. Scaling an unbounded series to its own min–max turns
/// a 0.06% drift across a million parked instances into a dramatic climb, which
/// is how a sparkline lies about a system that is not moving at all. A bounded
/// stage is drawn against its ceiling so height reads as occupancy; an
/// unbounded one against its own recent peak so height reads as "relative to
/// how busy this has lately been".
export function sparklinePath(
  history: (number | null)[],
  limit: number | null
): { line: string; area: string; lastX: number; lastY: number } | null {
  const points = history.filter((v): v is number => v !== null);
  if (points.length < 2) return null;

  const top = limit && limit > 0 ? limit : Math.max(...points) * 1.15 || 1;
  const coords = points.map((value, index) => {
    const x = (index / (points.length - 1)) * 100;
    const y = 100 - Math.min(100, Math.max(0, (value / top) * 100));
    return [x, y] as const;
  });

  const line = coords
    .map(([x, y], i) => `${i ? 'L' : 'M'}${x.toFixed(2)} ${y.toFixed(2)}`)
    .join('');
  const last = coords[coords.length - 1];

  return {
    line,
    area: `${line}L100 100L0 100Z`,
    lastX: last[0],
    lastY: last[1],
  };
}
