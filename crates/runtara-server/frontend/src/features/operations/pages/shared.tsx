import { useState, type ReactNode } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { queryKeys } from '@/shared/queries/query-keys';
import { Link, useNavigate } from 'react-router';
import { Clock3, Copy, RefreshCw } from 'lucide-react';
import { toast } from 'sonner';
import type {
  OperationViewConfig,
  WorkflowInstanceDto,
} from '@/generated/RuntaraRuntimeApi';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { Can } from '@/shared/components/Can';
import { useToken } from '@/shared/hooks';
import { RunStatusPill } from '@/features/invocation-history/components/RunLinks';
import { formatRunDuration } from '@/features/invocation-history/utils/run-duration';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import { StateValue, type StateField } from '../components/StateValue';
import { message, operationsRequest } from '../queries';
import { stateLabel } from '../state-label';

export function OperationHeader({
  title,
  description,
  actions,
  section,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
  section?: string;
}) {
  return (
    <header className="mb-5">
      <nav
        aria-label="Breadcrumb"
        className="mb-1 flex items-center gap-2 text-xs text-muted-foreground"
      >
        <Link to="/operations" className="hover:text-foreground">
          Operations
        </Link>
        {section ? (
          <>
            <span>/</span>
            <Link
              to={
                section === 'Queues' ? '/operations/queues' : '/operations/runs'
              }
            >
              {section}
            </Link>
          </>
        ) : null}
      </nav>
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div className="min-w-0">
          <h1 className="break-words text-2xl font-semibold tracking-tight">
            {title}
          </h1>
          {description ? (
            <div className="mt-2 text-sm text-muted-foreground">
              {description}
            </div>
          ) : null}
        </div>
        <div className="flex flex-wrap items-center gap-3">{actions}</div>
      </div>
    </header>
  );
}
export function OperationSection({
  title,
  aside,
  children,
  className = '',
}: {
  title: string;
  aside?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section
      className={`min-w-0 overflow-hidden rounded-lg border bg-background ${className}`}
    >
      <div className="flex min-h-12 flex-wrap items-center justify-between gap-2 border-b px-4 py-3">
        <h2 className="text-sm font-semibold">{title}</h2>
        <div className="text-xs text-muted-foreground">{aside}</div>
      </div>
      {children}
    </section>
  );
}
export function RefreshControls({
  updatedAt,
  busy,
  onRefresh,
  compact = false,
}: {
  updatedAt?: number;
  busy?: boolean;
  onRefresh: () => void;
  /** Icon only; the last update moves into the tooltip. */
  compact?: boolean;
}) {
  if (compact)
    return (
      <WithTooltip
        label={
          updatedAt ? (
            <>
              Refresh · updated{' '}
              <StateValue
                value={new Date(updatedAt).toISOString()}
                display={{ kind: 'relative' }}
              />
            </>
          ) : (
            'Refresh'
          )
        }
      >
        <Button
          variant="secondary"
          size="icon"
          aria-label="Refresh"
          disabled={busy}
          onClick={onRefresh}
        >
          <RefreshCw className={busy ? 'animate-spin' : ''} />
        </Button>
      </WithTooltip>
    );
  return (
    <>
      {updatedAt ? (
        <span className="hidden items-center gap-1.5 text-xs text-muted-foreground sm:inline-flex">
          <Clock3 className="size-3.5" />
          Updated{' '}
          <StateValue
            value={new Date(updatedAt).toISOString()}
            display={{ kind: 'relative' }}
          />
        </span>
      ) : null}
      <Button variant="secondary" bordered disabled={busy} onClick={onRefresh}>
        <RefreshCw className={busy ? 'animate-spin' : ''} />
        Refresh
      </Button>
    </>
  );
}
export function ReplayButton({
  run,
  initiallyConfirm = false,
  onClose,
}: {
  run: Pick<WorkflowInstanceDto, 'id' | 'workflowId' | 'runLabel'>;
  initiallyConfirm?: boolean;
  onClose?: () => void;
}) {
  const client = useQueryClient();
  const [confirm, setConfirm] = useState(initiallyConfirm);
  const [busy, setBusy] = useState(false);
  const token = useToken();
  const navigate = useNavigate();
  async function replay() {
    setBusy(true);
    try {
      const result = await operationsRequest<{ instanceId: string }>(
        token,
        `workflows/instances/${run.id}/replay`,
        'POST',
        {}
      );
      await Promise.all([
        client.invalidateQueries({ queryKey: ['operations'] }),
        client.invalidateQueries({ queryKey: queryKeys.executions.lists() }),
      ]);
      toast.success(`Replay queued for ${run.runLabel ?? 'run'}`);
      onClose?.();
      if (result.instanceId)
        navigate(`/operations/runs/${run.workflowId}/${result.instanceId}`);
      setConfirm(false);
    } catch (e) {
      toast.error(message(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Can permission="workflow:execute">
      {confirm ? (
        <div className="max-w-sm space-y-2 text-sm">
          <p>
            Replay starts from the beginning and repeats all side effects. The
            label stays the same.
          </p>
          <div className="flex gap-2">
            <Button disabled={busy} onClick={() => void replay()}>
              {busy ? 'Queueing…' : 'Confirm Replay'}
            </Button>
            <Button
              variant="secondary"
              bordered
              disabled={busy}
              onClick={() => {
                setConfirm(false);
                onClose?.();
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <Button onClick={() => setConfirm(true)}>
          <RefreshCw />
          Replay
        </Button>
      )}
    </Can>
  );
}
export function RunRows({
  rows,
  view,
  schema = {},
  showDuration = false,
}: {
  rows: WorkflowInstanceDto[];
  view?: OperationViewConfig;
  schema?: Record<string, StateField>;
  /** Add a Duration column after Started. */
  showDuration?: boolean;
}) {
  // Inside a bordered panel the console table's page-edge inset is too deep.
  const edge = 'md:first:pl-5 md:last:pr-5';
  return (
    <div className="overflow-x-auto">
      <Table variant="console">
        <TableHeader>
          <TableRow>
            <TableHead className={edge}>Run</TableHead>
            <TableHead className={edge}>Workflow</TableHead>
            <TableHead className={edge}>Status</TableHead>
            {view?.columns?.map((field) => (
              <TableHead className={edge} key={field}>
                {view.labels?.[field] || stateLabel(field, schema[field])}
              </TableHead>
            ))}
            <TableHead className={edge}>Started</TableHead>
            {showDuration ? (
              <TableHead className={edge}>Duration</TableHead>
            ) : null}
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((run) => {
            const label =
              (view?.roles?.key && run.state?.[view.roles.key]) ?? run.runLabel;
            return (
              <TableRow key={run.id}>
                <TableCell className={edge}>
                  <div className="flex items-center gap-2">
                    <Link
                      className={cn(
                        'whitespace-nowrap font-medium text-primary-text hover:underline',
                        label == null && 'font-mono text-[13px]'
                      )}
                      to={`/operations/runs/${run.workflowId}/${run.id}`}
                    >
                      {String(label ?? run.id.slice(0, 8))}
                    </Link>
                    <button
                      className="text-muted-foreground hover:text-foreground"
                      aria-label={`Copy run ID ${run.id}`}
                      title={run.id}
                      onClick={() => void navigator.clipboard.writeText(run.id)}
                    >
                      <Copy className="size-3" />
                    </button>
                  </div>
                </TableCell>
                <TableCell
                  className={cn('min-w-44 text-muted-foreground', edge)}
                >
                  {run.workflowName ?? run.workflowId}
                </TableCell>
                <TableCell className={cn('whitespace-nowrap', edge)}>
                  <RunStatusPill
                    status={run.status}
                    suspensionReason={run.suspensionReason}
                  />
                </TableCell>
                {view?.columns?.map((field) => (
                  <TableCell className={cn('max-w-xs', edge)} key={field}>
                    <StateValue
                      value={run.state?.[field]}
                      field={schema[field]}
                      display={view.formats?.[field]}
                    />
                  </TableCell>
                ))}
                <TableCell
                  className={cn(
                    'whitespace-nowrap tabular-nums text-muted-foreground',
                    edge
                  )}
                >
                  <StateValue
                    value={run.created}
                    display={{ kind: 'relative' }}
                  />
                </TableCell>
                {showDuration ? (
                  <TableCell
                    className={cn('whitespace-nowrap tabular-nums', edge)}
                  >
                    {formatRunDuration(run.executionDurationSeconds)}
                  </TableCell>
                ) : null}
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </div>
  );
}
