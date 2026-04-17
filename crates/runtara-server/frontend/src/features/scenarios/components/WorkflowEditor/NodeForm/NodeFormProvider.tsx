import { ScenarioDto } from '@/generated/RuntaraRuntimeApi';
import { ExtendedAgent } from '@/features/scenarios/queries';
import { StepTypeInfo } from '@/generated/RuntaraRuntimeApi.ts';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys.ts';
import { useRef, useMemo } from 'react';
import { composeExecutionGraph } from '../CustomNodes/utils.tsx';
import { NodeFormContext } from './NodeFormContext.tsx';
import { composePreviousSteps } from './shared.ts';
import { useWorkflowStore } from '@/features/scenarios/stores/workflowStore.ts';
import {
  getAgents,
  getScenarios,
  getScenarioStepTypes,
} from '@/features/scenarios/queries';
import { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';

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
  /** Scenario input schema fields for variable suggestions */
  inputSchemaFields?: SchemaField[];
  /** Scenario variables (constants) for variable suggestions */
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
    queryKey: queryKeys.scenarios.stepTypes(),
    queryFn: getScenarioStepTypes,
    placeholderData: { step_types: [] },
  });

  const scenariosQuery = useCustomQuery({
    queryKey: queryKeys.scenarios.all,
    queryFn: getScenarios,
    placeholderData: {
      data: { content: [] },
      message: '',
      success: true,
    } as any,
  });

  // Extract agents from wrapped response { agents: ExtendedAgent[] }
  const agents: ExtendedAgent[] = useMemo(
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

  // Extract scenarios from paginated response
  const scenarios: ScenarioDto[] = useMemo(
    () => (scenariosQuery.data as any)?.data?.content || [],
    [scenariosQuery.data]
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

  const previousSteps = useMemo(
    () =>
      !!nodeId || !!parentNodeId
        ? composePreviousSteps({
            stepId: nodeId,
            parentStepId: parentNodeId,
            agents,
            executionGraph,
            scenarios,
          })
        : [],
    [nodeId, parentNodeId, agents, executionGraph, scenarios]
  );

  // Detect if this step is inside a While loop container
  const isInsideWhileLoop = useMemo(() => {
    if (!parentNodeId || !executionGraph?.steps) return false;
    const parentStep = executionGraph.steps[parentNodeId];
    return parentStep?.stepType === 'While';
  }, [parentNodeId, executionGraph]);

  const isLoading =
    agentsQuery.isFetching ||
    stepTypesQuery.isFetching ||
    scenariosQuery.isFetching;

  const value = useMemo(
    () => ({
      nodeId,
      parentNodeId,
      agents,
      stepTypes,
      scenarios,
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
      scenarios,
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
