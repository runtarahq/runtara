import { useQueryClient } from '@tanstack/react-query';
import {
  bulkDeleteInstances,
  bulkCreateInstances,
  bulkUpdateInstancesByIds,
  createInstance,
  getInstanceById,
  getInstancesBySchema,
  updateInstance,
  exportCsv,
  importCsvPreview,
  importCsv,
  type BulkCreateOptions,
  type BulkCreateResult,
} from '../queries';
import {
  Instance,
  CreateInstanceRequest,
  UpdateInstanceRequest,
  CsvExportRequest,
  CsvImportJsonRequest,
  CsvPreviewJsonRequest,
  CsvImportResponse,
  ImportPreviewResponse,
} from '@/generated/RuntaraRuntimeApi';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';

// Define PageObjectInstance locally to match the old API structure
interface PageObjectInstance {
  content: Instance[];
  totalPages: number;
  totalElements: number;
  number: number;
}

/**
 * Hook to fetch all records for a specific object schema with pagination
 */
export function useObjectInstanceDtos(
  schemaId: string | undefined,
  schemaName: string | undefined,
  condition: unknown = null,
  page: number = 0,
  size: number = 20,
  sortBy?: string[],
  sortOrder?: string[]
) {
  return useCustomQuery<PageObjectInstance>({
    queryKey: queryKeys.objects.instances.list(schemaId || '', {
      condition,
      page,
      size,
      schemaName,
      sortBy,
      sortOrder,
    }),
    queryFn: (token, context) => getInstancesBySchema(token, context),
    enabled: !!schemaId,
  });
}

/**
 * Hook to fetch a single record by ID
 */
export function useObjectInstanceDto(
  schemaId: string | undefined,
  instanceId: string | undefined
) {
  return useCustomQuery<Instance | null>({
    queryKey: queryKeys.objects.instances.byId(
      schemaId || '',
      instanceId || ''
    ),
    queryFn: (token, context) => getInstanceById(token, context),
    enabled: !!instanceId,
  });
}

/**
 * Hook to create a new record
 */
export function useCreateObjectInstanceDto() {
  const queryClient = useQueryClient();

  return useCustomMutation<
    Instance,
    { schemaId: string; data: CreateInstanceRequest }
  >({
    mutationFn: (token, { data }) => createInstance(token, data),
    onSuccess: (newInstance, variables) => {
      // Optimistically add the new instance to all matching queries
      queryClient.setQueriesData<PageObjectInstance>(
        { queryKey: queryKeys.objects.instances.bySchema(variables.schemaId) },
        (oldData) => {
          if (!oldData) return oldData;

          return {
            ...oldData,
            content: [...oldData.content, newInstance],
            totalElements: oldData.totalElements + 1,
          };
        }
      );
    },
  });
}

/**
 * Hook to update an existing record with optimistic updates
 */
