import {
  ConnectionDto,
  ConnectionTypeDto,
} from '@/generated/RuntaraRuntimeApi';
import { getConnections } from '../queries';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery } from '@/shared/hooks/api';

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
