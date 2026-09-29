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
import { Button } from '@/shared/components/ui/button';
import { Can } from '@/shared/components/Can';
import { useToken } from '@/shared/hooks';
import { RunStatusPill } from '@/features/invocation-history/components/RunLinks';
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
}: {
  updatedAt?: number;
  busy?: boolean;
  onRefresh: () => void;
}) {
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
}: {
  rows: WorkflowInstanceDto[];
  view?: OperationViewConfig;
  schema?: Record<string, StateField>;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="bg-muted/50 text-xs text-muted-foreground">
          <tr>
            <th className="px-4 py-3">Run</th>
            <th className="px-4 py-3">Workflow</th>
            <th className="px-4 py-3">Status</th>
            {view?.columns?.map((field) => (
              <th className="px-4 py-3" key={field}>
                {view.labels?.[field] || stateLabel(field, schema[field])}
              </th>
            ))}
            <th className="px-4 py-3">Started</th>
          </tr>
        </thead>
        <tbody className="divide-y">
          {rows.map((run) => (
            <tr key={run.id}>
              <td className="px-4 py-3">
                <div className="flex items-center gap-2">
                  <Link
                    className="whitespace-nowrap font-medium text-primary-text hover:underline"
                    to={`/operations/runs/${run.workflowId}/${run.id}`}
                  >
                    {String(
                      (view?.roles?.key && run.state?.[view.roles.key]) ??
                        run.runLabel ??
                        run.id.slice(0, 8)
                    )}
                  </Link>
                  <button
                    className="text-muted-foreground"
                    aria-label={`Copy run ID ${run.id}`}
                    title={run.id}
                    onClick={() => void navigator.clipboard.writeText(run.id)}
                  >
                    <Copy className="size-3" />
                  </button>
                </div>
              </td>
              <td className="min-w-44 px-4 py-3">
                {run.workflowName ?? run.workflowId}
              </td>
              <td className="whitespace-nowrap px-4 py-3">
                <RunStatusPill
                  status={run.status}
                  suspensionReason={run.suspensionReason}
                />
              </td>
              {view?.columns?.map((field) => (
                <td className="max-w-xs px-4 py-3" key={field}>
                  <StateValue
                    value={run.state?.[field]}
                    field={schema[field]}
                    display={view.formats?.[field]}
                  />
                </td>
              ))}
              <td className="whitespace-nowrap px-4 py-3">
                <StateValue
                  value={run.created}
                  display={{ kind: 'relative' }}
                />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
