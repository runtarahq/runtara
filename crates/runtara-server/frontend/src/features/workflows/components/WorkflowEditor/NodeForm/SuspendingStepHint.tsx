import { useContext } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { Hourglass } from 'lucide-react';
import { cn } from '@/lib/utils';
import { findAgentById } from '@/shared/utils/agent-id';
import {
  capabilitySuspends,
  suspendingStepIssues,
} from '@/features/workflows/utils/capability-flags';
import { NodeFormContext } from './NodeFormContext';

/**
 * Inline hint on an Agent step whose capability suspends the run (a
 * long-polling agent): the step must stay durable (E028) and set a timeout
 * above 0 ms (E029). Turns into a warning while either requirement is unmet.
 */
export function SuspendingStepHint() {
  const form = useFormContext();
  const { agents } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const agentId = useWatch({ name: 'agentId', control: form.control });
  const capabilityId = useWatch({
    name: 'capabilityId',
    control: form.control,
  });
  const durable = useWatch({ name: 'durable', control: form.control });
  const timeout = useWatch({ name: 'timeout', control: form.control });

  if (stepType !== 'Agent' || !agentId || !capabilityId) return null;
  const capability = findAgentById(agents, agentId)?.supportedCapabilities?.[
    capabilityId
  ];
  if (!capabilitySuspends(capability)) return null;

  const { notDurable, missingTimeout } = suspendingStepIssues({
    durable,
    timeout,
  });
  const hasIssue = notDurable || missingTimeout;

  return (
    <div
      role={hasIssue ? 'alert' : 'note'}
      data-testid="suspending-step-hint"
      className={cn(
        'mb-2 flex gap-2 rounded-md border px-3 py-2 text-xs',
        hasIssue
          ? 'border-warning/40 bg-warning/10 text-foreground'
          : 'border-border bg-muted/40 text-muted-foreground'
      )}
    >
      <Hourglass className="mt-0.5 size-3.5 shrink-0 text-warning" />
      <div className="space-y-1">
        <p>
          This capability can park the run until it wakes up. The step must be{' '}
          <strong>durable</strong> and needs a <strong>timeout (ms)</strong>{' '}
          above 0 under Execution — its hard deadline, parked time included.
        </p>
        {notDurable && (
          <p className="font-medium text-warning">
            Durable is off: turn it on (E028).
          </p>
        )}
        {missingTimeout && (
          <p className="font-medium text-warning">
            Timeout is missing or 0: set it above 0 ms (E029).
          </p>
        )}
      </div>
    </div>
  );
}
