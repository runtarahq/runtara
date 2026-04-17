import {
  ApiResponseScenarioDto,
  CapabilityInfo,
  FoldersResponse,
  MoveScenarioRequest,
  RenameFolderRequest,
  ScenarioInstanceDto,
} from '@/generated/RuntaraRuntimeApi.ts';
import { executionGraphToReactFlow } from '@/features/scenarios/components/WorkflowEditor/CustomNodes/utils.tsx';
import { ExecutionGraphDto } from '@/features/scenarios/types/execution-graph';
import { parseSchema } from '@/features/scenarios/utils/schema';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders, getRuntimeBaseUrl } from '@/shared/queries/utils';

/** @lintignore Public query-context type kept for consumer use in useQueries callbacks. */
export interface AgentDetailsQueryContext {
  queryKey: readonly string[]; // flexible for useQueries
}

export interface StepEventsFilters {
  stepId?: string;
  eventType?: string;
  subtype?: string;
  limit?: number;
  offset?: number;
  sortOrder?: 'asc' | 'desc';
}

export interface StepSummariesFilters {
  limit?: number;
  offset?: number;
  sortOrder?: 'asc' | 'desc';
  status?: 'running' | 'completed' | 'failed';
  stepType?: string;
  scopeId?: string;
  parentScopeId?: string;
  rootScopesOnly?: boolean;
}

/**
 * Extended agent type that includes Runtime API fields and
 * transforms capabilities array to a Record for easier lookup.
 */
export interface ExtendedAgent {
  id: string;
  name: string;
  description: string;
  supportsConnections: boolean;
  integrationIds: string[];
  /** Capabilities indexed by capability ID for O(1) lookup */
  supportedCapabilities: Record<string, CapabilityInfo>;
}

export async function getScenarios(token: string) {
  const result = await RuntimeREST.api.listScenariosHandler(
    { recursive: true, pageSize: 100 }, // Use max page size to get all scenarios for dropdowns
    createAuthHeaders(token)
  );

  // Return the full wrapped response (ApiResponsePageScenarioDto)
  // The response structure is: { data: { content: ScenarioDto[], totalElements, ... }, message, success }
  return result.data;
}

export async function createScenario(
  token: string,
  scenario: { name: string; description?: string }
) {
  const result = await RuntimeREST.api.createScenarioHandler(
    {
      name: scenario.name,
      description: scenario.description || '', // Runtime API requires description
    },
    createAuthHeaders(token)
  );

  // Return the full wrapped response (ApiResponseScenarioDto)
  // The response structure is: { data: ScenarioDto, message: string, success: boolean }
  return result.data;
}

export async function getScenario(token: string, scenarioId: string) {
  const result = await RuntimeREST.api.getScenarioHandler(
    scenarioId,
    undefined,
    createAuthHeaders(token)
  );

  // Return the full wrapped response { data: ScenarioDto, message, success }
  // UI components will extract the data
  return result.data;
}

// Extended type to include both instance data and metadata
export interface ScenarioInstanceWithMetadata extends ScenarioInstanceDto {
  metadata?: {
    scenarioName?: string;
    scenarioDescription?: string;
    startedAt?: string;
    completedAt?: string;
    errorMessage?: string | null;
    retryCount?: number;
    maxRetries?: number;
    workerId?: string;
    heartbeatAt?: string;
    additionalMetadata?: Record<string, unknown>;
  };
}

export async function getScenarioInstance(
  token: string,
  scenarioId: string,
  instanceId: string
): Promise<ScenarioInstanceWithMetadata> {
  const result = await RuntimeREST.api.getInstanceHandler(
    scenarioId,
    instanceId,
    createAuthHeaders(token)
  );

  // The axios response is { data: { data: { instance, metadata }, message, success } }
  // result.data gives us { data: { instance, metadata }, message, success }
  // We need result.data.data to get { instance, metadata }
  // Note: The generated types may not reflect the actual API response structure,
  // so we cast through unknown to handle the actual response format
  const wrappedResponse = result.data as unknown as {
    data: {
      instance: ScenarioInstanceDto;
      metadata?: ScenarioInstanceWithMetadata['metadata'];
    };
    message: string;
    success: boolean;
  };

  // Extract instance and metadata from the nested response
  const instanceData = wrappedResponse.data.instance;
  const metadata = wrappedResponse.data.metadata;

  // Merge metadata into instance data for easier access
  return {
    ...instanceData,
    metadata,
  };
}

