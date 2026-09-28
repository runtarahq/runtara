import type { CapabilityInfo } from '@/generated/RuntaraRuntimeApi';
import { cn } from '@/lib/utils';
import { capabilitySuspends } from '@/features/workflows/utils/capability-flags';

export const SUSPENDS_BADGE_HINT =
  'Long-running: this capability can park the run until it wakes up. The step must stay durable and set a timeout (ms) above 0 — its hard deadline, parked time included.';

/**
 * "Long-running" chip for a capability that suspends the run (a long-polling
 * agent). Renders nothing for any other capability.
 */
export function SuspendsBadge({
  capability,
  className,
}: {
  capability: Pick<CapabilityInfo, 'suspends' | 'tags'> | null | undefined;
  className?: string;
}) {
  if (!capabilitySuspends(capability)) return null;
  return (
    <span
      data-testid="capability-suspends-badge"
      title={SUSPENDS_BADGE_HINT}
      className={cn(
        'shrink-0 rounded bg-warning/10 px-1.5 py-0.5 text-3xs font-medium text-warning',
        className
      )}
    >
      Long-running
    </span>
  );
}