export function useUpdateObjectInstanceDto() {
  const queryClient = useQueryClient();

  return useCustomMutation<
    Instance,
    { schemaId: string; instanceId: string; data: UpdateInstanceRequest }
  >({
    mutationFn: (token, { schemaId, instanceId, data }) =>
      updateInstance(token, schemaId, instanceId, data),
    onMutate: async ({ schemaId, instanceId, data }) => {
      // Cancel any outgoing refetches for all queries matching this schema
      await queryClient.cancelQueries({
        queryKey: queryKeys.objects.instances.bySchema(schemaId),
      });

      // Snapshot previous data for rollback
      const previousData = queryClient.getQueriesData<PageObjectInstance>({
        queryKey: queryKeys.objects.instances.bySchema(schemaId),
      });

      // Update all queries that match this schema ID
      queryClient.setQueriesData<PageObjectInstance>(
        { queryKey: queryKeys.objects.instances.bySchema(schemaId) },
        (oldData) => {
          if (!oldData) return oldData;

          return {
            ...oldData,
            content: oldData.content.map((item) =>
              item.id === instanceId
                ? {
                    ...item,
                    properties: { ...item.properties, ...data.properties },
                  }
                : item
            ),
          };
        }
      );

      return { previousData } as {
        previousData: [unknown, PageObjectInstance | undefined][];
      };
    },
    onSuccess: (returnedData, variables) => {
      // Update all matching queries with the server response
      queryClient.setQueriesData<PageObjectInstance>(
        { queryKey: queryKeys.objects.instances.bySchema(variables.schemaId) },
        (oldData) => {
          if (!oldData) return oldData;

          return {
            ...oldData,
            content: oldData.content.map((item) =>
              item.id === variables.instanceId ? returnedData : item
            ),
          };
        }
      );
    },
    onError: (_err, variables, onMutateResult) => {
      // Restore previous data from snapshot for immediate rollback
      const ctx = onMutateResult as
        | {
            previousData?: [
              readonly unknown[],
              PageObjectInstance | undefined,
            ][];
          }
        | undefined;
      if (ctx?.previousData) {
        for (const [queryKey, data] of ctx.previousData) {
          queryClient.setQueryData(queryKey, data);
        }
      }
      // Also refetch to ensure consistency
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.instances.bySchema(variables.schemaId),
      });
    },
  });
}

/**
 * Hook to bulk delete records
 */
export function useBulkDeleteObjectInstances() {
  const queryClient = useQueryClient();

  return useCustomMutation<number, { schemaId: string; instanceIds: string[] }>(
    {
      mutationFn: (token, { schemaId, instanceIds }) =>
        bulkDeleteInstances(token, schemaId, instanceIds),
      onSuccess: (_, variables) => {
        queryClient.invalidateQueries({
          queryKey: queryKeys.objects.instances.bySchema(variables.schemaId),
        });
      },
    }
  );
}

/**
 * Hook to bulk-insert records from a JSON array with opt-in conflict + error
 * handling.
 */
export function useBulkCreateObjectInstances() {
  const queryClient = useQueryClient();

  return useCustomMutation<
    BulkCreateResult,
    {
      schemaId: string;
      instances: unknown[];
      options: BulkCreateOptions;
    }
  >({
    mutationFn: (token, { schemaId, instances, options }) =>
      bulkCreateInstances(token, schemaId, instances, options),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.instances.bySchema(variables.schemaId),
      });
    },
  });
}

/**
 * Hook to bulk-update a set of instances by ID with a shared property payload.
 */
export function useBulkUpdateObjectInstances() {
  const queryClient = useQueryClient();

  return useCustomMutation<
    number,
    {
      schemaId: string;
      instanceIds: string[];
      properties: Record<string, unknown>;
    }
  >({
    mutationFn: (token, { schemaId, instanceIds, properties }) =>
      bulkUpdateInstancesByIds(token, schemaId, instanceIds, properties),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.instances.bySchema(variables.schemaId),
      });
    },
  });
}

/**
 * Hook to export instances as CSV
 */
export function useExportCsv() {
  return useCustomMutation<
    Blob,
    { schemaName: string; data: CsvExportRequest }
  >({
    mutationFn: (token, { schemaName, data }) =>
      exportCsv(token, schemaName, data),
  });
}

/**
 * Hook to preview CSV import (parse headers, suggest column mappings)
 */
export function useImportCsvPreview() {
  return useCustomMutation<
    ImportPreviewResponse,
    { schemaName: string; data: CsvPreviewJsonRequest }
  >({
    mutationFn: (token, { schemaName, data }) =>
      importCsvPreview(token, schemaName, data),
  });
}

/**
 * Hook to import CSV data
 */
export function useImportCsv() {
  const queryClient = useQueryClient();

  return useCustomMutation<
    CsvImportResponse,
    { schemaId: string; schemaName: string; data: CsvImportJsonRequest }
  >({
    mutationFn: (token, { schemaName, data }) =>
      importCsv(token, schemaName, data),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.objects.instances.bySchema(variables.schemaId),
      });
    },
  });
}
