import { useState } from 'react';
import { toast } from 'sonner';
import { queryClient } from '@/main';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { ConnectionCard } from '@/features/connections/components/ConnectionCard';
import { Icons } from '@/shared/components/icons.tsx';
import {
  getConnections,
  removeConnection,
} from '@/features/connections/queries';

export function ExistingConnections() {
  const [deletingId, setDeletingId] = useState<string | null>(null);

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
      setDeletingId(null);
    },
  });

  const handleDelete = (id: string) => {
    setDeletingId(id);
    mutation.mutate(id);
  };

  if (isFetching) {
    return (
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {[...Array(6)].map((_, i) => (
          <div
            key={i}
            className="rounded-xl bg-muted/20 px-4 py-5 sm:px-5 sm:py-6 animate-pulse"
          >
            <div className="flex flex-col gap-2">
              <div className="h-4 w-20 rounded bg-muted/60" />
              <div className="h-4 w-40 rounded bg-muted/60" />
              <div className="h-3 w-32 rounded bg-muted/60" />
            </div>
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
      <div className="rounded-2xl bg-muted/20 px-6 py-10 text-center">
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
      <div className="rounded-2xl bg-muted/20 px-6 py-10 text-center">
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
    <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
      {(connections as any[]).map((connection) => (
        <ConnectionCard
          key={connection.id}
          connection={connection}
          loading={deletingId === connection.id}
          onDelete={handleDelete}
        />
      ))}
    </div>
  );
}
