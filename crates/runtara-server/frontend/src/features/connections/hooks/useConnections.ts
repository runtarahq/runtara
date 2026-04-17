import { useQueryClient } from '@tanstack/react-query';
import {
  ConnectionDto,
  ConnectionTypeDto,
  CreateConnectionRequest,
} from '@/generated/RuntaraRuntimeApi';
import {
  getConnections,
  getConnectionById,
  getConnectionTypes,
  createConnection,
  updateConnection,
  removeConnection,
} from '../queries';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';

// Types for enriched connection data
interface EnrichedConnection extends ConnectionDto {
  connectionType: ConnectionTypeDto | null;
}

/**
 * Hook to fetch all connections with their connection type data
 */
export function useConnections() {
  return useCustomQuery<EnrichedConnection[]>({
    queryKey: queryKeys.connections.all,
    queryFn: (token) => getConnections(token),
  });
}

/**
 * Hook to fetch a single connection by ID
 */
export function useConnectionById(id: string | undefined) {
  return useCustomQuery<EnrichedConnection | null>({
    queryKey: queryKeys.connections.byId(id || ''),
    queryFn: (token, context) => getConnectionById(token, context),
    enabled: !!id,
  });
}

/**
 * Hook to fetch all connection types
 */
export function useConnectionTypes() {
  return useCustomQuery<ConnectionTypeDto[]>({
    queryKey: queryKeys.connections.types(),
    queryFn: (token) => getConnectionTypes(token),
  });
}

/**
 * Hook to create a new connection
 */
export function useCreateConnection() {
  const queryClient = useQueryClient();

  return useCustomMutation<string, CreateConnectionRequest>({
    mutationFn: (token, connection) => createConnection(token, connection),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
    },
  });
}

/**
 * Hook to update an existing connection
 */
export function useUpdateConnection() {
  const queryClient = useQueryClient();

  return useCustomMutation<
    void,
    {
      id: string;
      title?: string;
      parameters?: Record<string, string>;
      isDefaultFileStorage?: boolean;
    }
  >({
    mutationFn: (token, connection) => updateConnection(token, connection),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
      queryClient.invalidateQueries({
        queryKey: queryKeys.connections.byId(variables.id),
      });
    },
  });
}

/**
 * Hook to delete a connection
 */
export function useDeleteConnection() {
  const queryClient = useQueryClient();

  return useCustomMutation<void, string>({
    mutationFn: (token, connectionId) => removeConnection(token, connectionId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.connections.all });
    },
  });
}
