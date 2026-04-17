import { useQueryClient } from '@tanstack/react-query';
import { EnrichedTrigger } from '../types';
import {
  getInvocationTriggers,
  getInvocationTriggerById,
  createInvocationTrigger,
  updateInvocationTrigger,
  removeInvocationTrigger,
} from '../queries';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';

/**
 * Hook to fetch all invocation triggers
 */
export function useTriggers() {
  return useCustomQuery<EnrichedTrigger[]>({
    queryKey: queryKeys.triggers.all,
    queryFn: (token) => getInvocationTriggers(token),
  });
}

/**
 * Hook to fetch a single trigger by ID
 */
export function useTriggerById(id: string | undefined) {
  return useCustomQuery<EnrichedTrigger | null>({
    queryKey: queryKeys.triggers.byId(id || ''),
    queryFn: (token, context) => getInvocationTriggerById(token, context),
    enabled: !!id,
  });
}

/**
 * Hook to create a new trigger
 */
export function useCreateTrigger() {
  const queryClient = useQueryClient();

  return useCustomMutation<void, Partial<EnrichedTrigger>>({
    mutationFn: (token, trigger) => createInvocationTrigger(token, trigger),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.triggers.all,
      });
    },
  });
}

/**
 * Hook to update an existing trigger
 */
export function useUpdateTrigger() {
  const queryClient = useQueryClient();

  return useCustomMutation<void, Partial<EnrichedTrigger> & { id: string }>({
    mutationFn: (token, trigger) => updateInvocationTrigger(token, trigger),
    onSuccess: (_, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.triggers.all,
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.triggers.byId(variables.id),
      });
    },
  });
}

/**
 * Hook to delete a trigger
 */
export function useDeleteTrigger() {
  const queryClient = useQueryClient();

  return useCustomMutation<void, string>({
    mutationFn: (token, triggerId) => removeInvocationTrigger(token, triggerId),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.triggers.all,
      });
    },
  });
}
