import {
  keepPreviousData,
  useQueries,
  useQueryClient,
} from '@tanstack/react-query';
import { useAuth } from 'react-oidc-context';
import {
  Schema,
  CreateSchemaRequest,
  UpdateSchemaRequest,
} from '@/generated/RuntaraRuntimeApi';
import {
  createSchemaWithConnection,
  deleteSchema,
  getAllSchemas,
  getSchemaById,
  updateSchema,
} from '../queries';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';
import { isOidcAuth } from '@/shared/config/runtimeConfig';

/**
 * Hook to fetch all object schemas
 */
export function useObjectSchemaDtos(connectionId?: string | null) {
  return useCustomQuery<Schema[]>({
    queryKey: queryKeys.objects.schemas.all(connectionId),
    queryFn: (token) => getAllSchemas(token, connectionId),
    enabled: !!connectionId,
    retry: (
      failureCount,
      error: Error & { code?: string; response?: unknown }
    ) => {
      // Don't retry network errors (backend unavailable)
      if (
        error.message?.includes('fetch') ||
        error.code === 'ERR_NETWORK' ||
        !error.response
      ) {
        return false;
      }
      // Retry other errors up to 2 times
      return failureCount < 2;
    },
  });
}

/**
 * Fetch schemas for several Object Model database connections at once.
 * Report authoring uses this so a source-level connection switch can browse
 * that connection's schema list instead of the page default.
 */
export function useObjectSchemaDtosByConnectionIds(
  connectionIds: Array<string | null | undefined>
) {
  const auth = useAuth();
  const token = auth.user?.access_token;
  const uniqueConnectionIds = Array.from(
    new Set(connectionIds.filter((id): id is string => Boolean(id)))
  );

  const results = useQueries({
    queries: uniqueConnectionIds.map((connectionId) => ({
      queryKey: queryKeys.objects.schemas.all(connectionId),
      queryFn: () => getAllSchemas(token ?? '', connectionId),
      enabled: (!!token || !isOidcAuth) && Boolean(connectionId),
      refetchOnWindowFocus: false,
      placeholderData: keepPreviousData,
      retry: (
        failureCount: number,
        error: Error & { code?: string; response?: unknown }
      ) => {
        if (
          error.message?.includes('fetch') ||
          error.code === 'ERR_NETWORK' ||
          !error.response
        ) {
          return false;
        }
        return failureCount < 2;
      },
    })),
  });

  const schemasByConnectionId = Object.fromEntries(
    uniqueConnectionIds.map((connectionId, index) => [
      connectionId,
      results[index]?.data ?? [],
    ])
  ) as Record<string, Schema[]>;

  return {
    schemasByConnectionId,
    isFetching: results.some((result) => result.isFetching),
    isLoading: results.some((result) => result.isLoading),
  };
}

/**
 * Hook to fetch a single object schema by ID
 */
export function useObjectSchemaDtoById(
  id: string | undefined,
  connectionId?: string | null
) {
  return useCustomQuery<Schema | null>({
    queryKey: queryKeys.objects.schemas.byId(id || '', connectionId),
    queryFn: (token, context) => getSchemaById(token, context),
    enabled: !!id && !!connectionId,
  });
}

/**
 * Hook to create a new object schema
 */
export function useCreateObjectSchemaDto(connectionId?: string | null) {
  const queryClient = useQueryClient();

  return useCustomMutation<Schema, CreateSchemaRequest>({
    mutationFn: (token, objectSchemaDto) =>
      createSchemaWithConnection(token, objectSchemaDto, connectionId),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.schemas.all(connectionId),
      });
    },
  });
}

/**
 * Hook to update an existing object schema
 */
export function useUpdateObjectSchemaDto(connectionId?: string | null) {
  const queryClient = useQueryClient();

  return useCustomMutation<Schema, { id: string; data: UpdateSchemaRequest }>({
    mutationFn: (token, { id, data }) =>
      updateSchema(token, id, data, connectionId),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.schemas.all(connectionId),
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.schemas.byId(variables.id, connectionId),
      });
    },
  });
}

/**
 * Hook to delete an object schema
 */
export function useDeleteObjectSchema(connectionId?: string | null) {
  const queryClient = useQueryClient();

  return useCustomMutation<void, string>({
    mutationFn: (token, id) => deleteSchema(token, id, connectionId),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.schemas.all(connectionId),
      });
    },
  });
}
