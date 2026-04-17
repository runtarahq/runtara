/**
 * Centralized query key definitions for TanStack Query.
 *
 * Uses a factory pattern for type-safe, hierarchical query keys.
 * All features should use these keys instead of defining local ones.
 *
 * @example
 * // List all connections
 * queryKey: queryKeys.connections.all
 *
 * // Get single connection by ID
 * queryKey: queryKeys.connections.byId(connectionId)
 *
 * // Get connections filtered by operator
 * queryKey: queryKeys.connections.byOperator(operatorId)
 */

/**
 * Query key factories with hierarchical structure.
 * Provides type-safe query keys with proper invalidation patterns.
 */
export const queryKeys = {
  // Connections domain
  connections: {
    all: ['connections'] as const,
    lists: () => [...queryKeys.connections.all, 'list'] as const,
    list: (filters?: Record<string, unknown>) =>
      [...queryKeys.connections.lists(), filters] as const,
    details: () => [...queryKeys.connections.all, 'detail'] as const,
    byId: (id: string) => [...queryKeys.connections.details(), id] as const,
    byOperator: (operatorId: string) =>
      [...queryKeys.connections.all, 'byOperator', operatorId] as const,
    types: () => ['connectionTypes'] as const,
    categories: () => ['connectionCategories'] as const,
    schema: (id: string) =>
      [...queryKeys.connections.byId(id), 'schema'] as const,
  },

  // Triggers domain
  triggers: {
    all: ['triggers'] as const,
    lists: () => [...queryKeys.triggers.all, 'list'] as const,
    list: (filters?: Record<string, unknown>) =>
      [...queryKeys.triggers.lists(), filters] as const,
    details: () => [...queryKeys.triggers.all, 'detail'] as const,
    byId: (id: string) => [...queryKeys.triggers.details(), id] as const,
  },

  // Scenarios domain
  scenarios: {
    all: ['scenarios'] as const,
    lists: () => [...queryKeys.scenarios.all, 'list'] as const,
    list: (filters?: Record<string, unknown>) =>
      [...queryKeys.scenarios.lists(), filters] as const,
    details: () => [...queryKeys.scenarios.all, 'detail'] as const,
    byId: (id: string) => [...queryKeys.scenarios.details(), id] as const,
    workflow: (id: string, version?: number) =>
      version !== undefined
        ? ([...queryKeys.scenarios.byId(id), 'workflow', version] as const)
        : ([...queryKeys.scenarios.byId(id), 'workflow'] as const),
    versions: (id: string) =>
      [...queryKeys.scenarios.byId(id), 'versions'] as const,
    // All instances across all scenarios (for broad invalidation)
    allInstances: () => [...queryKeys.scenarios.all, 'instance'] as const,
    instances: (id: string) =>
      [...queryKeys.scenarios.byId(id), 'instances'] as const,
    instance: (scenarioId: string, instanceId: string) =>
      [...queryKeys.scenarios.instances(scenarioId), instanceId] as const,
    pendingInput: (scenarioId: string, instanceId: string) =>
      [
        ...queryKeys.scenarios.instance(scenarioId, instanceId),
        'pendingInput',
      ] as const,
    stepTypes: () => [...queryKeys.scenarios.all, 'stepTypes'] as const,
    logs: (instanceId: string) =>
      [...queryKeys.scenarios.all, 'logs', instanceId] as const,
    stepSubinstances: (instanceId: string, stepId: string) =>
      [
        ...queryKeys.scenarios.all,
        'stepSubinstances',
        instanceId,
        stepId,
      ] as const,
    stepEvents: (
      scenarioId: string | undefined,
      instanceId: string | undefined,
      stepId?: string
    ) =>
      [
        ...queryKeys.scenarios.all,
        'stepEvents',
        scenarioId,
        instanceId,
        stepId,
      ] as const,
    stepSummaries: (
      scenarioId: string,
      instanceId: string | null,
      filters?: unknown
    ) =>
      [
        ...queryKeys.scenarios.all,
        'stepSummaries',
        scenarioId,
        instanceId,
        filters,
      ] as const,
    invocationTriggers: (scenarioId: string) =>
      [...queryKeys.scenarios.byId(scenarioId), 'invocationTriggers'] as const,
    // Folder operations
    folders: () => [...queryKeys.scenarios.all, 'folders'] as const,
    inFolder: (path: string, includeSubfolders?: boolean) =>
      [
        ...queryKeys.scenarios.all,
        'inFolder',
        path,
        includeSubfolders,
      ] as const,
    // Chat operations
    chat: (scenarioId: string) =>
      [...queryKeys.scenarios.byId(scenarioId), 'chat'] as const,
    chatInstance: (scenarioId: string, instanceId: string) =>
      [...queryKeys.scenarios.chat(scenarioId), instanceId] as const,
  },

  // Executions domain (invocation history)
  executions: {
    all: ['executions'] as const,
    lists: () => [...queryKeys.executions.all, 'list'] as const,
    list: (params: {
      pageIndex?: number;
      pageSize?: number;
      filters?: unknown;
    }) => [...queryKeys.executions.lists(), params] as const,
  },

  // Objects domain (schemas and instances)
  objects: {
    all: ['objects'] as const,
    // Schema operations
    schemas: {
      all: () => [...queryKeys.objects.all, 'schemas'] as const,
      lists: () => [...queryKeys.objects.schemas.all(), 'list'] as const,
      list: (filters?: Record<string, unknown>) =>
        [...queryKeys.objects.schemas.lists(), filters] as const,
      details: () => [...queryKeys.objects.schemas.all(), 'detail'] as const,
      byId: (id: string) =>
        [...queryKeys.objects.schemas.details(), id] as const,
    },
    // Instance operations
    instances: {
      all: () => [...queryKeys.objects.all, 'instances'] as const,
      bySchema: (schemaId: string) =>
        [...queryKeys.objects.instances.all(), schemaId] as const,
      list: (
        schemaId: string,
        params?: {
          condition?: unknown;
          page?: number;
          size?: number;
          schemaName?: string;
          sortBy?: string[];
          sortOrder?: string[];
        }
      ) => [...queryKeys.objects.instances.bySchema(schemaId), params] as const,
      byId: (schemaId: string, instanceId: string) =>
        [
          ...queryKeys.objects.instances.bySchema(schemaId),
          instanceId,
        ] as const,
    },
  },

  // Files domain (S3-compatible storage)
  files: {
    all: ['files'] as const,
    buckets: (connectionId?: string) =>
      [...queryKeys.files.all, 'buckets', connectionId] as const,
    lists: () => [...queryKeys.files.all, 'list'] as const,
    list: (params?: {
      connectionId?: string;
      bucket?: string;
      prefix?: string;
      maxKeys?: number;
    }) => [...queryKeys.files.lists(), params] as const,
    details: () => [...queryKeys.files.all, 'detail'] as const,
    byKey: (bucket: string, key: string) =>
      [...queryKeys.files.details(), bucket, key] as const,
  },

  // Agents domain
  agents: {
    all: ['agents'] as const,
    lists: () => [...queryKeys.agents.all, 'list'] as const,
    details: () => [...queryKeys.agents.all, 'detail'] as const,
    byId: (id: string) => [...queryKeys.agents.details(), id] as const,
    withConnections: () =>
      [...queryKeys.agents.all, 'withConnections'] as const,
    connectionsByAgent: (agentId: string) =>
      [...queryKeys.agents.byId(agentId), 'connections'] as const,
    integrationEntities: (agentId: string) =>
      [...queryKeys.agents.byId(agentId), 'integrationEntities'] as const,
    test: (agentId: string) =>
      [...queryKeys.agents.byId(agentId), 'test'] as const,
  },

  // API Keys domain
  apiKeys: {
    all: ['apiKeys'] as const,
    lists: () => [...queryKeys.apiKeys.all, 'list'] as const,
  },

  // Integrations domain
  integrations: {
    all: ['integrations'] as const,
    lists: () => [...queryKeys.integrations.all, 'list'] as const,
    authRedirect: (params: Record<string, unknown>) =>
      [...queryKeys.integrations.all, 'authRedirect', params] as const,
  },

  // Analytics domain
  analytics: {
    all: ['analytics'] as const,
    tenant: (dateRange: string) =>
      [...queryKeys.analytics.all, 'tenant', dateRange] as const,
    scenario: (
      scenarioId: string,
      dateRange: string,
      version?: number,
      granularity?: string
    ) =>
      [
        ...queryKeys.analytics.all,
        'scenario',
        scenarioId,
        dateRange,
        version,
        granularity,
      ] as const,
    scenarioStats: (scenarioId: string, version?: number) =>
      [
        ...queryKeys.analytics.all,
        'scenarioStats',
        scenarioId,
        version,
      ] as const,
    sideEffects: (scenarioId?: string, version?: number) =>
      [...queryKeys.analytics.all, 'sideEffects', scenarioId, version] as const,
    system: () => [...queryKeys.analytics.all, 'system'] as const,
    rateLimitTimeline: (connectionId: string, dateRange: string) =>
      [
        ...queryKeys.analytics.all,
        'rateLimitTimeline',
        connectionId,
        dateRange,
      ] as const,
  },
} as const;

/** @lintignore Public type exported for consumer hooks needing the full queryKeys shape. */
export type QueryKeys = typeof queryKeys;
