import { useEffect, useMemo } from 'react';
import { useSearchParams } from 'react-router';
import { useConnections } from '@/features/connections/hooks/useConnections';

const OBJECT_MODEL_DEFAULT_FOR = 'object_model';
const OBJECT_MODEL_INTEGRATION_ID = 'postgres';

export function useObjectModelConnectionSelection() {
  const [searchParams, setSearchParams] = useSearchParams();
  const connectionsQuery = useConnections();

  const connections = useMemo(
    () =>
      (connectionsQuery.data ?? []).filter(
        (connection) => connection.integrationId === OBJECT_MODEL_INTEGRATION_ID
      ),
    [connectionsQuery.data]
  );

  const queryConnectionId = searchParams.get('connectionId');
  const defaultConnection = connections.find((connection) =>
    connection.defaultFor?.includes(OBJECT_MODEL_DEFAULT_FOR)
  );
  const selectedConnection =
    connections.find((connection) => connection.id === queryConnectionId) ??
    defaultConnection ??
    connections[0] ??
    null;

  const selectedConnectionId = selectedConnection?.id ?? null;

  useEffect(() => {
    if (queryConnectionId || !selectedConnectionId) return;
    setSearchParams(
      (current) => {
        const next = new URLSearchParams(current);
        next.set('connectionId', selectedConnectionId);
        return next;
      },
      { replace: true }
    );
  }, [queryConnectionId, selectedConnectionId, setSearchParams]);

  const setSelectedConnectionId = (connectionId: string) => {
    setSearchParams((current) => {
      const next = new URLSearchParams(current);
      if (connectionId) {
        next.set('connectionId', connectionId);
      } else {
        next.delete('connectionId');
      }
      return next;
    });
  };

  const connectionQuery = selectedConnectionId
    ? `?connectionId=${encodeURIComponent(selectedConnectionId)}`
    : '';

  return {
    connections,
    selectedConnection,
    selectedConnectionId,
    setSelectedConnectionId,
    connectionQuery,
    isLoading: connectionsQuery.isLoading,
    isError: connectionsQuery.isError,
    error: connectionsQuery.error,
  };
}