// Using the ScenarioVersionInfoDto from the Runtime API client
export type { ScenarioVersionInfoDto } from '@/generated/RuntaraRuntimeApi.ts';

export async function getScenarioVersions(token: string, scenarioId: string) {
  const result = await RuntimeREST.api.listScenarioVersionsHandler(
    scenarioId,
    createAuthHeaders(token)
  );

  // Return the full wrapped response { data: ScenarioVersionInfoDto[], message, success }
  // UI components will extract the data array
  return result.data;
}

export async function getScenarioWorkflow(
  token: string,
  scenarioId: string,
  versionNumber?: number
) {
  const result = await RuntimeREST.api.getScenarioHandler(
    scenarioId,
    versionNumber ? { versionNumber } : undefined,
    createAuthHeaders(token)
  );

  // Runtime API returns wrapped response: ApiResponseScenarioDto
  // Extract scenario data for workflow processing
  const responseData = result.data as ApiResponseScenarioDto;
  const scenarioData = responseData.data;

  const { executionGraph = {} } = scenarioData;

  // Backend now always provides Start and Finish steps
  const { nodes, edges } = executionGraphToReactFlow(executionGraph);

  // Parse variables from API format to array format used by the UI
  // Variables are now inside executionGraph (moved from scenario root)
  // API returns variables as an object: { varName: { type, value }, ... }
  // UI expects: [{ name, value, type }, ...]
  const variablesObj = (executionGraph.variables || {}) as Record<
    string,
    { type?: string; value?: unknown } | unknown
  >;
  const variables = Object.entries(variablesObj).map(([name, val]) => {
    const varObj = val as { type?: string; value?: unknown } | undefined;
    return {
      name,
      value: varObj?.value ?? val ?? '',
      type: varObj?.type ?? 'string',
    };
  });

  // Parse input and output schemas to field arrays
  // Schemas are now inside executionGraph (moved from scenario root)
  const inputSchemaFields = parseSchema(executionGraph.inputSchema);
  const outputSchemaFields = parseSchema(executionGraph.outputSchema);

  // Extract executionTimeoutSeconds from executionGraph (moved from scenario root)
  const executionTimeoutSeconds = executionGraph.executionTimeoutSeconds;

  // Extract rateLimitBudgetMs from executionGraph
  const rateLimitBudgetMs = executionGraph.rateLimitBudgetMs;

  // Name and description are now inside executionGraph (moved from scenario root).
  // Prefer executionGraph values; fall back to legacy top-level fields so scenarios
  // created before the migration still display correctly.
  const name = executionGraph.name ?? scenarioData.name ?? '';
  const description =
    executionGraph.description ?? scenarioData.description ?? '';

  // Return wrapped response with processed workflow
  return {
    data: {
      ...scenarioData,
      name,
      description,
      nodes,
      edges,
      variables,
      inputSchemaFields,
      outputSchemaFields,
      executionTimeoutSeconds,
      rateLimitBudgetMs,
    },
    message: responseData?.message,
    success: responseData?.success,
  };
}

export async function updateScenario(
  token: string,
  newScenario: {
    id: string;
    data: ExecutionGraphDto; // name and description are now inside the execution graph
  }
) {
  const { id, data } = newScenario;

  const result = await RuntimeREST.api.updateScenarioHandler(
    id,
    {
      executionGraph: data,
    },
    createAuthHeaders(token)
  );

  // Return the full wrapped response (ApiResponseScenarioDto)
  // The response structure is: { data: ScenarioDto, message: string, success: boolean }
  return result.data;
}

export async function removeScenario(token: string, scenarioId: string) {
  await RuntimeREST.api.deleteScenarioHandler(
    scenarioId,
    createAuthHeaders(token)
  );
}

export async function scheduleScenario(
  token: string,
  scenarioId: string,
  inputs?: Record<string, unknown>,
  version?: number,
  debug?: boolean
) {
  // API expects inputs in format: { data: {...}, variables: {...} }
  const formattedInputs = {
    data: inputs || {},
    variables: {},
  };

  const result = await RuntimeREST.api.executeScenarioHandler(
    scenarioId,
    { inputs: formattedInputs, ...(debug ? { debug: true } : {}) },
    version !== undefined ? { version } : undefined,
    createAuthHeaders(token)
  );

  // Return simplified response (ExecuteScenarioResponse now only has instanceId and status)
  return result.data;
}

