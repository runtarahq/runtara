import { stateLabel } from '../state-label';
import { useState, type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router';
import { toast } from 'sonner';
import type {
  OperationViewConfig,
  WorkflowInstanceDto,
} from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import { Can } from '@/shared/components/Can';
import { useToken } from '@/shared/hooks';
import { RunStatusPill } from '@/features/invocation-history/components/RunLinks';
import { StructuredErrorDisplay } from '@/shared/components/StructuredErrorDisplay';

import { StateValue, type StateField } from '../components/StateValue';
import { message, operationsRequest, failureText } from '../queries';

export function OperationHeader({
  title,
  actions,
}: {
  title: string;
  actions?: ReactNode;
}) {
  return (
    <header className="border-b px-6 py-5">
      <div className="mb-3 flex flex-wrap gap-5 text-sm text-muted-foreground">
        <Link to="/operations">Overview</Link>
        <Link to="/operations/monitor">Monitor</Link>
      </div>
      <div className="flex items-center justify-between gap-4">
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        <div className="flex gap-2">{actions}</div>
      </div>
    </header>
  );
}
export function ReplayButton({ run }: { run: WorkflowInstanceDto }) {
  const [confirm, setConfirm] = useState(false);
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
      toast.success(`Replay queued for ${run.runLabel ?? 'run'}`);
      const id = result.instanceId;
      if (id) navigate(`/operations/runs/${run.workflowId}/${id}`);
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
        <div className="max-w-xs space-y-2 text-sm">
          <p>
            Replay starts from the beginning and repeats all side effects. The
            label stays the same.
          </p>
          <Button size="sm" disabled={busy} onClick={() => void replay()}>
            {busy ? 'Queueing…' : 'Confirm Replay'}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => setConfirm(false)}
          >
            Cancel
          </Button>
        </div>
      ) : (
        <Button size="sm" variant="secondary" onClick={() => setConfirm(true)}>
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
  failures = false,
}: {
  rows: WorkflowInstanceDto[];
  view?: OperationViewConfig;
  schema?: Record<string, StateField>;
  failures?: boolean;
}) {
  return (
    <table className="w-full text-left text-sm">
      <thead className="bg-muted/70 text-xs text-muted-foreground">
        <tr>
          <th className="p-4">Run</th>
          <th className="p-4">Workflow</th>
          <th className="p-4">Status</th>
          {view?.columns?.map((field) => (
            <th className="p-4" key={field}>
              {view.labels?.[field] || stateLabel(field, schema[field])}
            </th>
          ))}
          <th className="p-4">Started</th>
          {failures ? (
            <>
              <th className="p-4">Error</th>
              <th className="p-4">Action</th>
            </>
          ) : null}
        </tr>
      </thead>
      <tbody>
        {rows.map((run) => (
          <tr key={run.id} className="border-b align-top">
            <td className="p-4">
              <Link
                className="font-medium text-primary hover:underline"
                to={`/operations/runs/${run.workflowId}/${run.id}`}
              >
                {String(
                  (view?.roles?.key && run.state?.[view.roles.key]) ??
                    run.runLabel ??
                    run.id.slice(0, 8)
                )}
              </Link>
              <button
                className="mt-1 block text-xs text-muted-foreground"
                title={run.id}
                onClick={() => void navigator.clipboard.writeText(run.id)}
              >
                Copy run ID
              </button>
            </td>
            <td className="p-4">{run.workflowName ?? run.workflowId}</td>
            <td className="p-4">
              <RunStatusPill
                status={run.status}
                suspensionReason={run.suspensionReason}
              />
            </td>
            {view?.columns?.map((field) => (
              <td className="max-w-xs p-4" key={field}>
                <StateValue
                  value={run.state?.[field]}
                  field={schema[field]}
                  display={view.formats?.[field]}
                />
              </td>
            ))}
            <td className="p-4">
              <StateValue value={run.created} display={{ kind: 'relative' }} />
            </td>
            {failures ? (
              <>
                <td className="max-w-md p-4">
                  <StructuredErrorDisplay
                    error={failureText(run.errorSummary, run.error)}
                    mode="compact"
                  />
                </td>
                <td className="p-4">
                  {run.errorSummary?.category === 'transient' ? (
                    <ReplayButton run={run} />
                  ) : (
                    <Link
                      className="text-primary"
                      to={`/operations/runs/${run.workflowId}/${run.id}`}
                    >
                      Review
                    </Link>
                  )}
                </td>
              </>
            ) : null}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
