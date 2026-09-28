/**
 * Display mapping for a step summary status (`StepSummaryResponse.status`):
 * "running", "suspended" (unfinished while its run is suspended — a parked
 * WaitForInstances, a durable Delay, a WaitForSignal), "completed", "failed",
 * or the run's terminal status for a step that never finished.
 */
export interface StepStatusDisplay {
  label: string;
  badgeVariant: 'default' | 'secondary' | 'destructive' | 'outline' | 'warning';
  /** Soft row/card tint (border + background). */
  rowClass: string;
  /** Text color class for inline status text. */
  textClass: string;
  spin: boolean;
  /** Hover text explaining the status, when it needs one. */
  title?: string;
}

/** Hover text for a parked step. */
export const SUSPENDED_STEP_TITLE =
  'Parked until its run resumes (a wait for instances, a durable Delay or a signal wait)';

export function stepStatusDisplay(
  status: string | null | undefined
): StepStatusDisplay {
  switch ((status || '').toLowerCase()) {
    case 'completed':
      return {
        label: 'Completed',
        badgeVariant: 'default',
        rowClass: 'border-success/50 bg-success/5',
        textClass: 'text-success',
        spin: false,
      };
    case 'failed':
      return {
        label: 'Failed',
        badgeVariant: 'destructive',
        rowClass: 'border-destructive/50 bg-destructive/5',
        textClass: 'text-destructive',
        spin: false,
      };
    case 'running':
      return {
        label: 'Running',
        badgeVariant: 'secondary',
        rowClass: 'border-info/50 bg-info/5',
        textClass: 'text-info',
        spin: true,
      };
    case 'suspended':
      return {
        label: 'Suspended',
        badgeVariant: 'warning',
        rowClass: 'border-warning/50 bg-warning/5',
        textClass: 'text-warning',
        spin: false,
        title: SUSPENDED_STEP_TITLE,
      };
    default:
      return {
        label: status || 'Unknown',
        badgeVariant: 'outline',
        rowClass: 'border-border',
        textClass: 'text-foreground',
        spin: false,
      };
  }
}
