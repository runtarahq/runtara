import { useState } from 'react';
import { Link } from 'react-router';
import { toast } from 'sonner';
import { Activity, Pencil, Trash2, Loader2 } from 'lucide-react';
import { queryClient } from '@/main';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Icons } from '@/shared/components/icons.tsx';
import { Button } from '@/shared/components/ui/button';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
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

export function ExistingConnections() {
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

  if (isFetching) {
    return (
      <div className="rounded-lg border divide-y">
        {[...Array(6)].map((_, i) => (
          <div key={i} className="flex items-center gap-4 px-3 py-2.5">
            <div className="h-4 w-40 rounded bg-muted/60 animate-pulse" />
            <div className="h-4 w-24 rounded bg-muted/60 animate-pulse" />
            <div className="ml-auto h-4 w-32 rounded bg-muted/60 animate-pulse" />
          </div>
        ))}
      </div>
    );
  }

  if (isError) {
    const err = error as any;
    const isNetworkError =
      err?.message?.includes('fetch') ||
      err?.code === 'ERR_NETWORK' ||
      !err?.response;

    return (
      <div className="rounded-lg border bg-muted/20 px-6 py-10 text-center">
        <Icons.warning className="mx-auto mb-4 h-10 w-10 text-destructive" />
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
          <div className="mt-4 max-w-md mx-auto rounded-lg bg-destructive/10 p-3 text-left">
            <p className="text-xs font-mono text-destructive break-words">
              {error.message || 'Unknown error'}
            </p>
          </div>
        )}
      </div>
    );
  }

  if (!connections || connections.length === 0) {
    return (
      <div className="rounded-lg border bg-muted/20 px-6 py-10 text-center">
        <Icons.inbox className="mx-auto mb-4 h-10 w-10 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No connections configured
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Add a connection from the available options below.
        </p>
      </div>
    );
  }

  return (
    <>
      <div className="rounded-lg border overflow-hidden">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Connection</TableHead>
              <TableHead>Integration</TableHead>
              <TableHead>Usage</TableHead>
              <TableHead className="w-0" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {(connections as EnrichedConnection[]).map((connection) => (
              <TableRow key={connection.id}>
                <TableCell className="font-medium text-foreground">
                  {connection.title}
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {connection.connectionType?.displayName || 'Connection'}
                </TableCell>
                <TableCell className="text-muted-foreground">
                  <ConnectionUsage connection={connection} />
                </TableCell>
                <TableCell className="text-right">
                  <div className="flex items-center justify-end gap-1">
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
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

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
