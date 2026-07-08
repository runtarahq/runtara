import * as RuntimeAPI from '@/generated/RuntaraRuntimeApi.ts';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

export async function getConnections(token: string) {
  try {
    // Fetch connections and connection types from Runtime API
    const [connectionsResult, connectionTypesResult] = await Promise.all([
      RuntimeREST.api.listConnectionsHandler(
        { includeRateLimitStats: true },
        createAuthHeaders(token)
      ),
      RuntimeREST.api.listConnectionTypesHandler(createAuthHeaders(token)),
    ]);

    const connections = connectionsResult.data.connections || [];
    const connectionTypes = connectionTypesResult.data.connectionTypes || [];

    // Create a map for quick lookup
    const connectionTypesMap = new Map(
      connectionTypes.map((ct) => [ct.integrationId, ct])
    );

    // Merge connection type data into connections
    const enrichedConnections = connections.map((connection) => {
      const connectionType = connection.integrationId
        ? connectionTypesMap.get(connection.integrationId) || null
        : null;
      return {
        ...connection,
        connectionType,
      };
    });

    return enrichedConnections;
  } catch (error) {
    console.error('Error fetching connections:', error);
    throw error;
  }
}

export async function getConnectionById(token: string, id: string) {
  try {
    const result = await RuntimeREST.api.getConnectionHandler(
      id,
      createAuthHeaders(token)
    );

    const connection = result.data.connection;

    // Fetch connection type data to enrich the connection
    if (connection?.integrationId) {
      try {
        const connectionTypesResult =
          await RuntimeREST.api.listConnectionTypesHandler(
            createAuthHeaders(token)
          );

        const connectionTypes =
          connectionTypesResult.data.connectionTypes || [];

        const connectionType = connectionTypes.find(
          (ct) => ct.integrationId === connection.integrationId
        );

        return {
          ...connection,
          connectionType: connectionType || null,
        };
      } catch (error) {
        console.error(
          '[getConnectionById] Error fetching connection type data:',
          error
        );
        return { ...connection, connectionType: null };
      }
    }

    return { ...connection, connectionType: null };
  } catch (error) {
    console.error('[getConnectionById] Error fetching connection:', error);
    throw error;
  }
}

export async function getConnectionsByOperator(
  token: string,
  operatorName: string
) {
  // Use the dedicated endpoint that filters connections by operator
  const result = await RuntimeREST.api.getConnectionsByOperatorHandler(
    operatorName,
    undefined,
    createAuthHeaders(token)
  );

  return result.data.connections || [];
}

export async function createConnection(
  token: string,
  connection: RuntimeAPI.CreateConnectionRequest
): Promise<string> {
  const result = await RuntimeREST.api.createConnectionHandler(
    connection,
    createAuthHeaders(token)
  );
  return result.data.connectionId;
}

export async function updateConnection(
  token: string,
  connection: {
    id: string;
    title?: string;
    parameters?: Record<string, unknown>;
    rateLimitConfig?: RuntimeAPI.RateLimitConfigDto | null;
    isDefaultFileStorage?: boolean;
    defaultFor?: string[];
  }
) {
  const {
    id,
    title,
    parameters,
    rateLimitConfig,
    isDefaultFileStorage,
    defaultFor,
  } = connection;

  const requestBody: RuntimeAPI.UpdateConnectionRequest = {
    title,
    connectionParameters: parameters,
    rateLimitConfig,
    isDefaultFileStorage,
    defaultFor,
  };

  await RuntimeREST.api.updateConnectionHandler(
    id,
    requestBody,
    createAuthHeaders(token)
  );
}

export async function removeConnection(token: string, connectionId: string) {
  await RuntimeREST.api.deleteConnectionHandler(
    connectionId,
    createAuthHeaders(token)
  );
}

export async function getOAuthAuthorizeUrl(
  token: string | null | undefined,
  connectionId: string
): Promise<string> {
  // Use the shared header builder: it omits Authorization when there is no
  // token (local / trust_proxy auth modes), matching every other runtime call.
  const result = await RuntimeREST.instance.get(
    `/api/runtime/connections/${connectionId}/oauth/authorize`,
    createAuthHeaders(token)
  );
  return result.data.authorizationUrl;
}

export async function getConnectionTypes(token: string) {
  const result = await RuntimeREST.api.listConnectionTypesHandler(
    createAuthHeaders(token)
  );

  return result.data.connectionTypes || [];
}
