import type { Edge, Node } from '@xyflow/react';

import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
  STEP_TYPES,
} from '@/features/workflows/config/workflow';
import type { ExecutionGraphStepDto } from '@/features/workflows/types/execution-graph';
import type { ValidationMessage } from '@/features/workflows/types/validation';
import {
  validateExecutionGraphWithRust,
  type RustWorkflowValidationResult,
} from '@/features/workflows/utils/rust-workflow-validation';
import {
  convertClientErrors,
  convertClientWarnings,
} from '@/features/workflows/utils/validation-helpers';
import {
  buildSchemaFromFields,
  type SchemaField,
} from '@/features/workflows/utils/schema';

import { composeExecutionGraph } from './CustomNodes/utils';
import type * as form from './NodeForm/NodeFormItem';

type WorkflowValidationContext = {
  name?: string;
  description?: string;
  variables?: Array<{ name: string; value: unknown; type: string }>;
  inputSchemaFields?: SchemaField[];
  outputSchemaFields?: SchemaField[];
  executionTimeoutSeconds?: number;
  rateLimitBudgetMs?: number;
};

export type PendingNodeCandidate = {
  id: string;
  data: Partial<ExecutionGraphStepDto>;
  position: { x: number; y: number };
  parentId?: string;
  sourceNodeId?: string;
  targetNodeId?: string;
  sourceHandle?: string;
  insertionEdge?: {
    source: string;
    target: string;
    sourceHandle: string;
  };
};

type ValidateNodeEditParams = {
  nodes: Node[];
  edges: Edge[];
  nodeId: string;
  data: form.SchemaType;
  workflow?: WorkflowValidationContext;
  pendingNode?: PendingNodeCandidate | null;
};

export type NodeEditRustValidationResult = {
  canApply: boolean;
  messages: ValidationMessage[];
  rustValidation: RustWorkflowValidationResult;
};

function stepNodeType(data: Partial<ExecutionGraphStepDto>, fallback?: string) {
  const stepType = data.stepType as keyof typeof STEP_TYPES | undefined;
  return stepType
    ? STEP_TYPES[stepType] || fallback || NODE_TYPES.BasicNode
    : fallback || NODE_TYPES.BasicNode;
}

function toNodeData(
  nodeId: string,
  data: form.SchemaType
): Partial<ExecutionGraphStepDto> {
  return {
    ...data,
    id: nodeId,
    inputMapping: data.inputMapping || [],
  } as unknown as Partial<ExecutionGraphStepDto>;
}

function buildExistingNodeCandidate(
  node: Node,
  nodeId: string,
  data: form.SchemaType
): Node {
  const nodeType = stepNodeType(
    data as unknown as Partial<ExecutionGraphStepDto>,
    node.type
  );

  return {
    ...node,
    type: nodeType,
    data: {
      ...node.data,
      ...toNodeData(nodeId, data),
    },
  };
}

function buildPendingNodeCandidate(
  pendingNode: PendingNodeCandidate,
  parentId: string | undefined,
  nodeId: string,
  data: form.SchemaType
): Node {
  const nodeData = toNodeData(nodeId, data);
  const nodeType = stepNodeType(nodeData);
  const size = NODE_TYPE_SIZES[nodeType] || { width: 180, height: 48 };

  return {
    id: nodeId,
    type: nodeType,
    position: pendingNode.position,
    data: nodeData,
    width: size.width,
    height: size.height,
    style: {
      width: size.width,
      height: size.height,
    },
    ...(parentId ? { parentId, extent: 'parent' as const } : {}),
  };
}

function edge(
  source: string,
  target: string,
  sourceHandle: string | undefined
): Edge {
  const handle = sourceHandle || 'source';
  return {
    id: `${source}-${target}-${handle}`,
    source,
    target,
    sourceHandle: handle,
    targetHandle: 'target',
  };
}

function addOutgoingEdges(
  edges: Edge[],
  source: string,
  target: string,
  isConditional: boolean
) {
  if (isConditional) {
    edges.push(edge(source, target, 'true'), edge(source, target, 'false'));
  } else {
    edges.push(edge(source, target, 'source'));
  }
}

