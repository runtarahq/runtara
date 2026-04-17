import { useMemo } from 'react';
import { useNavigate, useParams } from 'react-router';
import { toast } from 'sonner';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Loader2 } from '@/shared/components/loader.tsx';
import { ConnectionTypeDto } from '@/generated/RuntaraRuntimeApi';
import { DynamicConnectionForm } from '@/features/connections/components/Forms/DynamicConnectionForm';
import {
  getConnectionById,
  getConnectionTypes,
  updateConnection,
  removeConnection,
} from '@/features/connections/queries';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { queryClient } from '@/main.tsx';

export function Connection() {
  const { id } = useParams();

  const navigate = useNavigate();

  const connection = useCustomQuery({
    queryKey: queryKeys.connections.byId(id ?? ''),
    queryFn: (token: string) => getConnectionById(token, id!),
    enabled: !!id,
  });

  const connectionTypesQuery = useCustomQuery({
    queryKey: queryKeys.connections.types(),
    queryFn: getConnectionTypes,
  });

  const mutation = useCustomMutation({
    mutationFn: updateConnection,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
      queryClient.invalidateQueries({
        queryKey: queryKeys.connections.byId(id ?? ''),
      });
      navigate('/connections');
      toast.info('Connection successfully updated');
    },
  });

  const deleteMutation = useCustomMutation({
    mutationFn: removeConnection,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
      navigate('/connections');
      toast.info('Connection deleted');
    },
  });

  const currentConnectionType = useMemo(() => {
    const connData = connection.data as any;
    if (!connData?.integrationId || !connectionTypesQuery.data) {
      return null;
    }
    return (
      connectionTypesQuery.data.find(
        (ct: ConnectionTypeDto) => ct.integrationId === connData.integrationId
      ) || null
    );
  }, [connectionTypesQuery.data, connection.data]);

  // Set page title with connection name
  usePageTitle(
    connection.data?.title
      ? `Connections - ${connection.data.title}`
      : 'Edit Connection'
  );

  const handleSubmit = (data: Record<string, unknown>) => {
    const {
      title,
      rateLimitEnabled,
      requestsPerSecond,
      burstSize,
      maxRetries,
      maxWaitMs,
      retryOnLimit,
      isDefaultFileStorage,
      ...parameters
    } = data;

    const rateLimitConfig = rateLimitEnabled
      ? {
          requestsPerSecond: Number(requestsPerSecond),
          burstSize: Number(burstSize),
          maxRetries: Number(maxRetries),
          maxWaitMs: Number(maxWaitMs),
          retryOnLimit: Boolean(retryOnLimit),
        }
      : null;

    mutation.mutate({
      id: id as string,
      title: title as string | undefined,
      parameters: parameters as Record<string, unknown>,
      rateLimitConfig,
      isDefaultFileStorage:
        isDefaultFileStorage !== undefined
          ? Boolean(isDefaultFileStorage)
          : undefined,
    });
  };

  // Show loading indicator while fetching initial data
  if (connection.isLoading || connectionTypesQuery.isLoading) {
    return (
      <div className="mx-auto max-w-5xl px-4 py-10 sm:px-6 lg:px-10">
        <Loader2 />
      </div>
    );
  }

  if (!currentConnectionType || !connection.data) {
    return (
      <div className="mx-auto max-w-5xl px-4 py-10 sm:px-6 lg:px-10">
        <div className="rounded-2xl bg-muted/20 px-6 py-8 text-center text-muted-foreground">
          <p>Connection not found or configuration unavailable.</p>
        </div>
      </div>
    );
  }

  const handleDelete = () => {
    if (id) {
      deleteMutation.mutate(id);
    }
  };

  return (
    <DynamicConnectionForm
      connectionType={currentConnectionType}
      initValues={connection.data}
      isLoading={mutation.isPending || connection.isFetching}
      onSubmit={handleSubmit}
      mode="edit"
      onDelete={handleDelete}
      isDeleting={deleteMutation.isPending}
    />
  );
}
