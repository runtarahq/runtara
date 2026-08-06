import { isFinishedStatus } from '@/shared/utils/status-display';

/**
 * Copy for the empty states on the run-history page.
 *
 * A run that is still in flight and a finished run with nothing to show look
 * identical to the page — both just have no rows to render. Telling a user that
 * "results may appear soon" on a run that ended minutes ago makes the UI sound
 * like execution is still in progress, so the wording is chosen from the run's
 * status: keep the hopeful copy while the run can still produce something, and
 * state the outcome once it cannot.
 */
export interface RunEmptyState {
  title: string;
  description: string;
}

/** How a finished run ended, phrased for use inside a sentence. */
function outcomeClause(status: string | undefined | null): string {
  const normalizedStatus = status?.toLowerCase() || '';
  if (normalizedStatus === 'failed' || normalizedStatus === 'error') {
    return 'This run failed';
  }
  if (normalizedStatus === 'cancelled' || normalizedStatus === 'aborted') {
    return 'This run was cancelled';
  }
  if (normalizedStatus === 'timeout') {
    return 'This run timed out';
  }
  return 'This run completed';
}

/**
 * Empty-state copy for an events view (Timeline or List). `subject` is the noun
 * the view uses for its rows, so the title matches the tab the user is on.
 */
export function getRunEventsEmptyState(
  status: string | undefined | null,
  subject: 'Timeline Events' | 'Events'
): RunEmptyState {
  if (!isFinishedStatus(status)) {
    // The List view has always carried an extra nudge to come back; only the
    // finished-run wording is in question here, so leave it in place.
    const refreshHint =
      subject === 'Events'
        ? ' Check back or refresh the page to see the latest events.'
        : '';
    return {
      title: `No ${subject} Yet`,
      description: `${subject} will appear here as your workflow executes. If your workflow is still running, ${subject.toLowerCase()} may appear soon.${refreshHint}`,
    };
  }

  return {
    title: `No ${subject}`,
    description: `${outcomeClause(status)} without recording any ${subject.toLowerCase()}.`,
  };
}

/** Empty-state copy for the Output Data card. */
export function getRunOutputEmptyState(
  status: string | undefined | null
): RunEmptyState {
  if (!isFinishedStatus(status)) {
    return {
      title: 'No output data yet',
      description: 'Output will be available once the workflow completes',
    };
  }

  return {
    title: 'No output data',
    description: `${outcomeClause(status)} without returning any output`,
  };
}
