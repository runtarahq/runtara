import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { ExtendedAgent, toExtendedAgent } from '@/features/workflows/queries';
import { StepTypeInfo } from '@/generated/RuntaraRuntimeApi.ts';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys.ts';
import { useRef, useMemo } from 'react';
import { composeExecutionGraph } from '../CustomNodes/utils.tsx';
import { NodeFormContext } from './NodeFormContext.tsx';
import { composePreviousSteps } from './shared.ts';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import {
  getAgents,
  getWorkflows,
  getWorkflowStepTypes,
} from '@/features/workflows/queries';
import { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';
import { useMultipleAgentDetails } from '@/features/workflows/hooks';

/** Simple variable type matching the WorkflowEditor prop type */
interface SimpleVariable {
  name: string;
  value: string;
  type: string;
  description?: string | null;
}

interface Props {
  nodeId?: string;
  parentNodeId?: string;
  isAddingBefore?: boolean;
  outputSchemaFields?: SchemaField[];
  /** Workflow input schema fields for variable suggestions */
  inputSchemaFields?: SchemaField[];
  /** Workflow variables (constants) for variable suggestions */
  variables?: SimpleVariable[];
  children: React.ReactNode;
}

/**
 * Creates a stable signature of the graph structure (nodes and edges).
 * This ignores position/dimension changes and only captures structural changes
 * (node additions/removals, edge changes, data changes).
 */
function createGraphStructureSignature(
  nodes: { id: string; data: any; parentId?: string }[],
  edges: { source: string; target: string; sourceHandle?: string | null }[]
): string {
  // Sort nodes by id for consistent ordering
  const sortedNodes = [...nodes].sort((a, b) => a.id.localeCompare(b.id));

  // Create a signature from node ids and their data (excluding position info)
  const nodeSignatures = sortedNodes.map((node) => {
    // Extract only data fields that affect the execution graph
    const { stepType, name, agentId, capabilityId, inputMapping } =
      node.data || {};
    return `${node.id}:${node.parentId || ''}:${stepType || ''}:${name || ''}:${agentId || ''}:${capabilityId || ''}:${JSON.stringify(inputMapping || [])}`;
  });

  // Sort edges for consistent ordering
  const sortedEdges = [...edges].sort((a, b) => {
    const aKey = `${a.source}-${a.target}-${a.sourceHandle || ''}`;
    const bKey = `${b.source}-${b.target}-${b.sourceHandle || ''}`;
    return aKey.localeCompare(bKey);
  });

  const edgeSignatures = sortedEdges.map(
    (edge) => `${edge.source}->${edge.target}:${edge.sourceHandle || 'default'}`
  );

  return `nodes:[${nodeSignatures.join(',')}];edges:[${edgeSignatures.join(',')}]`;
}

export const NodeFormProvider = ({
  children,
  nodeId,
  parentNodeId,
  isAddingBefore,
  outputSchemaFields,
  inputSchemaFields,
  variables,
}: Props) => {
  const agentsQuery = useCustomQuery({
    queryKey: queryKeys.agents.all,
    queryFn: getAgents,
    placeholderData: { agents: [] },
  });

  const stepTypesQuery = useCustomQuery({
    queryKey: queryKeys.workflows.stepTypes(),
    queryFn: getWorkflowStepTypes,
    placeholderData: { step_types: [] },
  });

  const workflowsQuery = useCustomQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: getWorkflows,
    placeholderData: {
      data: { content: [] },
      message: '',
      success: true,
    } as any,
  });

  // Extract compact agents from wrapped response { agents: ExtendedAgent[] }.
  // Capability schemas are hydrated only for agents used by the current graph.
  const compactAgents: ExtendedAgent[] = useMemo(
    () => (agentsQuery.data as any)?.agents || [],
    [agentsQuery.data]
  );

  // Extract step types from wrapped response { step_types: StepTypeInfo[] }
  // Filter out Start based on context:
  // - When adding before: include Start (user can replace Start)
  // - When adding after: exclude Start
  const stepTypes: StepTypeInfo[] = useMemo(() => {
    const allStepTypes = (stepTypesQuery.data as any)?.step_types || [];
    const filtered = allStepTypes.filter((st: StepTypeInfo) => {
      if (st.name === 'Start') return isAddingBefore;
      return true;
    });

    // Deduplicate based on normalized name (remove spaces)
    const seen = new Map<string, StepTypeInfo>();
    filtered.forEach((st: StepTypeInfo) => {
      const normalized = (st.name || '').replace(/\s+/g, '');
      if (!seen.has(normalized)) {
        seen.set(normalized, st);
      }
    });

    return Array.from(seen.values());
  }, [stepTypesQuery.data, isAddingBefore]);

  // Extract workflows from paginated response
  const workflows: WorkflowDto[] = useMemo(
    () => (workflowsQuery.data as any)?.data?.content || [],
    [workflowsQuery.data]
  );

  // Use a stable signature selector that only changes when graph structure changes
  // This prevents re-renders on position/dimension changes
  const graphSignature = useWorkflowStore((state) =>
    createGraphStructureSignature(state.nodes, state.edges)
  );

  // Track the cached execution graph
  const graphCacheRef = useRef<{
    signature: string;
    executionGraph: ReturnType<typeof composeExecutionGraph>;
  } | null>(null);

  // Only recompute execution graph when the signature changes
  const executionGraph = useMemo(() => {
    // If signature hasn't changed, return cached result
    if (
      graphCacheRef.current &&
      graphCacheRef.current.signature === graphSignature
    ) {
      return graphCacheRef.current.executionGraph;
    }

    // Structure changed, get fresh data and recompute
    const { nodes, edges } = useWorkflowStore.getState();
    const newGraph = composeExecutionGraph(nodes, edges);
    graphCacheRef.current = {
      signature: graphSignature,
      executionGraph: newGraph,
    };
    return newGraph;
  }, [graphSignature]);

  const graphAgentIds = useMemo(() => {
    const ids = new Set<string>();

    for (const step of Object.values(executionGraph?.steps || {})) {
      const agentId = (step as any)?.agentId;
      if (typeof agentId === 'string' && agentId) {
        ids.add(agentId);
      }
    }

    return Array.from(ids).sort();
  }, [executionGraph]);

  const {
    agentDetailsMap,
    isLoading: agentDetailsLoading,
  } = useMultipleAgentDetails(graphAgentIds, {
    enabled: graphAgentIds.length > 0,
  });

  const agents: ExtendedAgent[] = useMemo(() => {
    if (agentDetailsMap.size === 0) {
      return compactAgents;
    }

    return compactAgents.map((agent) => {
      const details = agentDetailsMap.get(agent.id);
      return details ? toExtendedAgent(details) : agent;
    });
  }, [compactAgents, agentDetailsMap]);

  const previousSteps = useMemo(
    () =>
      !!nodeId || !!parentNodeId
        ? composePreviousSteps({
            stepId: nodeId,
            parentStepId: parentNodeId,
            agents,
            executionGraph,
            workflows,
          })
        : [],
    [nodeId, parentNodeId, agents, executionGraph, workflows]
  );

  // Detect if this step is inside a While loop container
  const isInsideWhileLoop = useMemo(() => {
    if (!parentNodeId || !executionGraph?.steps) return false;
    const parentStep = executionGraph.steps[parentNodeId];
    return parentStep?.stepType === 'While';
  }, [parentNodeId, executionGraph]);

  const isLoading =
    agentsQuery.isFetching ||
    agentDetailsLoading ||
    stepTypesQuery.isFetching ||
    workflowsQuery.isFetching;

  const value = useMemo(
    () => ({
      nodeId,
      parentNodeId,
      agents,
      stepTypes,
      workflows,
      executionGraph,
      isLoading,
      previousSteps,
      outputSchemaFields,
      inputSchemaFields,
      variables,
      isInsideWhileLoop,
    }),
    [
      nodeId,
      parentNodeId,
      executionGraph,
      previousSteps,
      stepTypes,
      agents,
      workflows,
      isLoading,
      outputSchemaFields,
      inputSchemaFields,
      variables,
      isInsideWhileLoop,
    ]
  );

  return (
    <NodeFormContext.Provider value={value}>
      {children}
    </NodeFormContext.Provider>
  );
};