export async function resumeInstance(token: string, instanceId: string) {
  const result = await RuntimeREST.api.resumeInstanceHandler(
    instanceId,
    createAuthHeaders(token)
  );

  return result.data;
}

export async function cloneScenario(
  token: string,
  scenarioId: string,
  name?: string
) {
  const result = await RuntimeREST.api.cloneScenarioHandler(
    scenarioId,
    { name: name || `Copy ${Date.now()}` },
    createAuthHeaders(token)
  );

  return result.data;
}

export async function setCurrentVersion(
  token: string,
  params: { scenarioId: string; versionNumber: number }
) {
  const { scenarioId, versionNumber } = params;
  const result = await RuntimeREST.api.setCurrentVersionHandler(
    scenarioId,
    versionNumber,
    createAuthHeaders(token)
  );

  return result.data;
}

export async function getScenarioStepTypes(token: string) {
  const result = await RuntimeREST.api.listStepTypesHandler(
    createAuthHeaders(token)
  );

  // Return the full wrapped response { step_types: StepTypeInfo[] }
  // UI components will extract the step_types array
  return result.data;
}

// Helper function to fetch agent details using path parameter
async function fetchAgentDetails(token: string, agentId: string) {
  const url = `${getRuntimeBaseUrl()}/agents/${encodeURIComponent(agentId)}`;
  console.log('[fetchAgentDetails] URL:', url, 'agentId:', agentId);
  const response = await fetch(url, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch agent details: ${response.statusText}`);
  }

  return response.json();
}

export async function getAgents(token: string) {
  const result = await RuntimeREST.api.listAgentsHandler(
    createAuthHeaders(token)
  );

  // Fetch full details for each agent to get capability schemas
  // Use agent ID (not name) as the API identifier
  const agentDetailsPromises = result.data.agents.map((agentSummary) =>
    fetchAgentDetails(token, agentSummary.id)
  );

  const agentsWithDetails = await Promise.all(agentDetailsPromises);

  // Convert AgentInfo[] to ExtendedAgent[] format
  const agents: ExtendedAgent[] = agentsWithDetails.map((agentInfo) => {
    // Convert capabilities array to supportedCapabilities Record for O(1) lookup
    const supportedCapabilities: Record<string, CapabilityInfo> = {};

    for (const capability of agentInfo.capabilities) {
      supportedCapabilities[capability.id] = capability;
    }

    return {
      id: agentInfo.id,
      name: agentInfo.name,
      description: agentInfo.description,
      supportsConnections: agentInfo.supportsConnections,
      integrationIds: agentInfo.integrationIds,
      supportedCapabilities,
    };
  });

  return { agents };
}

export async function getAgentDetails(token: string, agentId: string) {
  if (!agentId) {
    return null;
  }

  // Return the AgentInfo with full details including capabilities
  return fetchAgentDetails(token, agentId);
}

export async function replayScenario(token: string, instanceId: string) {
  const result = await RuntimeREST.api.replayInstanceHandler(
    instanceId,
    createAuthHeaders(token)
  );

  return result.data;
}

export async function stopInstance(token: string, instanceId: string) {
  const result = await RuntimeREST.api.stopInstanceHandler(
    instanceId,
    createAuthHeaders(token)
  );

  return result.data;
}

export async function testAgent(
  token: string,
  agentId: string,
  capabilityId: string,
  input: Record<string, unknown>,
  connectionId?: string
) {
  const result = await RuntimeREST.api.testAgentHandler(
    agentId,
    capabilityId,
    { input, connectionId },
    createAuthHeaders(token)
  );

  return result.data;
}

export async function getStepEvents(
  token: string,
  scenarioId: string,
  instanceId: string,
  filters?: StepEventsFilters
) {
  const result = await RuntimeREST.api.getStepEvents(
    scenarioId,
    instanceId,
    filters ?? {},
    createAuthHeaders(token)
  );

  // Return the full wrapped response { data: StepEventsData, message, success }
  return result.data;
}

export async function getStepSummaries(
  token: string,
  scenarioId: string,
  instanceId: string,
  filters?: StepSummariesFilters
) {
  const result = await RuntimeREST.api.getStepSummaries(
    scenarioId,
    instanceId,
    filters ?? {},
    createAuthHeaders(token)
  );

  // Return the full wrapped response { data: StepSummariesResponseData, message, success }
  return result.data;
}

export async function toggleTrackEvents(
  token: string,
  params: { scenarioId: string; version: number; trackEvents: boolean }
) {
  const { scenarioId, version, trackEvents } = params;
  const result = await RuntimeREST.api.toggleTrackEventsHandler(
    scenarioId,
    version,
    { trackEvents },
    createAuthHeaders(token)
  );

  // Return the full wrapped response (ApiResponseScenarioDto)
  return result.data;
}

// ==================== Folder Operations ====================

/**
 * Fetch all distinct folder paths for the current tenant
 */
export async function getFolders(token: string): Promise<FoldersResponse> {
  const result = await RuntimeREST.api.listFoldersHandler(
    createAuthHeaders(token)
  );
  return result.data;
}

/**
 * Fetch scenarios with optional folder filtering and pagination
 */
export async function getScenariosInFolder(
  token: string,
  params: {
    path?: string;
    recursive?: boolean;
    page?: number;
    pageSize?: number;
    search?: string;
  }
) {
  const result = await RuntimeREST.api.listScenariosHandler(
    {
      path: params.path,
      recursive: params.recursive ?? false,
      page: params.page,
      pageSize: params.pageSize,
      search: params.search || undefined,
    } as any,
    createAuthHeaders(token)
  );
  return result.data;
}

/**
 * Move a scenario to a different folder
 */
export async function moveScenarioToFolder(
  token: string,
  params: { scenarioId: string; path: string }
) {
  const request: MoveScenarioRequest = { path: params.path };
  const result = await RuntimeREST.api.moveScenarioHandler(
    params.scenarioId,
    request,
    createAuthHeaders(token)
  );
  return result.data;
}

/**
 * Rename a folder (updates all scenarios with matching path prefix)
 */
export async function renameFolder(
  token: string,
  params: { currentPath: string; newPath: string }
) {
  const request: RenameFolderRequest = {
    oldPath: params.currentPath,
    newPath: params.newPath,
  };
  const result = await RuntimeREST.api.renameFolderHandler(
    request,
    createAuthHeaders(token)
  );
  return result.data;
}

/**
 * Delete a folder by moving all its scenarios to root
 * (This is a convention - actual deletion moves scenarios to root)
 */
export async function deleteFolder(token: string, folderPath: string) {
  // To delete a folder, we rename it to root (which effectively removes the folder)
  // Or we can move all scenarios in the folder to root
  // For now, we'll use rename to move everything to root
  const request: RenameFolderRequest = {
    oldPath: folderPath,
    newPath: '/',
  };
  const result = await RuntimeREST.api.renameFolderHandler(
    request,
    createAuthHeaders(token)
  );
  return result.data;
}

// --- WaitForSignal: Pending Input & Signal Delivery ---

export interface PendingInput {
  signalId: string;
  toolName: string;
  message: string;
  responseSchema: Record<string, any>;
  aiAgentStepId: string;
  iteration: number;
  callNumber: number;
  requestedAt: string;
}

export async function getPendingInput(
  token: string,
  scenarioId: string,
  instanceId: string
) {
  const url = `${getRuntimeBaseUrl()}/scenarios/${encodeURIComponent(scenarioId)}/instances/${encodeURIComponent(instanceId)}/pending-input`;

  const response = await fetch(url, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch pending input: ${response.statusText}`);
  }

  const json = await response.json();
  // API returns { success, data: { pendingInputs: [...], count } }
  return json?.data?.pendingInputs ?? [];
}

export async function deliverSignal(
  token: string,
  instanceId: string,
  body: { signalId: string; payload: Record<string, any> }
) {
  const url = `${getRuntimeBaseUrl()}/signals/${encodeURIComponent(instanceId)}`;

  const response = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify(body),
  });

  if (!response.ok) {
    throw new Error(`Failed to deliver signal: ${response.statusText}`);
  }

  return response.json();
}
