import {
  AgentInfo,
  ApiResponseWorkflowDto,
  CapabilityInfo,
  FoldersResponse,
  ListStepTypesResponse,
  MoveWorkflowRequest,
  RenameFolderRequest,
  WorkflowInstanceDto,
} from '@/generated/RuntaraRuntimeApi.ts';
import { executionGraphToReactFlow } from '@/features/workflows/components/WorkflowEditor/CustomNodes/utils.tsx';
import { ExecutionGraphDto } from '@/features/workflows/types/execution-graph';
import { parseSchema } from '@/features/workflows/utils/schema';
import {
  getStaticAgentWithRust,
  getStaticAgentsWithRust,
  getStaticStepTypesWithRust,
  StaticAgentSummary,
} from '@/features/workflows/utils/rust-workflow-validation';
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

type AgentMetadata = StaticAgentSummary | AgentInfo;

export function toExtendedAgent(agentInfo: AgentMetadata): ExtendedAgent {
  const supportedCapabilities: Record<string, CapabilityInfo> = {};

  const capabilities =
    'capabilities' in agentInfo ? agentInfo.capabilities || [] : [];

  for (const capability of capabilities) {
    supportedCapabilities[capability.id] = capability;
  }

  return {
    id: agentInfo.id,
    name: agentInfo.name,
    description: agentInfo.description,
    supportsConnections: agentInfo.supportsConnections ?? false,
    integrationIds: agentInfo.integrationIds ?? [],
    supportedCapabilities,
  };
}

export async function getWorkflows(token: string) {
  const result = await RuntimeREST.api.listWorkflowsHandler(
    { recursive: true, pageSize: 100 }, // Use max page size to get all workflows for dropdowns
    createAuthHeaders(token)
  );

  // Return the full wrapped response (ApiResponsePageWorkflowDto)
  // The response structure is: { data: { content: WorkflowDto[], totalElements, ... }, message, success }
  return result.data;
}

export async function createWorkflow(
  token: string,
  workflow: { name: string; description?: string }
) {
  const result = await RuntimeREST.api.createWorkflowHandler(
    {
      name: workflow.name,
      description: workflow.description || '', // Runtime API requires description
    },
    createAuthHeaders(token)
  );

  // Return the full wrapped response (ApiResponseWorkflowDto)
  // The response structure is: { data: WorkflowDto, message: string, success: boolean }
  return result.data;
}

export async function getWorkflow(token: string, workflowId: string) {
  const result = await RuntimeREST.api.getWorkflowHandler(
    workflowId,
    undefined,
    createAuthHeaders(token)
  );

  // Return the full wrapped response { data: WorkflowDto, message, success }
  // UI components will extract the data
  return result.data;
}

