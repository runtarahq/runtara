import { StepSummaryResponse } from '@/generated/RuntaraRuntimeApi';

/**
 * Extended step type for hierarchical timeline display
 */
export interface HierarchicalStep extends StepSummaryResponse {
  // Hierarchy state
  /** Whether this step has children (derived from stepType for Split/While/StartScenario) */
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
  'StartScenario',
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
 * For scope-creating steps (Split, While, StartScenario), use scopeId if available,
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
 * Transform API response to HierarchicalStep
 */
export function toHierarchicalStep(
  step: StepSummaryResponse,
  depth: number,
  minTimestamp: number
): HierarchicalStep {
  const absoluteStartMs = new Date(step.startedAt).getTime();
  const childrenScopeId = getChildrenScopeId(step);

  return {
    ...step,
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

  return Math.min(...steps.map((step) => new Date(step.startedAt).getTime()));
}

/**
 * Calculate the maximum end timestamp from a list of steps
 */
export function calculateMaxTimestamp(steps: StepSummaryResponse[]): number {
  if (steps.length === 0) return 0;

  return Math.max(
    ...steps.map((step) => {
      const startMs = new Date(step.startedAt).getTime();
      return startMs + (step.durationMs || 0);
    })
  );
}
