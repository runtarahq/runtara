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

  // Workflows domain
  workflows: {
    all: ['workflows'] as const,
    lists: () => [...queryKeys.workflows.all, 'list'] as const,
    list: (filters?: Record<string, unknown>) =>
      [...queryKeys.workflows.lists(), filters] as const,
    details: () => [...queryKeys.workflows.all, 'detail'] as const,
    byId: (id: string) => [...queryKeys.workflows.details(), id] as const,
    workflow: (id: string, version?: number) =>
      version !== undefined
        ? ([...queryKeys.workflows.byId(id), 'workflow', version] as const)
        : ([...queryKeys.workflows.byId(id), 'workflow'] as const),
    versions: (id: string) =>
      [...queryKeys.workflows.byId(id), 'versions'] as const,
    // All instances across all workflows (for broad invalidation)
    allInstances: () => [...queryKeys.workflows.all, 'instance'] as const,
    instances: (id: string) =>
      [...queryKeys.workflows.byId(id), 'instances'] as const,
    instance: (workflowId: string, instanceId: string) =>
      [...queryKeys.workflows.instances(workflowId), instanceId] as const,
    pendingInput: (workflowId: string, instanceId: string) =>
      [
        ...queryKeys.workflows.instance(workflowId, instanceId),
        'pendingInput',
      ] as const,
    stepTypes: () => [...queryKeys.workflows.all, 'stepTypes'] as const,
    logs: (instanceId: string) =>
      [...queryKeys.workflows.all, 'logs', instanceId] as const,
    stepSubinstances: (instanceId: string, stepId: string) =>
      [
        ...queryKeys.workflows.all,
        'stepSubinstances',
        instanceId,
        stepId,
      ] as const,
    stepEvents: (
      workflowId: string | undefined,
      instanceId: string | undefined,
      stepId?: string
    ) =>
      [
        ...queryKeys.workflows.all,
        'stepEvents',
        workflowId,
        instanceId,
        stepId,
      ] as const,
    stepSummaries: (
      workflowId: string,
      instanceId: string | null,
      filters?: unknown
    ) =>
      [
        ...queryKeys.workflows.all,
        'stepSummaries',
        workflowId,
        instanceId,
        filters,
      ] as const,
    invocationTriggers: (workflowId: string) =>
      [...queryKeys.workflows.byId(workflowId), 'invocationTriggers'] as const,
    // Folder operations
    folders: () => [...queryKeys.workflows.all, 'folders'] as const,
    inFolder: (path: string, includeSubfolders?: boolean) =>
      [
        ...queryKeys.workflows.all,
        'inFolder',
        path,
        includeSubfolders,
      ] as const,
    // Chat operations
    chat: (workflowId: string) =>
      [...queryKeys.workflows.byId(workflowId), 'chat'] as const,
    chatInstance: (workflowId: string, instanceId: string) =>
      [...queryKeys.workflows.chat(workflowId), instanceId] as const,
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

  // Reports domain
  reports: {
    all: ['reports'] as const,
    lists: () => [...queryKeys.reports.all, 'list'] as const,
    details: () => [...queryKeys.reports.all, 'detail'] as const,
    byId: (id: string) => [...queryKeys.reports.details(), id] as const,
    render: (id: string, request: unknown) =>
      [...queryKeys.reports.byId(id), 'render', request] as const,
    block: (id: string, blockId: string, request: unknown) =>
      [...queryKeys.reports.byId(id), 'block', blockId, request] as const,
    filterOptions: (id: string, filterId: string, request: unknown) =>
      [
        ...queryKeys.reports.byId(id),
        'filterOptions',
        filterId,
        request,
      ] as const,
    lookupOptions: (
      id: string,
      blockId: string,
      field: string,
      request: unknown
    ) =>
      [
        ...queryKeys.reports.byId(id),
        'lookupOptions',
        blockId,
        field,
        request,
      ] as const,
    dataset: (id: string, datasetId: string, request: unknown) =>
      [...queryKeys.reports.byId(id), 'dataset', datasetId, request] as const,
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

  // LLM model metadata
  llmModels: {
    all: ['llmModels'] as const,
    byProvider: (provider: string) =>
      [...queryKeys.llmModels.all, provider] as const,
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
    workflow: (
      workflowId: string,
      dateRange: string,
      version?: number,
      granularity?: string
    ) =>
      [
        ...queryKeys.analytics.all,
        'workflow',
        workflowId,
        dateRange,
        version,
        granularity,
      ] as const,
    workflowStats: (workflowId: string, version?: number) =>
      [
        ...queryKeys.analytics.all,
        'workflowStats',
        workflowId,
        version,
      ] as const,
    sideEffects: (workflowId?: string, version?: number) =>
      [...queryKeys.analytics.all, 'sideEffects', workflowId, version] as const,
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
