import { ReactNode, useState } from 'react';
import { Link } from 'react-router';
import { toast } from 'sonner';
import { Activity, Pencil, Trash2, RefreshCw } from 'lucide-react';
import { queryClient } from '@/main';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
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

  const statsText =
    rateLimitStats.rateLimitedCount > 0
      ? `${formatNumber(rateLimitStats.totalRequests)} req (${formatNumber(rateLimitStats.rateLimitedCount)} limited) 24h`
      : `${formatNumber(rateLimitStats.totalRequests)} req 24h`;

  return (
    <span className="inline-flex items-center gap-1 text-xs">
      <Activity className="size-3" />
      {statsText}
    </span>
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
      <Table variant="console">
        <TableHeader>
          <TableRow>
            <TableHead>Connection</TableHead>
            <TableHead>Integration</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Usage</TableHead>
            <TableHead className="w-0" />
          </TableRow>
        </TableHeader>
        <TableBody>
          {(connections as EnrichedConnection[]).map((connection) => {
            const statusPill = connectionStatusPill(connection.status);
            return (
              <TableRow key={connection.id}>
                <TableCell className="font-medium text-foreground">
                  {connection.title}
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {connection.connectionType?.displayName || 'Connection'}
                </TableCell>
                <TableCell>
                  <StatusPill tone={statusPill.tone} label={statusPill.label} />
                </TableCell>
                <TableCell className="text-muted-foreground">
                  <ConnectionUsage connection={connection} />
                </TableCell>
                <TableCell className="text-right">
                  <div className="flex items-center justify-end gap-1">
                    {connection.connectionType?.oauthConfig &&
                      connection.status === 'REQUIRES_RECONNECTION' && (
                        <Can permission="connection:update">
                          <WithTooltip label="Reconnect (re-authorize with saved credentials)">
                            <Button
                              variant="ghost"
                              size="icon-sm"
                              className="text-warning hover:text-warning"
                              aria-label="Reconnect (re-authorize with saved credentials)"
                              disabled={isAuthorizing(connection.id)}
                              onClick={() => authorize(connection.id)}
                            >
                              {isAuthorizing(connection.id) ? (
                                <Spinner className="size-4" />
                              ) : (
                                <RefreshCw className="size-4" />
                              )}
                            </Button>
                          </WithTooltip>
                        </Can>
                      )}
                    <Can permission="connection:update">
                      <Link to={`/connections/${connection.id}`}>
                        <WithTooltip label="Edit connection">
                          <Button
                            variant="quiet"
                            size="icon-sm"
                            aria-label="Edit connection"
                          >
                            <Pencil className="size-4" />
                          </Button>
                        </WithTooltip>
                      </Link>
                    </Can>
                    <Can permission="connection:delete">
                      <WithTooltip label="Delete connection">
                        <Button
                          variant="quietDestructive"
                          size="icon-sm"
                          aria-label="Delete connection"
                          disabled={deletingId === connection.id}
                          onClick={() => setDeleteTarget(connection)}
                        >
                          {deletingId === connection.id ? (
                            <Spinner className="size-4" />
                          ) : (
                            <Trash2 className="size-4" />
                          )}
                        </Button>
                      </WithTooltip>
                    </Can>
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
              left={`${connections.length} connection${
                connections.length === 1 ? '' : 's'
              }`}
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
            <Button type="button" variant="outline">
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
