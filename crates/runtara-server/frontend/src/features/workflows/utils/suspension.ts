import type { SuspensionReason } from '@/generated/RuntaraRuntimeApi';

interface SuspendableRun {
  status?: string | null;
  suspensionReason?: SuspensionReason | null;
}

const REASON_LABELS: Record<SuspensionReason, string> = {
  paused: 'Paused',
  waiting_signal: 'Waiting for signal',
  waiting_instances: 'Waiting for runs',
  sleeping: 'Sleeping',
  shutdown: 'Shutdown',
};

/** Human label for why a suspended run is not running; null when unknown. */
export function suspensionReasonLabel(
  reason: SuspensionReason | string | null | undefined
): string | null {
  if (!reason) return null;
  return REASON_LABELS[reason as SuspensionReason] ?? reason;
}

function isSuspended(run: SuspendableRun | null | undefined): boolean {
  return run?.status?.toLowerCase() === 'suspended';
}

/**
 * Only a paused run needs a resume. The other suspended runs wake on their own
 * (a signal, the runs they wait on, a timer, recovery), and the server answers
 * `NotResumable` for failed/cancelled/completed runs.
 */
export function canResume(run: SuspendableRun | null | undefined): boolean {
  return isSuspended(run) && run?.suspensionReason === 'paused';
}

/**
 * A suspended run that wakes on its own (signal, child runs, timer, recovery)
 * rather than waiting for someone to press Resume.
 */
export function isWaitingSuspension(
  run: SuspendableRun | null | undefined
): boolean {
  return (
    isSuspended(run) &&
    !!run?.suspensionReason &&
    run.suspensionReason !== 'paused'
  );
}

/**
 * Whether a debug run is stopped at a breakpoint (or an explicit pause), i.e.
 * a Continue makes sense. A run parked on a signal, child runs or a timer is
 * waiting, not at a breakpoint.
 */
export function isAtBreakpoint(
  run: SuspendableRun | null | undefined
): boolean {
  return canResume(run);
}

/**
 * Toolbar text for a run that is suspended but waiting, e.g. "Waiting for
 * signal" or "Waiting (sleeping)".
 */
export function waitingStatusText(
  reason: SuspensionReason | string | null | undefined
): string {
  const label = suspensionReasonLabel(reason);
  if (!label) return 'Waiting';
  return label.startsWith('Waiting')
    ? label
    : `Waiting (${label.toLowerCase()})`;
}

/**
 * Status label for a run: a suspended run reads as its reason ("Paused",
 * "Waiting for signal", ...) so a parked run is not mistaken for one that needs
 * a resume. Null when the default status label applies.
 */
export function suspendedStatusLabel(
  run: SuspendableRun | null | undefined
): string | null {
  return isSuspended(run) ? suspensionReasonLabel(run?.suspensionReason) : null;
}
