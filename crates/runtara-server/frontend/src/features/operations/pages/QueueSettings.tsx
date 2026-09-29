import { useState } from 'react';
import { useNavigate, useParams } from 'react-router';
import { Can } from '@/shared/components/Can';
import { useOperations, defaultView } from '../queries';
import { ViewEditor } from '../components/ViewEditor';
import { OperationHeader } from './shared';

export function QueueSettingsPage() {
  const { queueId } = useParams();
  const navigate = useNavigate();
  const { queues: sources, views, processes } = useOperations();
  const [workflowId, setWorkflowId] = useState('');
  const saved = views.data?.find((view) => view.id === queueId);
  const selectedId = saved?.configuration.workflow ?? workflowId;
  const workflows = [
    ...new Map([
      ...(processes.data ?? []).map(
        (workflow) => [workflow.workflowId, workflow] as const
      ),
      ...(sources.data ?? [])
        .filter(
          (source) =>
            !processes.data?.some(
              (workflow) => workflow.workflowId === source.workflowId
            )
        )
        .map(
          (source) =>
            [
              source.workflowId,
              {
                workflowId: source.workflowId,
                name: source.workflowName,
                stateSchema: source.stateSchema,
              },
            ] as const
        ),
    ]).values(),
  ];
  const workflow = workflows.find(
    (workflow) => workflow.workflowId === selectedId
  );
  const source = sources.data?.find(
    (source) => source.workflowId === selectedId
  );
  const initial =
    saved?.configuration ??
    (source
      ? {
          ...defaultView(source),
          name: `${workflow?.name ?? source.workflowName} queue`,
          answers: { bulk: false },
        }
      : {
          name: `${workflow?.name ?? 'New'} queue`,
          workflow: selectedId,
          columns: Object.keys(workflow?.stateSchema ?? {}).slice(0, 29),
          where: {},
          roles: {},
          answers: {},
        });
  return (
    <div className="mx-auto min-h-full w-full max-w-[1600px] bg-background p-5 lg:px-10 lg:py-7">
      <OperationHeader
        title={queueId ? 'Edit queue' : 'Create queue'}
        section="Queues"
        description="Choose the work this queue shows and how your team sees it."
      />
      <Can
        permission="workflow:update"
        fallback={
          <p className="text-sm text-muted-foreground">
            You do not have permission to manage queues.
          </p>
        }
      >
        {sources.error || views.error || processes.error ? (
          <p role="alert">
            Could not load queue settings. Refresh to try again.
          </p>
        ) : sources.isPending || views.isPending || processes.isPending ? (
          <p>Loading settings…</p>
        ) : queueId && !saved ? (
          <p>Queue not found.</p>
        ) : (
          <>
            <label className="block max-w-lg text-sm">
              Workflow
              <select
                aria-label="Workflow"
                className="mt-1 block h-9 w-full rounded border bg-background px-3 disabled:opacity-70"
                disabled={!!saved}
                value={selectedId}
                onChange={(event) => setWorkflowId(event.target.value)}
              >
                <option value="">Choose a workflow</option>
                {saved && !workflow && (
                  <option value={selectedId}>{selectedId}</option>
                )}
                {workflows.map((workflow) => (
                  <option key={workflow.workflowId} value={workflow.workflowId}>
                    {workflow.name}
                  </option>
                ))}
              </select>
            </label>
            {!workflows.length && !saved && (
              <p className="mt-3 text-sm text-muted-foreground">
                Create a workflow before creating a queue.
              </p>
            )}
            {selectedId && (
              <ViewEditor
                key={queueId ?? selectedId}
                initial={initial}
                saved={saved}
                schema={workflow?.stateSchema ?? {}}
                requestSources={(sources.data ?? []).filter(
                  (source) => source.workflowId === selectedId
                )}
                onCancel={() =>
                  navigate(
                    saved
                      ? `/operations/queues/${saved.id}`
                      : '/operations/queues'
                  )
                }
                onSaved={(result) =>
                  navigate(`/operations/queues/${result.id}`)
                }
              />
            )}
          </>
        )}
      </Can>
    </div>
  );
}
