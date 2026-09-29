import { Link } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { Pencil, Plus } from 'lucide-react';
import { Can } from '@/shared/components/Can';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { useOperations } from '../queries';
import { DeleteQueue } from '../components/DeleteQueue';
import { OperationHeader, OperationSection, RefreshControls } from './shared';
export function QueuesPage() {
  const { views, processes } = useOperations();
  const client = useQueryClient();
  return (
    <div className="mx-auto min-h-full w-full max-w-[1600px] bg-background p-5 lg:px-10 lg:py-7">
      <OperationHeader
        title="Queues"
        description="Custom queues shared with your team."
        actions={
          <>
            <RefreshControls
              updatedAt={views.dataUpdatedAt}
              busy={views.isFetching}
              onRefresh={() =>
                void client.invalidateQueries({ queryKey: ['operations'] })
              }
            />
            <Can permission="workflow:update">
              <Button asChild>
                <Link to="/operations/queues/new">
                  <Plus className="size-4" />
                  Create queue
                </Link>
              </Button>
            </Can>
          </>
        }
      />
      <main>
        <OperationSection title="Queues">
          <ul className="divide-y">
            {views.data?.map((queue) => (
              <li
                key={queue.id}
                className="flex min-w-0 items-center gap-4 px-4 py-3"
              >
                <Link
                  className="min-w-0 flex-1 hover:text-primary-text"
                  to={`/operations/queues/${queue.id}`}
                >
                  <h3
                    className="truncate text-sm font-medium"
                    title={queue.configuration.name}
                  >
                    {queue.configuration.name}
                  </h3>
                  <p className="mt-1 truncate text-xs text-muted-foreground">
                    {processes.data?.find(
                      (workflow) =>
                        workflow.workflowId === queue.configuration.workflow
                    )?.name ?? queue.configuration.workflow}{' '}
                    ·{' '}
                    {queue.configuration.where?.openRequest
                      ? 'Open requests'
                      : 'Runs'}
                  </p>
                </Link>
                <Can permission="workflow:update">
                  <div className="flex shrink-0 items-center gap-1">
                    <WithTooltip label="Edit queue">
                      <Button
                        asChild
                        variant="secondary"
                        size="icon"
                        className="h-8 w-8 text-muted-foreground"
                      >
                        <Link
                          aria-label={`Edit queue ${queue.configuration.name}`}
                          to={`/operations/queues/${queue.id}/edit`}
                        >
                          <Pencil className="size-4" />
                        </Link>
                      </Button>
                    </WithTooltip>
                    <DeleteQueue queue={queue} icon />
                  </div>
                </Can>
              </li>
            ))}
          </ul>
          {views.error ? (
            <p role="alert" className="p-4 text-sm text-destructive">
              Could not load queues. Try Refresh.
            </p>
          ) : views.isPending ? (
            <p className="p-4 text-sm">Loading queues…</p>
          ) : !views.data?.length ? (
            <div className="space-y-2 p-5 text-sm">
              <p>No queues yet.</p>
              <p className="text-muted-foreground">
                Create a queue to choose its workflow, requests, columns, and
                filters.
              </p>
            </div>
          ) : null}
        </OperationSection>
      </main>
    </div>
  );
}
