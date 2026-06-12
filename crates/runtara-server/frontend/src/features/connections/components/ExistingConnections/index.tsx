import { ReactNode, useState } from 'react';
import { Link } from 'react-router';
import { toast } from 'sonner';
import { Activity, Pencil, Trash2, Loader2 } from 'lucide-react';
import { queryClient } from '@/main';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Icons } from '@/shared/components/icons.tsx';
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
  ConsoleTableShell,
  StatusPill,
  TableStatusFooter,
  type StatusTone,
} from '@/shared/components/console';
import { ModalDialog } from '@/shared/components/next-dialog';
import {
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { ConnectionStatus } from '@/generated/RuntaraRuntimeApi';
import { EnrichedConnection } from '@/features/connections/types';
import {
  getConnections,
  removeConnection,
} from '@/features/connections/queries';

function connectionStatusPill(status: ConnectionStatus): {
  tone: StatusTone;
  label: string;
} {
  switch (status) {
    case 'ACTIVE':
      return { tone: 'success', label: 'Connected' };
    case 'REQUIRES_RECONNECTION':
      return { tone: 'warning', label: 'Reconnect required' };
    case 'INVALID_CREDENTIALS':
      return { tone: 'error', label: 'Invalid credentials' };
    default:
      return { tone: 'neutral', label: 'Unknown' };
  }
}

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
      <Activity className="h-3 w-3" />
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
      toast.info('Connection has been removed');
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
    },
    onSettled: () => {
      setDeleteTarget(null);
    },
  });

  const deletingId = mutation.isPending ? deleteTarget?.id : null;

  const handleDelete = () => {
    if (deleteTarget) {
      mutation.mutate(deleteTarget.id);
    }
  };

  const hasConnections = !!connections && connections.length > 0;

  let body: ReactNode;
  if (isFetching) {
    body = (
      <div className="divide-y divide-border/50">
        {[...Array(8)].map((_, i) => (
          <div key={i} className="flex items-center gap-4 px-5 py-3.5">
            <div className="h-4 w-40 animate-pulse rounded bg-muted/60" />
            <div className="h-4 w-24 animate-pulse rounded bg-muted/60" />
            <div className="ml-auto h-4 w-32 animate-pulse rounded bg-muted/60" />
          </div>
        ))}
      </div>
    );
  } else if (isError) {
    const err = error as any;
    const isNetworkError =
      err?.message?.includes('fetch') ||
      err?.code === 'ERR_NETWORK' ||
      !err?.response;

    body = (
      <div className="flex h-full flex-col items-center justify-center px-6 py-10 text-center">
        <Icons.warning className="mb-4 h-10 w-10 text-destructive" />
        <p className="text-base font-semibold text-foreground">
          {isNetworkError
            ? 'Unable to connect to backend'
            : 'An error occurred'}
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          {isNetworkError
            ? 'Please check that the backend service is running and try again.'
            : 'There was a problem loading connections. Please try again.'}
        </p>
        {import.meta.env.DEV && error && (
          <div className="mt-4 max-w-md rounded-lg bg-destructive/10 p-3 text-left">
            <p className="break-words font-mono text-xs text-destructive">
              {error.message || 'Unknown error'}
            </p>
          </div>
        )}
      </div>
    );
  } else if (!hasConnections) {
    body = (
      <div className="flex h-full flex-col items-center justify-center px-6 py-10 text-center">
        <Icons.inbox className="mb-4 h-10 w-10 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No connections configured
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Add a connection using the New connection button above.
        </p>
      </div>
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
                    <Can permission="connection:update">
                    <Link to={`/connections/${connection.id}`}>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground"
                        title="Edit connection"
                      >
                        <Pencil className="h-4 w-4" />
                      </Button>
                    </Link>
                    </Can>
                    <Can permission="connection:delete">
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      title="Delete connection"
                      disabled={deletingId === connection.id}
                      onClick={() => setDeleteTarget(connection)}
                    >
                      {deletingId === connection.id ? (
                        <Loader2 className="h-4 w-4 animate-spin" />
                      ) : (
                        <Trash2 className="h-4 w-4" />
                      )}
                    </Button>
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

      <ModalDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
      >
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