// Extended type to include both instance data and metadata
export interface WorkflowInstanceWithMetadata extends WorkflowInstanceDto {
  metadata?: {
    workflowName?: string;
    workflowDescription?: string;
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

export async function getWorkflowInstance(
  token: string,
  workflowId: string,
  instanceId: string
): Promise<WorkflowInstanceWithMetadata> {
  const result = await RuntimeREST.api.getInstanceHandler(
    workflowId,
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
      instance: WorkflowInstanceDto;
      metadata?: WorkflowInstanceWithMetadata['metadata'];
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

// Using the WorkflowVersionInfoDto from the Runtime API client
export type { WorkflowVersionInfoDto } from '@/generated/RuntaraRuntimeApi.ts';

export async function getWorkflowVersions(token: string, workflowId: string) {
  const result = await RuntimeREST.api.listWorkflowVersionsHandler(
    workflowId,
    createAuthHeaders(token)
  );

  // Return the full wrapped response { data: WorkflowVersionInfoDto[], message, success }
  // UI components will extract the data array
  return result.data;
}

export async function getWorkflowWorkflow(
  token: string,
  workflowId: string,
  versionNumber?: number
) {
  const result = await RuntimeREST.api.getWorkflowHandler(
    workflowId,
    versionNumber ? { versionNumber } : undefined,
    createAuthHeaders(token)
  );

  // Runtime API returns wrapped response: ApiResponseWorkflowDto
  // Extract workflow data for workflow processing
  const responseData = result.data as ApiResponseWorkflowDto;
  const workflowData = responseData.data;

  const { executionGraph = {} } = workflowData;

  // Backend now always provides Start and Finish steps
  const { nodes, edges } = executionGraphToReactFlow(executionGraph);

  // Parse variables from API format to array format used by the UI
  // Variables are now inside executionGraph (moved from workflow root)
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
  // Schemas are now inside executionGraph (moved from workflow root)
  const inputSchemaFields = parseSchema(executionGraph.inputSchema);
  const outputSchemaFields = parseSchema(executionGraph.outputSchema);

  // Extract executionTimeoutSeconds from executionGraph (moved from workflow root)
  const executionTimeoutSeconds = executionGraph.executionTimeoutSeconds;

  // Extract rateLimitBudgetMs from executionGraph
  const rateLimitBudgetMs = executionGraph.rateLimitBudgetMs;

  // Name and description are now inside executionGraph (moved from workflow root).
  // Prefer executionGraph values; fall back to legacy top-level fields so workflows
  // created before the migration still display correctly.
  const name = executionGraph.name ?? workflowData.name ?? '';
  const description =
    executionGraph.description ?? workflowData.description ?? '';

  // Return wrapped response with processed workflow
  return {
    data: {
      ...workflowData,
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

export async function updateWorkflow(
  token: string,
  newWorkflow: {
    id: string;
    data: ExecutionGraphDto; // name and description are now inside the execution graph
  }
) {
  const { id, data } = newWorkflow;

  const result = await RuntimeREST.api.updateWorkflowHandler(
    id,
    {
      executionGraph: data,
    },
    createAuthHeaders(token)
  );

  // Return the full wrapped response (ApiResponseWorkflowDto)
  // The response structure is: { data: WorkflowDto, message: string, success: boolean }
  return result.data;
}

export async function removeWorkflow(token: string, workflowId: string) {
  await RuntimeREST.api.deleteWorkflowHandler(
    workflowId,
    createAuthHeaders(token)
  );
}

export async function scheduleWorkflow(
  token: string,
  workflowId: string,
  inputs?: Record<string, unknown>,
  version?: number,
  debug?: boolean
) {
  // API expects inputs in format: { data: {...}, variables: {...} }
  const formattedInputs = {
    data: inputs || {},
    variables: {},
  };

  const result = await RuntimeREST.api.executeWorkflowHandler(
    workflowId,
    { inputs: formattedInputs, ...(debug ? { debug: true } : {}) },
    version !== undefined ? { version } : undefined,
    createAuthHeaders(token)
  );

  // Return simplified response (ExecuteWorkflowResponse now only has instanceId and status)
  return result.data;
}

export async function resumeInstance(token: string, instanceId: string) {
  const result = await RuntimeREST.api.resumeInstanceHandler(
    instanceId,
    createAuthHeaders(token)
  );

  return result.data;
}

export async function cloneWorkflow(
  token: string,
  workflowId: string,
  name?: string
) {
  const result = await RuntimeREST.api.cloneWorkflowHandler(
    workflowId,
    { name: name || `Copy ${Date.now()}` },
    createAuthHeaders(token)
  );

  return result.data;
}

export async function setCurrentVersion(
  token: string,
  params: { workflowId: string; versionNumber: number }
) {
  const { workflowId, versionNumber } = params;
  const result = await RuntimeREST.api.setCurrentVersionHandler(
    workflowId,
    versionNumber,
    createAuthHeaders(token)
  );

  return result.data;
}

export async function getWorkflowStepTypes(
  token: string
): Promise<ListStepTypesResponse> {
  void token;
  return getStaticStepTypesWithRust();
}

export async function getAgents(token: string) {
  void token;
  const agentSummaries = await getStaticAgentsWithRust();
  const agents = await Promise.all(
    agentSummaries.map(async (summary) => {
      const details = await getStaticAgentWithRust(summary.id);
      return toExtendedAgent(details ?? summary);
    })
  );
  return { agents };
}

export async function getAgentDetails(
  token: string,
  agentId: string
): Promise<AgentInfo | null> {
  void token;
  if (!agentId) {
    return null;
  }

  return getStaticAgentWithRust(agentId);
}

export async function replayWorkflow(token: string, instanceId: string) {
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
  workflowId: string,
  instanceId: string,
  filters?: StepEventsFilters
) {
  const result = await RuntimeREST.api.getStepEvents(
    workflowId,
    instanceId,
    filters ?? {},
    createAuthHeaders(token)
  );

  // Return the full wrapped response { data: StepEventsData, message, success }
  return result.data;
}

export async function getStepSummaries(
  token: string,
  workflowId: string,
  instanceId: string,
  filters?: StepSummariesFilters
) {
  const result = await RuntimeREST.api.getStepSummaries(
    workflowId,
    instanceId,
    filters ?? {},
    createAuthHeaders(token)
  );

  // Return the full wrapped response { data: StepSummariesResponseData, message, success }
  return result.data;
}

export async function toggleTrackEvents(
  token: string,
  params: { workflowId: string; version: number; trackEvents: boolean }
) {
  const { workflowId, version, trackEvents } = params;
  const result = await RuntimeREST.api.toggleTrackEventsHandler(
    workflowId,
    version,
    { trackEvents },
    createAuthHeaders(token)
  );

  // Return the full wrapped response (ApiResponseWorkflowDto)
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
 * Fetch workflows with optional folder filtering and pagination
 */
export async function getWorkflowsInFolder(
  token: string,
  params: {
    path?: string;
    recursive?: boolean;
    page?: number;
    pageSize?: number;
    search?: string;
  }
) {
  const result = await RuntimeREST.api.listWorkflowsHandler(
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
 * Move a workflow to a different folder
 */
export async function moveWorkflowToFolder(
  token: string,
  params: { workflowId: string; path: string }
) {
  const request: MoveWorkflowRequest = { path: params.path };
  const result = await RuntimeREST.api.moveWorkflowHandler(
    params.workflowId,
    request,
    createAuthHeaders(token)
  );
  return result.data;
}

/**
 * Rename a folder (updates all workflows with matching path prefix)
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
 * Delete a folder by moving all its workflows to root
 * (This is a convention - actual deletion moves workflows to root)
 */
export async function deleteFolder(token: string, folderPath: string) {
  // To delete a folder, we rename it to root (which effectively removes the folder)
  // Or we can move all workflows in the folder to root
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
  workflowId: string,
  instanceId: string
) {
  const url = `${getRuntimeBaseUrl()}/workflows/${encodeURIComponent(workflowId)}/instances/${encodeURIComponent(instanceId)}/pending-input`;

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
