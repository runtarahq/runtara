import { useQueryClient } from '@tanstack/react-query';
import {
  Schema,
  CreateSchemaRequest,
  UpdateSchemaRequest,
} from '@/generated/RuntaraRuntimeApi';
import {
  createSchema,
  deleteSchema,
  getAllSchemas,
  getSchemaById,
  updateSchema,
} from '../queries';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';

/**
 * Hook to fetch all object schemas
 */
export function useObjectSchemaDtos() {
  return useCustomQuery<Schema[]>({
    queryKey: queryKeys.objects.schemas.all(),
    queryFn: (token) => getAllSchemas(token),
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
 * Hook to fetch a single object schema by ID
 */
export function useObjectSchemaDtoById(id: string | undefined) {
  return useCustomQuery<Schema | null>({
    queryKey: queryKeys.objects.schemas.byId(id || ''),
    queryFn: (token, context) => getSchemaById(token, context),
    enabled: !!id,
  });
}

/**
 * Hook to create a new object schema
 */
export function useCreateObjectSchemaDto() {
  const queryClient = useQueryClient();

  return useCustomMutation<Schema, CreateSchemaRequest>({
    mutationFn: (token, objectSchemaDto) =>
      createSchema(token, objectSchemaDto),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.schemas.all(),
      });
    },
  });
}

/**
 * Hook to update an existing object schema
 */
export function useUpdateObjectSchemaDto() {
  const queryClient = useQueryClient();

  return useCustomMutation<Schema, { id: string; data: UpdateSchemaRequest }>({
    mutationFn: (token, { id, data }) => updateSchema(token, id, data),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.schemas.all(),
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.schemas.byId(variables.id),
      });
    },
  });
}

/**
 * Hook to delete an object schema
 */
export function useDeleteObjectSchema() {
  const queryClient = useQueryClient();

  return useCustomMutation<void, string>({
    mutationFn: (token, id) => deleteSchema(token, id),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.schemas.all(),
      });
    },
  });
}
