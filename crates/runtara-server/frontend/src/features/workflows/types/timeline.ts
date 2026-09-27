import { StepSummaryResponse } from '@/generated/RuntaraRuntimeApi';

/**
 * Extended step type for hierarchical timeline display
 */
export interface HierarchicalStep extends StepSummaryResponse {
  // Hierarchy state
  /** Whether this step has children (derived from stepType for Split/While/EmbedWorkflow) */
  hasChildren: boolean;
  /** The scope ID to use when fetching children (may differ from scopeId) */
  childrenScopeId: string | null;
  /** UI state: whether this step's children are visible */
  isExpanded: boolean;
  /** Loading state for children fetch */
  isLoadingChildren: boolean;
  /** Loaded child steps */
  children?: HierarchicalStep[];
  /** Total children count from API */
  childrenTotalCount?: number;
  /** How many children have been loaded so far */
  childrenLoadedCount?: number;
  /** Nesting level (0 for root) */
  depth: number;

  // Timeline positioning (calculated)
  /** Start time relative to timeline start (ms) */
  startMs: number;
  /** Absolute start timestamp (ms) */
  absoluteStartMs: number;
  /**
   * Width of the step's bar (ms). The recorded duration for a finished step;
   * for an unfinished running or suspended (parked) step, the time from its
   * start to now.
   */
  spanMs: number;
  /** Unfinished running/suspended step whose bar runs up to now. */
  isOpenEnded: boolean;
}

/**
 * Step types that create child scopes
 */
const SCOPE_CREATING_STEP_TYPES = [
  'Split',
  'While',
  'EmbedWorkflow',
  'AiAgent',
];

/**
 * Check if a step type creates child scopes
 */
function isStepTypeWithChildren(stepType: string): boolean {
  return SCOPE_CREATING_STEP_TYPES.includes(stepType);
}

/**
 * Get the scope ID to use for fetching children.
 * For scope-creating steps (Split, While, EmbedWorkflow), use scopeId if available,
 * otherwise fall back to stepId as the scope identifier.
 */
function getChildrenScopeId(step: StepSummaryResponse): string | null {
  if (!isStepTypeWithChildren(step.stepType)) {
    return null;
  }
  // Use scopeId if available, otherwise use stepId for scope-creating steps
  return step.scopeId || step.stepId;
}

/**
 * Whether a step carries a real parallel-branch launch/settle interval (both
 * epoch-ms bounds present and positive). These OVERLAP across sibling branches,
 * so preferring them makes the timeline show true concurrency instead of the
 * sequential assemble cascade recorded in `startedAt`/`durationMs`.
 */
function hasRealInterval(step: StepSummaryResponse): boolean {
  return (
    step.launchedAtMs != null &&
    step.settledAtMs != null &&
    step.launchedAtMs > 0 &&
    step.settledAtMs > 0
  );
}

/** Absolute start wall-clock (epoch ms): real launch when present, else `startedAt`. */
export function stepStartMs(step: StepSummaryResponse): number {
  if (hasRealInterval(step)) return step.launchedAtMs!;
  return new Date(step.startedAt).getTime();
}

const OPEN_ENDED_STATUSES = new Set(['running', 'suspended']);

/**
 * A step that has not finished yet: running, or suspended while its run is
 * parked (a control `wait`, a durable Delay, a WaitForSignal). It has no
 * duration, so its bar runs from its start to now.
 */
export function isOpenEndedStep(step: StepSummaryResponse): boolean {
  return (
    !hasRealInterval(step) &&
    step.durationMs == null &&
    OPEN_ENDED_STATUSES.has((step.status || '').toLowerCase())
  );
}

/**
 * Absolute end wall-clock (epoch ms): real settle when present, now for an
 * unfinished step, else start + duration.
 */
export function stepEndMs(
  step: StepSummaryResponse,
  nowMs: number = Date.now()
): number {
  if (hasRealInterval(step))
    return Math.max(step.settledAtMs!, step.launchedAtMs!);
  const startMs = new Date(step.startedAt).getTime();
  if (isOpenEndedStep(step)) return Math.max(startMs, nowMs);
  return startMs + (step.durationMs || 0);
}

/**
 * Transform API response to HierarchicalStep
 */