function buildPendingEdges(
  edges: Edge[],
  pendingNode: PendingNodeCandidate,
  nodeId: string,
  data: form.SchemaType
): Edge[] {
  const nodeType = stepNodeType(
    data as unknown as Partial<ExecutionGraphStepDto>
  );
  const isConditional =
    nodeType === NODE_TYPES.ConditionalNode || data.stepType === 'Conditional';
  let candidateEdges = [...edges];

  if (pendingNode.insertionEdge) {
    const { source, target, sourceHandle } = pendingNode.insertionEdge;
    candidateEdges = candidateEdges.filter(
      (existing) =>
        !(
          existing.source === source &&
          existing.target === target &&
          existing.sourceHandle === sourceHandle
        )
    );
    candidateEdges.push(edge(source, nodeId, sourceHandle));
    addOutgoingEdges(candidateEdges, nodeId, target, isConditional);
    return candidateEdges;
  }

  if (pendingNode.targetNodeId) {
    addOutgoingEdges(
      candidateEdges,
      nodeId,
      pendingNode.targetNodeId,
      isConditional
    );
  }

  if (pendingNode.sourceNodeId) {
    candidateEdges.push(
      edge(pendingNode.sourceNodeId, nodeId, pendingNode.sourceHandle)
    );
  }

  return candidateEdges;
}

function buildGraphOptions(workflow?: WorkflowValidationContext) {
  const variables = workflow?.variables?.length
    ? workflow.variables.reduce(
        (acc, variable) => {
          if (!variable.name) return acc;
          acc[variable.name] = {
            type: variable.type || 'string',
            value: variable.value,
          };
          return acc;
        },
        {} as Record<string, { type: string; value: unknown }>
      )
    : undefined;

  return {
    name: workflow?.name ?? '',
    description: workflow?.description ?? '',
    variables,
    inputSchema: workflow?.inputSchemaFields?.length
      ? buildSchemaFromFields(workflow.inputSchemaFields)
      : undefined,
    outputSchema: workflow?.outputSchemaFields?.length
      ? buildSchemaFromFields(workflow.outputSchemaFields)
      : undefined,
    executionTimeoutSeconds: workflow?.executionTimeoutSeconds,
    rateLimitBudgetMs: workflow?.rateLimitBudgetMs,
  };
}

function inferPendingParentId(
  pendingNode: PendingNodeCandidate,
  nodes: Node[]
): string | undefined {
  if (pendingNode.parentId) return pendingNode.parentId;

  const sourceNodeId =
    pendingNode.insertionEdge?.source ?? pendingNode.sourceNodeId;
  if (!sourceNodeId) return undefined;

  return nodes.find((node) => node.id === sourceNodeId)?.parentId;
}

function applyTargetFallback(
  messages: ValidationMessage[],
  candidateNodes: Node[],
  nodeId: string
): ValidationMessage[] {
  const candidateNode = candidateNodes.find((node) => node.id === nodeId);
  const candidateName =
    typeof candidateNode?.data?.name === 'string'
      ? candidateNode.data.name
      : undefined;

  return messages.map((message) => ({
    ...message,
    stepId: message.stepId ?? nodeId,
    stepName: message.stepName ?? candidateName,
  }));
}

export async function validateNodeEditWithRust({
  nodes,
  edges,
  nodeId,
  data,
  workflow,
  pendingNode,
}: ValidateNodeEditParams): Promise<NodeEditRustValidationResult> {
  let candidateNodes: Node[];
  let candidateEdges: Edge[];

  if (pendingNode) {
    candidateNodes = [
      ...nodes,
      buildPendingNodeCandidate(
        pendingNode,
        inferPendingParentId(pendingNode, nodes),
        nodeId,
        data
      ),
    ];
    candidateEdges = buildPendingEdges(edges, pendingNode, nodeId, data);
  } else {
    candidateNodes = nodes.map((node) =>
      node.id === nodeId ? buildExistingNodeCandidate(node, nodeId, data) : node
    );
    candidateEdges = edges;
  }

  const executionGraph =
    composeExecutionGraph(
      candidateNodes,
      candidateEdges,
      buildGraphOptions(workflow)
    ) ?? {};

  const rustValidation = await validateExecutionGraphWithRust(executionGraph);
  const errors =
    rustValidation.status === 'invalid'
      ? convertClientErrors(
          rustValidation.errors.length > 0
            ? rustValidation.errors
            : [rustValidation.message],
          candidateNodes
        )
      : [];
  const warnings = convertClientWarnings(
    [
      ...rustValidation.warnings,
      ...(rustValidation.status === 'unavailable'
        ? [rustValidation.message]
        : []),
    ],
    candidateNodes
  );

  return {
    canApply: rustValidation.status !== 'invalid',
    messages: applyTargetFallback(
      [...errors, ...warnings],
      candidateNodes,
      nodeId
    ),
    rustValidation,
  };
}
