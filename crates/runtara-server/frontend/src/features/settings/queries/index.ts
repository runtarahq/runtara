import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { CreateApiKeyRequest } from '@/generated/RuntaraRuntimeApi';

export async function listApiKeys(token: string) {
  const result = await RuntimeREST.api.listApiKeys(createAuthHeaders(token));
  return result.data;
}

export async function createApiKey(token: string, data: CreateApiKeyRequest) {
  const result = await RuntimeREST.api.createApiKey(
    data,
    createAuthHeaders(token)
  );
  return result.data;
}

export async function revokeApiKey(token: string, id: string) {
  const result = await RuntimeREST.api.revokeApiKey(
    id,
    createAuthHeaders(token)
  );
  return result.data;
}
