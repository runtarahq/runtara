import { Link } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { useOperations } from '../queries';
import { OperationHeader, OperationSection, RefreshControls } from './shared';
export function QueuesPage() {
  const { queues, views } = useOperations();
  const client = useQueryClient();
  return (
    <div className="mx-auto min-h-full w-full max-w-[1600px] bg-background p-5 lg:px-10 lg:py-7">
      <OperationHeader
        title="Queues"
        section="Queues"
        description="Requests waiting for an answer, grouped by workflow."
        actions={
          <RefreshControls
            updatedAt={queues.dataUpdatedAt}
            busy={queues.isFetching}
            onRefresh={() =>
              void client.invalidateQueries({ queryKey: ['operations'] })
            }
          />
        }
      />
      <main className="space-y-5">
        <OperationSection title="Queues">
          <ul className="divide-y">
            {queues.data?.map((queue) => (
              <li key={`${queue.workflowId}/${queue.actionKey}`}>
                <Link
                  className="flex items-center justify-between gap-4 px-4 py-4 hover:bg-muted/30"
                  to={`/operations/queues/${encodeURIComponent(queue.workflowId)}/${encodeURIComponent(queue.actionKey)}`}
                >
                  <div className="min-w-0">
                    <h3 className="break-words text-sm font-semibold">
                      {queue.name}
                    </h3>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {queue.workflowName}
                    </p>
                  </div>
                  <span className="shrink-0 rounded-full bg-muted px-3 py-1 text-xs tabular-nums">
                    {queue.count} {queue.count === 1 ? 'request' : 'requests'}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
          {queues.isPending ? (
            <p className="p-4 text-sm">Loading queues…</p>
          ) : queues.error ? (
            <p role="alert" className="p-4">
              Could not load queues.
            </p>
          ) : !queues.data?.length ? (
            <p className="p-4 text-sm text-muted-foreground">
              No action queues yet.
            </p>
          ) : null}
        </OperationSection>
        <OperationSection title="Shared views">
          <ul className="divide-y">
            {views.data?.map((view) => (
              <li key={view.id}>
                <Link
                  className="block px-4 py-4 text-sm font-medium text-primary-text hover:bg-muted/30"
                  to={`/operations/views/${view.id}`}
                >
                  {view.configuration.name}
                </Link>
              </li>
            ))}
          </ul>
          {views.error ? (
            <p role="alert" className="p-4">
              Could not load shared views.
            </p>
          ) : views.isPending ? (
            <p className="p-4 text-sm">Loading views…</p>
          ) : !views.data?.length ? (
            <p className="p-4 text-sm text-muted-foreground">
              Save a view from a queue to share its columns and filters.
            </p>
          ) : null}
        </OperationSection>
      </main>
    </div>
  );
}
