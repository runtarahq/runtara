import { Link } from 'react-router';
import { toast } from 'sonner';
import { Bug, CirclePlay, Copy, Eye, MessageSquare } from 'lucide-react';
import type { ExecutionHistoryItem } from '../types';
import { RunStatusPill } from './RunLinks';
import { describeFailure } from '@/features/operations/queries';
import { StateValue } from '@/features/operations/components/StateValue';
import { Can } from '@/shared/components/Can';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { ResumeButton } from '@/features/workflows/components/ResumeButton';
import { StopButton } from '@/features/workflows/components/StopButton';
import { canResume } from '@/features/workflows/utils/suspension';
import { formatDate } from '@/lib/utils';
import { isActiveStatus } from '@/shared/utils/status-display';

export function RunIdentity({ run }: { run: ExecutionHistoryItem }) {
  return (
    <div className="min-w-0 space-y-1">
      <div className="flex min-w-0 items-center gap-1.5">
        <Link
          className="truncate font-medium text-primary-text"
          title={run.runLabel || run.instanceId}
          to={`/operations/runs/${run.workflowId}/${run.instanceId}`}
        >
          {run.runLabel || run.instanceId.slice(0, 8)}
        </Link>
      </div>
      <p
        className="truncate text-xs text-muted-foreground"
        title={run.workflowName || run.workflowId}
      >
        {run.workflowName || run.workflowId}
      </p>
    </div>
  );
}

export function RunContext({ run }: { run: ExecutionHistoryItem }) {
  const failed = run.status === 'failed' || run.status === 'timeout';
  return (
    <div className="min-w-0 space-y-1">
      <RunStatusPill
        status={run.status}
        suspensionReason={run.suspensionReason}
      />
      {failed && (
        <p className="truncate text-xs text-muted-foreground">
          {describeFailure(run).message}
        </p>
      )}
    </div>
  );
}

export function RunTime({ value }: { value: string }) {
  return (
    <span className="whitespace-nowrap text-sm" title={formatDate(value)}>
      <StateValue value={value} display={{ kind: 'relative' }} />
    </span>
  );
}

export function RunActions({
  run,
  onReplay,
}: {
  run: ExecutionHistoryItem;
  onReplay: (run: ExecutionHistoryItem) => void;
}) {
  const iconClass =
    'h-8 w-8 rounded-lg p-2 text-muted-foreground transition-colors';
  return (
    <div className="flex w-max shrink-0 flex-nowrap items-center gap-1">
      {run.status === 'suspended' && (
        <WithTooltip
          label={
            canResume(run)
              ? 'Open in editor — resume debugging'
              : 'Open in editor'
          }
        >
          <Button
            asChild
            variant="secondary"
            size="icon"
            className={`${iconClass} hover:bg-warning/10 hover:text-warning`}
          >
            <Link
              aria-label="Open in editor"
              to={`/workflows/${run.workflowId}?attachInstance=${run.instanceId}`}
            >
              <Bug className="size-4" />
            </Link>
          </Button>
        </WithTooltip>
      )}
      {run.hasPendingInput && (
        <WithTooltip label="Continue chat">
          <Button
            asChild
            variant="secondary"
            size="icon"
            className={`${iconClass} hover:bg-warning/10 hover:text-warning`}
          >
            <Link
              aria-label="Continue chat"
              to={`/workflows/${run.workflowId}/chat/${run.instanceId}`}
            >
              <MessageSquare className="size-4" />
            </Link>
          </Button>
        </WithTooltip>
      )}
      <WithTooltip label="Open execution">
        <Button
          asChild
          variant="secondary"
          size="icon"
          className={`${iconClass} hover:bg-primary/10 hover:text-primary`}
        >
          <Link
            aria-label="Open execution"
            to={`/workflows/${run.workflowId}/history/${run.instanceId}`}
          >
            <Eye className="size-4" />
          </Link>
        </Button>
      </WithTooltip>
      <WithTooltip label="Copy run ID">
        <Button
          variant="secondary"
          size="icon"
          className={`${iconClass} hover:bg-primary/10 hover:text-primary`}
          aria-label="Copy run ID"
          onClick={() =>
            void navigator.clipboard
              .writeText(run.instanceId)
              .then(() => toast.success('Run ID copied'))
              .catch(() => toast.error('Could not copy run ID'))
          }
        >
          <Copy className="size-4" />
        </Button>
      </WithTooltip>
      <Can permission="workflow:execute">
        {canResume(run) && (
          <ResumeButton
            instanceId={run.instanceId}
            variant="secondary"
            size="icon"
            className={`${iconClass} hover:bg-primary/10 hover:text-primary`}
          />
        )}
        {isActiveStatus(run.status) ? (
          <StopButton
            instanceId={run.instanceId}
            variant="secondary"
            size="icon"
            className={`${iconClass} hover:bg-destructive/10 hover:text-destructive`}
          />
        ) : (
          <WithTooltip label="Replay">
            <Button
              variant="secondary"
              size="icon"
              className={`${iconClass} hover:bg-success/10 hover:text-success`}
              aria-label="Replay"
              onClick={() => onReplay(run)}
            >
              <CirclePlay className="size-4" />
            </Button>
          </WithTooltip>
        )}
      </Can>
    </div>
  );
}
