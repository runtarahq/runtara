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

/** Absolute end wall-clock (epoch ms): real settle when present, else start + duration. */
export function stepEndMs(step: StepSummaryResponse): number {
  if (hasRealInterval(step)) return Math.max(step.settledAtMs!, step.launchedAtMs!);
  return new Date(step.startedAt).getTime() + (step.durationMs || 0);
}

/**
 * Transform API response to HierarchicalStep
 */
export function toHierarchicalStep(
  step: StepSummaryResponse,
  depth: number,
  minTimestamp: number
): HierarchicalStep {
  const absoluteStartMs = stepStartMs(step);
  const childrenScopeId = getChildrenScopeId(step);
  // When a real launch/settle interval is present, the bar's span is settle −
  // launch (the true overlapping window); otherwise keep the recorded duration
  // (which may be null while the step is still running).
  const durationMs = hasRealInterval(step)
    ? stepEndMs(step) - absoluteStartMs
    : step.durationMs;

  return {
    ...step,
    durationMs,
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
export function calculateMaxTimestamp(steps: StepSummaryResponse[]): number {
  if (steps.length === 0) return 0;

  return Math.max(...steps.map(stepEndMs));
}
