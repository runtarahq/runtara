import { useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { AlertTriangle, CirclePlay, Copy, Eye } from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '@/lib/utils';
import type {
  OperationRequest,
  WorkflowInstanceDto,
} from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { Can } from '@/shared/components/Can';
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from '@/shared/components/ui/dialog';
import { describeFailure } from '../queries';
import { ReplayButton } from '../pages/shared';
import { StateValue } from './StateValue';

export interface AttentionRequest {
  row: OperationRequest;
  workflowName: string;
  due?: string;
  isOverdue: boolean;
}

const rowClass =
  'grid min-h-16 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 px-4 py-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_minmax(0,1.5fr)_minmax(0,1fr)_6.5rem]';
const textClass = 'col-start-1 min-w-0 truncate lg:col-auto';
const actionsClass =
  'col-start-2 row-span-4 row-start-1 flex shrink-0 items-center justify-end gap-1 lg:col-auto lg:row-span-1 lg:row-auto';
const iconClass =
  'h-8 w-8 shrink-0 rounded-lg p-2 text-muted-foreground hover:bg-primary/10 hover:text-primary';

function AttentionGroup({
  title,
  count,
  link,
  to,
  columns,
  children,
}: {
  title: string;
  count?: number;
  link: string;
  to: string;
  columns: string[];
  children: ReactNode;
}) {
  return (
    <section aria-label={title}>
      <div className="flex flex-wrap items-center justify-between gap-2 bg-muted/20 px-4 py-3">
        <h3 className="text-sm font-medium">
          {title}{' '}
          <span className="ml-1 tabular-nums text-muted-foreground">
            {count == null ? '—' : count.toLocaleString()}
          </span>
        </h3>
        <Link
          className="text-xs font-medium text-primary-text hover:underline"
          to={to}
        >
          {link}
        </Link>
      </div>
      <div
        aria-hidden="true"
        className={cn(
          rowClass,
          'hidden min-h-0 border-y py-2 text-xs font-medium text-muted-foreground lg:grid'
        )}
      >
        {columns.map((column) => (
          <span
            key={column}
            className={column === 'Actions' ? 'text-right' : ''}
          >
            {column}
          </span>
        ))}
      </div>
      {children}
    </section>
  );
}

function EmptyGroup({
  loading,
  failed,
  empty,
}: {
  loading: boolean;
  failed: boolean;
  empty: string;
}) {
  return (
    <p className="px-4 py-5 text-sm text-muted-foreground">
      {failed
        ? 'Could not load this group. Try Refresh.'
        : loading
          ? 'Loading…'
          : empty}
    </p>
  );
}

export function OverviewAttention({
  requests,
  requestCount,
  requestsPending,
  requestsError,
  failures,
  failureCount,
  failuresPending,
  failuresError,
}: {
  requests: AttentionRequest[];
  requestCount?: number;
  requestsPending: boolean;
  requestsError: boolean;
  failures: WorkflowInstanceDto[];
  failureCount?: number;
  failuresPending: boolean;
  failuresError: boolean;
}) {
  const [replayRun, setReplayRun] = useState<WorkflowInstanceDto | null>(null);
  return (
    <>
      <div className="divide-y">
        <AttentionGroup
          title="Requests requiring input"
          count={requestCount}
          link="View all requests"
          to="/operations/queues"
          columns={['Run', 'Request', 'Workflow', 'Due', 'Actions']}
        >
          {requests.length ? (
            <ul
              className="divide-y divide-border/50"
              aria-label="Requests requiring input"
            >
              {requests.map(({ row, workflowName, due, isOverdue }) => {
                const to = `/operations/runs/${row.workflowId}/${row.instanceId}`;
                const label = row.runLabel || row.instanceId.slice(0, 8);
                return (
                  <li
                    key={`${row.instanceId}/${row.requestId}`}
                    className={rowClass}
                  >
                    <Link
                      className={`${textClass} text-sm font-medium text-primary-text hover:underline`}
                      title={label}
                      to={to}
                    >
                      {label}
                    </Link>
                    <div className={textClass}>
                      <p className="truncate text-sm" title={row.label}>
                        {row.label}
                      </p>
                      {row.message && (
                        <p
                          className="truncate text-xs text-muted-foreground"
                          title={row.message}
                        >
                          {row.message}
                        </p>
                      )}
                    </div>
                    <span
                      className={`${textClass} text-xs text-muted-foreground`}
                      title={workflowName}
                    >
                      {workflowName}
                    </span>
                    <div
                      className={`${textClass} text-xs ${isOverdue ? 'text-warning' : 'text-muted-foreground'}`}
                    >
                      {isOverdue && (
                        <span className="mb-0.5 flex items-center gap-1">
                          <AlertTriangle
                            aria-hidden="true"
                            className="size-3"
                          />
                          Overdue
                        </span>
                      )}
                      {due ? (
                        <>
                          <span className="lg:hidden">Due </span>
                          <StateValue
                            value={due}
                            display={{ kind: 'relative' }}
                          />
                        </>
                      ) : (
                        <span aria-label="No due date">—</span>
                      )}
                    </div>
                    <div className={actionsClass}>
                      <WithTooltip label="Review request">
                        <Button
                          asChild
                          variant="secondary"
                          size="icon"
                          className={iconClass}
                        >
                          <Link
                            aria-label={`Review request for ${label}`}
                            to={to}
                          >
                            <Eye className="size-4" />
                          </Link>
                        </Button>
                      </WithTooltip>
                    </div>
                  </li>
                );
              })}
            </ul>
          ) : (
            <EmptyGroup
              loading={requestsPending}
              failed={requestsError}
              empty="No requests need input."
            />
          )}
        </AttentionGroup>
        <AttentionGroup
          title="Recent failures"
          count={failureCount}
          link="View failed runs"
          to="/operations/runs?status=failed,timeout&range=24h&dateBasis=completed&sortBy=completedAt&sortOrder=desc"
          columns={['Run', 'Workflow', 'Error summary', 'Failed', 'Actions']}
        >
          {failures.length ? (
            <ul
              className="divide-y divide-border/50"
              aria-label="Recent failures"
            >
              {failures.map((run) => {
                const error = describeFailure(run);
                const label = run.runLabel || run.id.slice(0, 8);
                return (
                  <li key={run.id} data-run-id={run.id} className={rowClass}>
                    <Link
                      className={`${textClass} text-sm font-medium text-primary-text hover:underline`}
                      title={label}
                      to={`/operations/runs/${run.workflowId}/${run.id}`}
                    >
                      {label}
                    </Link>
                    <span
                      className={`${textClass} text-xs text-muted-foreground`}
                      title={run.workflowName ?? run.workflowId}
                    >
                      {run.workflowName ?? run.workflowId}
                    </span>
                    <p className={`${textClass} text-sm`} title={error.message}>
                      {error.message}
                    </p>
                    <div
                      className={`${textClass} text-xs text-muted-foreground`}
                    >
                      <span className="lg:hidden">Failed </span>
                      <StateValue
                        value={run.completedAt ?? run.created}
                        display={{ kind: 'relative' }}
                      />
                    </div>
                    <div className={actionsClass}>
                      <WithTooltip label="Open execution">
                        <Button
                          asChild
                          variant="secondary"
                          size="icon"
                          className={iconClass}
                        >
                          <Link
                            aria-label="Open execution"
                            to={`/workflows/${run.workflowId}/history/${run.id}`}
                          >
                            <Eye className="size-4" />
                          </Link>
                        </Button>
                      </WithTooltip>
                      <WithTooltip label="Copy run ID">
                        <Button
                          variant="secondary"
                          size="icon"
                          className={iconClass}
                          aria-label="Copy run ID"
                          onClick={() =>
                            void navigator.clipboard
                              .writeText(run.id)
                              .then(() => toast.success('Run ID copied'))
                              .catch(() => toast.error('Could not copy run ID'))
                          }
                        >
                          <Copy className="size-4" />
                        </Button>
                      </WithTooltip>
                      {error.category === 'transient' && (
                        <Can permission="workflow:execute">
                          <WithTooltip label="Replay">
                            <Button
                              variant="secondary"
                              size="icon"
                              className={iconClass}
                              aria-label="Replay"
                              onClick={() => setReplayRun(run)}
                            >
                              <CirclePlay className="size-4" />
                            </Button>
                          </WithTooltip>
                        </Can>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          ) : (
            <EmptyGroup
              loading={failuresPending}
              failed={failuresError}
              empty="No failed runs in the last 24 hours."
            />
          )}
        </AttentionGroup>
      </div>
      <Dialog
        open={!!replayRun}
        onOpenChange={(open) => {
          if (!open) setReplayRun(null);
        }}
      >
        <DialogContent>
          <DialogTitle>
            Replay {replayRun?.runLabel || replayRun?.id.slice(0, 8)}
          </DialogTitle>
          <DialogDescription>
            Start a new run with the original input.
          </DialogDescription>
          {replayRun && (
            <ReplayButton
              run={replayRun}
              initiallyConfirm
              onClose={() => setReplayRun(null)}
            />
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}
