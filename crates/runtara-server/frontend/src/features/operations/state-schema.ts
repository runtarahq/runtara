import { useCustomQuery } from '@/shared/hooks/api';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { queryKeys } from '@/shared/queries/query-keys';
import type { StateField } from './components/StateValue';

/** The state schema of a workflow version: field labels, formats and order. */
export function useVersionStateSchema(workflowId: string, version: number) {
  const { data, error } = useCustomQuery({
    queryKey: queryKeys.workflows.schemas(workflowId, version),
    queryFn: async (token: string) =>
      (
        await RuntimeREST.api.getVersionSchemasHandler(
          workflowId,
          version,
          createAuthHeaders(token)
        )
      ).data,
    enabled: Boolean(workflowId) && version > 0,
    placeholderData: undefined,
  });
  return {
    schema: (data?.stateSchema ?? {}) as Record<string, StateField>,
    error,
  };
}