export function toHierarchicalStep(
  step: StepSummaryResponse,
  depth: number,
  minTimestamp: number,
  nowMs: number = Date.now()
): HierarchicalStep {
  const absoluteStartMs = stepStartMs(step);
  const childrenScopeId = getChildrenScopeId(step);
  // When a real launch/settle interval is present, the bar's span is settle −
  // launch (the true overlapping window); otherwise keep the recorded duration
  // (which may be null while the step is still running).
  const durationMs = hasRealInterval(step)
    ? stepEndMs(step) - absoluteStartMs
    : step.durationMs;
  const isOpenEnded = isOpenEndedStep(step);

  return {
    ...step,
    durationMs,
    spanMs: isOpenEnded
      ? stepEndMs(step, nowMs) - absoluteStartMs
      : durationMs || 0,
    isOpenEnded,
    hasChildren: isStepTypeWithChildren(step.stepType),
    childrenScopeId,
    isExpanded: false,
    isLoadingChildren: false,
    depth,
    startMs: absoluteStartMs - minTimestamp,
    absoluteStartMs,
  };
}

/**
 * Calculate the minimum timestamp from a list of steps
 */
export function calculateMinTimestamp(steps: StepSummaryResponse[]): number {
  if (steps.length === 0) return 0;

  return Math.min(...steps.map(stepStartMs));
}

/**
 * Calculate the maximum end timestamp from a list of steps
 */
export function calculateMaxTimestamp(
  steps: StepSummaryResponse[],
  nowMs: number = Date.now()
): number {
  if (steps.length === 0) return 0;

  return Math.max(...steps.map((step) => stepEndMs(step, nowMs)));
}

/** Minimum bar width (px), matching the bar's `min-w-[60px]`. */
export const MIN_BAR_PX = 60;

/**
 * Horizontal placement of a step's bar on a track `totalDuration` ms wide, as
 * CSS `left`/`width`. The bar never starts past the track's end: a step that
 * starts at the very end (an unfinished step whose start is the latest
 * timestamp) is pulled back so its minimum-width bar stays visible.
 */
export function timelineBarPosition(
  step: Pick<HierarchicalStep, 'startMs' | 'spanMs'>,
  totalDuration: number
): { left: string; width: string; leftPct: number; widthPct: number } {
  if (totalDuration <= 0) {
    return { left: '0%', width: '100%', leftPct: 0, widthPct: 100 };
  }
  const leftPct = Math.min(
    Math.max((step.startMs / totalDuration) * 100, 0),
    100
  );
  const widthPct = Math.min(
    Math.max(((step.spanMs || 1) / totalDuration) * 100, 1),
    Math.max(100 - leftPct, 1)
  );
  return {
    left: `min(${leftPct}%, calc(100% - ${MIN_BAR_PX}px))`,
    width: `${widthPct}%`,
    leftPct,
    widthPct,
  };
}

/** Gap (px) between a bar and a label drawn outside it. */
export const OUTSIDE_LABEL_GAP_PX = 6;

/**
 * Rough rendered width (px) of a bar's duration label plus status badge
 * (text-xs, ~6.5px per character), including the bar's padding and gap.
 */
export function estimateBarLabelPx(
  durationText: string,
  badgeText: string,
  badgeIcon = false
): number {
  const CHAR_PX = 6.5;
  const BAR_PADDING_PX = 16 + 3; // px-2 both sides + left border
  const GAP_PX = 8;
  const BADGE_CHROME_PX = 14 + (badgeIcon ? 16 : 0); // px-1.5 + border (+ icon)
  return Math.ceil(
    BAR_PADDING_PX +
      durationText.length * CHAR_PX +
      GAP_PX +
      badgeText.length * CHAR_PX +
      BADGE_CHROME_PX
  );
}

export type BarLabelPlacement =
  | { side: 'inside' }
  | { side: 'right'; leftPx: number }
  | { side: 'left'; rightPx: number };

/**
 * Where a bar's duration label and status badge go on a track `trackPx`
 * wide: inside the bar when they fit, else just outside it: to its right,
 * or to its left when there is no room before the track end. An unmeasured
 * track (`trackPx` 0) keeps the label inside.
 */
export function barLabelPlacement(
  bar: { leftPct: number; widthPct: number },
  trackPx: number,
  labelPx: number
): BarLabelPlacement {
  if (trackPx <= 0) return { side: 'inside' };
  // Same geometry as the bar's CSS: left is pulled back so the minimum-width
  // bar fits, and the bar is never narrower than MIN_BAR_PX.
  const leftPx = Math.max(
    Math.min((bar.leftPct / 100) * trackPx, trackPx - MIN_BAR_PX),
    0
  );
  const barPx = Math.max((bar.widthPct / 100) * trackPx, MIN_BAR_PX);
  if (barPx >= labelPx) return { side: 'inside' };
  const rightEdgePx = leftPx + barPx;
  if (rightEdgePx + OUTSIDE_LABEL_GAP_PX + labelPx <= trackPx) {
    return { side: 'right', leftPx: rightEdgePx + OUTSIDE_LABEL_GAP_PX };
  }
  return {
    side: 'left',
    rightPx: Math.max(trackPx - leftPx + OUTSIDE_LABEL_GAP_PX, 0),
  };
}
