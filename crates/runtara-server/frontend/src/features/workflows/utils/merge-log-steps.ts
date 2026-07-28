/** A step summary row as the views consume it. */
export interface StepSummaryLike {
  stepId?: string | null;
  stepName?: string | null;
  stepType?: string | null;
  status?: string | null;
  startedAt?: string | null;
  completedAt?: string | null;
  durationMs?: number | null;
  scopeId?: string | null;
  parentScopeId?: string | null;
  outputs?: unknown;
  [key: string]: unknown;
}

/** A raw `workflow_log` event from the step-events endpoint. */
export interface WorkflowLogEvent {
  createdAt?: string | null;
  payload?: {
    step_id?: string | null;
    step_name?: string | null;
    level?: string | null;
    message?: string | null;
    context?: unknown;
    scope_id?: string | null;
    parent_scope_id?: string | null;
  } | null;
}

/**
 * Turn one `workflow_log` event into a step-summary row.
 *
 * A Log step emits no step-debug start/end pair — deliberately, so logging-heavy
 * workflows do not triple their event volume — which is why it never appears in
 * the summary-derived views. Its log event carries everything needed to stand in
 * for one: which step, when, and in which scope.
 *
 * Duration is 0 rather than unknown: the step did run, and it completed within
 * the same instant it started.
 */
function logEventToSummary(event: WorkflowLogEvent): StepSummaryLike | null {
  const payload = event.payload;
  const stepId = payload?.step_id;
  if (!stepId || !event.createdAt) return null;

  return {
    stepId,
    stepName: payload?.step_name ?? 'Log',
    stepType: 'Log',
    status: 'completed',
    startedAt: event.createdAt,
    completedAt: event.createdAt,
    durationMs: 0,
    // Absent on events emitted before Log carried scope; such a log is treated
    // as root-level, which is where it would have been shown anyway.
    scopeId: payload?.scope_id ?? null,
    parentScopeId: payload?.parent_scope_id ?? null,
    outputs: {
      level: payload?.level ?? 'info',
      message: payload?.message ?? '',
      ...(payload?.context && typeof payload.context === 'object'
        ? { context: payload.context }
        : {}),
    },
  };
}

function timeOf(value?: string | null): number {
  const parsed = value ? Date.parse(value) : Number.NaN;
  return Number.isNaN(parsed) ? 0 : parsed;
}

/**
 * Merge Log steps into a set of step summaries, in time order.
 *
 * Summaries are paginated server-side while log events are not, so the two can
 * only be interleaved safely when the caller holds the whole run: pages are cut
 * on summary offsets, and a log landing between one page's last summary and the
 * next page's first belongs to neither, so any per-page rule either drops it or
 * repeats it. Rather than pick a corruption, a partial page is returned
 * untouched.
 *
 * That covers the views that matter: the Timeline and Graph read every summary,
 * and a run short enough to fit one page — which is most of them, and every run
 * the issue describes — is by definition whole. A run long enough to paginate
 * still shows its logs on the Activity Log page. Interleaving those correctly
 * needs the merge to happen server-side, where the pagination is decided.
 */
export function mergeLogSteps(
  summaries: StepSummaryLike[],
  logEvents: WorkflowLogEvent[],
  options: {
    /** Whether these summaries are the complete set, not one page of several. */
    isCompleteSet: boolean;
    sortOrder?: 'asc' | 'desc';
  }
): StepSummaryLike[] {
  if (!options.isCompleteSet) return summaries;

  const logs = logEvents
    .map(logEventToSummary)
    .filter((entry): entry is StepSummaryLike => entry !== null);

  if (logs.length === 0) return summaries;

  const sortOrder = options.sortOrder ?? 'asc';
  const merged = [...summaries, ...logs];
  merged.sort((a, b) =>
    sortOrder === 'desc'
      ? timeOf(b.startedAt) - timeOf(a.startedAt)
      : timeOf(a.startedAt) - timeOf(b.startedAt)
  );
  return merged;
}
