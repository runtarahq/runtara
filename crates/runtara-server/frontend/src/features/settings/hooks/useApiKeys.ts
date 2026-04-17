import { useQueryClient } from '@tanstack/react-query';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { listApiKeys, createApiKey, revokeApiKey } from '../queries';
import type {
  ApiKey,
  CreateApiKeyRequest,
  CreateApiKeyResponse,
} from '@/generated/RuntaraRuntimeApi';

export function useApiKeys() {
  return useCustomQuery<ApiKey[]>({
    queryKey: queryKeys.apiKeys.all,
    queryFn: listApiKeys,
  });
}

export function useCreateApiKey() {
  const queryClient = useQueryClient();
  return useCustomMutation<CreateApiKeyResponse, CreateApiKeyRequest>({
    mutationFn: createApiKey,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.apiKeys.all });
    },
  });
}

export function useRevokeApiKey() {
  const queryClient = useQueryClient();
  return useCustomMutation<void, string>({
    mutationFn: revokeApiKey,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.apiKeys.all });
    },
  });
}
