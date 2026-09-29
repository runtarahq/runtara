import { Link } from 'react-router';
import { toast } from 'sonner';
import {
  Bug,
  ChevronRight,
  CirclePlay,
  Copy,
  Eye,
  MessageSquare,
} from 'lucide-react';
import type { ExecutionHistoryItem } from '../types';
import { ParentRunLink, RunStatusPill } from './RunLinks';
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
        <button
          className="shrink-0 rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
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
        </button>
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

export function RunDetailsToggle({
  run,
  open,
  onChange,
}: {
  run: ExecutionHistoryItem;
  open: boolean;
  onChange: (id: string, open: boolean) => void;
}) {
  return (
    <button
      type="button"
      className="shrink-0 rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
      aria-label={`Details for ${run.runLabel || run.instanceId}`}
      aria-expanded={open}
      onClick={() => onChange(run.instanceId, !open)}
    >
      <ChevronRight
        className={`size-4 transition-transform ${open ? 'rotate-90' : ''}`}
      />
    </button>
  );
}

export function RunDetails({ run }: { run: ExecutionHistoryItem }) {
  const failed = run.status === 'failed' || run.status === 'timeout';
  const error = describeFailure(run);
  return (
    <div className="space-y-3 whitespace-normal text-sm">
      {failed && (
        <div className="space-y-1">
          <h3 className="text-xs font-medium text-muted-foreground">
            Error details
          </h3>
          <p className="whitespace-pre-wrap break-words">{error.message}</p>
          {(error.code || error.category) && (
            <p className="break-all text-xs text-muted-foreground">
              {[error.code, error.category].filter(Boolean).join(' · ')}
            </p>
          )}
        </div>
      )}
      <dl className="flex flex-wrap gap-x-8 gap-y-3 text-xs">
        <div>
          <dt className="text-muted-foreground">Run ID</dt>
          <dd className="mt-1 break-all font-mono">{run.instanceId}</dd>
        </div>
        <div>
          <dt className="text-muted-foreground">Started</dt>
          <dd className="mt-1">{formatDate(run.createdAt)}</dd>
        </div>
        <div>
          <dt className="text-muted-foreground">Completed</dt>
          <dd className="mt-1">
            {!isActiveStatus(run.status) && run.completedAt
              ? formatDate(run.completedAt)
              : '—'}
          </dd>
        </div>
        <div>
          <dt className="text-muted-foreground">Version</dt>
          <dd className="mt-1">{run.version ?? '—'}</dd>
        </div>
        {run.parentInstanceId && (
          <div>
            <dt className="text-muted-foreground">Parent</dt>
            <dd className="mt-1">
              <ParentRunLink parentInstanceId={run.parentInstanceId} compact />
            </dd>
          </div>
        )}
      </dl>
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
