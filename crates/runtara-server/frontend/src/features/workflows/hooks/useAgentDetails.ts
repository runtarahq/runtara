import { useQueries } from '@tanstack/react-query';
import { queryKeys } from '@/shared/queries/query-keys';
import { useToken } from '@/shared/hooks';
import { getAgentDetails } from '@/features/workflows/queries';
import { AgentInfo } from '@/generated/RuntaraRuntimeApi';

interface UseMultipleAgentDetailsOptions {
  /** Whether the queries should be enabled */
  enabled?: boolean;
  /** Stale time in milliseconds (default: 5 minutes) */
  staleTime?: number;
}

interface UseMultipleAgentDetailsResult {
  /** Map of agent ID to agent details */
  agentDetailsMap: Map<string, AgentInfo>;
  /** Whether all agents have finished loading */
  allLoaded: boolean;
  /** Whether any agents are currently loading */
  isLoading: boolean;
}

/**
 * Hook to fetch details for multiple agents in parallel.
 * Uses the existing getAgentDetails query function.
 *
 * @param agentIds - Array of agent IDs to fetch details for
 * @param options - Query options
 * @returns Object with agentDetailsMap, allLoaded, and isLoading states
 */
export function useMultipleAgentDetails(
  agentIds: string[],
  options: UseMultipleAgentDetailsOptions = {}
): UseMultipleAgentDetailsResult {
  const { enabled = true, staleTime = 5 * 60 * 1000 } = options;
  const token = useToken();

  const agentQueries = useQueries({
    queries: agentIds.map((agentId) => ({
      queryKey: queryKeys.agents.byId(agentId),
      queryFn: () => getAgentDetails(token!, agentId),
      enabled: enabled && !!token && !!agentId,
      staleTime,
    })),
  });

  const allLoaded = agentQueries.every((q) => !q.isLoading);
  const isLoading = agentQueries.some((q) => q.isLoading);

  const agentDetailsMap = new Map<string, AgentInfo>();
  agentQueries.forEach((query, index) => {
    const agentId = agentIds[index];
    if (agentId && query.data) {
      agentDetailsMap.set(agentId, query.data as AgentInfo);
    }
  });

  return {
    agentDetailsMap,
    allLoaded,
    isLoading,
  };
}
