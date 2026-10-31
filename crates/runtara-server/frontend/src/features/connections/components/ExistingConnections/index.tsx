import { ReactNode, useState } from 'react';
import { Link } from 'react-router';
import { toast } from 'sonner';
import { RefreshCw } from 'lucide-react';
import { cn } from '@/lib/utils';
import { queryClient } from '@/bootstrap';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Button } from '@/shared/components/ui/button';
import { Can } from '@/shared/components/Can';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  ConsoleEmptyState,
  ConsoleErrorState,
  ConsoleTableShell,
  RowActionsMenu,
  StatusPill,
  TableSkeletonRows,
  TableStatusFooter,
} from '@/shared/components/console';
import { ModalDialog } from '@/shared/components/next-dialog';
import {
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { EnrichedConnection } from '@/features/connections/types';
import {
  getConnections,
  removeConnection,
} from '@/features/connections/queries';
import { useConnectionOAuth } from '@/features/connections/hooks/useConnectionOAuth';
import { connectionStatusPill } from '@/features/connections/utils/status';
import { Spinner } from '@/shared/components/ui/spinner';

function formatNumber(num: number): string {
  if (num >= 1000000) {
    return `${(num / 1000000).toFixed(1)}M`;
  }
  if (num >= 1000) {
    return `${(num / 1000).toFixed(1)}K`;
  }
  return num.toString();
}

function ConnectionUsage({ connection }: { connection: EnrichedConnection }) {
  const { rateLimitStats } = connection;

  if (!rateLimitStats) {
    return <span className="text-muted-foreground/60">—</span>;
  }

  const { totalRequests, rateLimitedCount } = rateLimitStats;
  const statsText =
    rateLimitedCount > 0
      ? `${formatNumber(totalRequests)} req (${formatNumber(rateLimitedCount)} limited)`
      : `${formatNumber(totalRequests)} req`;

  // Idle connections recede so the ones carrying traffic stand out.
  return (
    <Link
      to={`/connections/rate-limits?connection=${encodeURIComponent(connection.id)}`}
      className={cn(
        'whitespace-nowrap tabular-nums underline-offset-2 hover:underline',
        totalRequests > 0
          ? 'font-medium text-foreground'
          : 'text-muted-foreground/60 hover:text-foreground'
      )}
      aria-label={`Rate limits for ${connection.title}: ${statsText} in the last 24 hours`}
    >
      {formatNumber(totalRequests)} req
      {rateLimitedCount > 0 && (
        <span className="font-normal text-warning-text">
          {' '}
          ({formatNumber(rateLimitedCount)} limited)
        </span>
      )}
    </Link>
  );
}

interface ExistingConnectionsProps {
  /** Pinned console toolbar (breadcrumb + actions) from the page. */
  toolbar?: ReactNode;
}

export function ExistingConnections({ toolbar }: ExistingConnectionsProps) {
  const [deleteTarget, setDeleteTarget] = useState<EnrichedConnection | null>(
    null
  );

  const {
    data: connections = [],
    isFetching,
    isError,
    error,
  } = useCustomQuery({
    queryKey: queryKeys.connections.all,
    queryFn: getConnections,
  });

  const mutation = useCustomMutation({
    mutationFn: removeConnection,
    onSuccess: () => {
      toast.success('Connection deleted.');
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
    },
    onSettled: () => {
      setDeleteTarget(null);
    },
  });

  const deletingId = mutation.isPending ? deleteTarget?.id : null;

  // One-click reconnect for OAuth connections whose access has expired/been
  // revoked — reuses the stored credentials, no re-entry.
  const { authorize, isAuthorizing } = useConnectionOAuth();

  const handleDelete = () => {
    if (deleteTarget) {
      mutation.mutate(deleteTarget.id);
    }
  };

  const hasConnections = !!connections && connections.length > 0;

  let body: ReactNode;
  if (isFetching) {
    body = (
      <TableSkeletonRows rows={8} widths={['w-40', 'w-24', 'ml-auto w-32']} />
    );
  } else if (isError) {
    body = <ConsoleErrorState error={error} entityLabel="connections" />;
  } else if (!hasConnections) {
    body = (
      <ConsoleEmptyState
        title="No connections configured"
        description="Add a connection using the New connection button above."
      />
    );
  } else {
    body = (
      <Table variant="console" className="min-w-[760px] table-fixed">
        <TableHeader>
          <TableRow>
            <TableHead>Connection</TableHead>
            <TableHead className="w-[28%]">Integration</TableHead>
            <TableHead className="w-52">Status</TableHead>
            <TableHead className="w-32">Usage · 24 h</TableHead>
            <TableHead className="w-48">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {(connections as EnrichedConnection[]).map((connection) => {
            const statusPill = connectionStatusPill(connection.status);
            // A connection that needs a person is tinted, so it is found
            // without reading every status.
            const needsAttention =
              statusPill.tone === 'warning' || statusPill.tone === 'error';
            const canReconnect =
              !!connection.connectionType?.oauthConfig &&
              connection.status === 'REQUIRES_RECONNECTION';
            return (
              <TableRow
                key={connection.id}
                className={cn(
                  needsAttention &&
                    (statusPill.tone === 'error'
                      ? 'bg-destructive/[0.04] hover:bg-destructive/[0.07]'
                      : 'bg-warning/[0.06] hover:bg-warning/10')
                )}
              >
                <TableCell className="font-medium text-foreground">
                  <Link
                    to={`/connections/${connection.id}`}
                    className="block truncate hover:text-primary-text hover:underline"
                    title={connection.title}
                  >
                    {connection.title}
                  </Link>
                </TableCell>
                <TableCell className="truncate text-muted-foreground">
                  {connection.connectionType?.displayName || 'Connection'}
                </TableCell>
                <TableCell>
                  <StatusPill tone={statusPill.tone} label={statusPill.label} />
                </TableCell>
                <TableCell>
                  <ConnectionUsage connection={connection} />
                </TableCell>
                <TableCell className="text-right">
                  <div className="flex items-center justify-end gap-1.5">
                    {canReconnect && (
                      <Can permission="connection:update">
                        <Button
                          variant="secondary"
                          bordered
                          size="sm"
                          className="border-warning/40 text-warning-text hover:bg-warning/10"
                          title="Re-authorize with the saved credentials"
                          disabled={isAuthorizing(connection.id)}
                          onClick={() => authorize(connection.id)}
                        >
                          {isAuthorizing(connection.id) ? (
                            <Spinner className="size-3.5" />
                          ) : (
                            <RefreshCw className="size-3.5" />
                          )}
                          Reconnect
                        </Button>
                      </Can>
                    )}
                    <RowActionsMenu
                      label={connection.title}
                      editTo={`/connections/${connection.id}`}
                      editPermission="connection:update"
                      onDelete={() => setDeleteTarget(connection)}
                      deletePermission="connection:delete"
                      deleting={deletingId === connection.id}
                    />
                  </div>
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    );
  }

  return (
    <>
      <ConsoleTableShell
        toolbar={toolbar}
        footer={
          hasConnections && !isFetching && !isError ? (
            <TableStatusFooter
              left={
                <span>
                  <strong className="font-semibold text-foreground">
                    {connections.length}
                  </strong>{' '}
                  connection{connections.length === 1 ? '' : 's'}
                </span>
              }
            />
          ) : undefined
        }
      >
        {body}
      </ConsoleTableShell>

      <ModalDialog open={!!deleteTarget} onClose={() => setDeleteTarget(null)}>
        <DialogHeader>
          <DialogTitle>Delete Connection</DialogTitle>
          <DialogDescription>
            Are you sure you want to delete the connection "
            {deleteTarget?.title}"?
          </DialogDescription>
        </DialogHeader>
        <div className="py-2">
          This action cannot be undone and may affect any workflows using this
          connection.
        </div>
        <DialogFooter className="gap-2 sm:gap-0">
          <DialogClose asChild>
            <Button type="button" variant="secondary" bordered>
              Cancel
            </Button>
          </DialogClose>
          <Button
            type="button"
            variant="destructive"
            onClick={handleDelete}
            disabled={mutation.isPending}
          >
            {mutation.isPending ? 'Deleting...' : 'Delete Connection'}
          </Button>
        </DialogFooter>
      </ModalDialog>
    </>
  );
}
