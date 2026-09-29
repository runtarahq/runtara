import { Link } from 'react-router';
import { toast } from 'sonner';
import { Copy } from 'lucide-react';
import type { ExecutionHistoryItem } from '../types';
import { RunStatusPill } from './RunLinks';
import { describeFailure } from '@/features/operations/queries';
import { StateValue } from '@/features/operations/components/StateValue';
import { Can } from '@/shared/components/Can';
import { Button } from '@/shared/components/ui/button';
import { ResumeButton } from '@/features/workflows/components/ResumeButton';
import { StopButton } from '@/features/workflows/components/StopButton';
import { canResume } from '@/features/workflows/utils/suspension';
import { isActiveStatus } from '@/shared/utils/status-display';

export function RunIdentity({ run }: { run: ExecutionHistoryItem }) {
  return (
    <div className="min-w-0 space-y-1 whitespace-normal">
      <Link
        className="break-words font-medium text-primary-text"
        to={`/operations/runs/${run.workflowId}/${run.instanceId}`}
      >
        {run.runLabel || run.instanceId.slice(0, 8)}
      </Link>
      <p className="break-words text-xs text-muted-foreground">
        {run.workflowName || run.workflowId}
      </p>
      <button
        className="flex items-center gap-1 text-xs text-muted-foreground"
        title={run.instanceId}
        aria-label={`Copy run ID ${run.instanceId}`}
        onClick={() =>
          void navigator.clipboard
            .writeText(run.instanceId)
            .then(() => toast.success('Run ID copied'))
            .catch(() => toast.error('Could not copy run ID'))
        }
      >
        <Copy className="size-3" />
        {run.instanceId.slice(0, 8)}…
      </button>
    </div>
  );
}
export function RunContext({
  run,
  detailsOpen,
  onDetailsChange,
}: {
  run: ExecutionHistoryItem;
  detailsOpen?: boolean;
  onDetailsChange?: (id: string, open: boolean) => void;
}) {
  const failed = run.status === 'failed' || run.status === 'timeout';
  const error = describeFailure(run);
  return (
    <div className="space-y-2 whitespace-normal">
      <RunStatusPill
        status={run.status}
        suspensionReason={run.suspensionReason}
      />
      {failed && (
        <>
          <p className="break-words text-sm">{error.message}</p>
          {(error.code || error.category) && (
            <details
              className="text-xs text-muted-foreground"
              open={detailsOpen}
              onToggle={(event) =>
                onDetailsChange?.(run.instanceId, event.currentTarget.open)
              }
            >
              <summary className="cursor-pointer">Error details</summary>
              <p className="break-all">
                {[error.code, error.category].filter(Boolean).join(' · ')}
              </p>
            </details>
          )}
        </>
      )}
      {run.status === 'suspended' && (
        <Link
          className="block text-xs text-primary-text"
          to={`/operations/runs/${run.workflowId}/${run.instanceId}`}
        >
          Review run and requests
        </Link>
      )}
      {run.hasPendingInput && (
        <Link
          className="block text-xs text-primary-text"
          to={`/workflows/${run.workflowId}/chat/${run.instanceId}`}
        >
          Continue chat
        </Link>
      )}
      <p className="text-xs text-muted-foreground" title={run.createdAt}>
        Started{' '}
        <StateValue value={run.createdAt} display={{ kind: 'relative' }} />
      </p>
      {!isActiveStatus(run.status) && run.completedAt && (
        <p className="text-xs text-muted-foreground" title={run.completedAt}>
          Completed{' '}
          <StateValue value={run.completedAt} display={{ kind: 'relative' }} />
        </p>
      )}
    </div>
  );
}
export function RunActions({
  run,
  onReplay,
}: {
  run: ExecutionHistoryItem;
  onReplay: (run: ExecutionHistoryItem) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Link
        className="text-xs text-primary-text"
        to={`/workflows/${run.workflowId}/history/${run.instanceId}`}
      >
        Open execution
      </Link>
      {run.status === 'suspended' && (
        <Link
          className="text-xs text-primary-text"
          to={`/workflows/${run.workflowId}?attachInstance=${run.instanceId}`}
        >
          Open in editor
        </Link>
      )}
      <Can permission="workflow:execute">
        {canResume(run) && (
          <ResumeButton instanceId={run.instanceId} size="sm" />
        )}
        {isActiveStatus(run.status) ? (
          <StopButton instanceId={run.instanceId} size="sm" />
        ) : (
          <Button size="sm" onClick={() => onReplay(run)}>
            Replay
          </Button>
        )}
      </Can>
    </div>
  );
}
