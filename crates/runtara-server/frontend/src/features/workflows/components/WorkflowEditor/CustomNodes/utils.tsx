import { Edge, Node } from '@xyflow/react';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
  STEP_TYPES,
} from '@/features/workflows/config/workflow.ts';
import {
  ExecutionGraphDto,
  ExecutionGraphStepDto,
  ExecutionGraphTransitionDto,
} from '@/features/workflows/types/execution-graph';
import { type Note, ValueType } from '@/generated/RuntaraRuntimeApi.ts';
import {
  parseSchema,
  buildSchemaFromFields,
  type SchemaField,
} from '@/features/workflows/utils/schema';
import {
  snapToGrid,
  snapPositionToGrid,
} from '@/features/workflows/config/workflow-editor';
import {
  migrateStartStep,
  needsStartStepMigration,
} from '@/features/workflows/utils/start-step-migration';
import { convertConditionArguments } from '@/shared/utils/condition-type-conversion';
import {
  BASE_HEIGHT,
  BASE_WIDTH,
  ensureContainersContainChildren,
  layoutReactFlowElements,
} from './layout';

export { BASE_GROUP_WIDTH, BASE_WIDTH } from './layout';

export type ExecutionGraphTransition = Required<ExecutionGraphTransitionDto>;

export type ExecutionGraphStep = ExecutionGraphStepDto &
  Required<
    Pick<
      ExecutionGraphStepDto,
      'id' | 'inputMapping' | 'name' | 'stepType' | 'renderingParameters'
    >
  > & { subgraph?: ExecutionGraph };

export interface ExecutionGraph {
  steps?: Record<string, ExecutionGraphStep>;
  executionPlan?: ExecutionGraphTransition[];
  entryPoint: string;
  notes?: Note[];
  // Static variables defined at workflow version level
  variables?: Record<string, any>;
}

// Switch match type mapping: UI lowercase <-> API uppercase
const MATCH_TYPE_UI_TO_API: Record<string, string> = {
  exact: 'EQ',
  ne: 'NE',
  in: 'IN',
  not_in: 'NOT_IN',
  gt: 'GT',
  gte: 'GTE',
  lt: 'LT',
  lte: 'LTE',
  starts_with: 'STARTS_WITH',
  ends_with: 'ENDS_WITH',
  contains: 'CONTAINS',
  is_defined: 'IS_DEFINED',
  is_empty: 'IS_EMPTY',
  is_not_empty: 'IS_NOT_EMPTY',
  between: 'BETWEEN',
  range: 'RANGE',
};

const MATCH_TYPE_API_TO_UI: Record<string, string> = Object.fromEntries(
  Object.entries(MATCH_TYPE_UI_TO_API).map(([k, v]) => [v, k])
);

function mapMatchTypeToAPI(matchType: string): string {
  // Already uppercase (from API) → pass through
  if (matchType === matchType.toUpperCase() && matchType.length > 1)
    return matchType;
  return MATCH_TYPE_UI_TO_API[matchType] || matchType.toUpperCase();
}

function mapMatchTypeFromAPI(matchType: string): string {
  return MATCH_TYPE_API_TO_UI[matchType] || matchType.toLowerCase();
}

function normalizeSchemaFieldForEditor(field: SchemaField): SchemaField {
  return {
    ...field,
    type: field.type || 'string',
    required: field.required !== false,
    description: field.description || '',
  };
}

export function getLayoutedElements(nodes: Node[], edges: Edge[]) {
  return layoutReactFlowElements(nodes, edges);
}

export const getNodePositionInsideParent = (
  node: Partial<Node>,
  groupNode: Node
) => {
  const position = node.position ?? { x: 0, y: 0 };
  const nodeWidth = node.measured?.width ?? 0;
  const nodeHeight = node.measured?.height ?? 0;
  const groupWidth = groupNode.measured?.width ?? 0;
  const groupHeight = groupNode.measured?.height ?? 0;

  if (position.x < groupNode.position.x) {
    position.x = 0;
  } else if (position.x + nodeWidth > groupNode.position.x + groupWidth) {
    position.x = snapToGrid(groupWidth - nodeWidth);
  } else {
    position.x = snapToGrid(position.x - groupNode.position.x);
  }

  if (position.y < groupNode.position.y) {
    position.y = 0;
  } else if (position.y + nodeHeight > groupNode.position.y + groupHeight) {
    position.y = snapToGrid(groupHeight - nodeHeight);
  } else {
    position.y = snapToGrid(position.y - groupNode.position.y);
  }

  return position;
};

/*
  Payload schema

  |executionPlan                            |executionPlan
  |steps   -->   steps[id].subgraph   -->   |steps -->   ...
  |entryPoint                               |entryPoint
*/
export function composeExecutionGraph(
  nodes: Node[],
  edges: Edge[],
  options?: {
    name?: string;
    description?: string;
    variables?: Record<
      string,
      { type: string; value: unknown; description?: string | null }
    >;
    inputSchema?: Record<string, unknown>;
    outputSchema?: Record<string, unknown>;
    executionTimeoutSeconds?: number;
    rateLimitBudgetMs?: number;
    durable?: boolean | null;
    entryPoint?: string;
  }
): ExecutionGraph | null {
  const nodesMap: any = new Map();
  const executionGraph: any = {};

  // Include name and description in the execution graph
  // Name is required for updates, so always include it if provided
  if (options?.name !== undefined) {
    executionGraph.name = options.name;
  }
  if (options?.description !== undefined) {
    executionGraph.description = options.description;
  }

  // Include variables and schemas in the execution graph if provided
  if (options?.variables) {
    executionGraph.variables = options.variables;
  }
  if (options?.inputSchema) {
    executionGraph.inputSchema = options.inputSchema;
  }
  if (options?.outputSchema) {
    executionGraph.outputSchema = options.outputSchema;
  }
  if (options?.executionTimeoutSeconds !== undefined) {
    executionGraph.executionTimeoutSeconds = options.executionTimeoutSeconds;
  }
  if (options?.rateLimitBudgetMs !== undefined) {
    executionGraph.rateLimitBudgetMs = options.rateLimitBudgetMs;
  }
  if (options?.durable !== undefined) {
    executionGraph.durable = options.durable;
  }
  if (options?.entryPoint) {
    executionGraph.entryPoint = options.entryPoint;
  }

  // Separate notes from regular nodes
  const noteNodes = nodes.filter((node) => node.type === NODE_TYPES.NoteNode);
  const nds = nodes.filter(
    (node) =>
      node.type !== NODE_TYPES.CreateNode && node.type !== NODE_TYPES.NoteNode
  );
  const stepNodeIds = new Set(nds.map((node) => node.id));

  if (nds.length > 0) {
    executionGraph.nodes = nds.map((node) => {
      const width =
        typeof node.style?.width === 'number'
          ? node.style.width
          : typeof node.width === 'number'
            ? node.width
            : undefined;
      const height =
        typeof node.style?.height === 'number'
          ? node.style.height
          : typeof node.height === 'number'
            ? node.height
            : undefined;

      return {
        id: node.id,
        type: node.type,
        position: node.position,
        ...(width !== undefined ? { width } : {}),
        ...(height !== undefined ? { height } : {}),
        ...(node.parentId ? { parentId: node.parentId } : {}),
      };
    });
  }

  const visualEdges = edges
    .filter(
      (edge) => stepNodeIds.has(edge.source) && stepNodeIds.has(edge.target)
    )
    .map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      ...(edge.sourceHandle ? { sourceHandle: edge.sourceHandle } : {}),
      ...(edge.targetHandle ? { targetHandle: edge.targetHandle } : {}),
    }));

  if (visualEdges.length > 0) {
    executionGraph.edges = visualEdges;
  }

  if (!nds.length && !noteNodes.length) {
    return null;
  }

  // Process notes into the execution graph
  // New format uses: { text, position: {x, y} }
  if (noteNodes.length > 0) {
    executionGraph.notes = noteNodes.map((node) => ({
      id: node.data.id || node.id,
      text: node.data.content || '',
      position: {
        x: node.position.x,
        y: node.position.y,
      },
      metadata: {
        width: node.width || NODE_TYPE_SIZES[NODE_TYPES.NoteNode]?.width || 240,
        height:
          node.height || NODE_TYPE_SIZES[NODE_TYPES.NoteNode]?.height || 120,
      },
    }));
  }

  if (!nds.length) {
    return executionGraph;
  }

  // Create deep copies of nodes to avoid mutating the originals
  nds.forEach((node) => {
    nodesMap.set(node.id, JSON.parse(JSON.stringify(node)));
  });

  nds.forEach((node) => {
    const stepType = node.data?.stepType;
    if (stepType === 'Split' || stepType === 'While') {
      const containerStep = nodesMap.get(node.id);
      if (containerStep && !containerStep.subgraph) {
        // Seed the rebuilt subgraph with the graph-level fields captured at
        // load time (variables, schemas, name, description, notes, ...).
        containerStep.subgraph = {
          ...(containerStep.data?.subgraphMeta || {}),
          steps: {},
        };
      }
    }
  });

  nds.forEach((node) => {
    if (node.parentId) {
      const parent = nodesMap.get(node.parentId);
      if (parent) {
        if (!parent.subgraph) {
          parent.subgraph = {
            ...(parent.data?.subgraphMeta || {}),
          };
          parent.subgraph.steps = {};
        }
        parent.subgraph.steps[node.id] = nodesMap.get(node.id);
      }
    } else {
      if (!executionGraph.steps) {
        executionGraph.steps = {};
      }
      executionGraph.steps[node.id] = nodesMap.get(node.id);
    }
  });

  edges.forEach((edge) => {
    const sourceNode = nodesMap.get(edge.source);
    const targetNode = nodesMap.get(edge.target);

    const groupId = sourceNode?.parentId || targetNode?.parentId;

    // Map edge sourceHandle to spec label
    // Per DSL v2.0.0: use "next" for sequential, "true"/"false" for Conditional
    const rawLabel = edge.sourceHandle || '';
    let specLabel =
      rawLabel === '' || rawLabel === 'source' ? 'next' : rawLabel;

    // For Switch routing mode, convert case-N handles to route labels
    if (rawLabel.startsWith('case-') && sourceNode) {
      const caseIndex = parseInt(rawLabel.split('-')[1], 10);
      const casesField = (sourceNode.data?.inputMapping || []).find(
        (item: any) => item.type === 'cases'
      );
      const cases = Array.isArray(casesField?.value) ? casesField.value : [];
      if (cases[caseIndex]?.route) {
        specLabel = cases[caseIndex].route;
      }
    }

    if (groupId && sourceNode?.parentId === targetNode?.parentId) {
      const parent = nodesMap.get(groupId);
      if (parent.subgraph) {
        if (!parent.subgraph.executionPlan) {
          parent.subgraph.executionPlan = [];
        }
        const planEdge: Record<string, unknown> = {
          fromStep: edge.source,
          toStep: edge.target,
          label: specLabel,
        };
        if ((edge.data as any)?.condition !== undefined) {
          planEdge.condition = (edge.data as any).condition;
        }
        if ((edge.data as any)?.priority !== undefined) {
          planEdge.priority = Number((edge.data as any).priority);
        }
        parent.subgraph.executionPlan.push(planEdge);
      }
    } else {
      if (!executionGraph.executionPlan) {
        executionGraph.executionPlan = [];
      }
      const planEdge: Record<string, unknown> = {
        fromStep: edge.source,
        toStep: edge.target,
        label: specLabel,
      };
      if ((edge.data as any)?.condition !== undefined) {
        planEdge.condition = (edge.data as any).condition;
      }
      if ((edge.data as any)?.priority !== undefined) {
        planEdge.priority = Number((edge.data as any).priority);
      }
      executionGraph.executionPlan.push(planEdge);
    }
  });

  executionGraph.steps = cleanNodeData(executionGraph.steps);

  addStarts(executionGraph);
  stripEditorOnlyStepFields(executionGraph.steps);

  return executionGraph;
}

function stripEditorOnlyStepFields(steps?: Record<string, any>) {
  if (!steps) {
    return;
  }

  for (const step of Object.values(steps)) {
    delete step.renderingParameters;
    if (step.subgraph?.steps) {
      stripEditorOnlyStepFields(step.subgraph.steps);
    }
  }
}

function applyStoredNodeVisualState(nodes: Node[], storedNodes: unknown) {
  if (!Array.isArray(storedNodes)) {
    return nodes;
  }

  const visualStateById = new Map<string, any>();
  for (const storedNode of storedNodes) {
    if (
      storedNode &&
      typeof storedNode === 'object' &&
      typeof storedNode.id === 'string'
    ) {
      visualStateById.set(storedNode.id, storedNode);
    }
  }

  if (visualStateById.size === 0) {
    return nodes;
  }

  return nodes.map((node) => {
    const visualState = visualStateById.get(node.id);
    if (!visualState) {
      return node;
    }

    const position =
      visualState.position &&
      typeof visualState.position.x === 'number' &&
      typeof visualState.position.y === 'number'
        ? snapPositionToGrid({
            x: visualState.position.x,
            y: visualState.position.y,
          })
        : node.position;
    const width =
      typeof visualState.width === 'number'
        ? snapToGrid(visualState.width)
        : node.width;
    const height =
      typeof visualState.height === 'number'
        ? snapToGrid(visualState.height)
        : node.height;

    return {
      ...node,
      position,
      width,
      height,
      style: {
        ...node.style,
        ...(width !== undefined ? { width } : {}),
        ...(height !== undefined ? { height } : {}),
      },
    };
  });
}

function addStarts(executionGraph: ExecutionGraphDto) {
  function findStart(
    steps: Record<string, ExecutionGraphStepDto>,
    executionPlan: ExecutionGraphTransitionDto[]
  ) {
    const allIds = new Set(Object.keys(steps));
    const targets = new Set<string>();

    for (const { toStep = '' } of executionPlan) {
      if (allIds.has(toStep)) {
        targets.add(toStep);
      }
    }

    // Find all candidate entry points (nodes without incoming edges)
    const candidates: string[] = [];
    for (const id of allIds) {
      if (!targets.has(id)) {
        candidates.push(id);
      }
    }

    if (candidates.length === 0) {
      return '';
    }

    // If multiple candidates, pick the leftmost one (smallest x position)
    // This ensures the entry point stays stable when edges are deleted
    if (candidates.length === 1) {
      return candidates[0];
    }

    return candidates.reduce((leftmostId, id) => {
      const leftmostX = steps[leftmostId]?.renderingParameters?.x ?? Infinity;
      const currentX = steps[id]?.renderingParameters?.x ?? Infinity;
      return currentX < leftmostX ? id : leftmostId;
    });
  }

  function findAllStarts(executionGraph: ExecutionGraphDto) {
    const { steps = {}, executionPlan = [] } = executionGraph;
    if (!executionGraph.entryPoint || !steps[executionGraph.entryPoint]) {
      const entry = findStart(steps, executionPlan);
      executionGraph.entryPoint = entry;
    }

    for (const step of Object.values(steps)) {
      if (step.subgraph) {
        findAllStarts(step.subgraph);
      }
    }
  }

  findAllStarts(executionGraph);
}

// Valid ValueType values from the spec
const VALID_VALUE_TYPES: ReadonlySet<ValueType> = new Set<ValueType>([
  'string',
  'integer',
  'number',
  'boolean',
  'json',
  'file',
]);

/**
 * Coerces a value to match the given type hint (using API ValueType convention).
 * e.g., "150" with type "integer" becomes 150
 */
function coerceValueToType(value: any, typeHint?: string): any {
  if (typeHint === 'integer' || typeHint === 'number') {
    const numValue = Number(value);
    if (!isNaN(numValue)) {
      return typeHint === 'integer' ? Math.trunc(numValue) : numValue;
    }
  }
  if (typeHint === 'boolean' && typeof value === 'string') {
    const lower = value.toLowerCase();
    if (lower === 'true' || lower === '1') return true;
    if (lower === 'false' || lower === '0') return false;
  }
  return value;
}

// Check if a typeHint is a valid ValueType
function isValidValueType(typeHint?: string): typeHint is ValueType {
  return typeHint !== undefined && VALID_VALUE_TYPES.has(typeHint as ValueType);
}

function normalizeConditionExpression(condition: any): any {
  if (!condition || typeof condition !== 'object') return condition;
  if (!('op' in condition) || !Array.isArray(condition.arguments)) {
    return condition;
  }

  return {
    ...condition,
    type: condition.type || 'operation',
    arguments: convertConditionArguments(
      condition.op,
      condition.arguments,
      undefined,
      // Save path: string IN/NOT_IN right-hand sides become real arrays
      // (the runtime requires an array; a string RHS is a dead condition).
      { parseInLists: true }
    ),
  };
}

function cleanNodeData(steps: Record<string, any>) {
  const cleaned: Record<string, any> = {};

  if (!steps) {
    return {};
  }

  for (const [id, step] of Object.entries(steps)) {
    const { measured, position, subgraph, data } = step;
    // Destructure to exclude UI-only fields from the cleaned data
    const {
      inputMapping = [],
      inputSchema,
      outputSchema,
      childWorkflowId,
      childVersion,
      inputSchemaFields: _1,
      variablesFields: _2,
      splitInputSchemaFields: _3,
      splitOutputSchemaFields: _4,
      embedWorkflowConfig: _5,
      splitVariablesFields: _6,
      splitParallelism: _7,
      splitSequential: _8,
      splitDontStopOnFailed: _9,
      splitMaxRetries,
      splitRetryDelay,
      splitTimeout,
      splitAllowNull,
      splitConvertSingleValue,
      splitBatchSize,
      formTabs: _10,
      startMode: _11,
      selectedTriggerId: _12,
      executionTimeout: _13,
      retryStrategy: _14,
      groupByKey,
      groupByExpectedKeys,
      filterCondition,
      whileCondition,
      whileMaxIterations,
      whileTimeout,
      subgraphMeta: _15,
      ...restData
    } = data;
    // Suppress unused variable warnings for destructured exclusions
    void _1;
    void _2;
    void _3;
    void _4;
    void _5;
    void _6;
    void _7;
    void _8;
    void _9;
    void _10;
    void _11;
    void _12;
    void _13;
    void _14;
    void _15;

    if (subgraph) {
      data.subgraph = {
        ...(subgraph || {}),
        steps: cleanNodeData(subgraph.steps),
      };
    }

    // Handle inputMapping - convert array format to object format
    let cleanedInputMapping = inputMapping;
    // console.log("[DEBUG] cleanNodeData - inputMapping for node', id, ':', inputMapping);

    // Helper function to recursively process composite values.
    // Mirror of convertCompositeToUIFormat below — preserves typeHint/defaultValue for
    // every non-composite valueType (not only `immediate`) so the UI→backend round-trip is lossless.
    const processCompositeValue = (
      compositeVal: any
    ): {
      valueType: 'reference' | 'immediate' | 'composite';
      value: any;
    } => {
      const processEntry = (val: any) => {
        if (
          typeof val !== 'object' ||
          val === null ||
          !('valueType' in (val as Record<string, unknown>))
        ) {
          return {
            valueType: 'immediate',
            value: val,
          };
        }

        const typedVal = val as {
          valueType: 'reference' | 'immediate' | 'composite' | 'template';
          value: any;
          type?: string;
          typeHint?: string;
          default?: any;
          defaultValue?: any;
        };

        if (typedVal.valueType === 'composite') {
          const nestedValue =
            typedVal.value && typeof typedVal.value === 'object'
              ? typedVal.value
              : {};
          return {
            valueType: 'composite',
            value: processCompositeValue(nestedValue).value,
          };
        }

        const coercedValue =
          typedVal.valueType === 'immediate' &&
          typedVal.typeHint &&
          typedVal.value !== null
            ? coerceValueToType(typedVal.value, typedVal.typeHint)
            : typedVal.value === undefined
              ? ''
              : typedVal.value;

        const out: {
          valueType: string;
          value: any;
          type?: string;
          default?: any;
        } = {
          valueType: typedVal.valueType || 'immediate',
          value: coercedValue,
        };
        const typeHint = typedVal.typeHint ?? typedVal.type;
        if (typedVal.valueType === 'reference' && isValidValueType(typeHint)) {
          out.type = typeHint;
        }
        const defaultValue = typedVal.defaultValue ?? typedVal.default;
        if (typedVal.valueType === 'reference' && defaultValue !== undefined) {
          out.default = defaultValue;
        }
        return out;
      };

      // Handle composite object
      if (
        compositeVal &&
        typeof compositeVal === 'object' &&
        !Array.isArray(compositeVal)
      ) {
        const processedObject: Record<string, any> = {};
        for (const [key, val] of Object.entries(compositeVal)) {
          processedObject[key] = processEntry(val);
        }
        return { valueType: 'composite', value: processedObject };
      }

      // Handle composite array
      if (Array.isArray(compositeVal)) {
        return {
          valueType: 'composite',
          value: compositeVal.map(processEntry),
        };
      }

      // Fallback - shouldn't happen for properly structured data
      return { valueType: 'immediate', value: compositeVal };
    };

    // Helper function to process a single mapping entry
    const processMappingEntry = ({
      type,
      value,
      typeHint,
      valueType,
      defaultValue,
    }: {
      type: string;
      value: any;
      typeHint?: string;
      valueType?: 'reference' | 'immediate' | 'composite' | 'template';
      defaultValue?: any;
    }) => {
      // Handle template values - always a string, no type coercion
      if (valueType === 'template') {
        return [type, { valueType: 'template', value: String(value) }];
      }

      // Handle composite values - process recursively
      if (valueType === 'composite') {
        const processed = processCompositeValue(value);
        const mappingValue: {
          valueType: 'composite';
          value: any;
        } = {
          valueType: 'composite',
          value: processed.value,
        };
        return [type, mappingValue];
      }

      // Parse JSON strings into actual arrays/objects before sending to backend
      let finalValue = value;

      if (typeof value === 'string' && value) {
        // Skip parsing for template variables (they're resolved at runtime)
        const isTemplate = value.includes('{{');

        if (!isTemplate) {
          // For non-template strings, only parse as JSON if the typeHint is
          // explicitly JSON-shaped. 'object'/'array' are form-level hints
          // (e.g. Finish output types) that keep the editors' object-vs-array
          // distinction; they carry the same parse semantics as 'json' and
          // are never emitted as backend type hints (isValidValueType).
          // No auto-detection - explicit typeHint required.
          if (
            typeHint === 'json' ||
            typeHint === 'object' ||
            typeHint === 'array'
          ) {
            try {
              finalValue = JSON.parse(value);
            } catch {
              // If parsing fails, keep as string
              finalValue = value;
            }
          }

          // Convert numeric strings to actual numbers for integer/number type hints
          if (typeHint === 'integer' || typeHint === 'number') {
            const numValue = Number(value);
            if (!isNaN(numValue)) {
              // For integers, ensure we get a whole number
              finalValue =
                typeHint === 'integer' ? Math.trunc(numValue) : numValue;
            }
          }

          // Convert boolean strings to actual booleans for boolean type hint
          if (typeHint === 'boolean') {
            const lowerValue = value.toLowerCase();
            if (lowerValue === 'true' || lowerValue === '1') {
              finalValue = true;
            } else if (lowerValue === 'false' || lowerValue === '0') {
              finalValue = false;
            }
          }
        }
      }

      // Use explicit valueType from UI, fallback to auto-detection for backward compatibility
      const resolvedValueType: 'reference' | 'immediate' | 'template' =
        valueType ||
        (typeof finalValue === 'string' && finalValue.includes('{{')
          ? 'reference'
          : 'immediate');

      // Create the new format per DSL v2.0.0 spec: { valueType, value, type?, default? }
      const mappingValue: {
        valueType: 'reference' | 'immediate' | 'template';
        value: any;
        type?: string;
        default?: any;
      } = {
        valueType: resolvedValueType,
        value: finalValue,
      };

      // Only reference values carry backend type hints. Immediate, composite,
      // and template values reject unknown `type` fields.
      if (resolvedValueType === 'reference' && isValidValueType(typeHint)) {
        mappingValue.type = typeHint;
      }

      // Preserve ReferenceValue.default — only references carry this field on the backend.
      if (resolvedValueType === 'reference' && defaultValue !== undefined) {
        mappingValue.default = defaultValue;
      }

      return [type, mappingValue];
    };

    const hasMappingValuePayload = (mappingValue: any): boolean => {
      if (!mappingValue || typeof mappingValue !== 'object') return false;
      if (!('value' in mappingValue)) return false;
      if (mappingValue.value === undefined) return false;
      if (mappingValue.value === null) {
        return mappingValue.valueType === 'immediate';
      }
      if (
        typeof mappingValue.value === 'string' &&
        mappingValue.value.trim() === ''
      ) {
        return false;
      }
      return true;
    };

    const serializeSourceMappingValue = (mapping: any[]): any | undefined => {
      if (!Array.isArray(mapping) || mapping.length === 0) return undefined;
      const sourceEntry =
        mapping.find((item: any) => item?.type === 'value') ?? mapping[0];
      if (!sourceEntry) return undefined;
      if (
        sourceEntry.value === undefined ||
        (typeof sourceEntry.value === 'string' &&
          sourceEntry.value.trim() === '')
      ) {
        return undefined;
      }

      const [, sourceValue] = processMappingEntry({
        type: 'value',
        value: sourceEntry.value,
        typeHint: sourceEntry.typeHint,
        valueType: sourceEntry.valueType,
        defaultValue: sourceEntry.defaultValue,
      }) as [string, any];

      return hasMappingValuePayload(sourceValue) ? sourceValue : undefined;
    };

    const parseObjectValue = (value: any): Record<string, any> | undefined => {
      if (value && typeof value === 'object' && !Array.isArray(value)) {
        return value;
      }
      if (typeof value === 'string' && value.trim()) {
        try {
          const parsed = JSON.parse(value);
          if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
            return parsed;
          }
        } catch {
          return undefined;
        }
      }
      return undefined;
    };

    const serializeMappingObject = (
      value: any,
      valueType?: string
    ): Record<string, any> | undefined => {
      const objectValue = parseObjectValue(value);
      if (!objectValue) {
        return undefined;
      }

      if (valueType === 'composite') {
        const processed = processCompositeValue(objectValue);
        return processed.value &&
          typeof processed.value === 'object' &&
          !Array.isArray(processed.value)
          ? processed.value
          : undefined;
      }

      return objectValue;
    };

    // Filter out empty optional fields that shouldn't be sent to the API
    const optionalFieldsToFilterIfEmpty = [
      'agentId',
      'capabilityId',
      'connectionId',
    ];
    const filteredRestData = Object.fromEntries(
      Object.entries(restData).filter(
        ([key, value]) =>
          !(optionalFieldsToFilterIfEmpty.includes(key) && value === '')
      )
    );

    if (Array.isArray(inputMapping)) {
      const preserveInvalidMappingsForRustValidation =
        data.stepType === 'Finish';
      // Error/Log extract direct fields (code, message, level, ...) from the
      // filtered mapping, where '' is the form's "field cleared" signal and
      // must keep deleting the key. Every other consumer serializes a real
      // inputMapping object, where immediate '' is a legal DSL value.
      const emptyStringMeansCleared =
        data.stepType === 'Error' || data.stepType === 'Log';
      const filteredMapping = inputMapping.filter(
        ({
          type,
          value,
          valueType,
          autoSeeded,
        }: {
          type: string;
          value: any;
          valueType?: string;
          autoSeeded?: boolean;
        }) => {
          // Filter out entries with empty keys (field names)
          if (!type || type.trim() === '') {
            return preserveInvalidMappingsForRustValidation;
          }
          // Keep reference/template entries even with empty values — they're resolved at runtime
          if (valueType === 'reference' || valueType === 'template') {
            return true;
          }
          // Filter out empty optional fields
          if (value === undefined || value === null || value === '') {
            if (preserveInvalidMappingsForRustValidation) {
              return true;
            }
            if (value === null && valueType === 'immediate') {
              return true;
            }
            // Immediate '' is legal DSL: keep entries that were loaded from the
            // step JSON or explicitly authored by the user. Only drop rows the
            // editor auto-seeded from the capability/child-workflow schema and
            // that still hold their untouched empty value (autoSeeded flag set
            // at seed time, see InputMappingField auto-populate).
            return (
              !emptyStringMeansCleared &&
              value === '' &&
              valueType === 'immediate' &&
              !autoSeeded
            );
          }
          return true;
        }
      );
      const normalizedMapping = preserveInvalidMappingsForRustValidation
        ? filteredMapping.map((entry: { value?: any }) =>
            entry.value === undefined ? { ...entry, value: '' } : entry
          )
        : filteredMapping;

      // Error steps need direct field values, not InputMapping wrapping
      if (data.stepType === 'Error') {
        // Extract error fields as direct values to match backend DSL
        const errorFields = ['code', 'message', 'category', 'severity'];
        errorFields.forEach((field) => {
          delete filteredRestData[field];
        });
        normalizedMapping.forEach(
          ({
            type,
            value,
            valueType,
          }: {
            type: string;
            value: any;
            valueType?: string;
          }) => {
            if (errorFields.includes(type)) {
              filteredRestData[type] = value; // Direct string value
            } else if (type === 'context') {
              const context = serializeMappingObject(value, valueType);
              if (context && Object.keys(context).length > 0) {
                filteredRestData.context = context;
              } else {
                delete filteredRestData.context;
              }
            }
          }
        );
        // Don't include inputMapping for Error steps
        cleanedInputMapping = undefined;
      } else if (data.stepType === 'Log') {
        // Log steps need direct field values (message, level) to match backend DSL
        const logFields = ['message', 'level'];
        logFields.forEach((field) => {
          delete filteredRestData[field];
        });
        normalizedMapping.forEach(
          ({
            type,
            value,
            valueType,
          }: {
            type: string;
            value: any;
            valueType?: string;
          }) => {
            if (logFields.includes(type)) {
              filteredRestData[type] = value;
            } else if (type === 'context') {
              const context = serializeMappingObject(value, valueType);
              if (context && Object.keys(context).length > 0) {
                filteredRestData.context = context;
              } else {
                delete filteredRestData.context;
              }
            }
          }
        );
        // Don't include inputMapping for Log steps
        cleanedInputMapping = undefined;
      } else {
        // Regular steps - flat object format with InputMapping wrapping
        const mappingObject = Object.fromEntries(
          normalizedMapping.map(processMappingEntry)
        );
        // Only include inputMapping if it has entries
        cleanedInputMapping =
          Object.keys(mappingObject).length > 0 ? mappingObject : undefined;
      }
    }

    const normalizedInputSchema =
      inputSchema &&
      typeof inputSchema === 'object' &&
      Object.keys(inputSchema).length > 0
        ? inputSchema
        : undefined;

    cleaned[id] = {
      ...filteredRestData,
      ...(normalizedInputSchema ? { inputSchema: normalizedInputSchema } : {}),
      ...(cleanedInputMapping !== undefined
        ? { inputMapping: cleanedInputMapping }
        : {}),
      renderingParameters: {
        ...measured,
        ...position,
      },
    };

    if (restData.stepType === 'Agent' && data.capabilityId) {
      cleaned[id].capabilityId = data.capabilityId;
    }

    if (restData.stepType !== 'Agent') {
      delete cleaned[id].agentId;
      delete cleaned[id].capabilityId;
      delete cleaned[id].compensation;
    }
    if (restData.stepType !== 'Agent' && restData.stepType !== 'AiAgent') {
      delete cleaned[id].connectionId;
    }
    if (
      restData.stepType !== 'Agent' &&
      restData.stepType !== 'EmbedWorkflow'
    ) {
      delete cleaned[id].maxRetries;
      delete cleaned[id].retryDelay;
      delete cleaned[id].timeout;
    }
    if (restData.stepType !== 'Split') {
      delete cleaned[id].inputSchema;
    }

    // Include processed subgraph for container steps (Split)
    // The subgraph is reconstructed by composeExecutionGraph from child nodes with parentId,
    // and processed via the recursive cleanNodeData call at lines 504-510.
    if (subgraph) {
      cleaned[id].subgraph = data.subgraph;
    }

    if (restData.stepType === 'Conditional' && (restData as any).condition) {
      delete cleaned[id].inputMapping;
      cleaned[id].condition = normalizeConditionExpression(
        (restData as any).condition
      );
    }

    if (restData.stepType === 'Delay') {
      const durationItem = Array.isArray(inputMapping)
        ? inputMapping.find((item: any) => item.type === 'durationMs')
        : undefined;
      delete cleaned[id].inputMapping;
      if (durationItem?.value !== undefined && durationItem.value !== '') {
        const [, durationValue] = processMappingEntry({
          type: 'durationMs',
          value: durationItem.value,
          typeHint: durationItem.typeHint || 'number',
          valueType: durationItem.valueType || 'immediate',
          defaultValue: durationItem.defaultValue,
        }) as [string, unknown];
        cleaned[id].durationMs = durationValue;
      }
    }

    // Ensure EmbedWorkflow has childWorkflowId and childVersion at root level (DSL v2.0.0 requirement)
    if (restData.stepType === 'EmbedWorkflow') {
      if (childWorkflowId) {
        cleaned[id].childWorkflowId = childWorkflowId;
      }
      if (childVersion !== undefined) {
        // Backend ChildVersion is an untagged enum: string ("latest"/"current") or integer.
        // Convert numeric strings to integers so serde deserializes to Specific(i32).
        const v = childVersion;
        const num = Number(v);
        cleaned[id].childVersion =
          typeof v === 'string' &&
          v !== '' &&
          !isNaN(num) &&
          v !== 'latest' &&
          v !== 'current'
            ? num
            : v;
      }
    }

    // Split step: use config instead of inputMapping, include schemas
    if (restData.stepType === 'Split') {
      // Remove inputMapping for Split steps - we use config instead
      delete cleaned[id].inputMapping;
      const existingSplitConfig = (restData as any).config;

      // Build the config object for Split step
      const splitConfig: {
        value?: {
          valueType: 'reference' | 'immediate' | 'template' | 'composite';
          value: unknown;
          type?: string;
          default?: unknown;
        };
        parallelism?: number;
        sequential?: boolean;
        dontStopOnFailed?: boolean;
        variables?: Record<
          string,
          {
            valueType: 'reference' | 'immediate' | 'composite' | 'template';
            value: unknown;
            type?: string;
          }
        >;
        maxRetries?: number;
        retryDelay?: number;
        timeout?: number;
        allowNull?: boolean;
        convertSingleValue?: boolean;
        batchSize?: number;
      } = {};

      const sourceValue = serializeSourceMappingValue(inputMapping);
      if (sourceValue) {
        splitConfig.value = sourceValue;
      }

      // Keep existing value if form inputMapping is temporarily empty.
      // This prevents emitting invalid Split config without the required source value.
      if (
        !splitConfig.value &&
        hasMappingValuePayload(existingSplitConfig?.value)
      ) {
        splitConfig.value = existingSplitConfig.value;
      }

      // Add execution options from the form data
      if (data.splitParallelism !== undefined && data.splitParallelism !== 0) {
        splitConfig.parallelism = data.splitParallelism;
      }
      if (data.splitSequential === true) {
        splitConfig.sequential = true;
      }
      if (data.splitDontStopOnFailed === true) {
        splitConfig.dontStopOnFailed = true;
      }
      const advancedSplitFields: Array<
        [keyof typeof splitConfig, unknown, (value: unknown) => unknown]
      > = [
        ['maxRetries', splitMaxRetries, Number],
        ['retryDelay', splitRetryDelay, Number],
        ['timeout', splitTimeout, Number],
        ['allowNull', splitAllowNull, Boolean],
        ['convertSingleValue', splitConvertSingleValue, Boolean],
        ['batchSize', splitBatchSize, Number],
      ];
      for (const [field, formValue, coerce] of advancedSplitFields) {
        if (formValue !== undefined && formValue !== null && formValue !== '') {
          (splitConfig as any)[field] = coerce(formValue);
        } else if (existingSplitConfig?.[field] !== undefined) {
          (splitConfig as any)[field] = existingSplitConfig[field];
        }
      }

      // Add variables from splitVariablesFields. Route every variable through
      // the shared processMappingEntry path so templates serialize as real
      // template MappingValues, typed immediates (number/boolean) are coerced
      // from their form strings, composites are normalized to backend format,
      // and reference type hints only carry legal backend ValueTypes.
      if (
        Array.isArray(data.splitVariablesFields) &&
        data.splitVariablesFields.length > 0
      ) {
        const variables: Record<
          string,
          {
            valueType: 'reference' | 'immediate' | 'composite' | 'template';
            value: unknown;
            type?: string;
          }
        > = {};
        for (const varField of data.splitVariablesFields) {
          const variableName =
            typeof varField.name === 'string' ? varField.name.trim() : '';
          if (variableName && varField.value !== undefined) {
            const resolvedValueType:
              | 'reference'
              | 'immediate'
              | 'composite'
              | 'template' =
              varField.valueType ||
              (typeof varField.value === 'object' && varField.value !== null
                ? 'composite'
                : 'immediate');
            const [, mappingValue] = processMappingEntry({
              type: variableName,
              value: varField.value,
              typeHint: varField.type,
              valueType: resolvedValueType,
            }) as [string, (typeof variables)[string]];
            variables[variableName] = mappingValue;
          }
        }
        if (Object.keys(variables).length > 0) {
          splitConfig.variables = variables;
        }
      }

      // Backend requires config.value to exist for Split config.
      // Preserve variables/options even when source is not chosen yet by sending an empty value placeholder.
      if (
        !splitConfig.value &&
        (splitConfig.variables ||
          splitConfig.parallelism !== undefined ||
          splitConfig.sequential !== undefined ||
          splitConfig.dontStopOnFailed !== undefined ||
          splitConfig.maxRetries !== undefined ||
          splitConfig.retryDelay !== undefined ||
          splitConfig.timeout !== undefined ||
          splitConfig.allowNull !== undefined ||
          splitConfig.convertSingleValue !== undefined ||
          splitConfig.batchSize !== undefined)
      ) {
        splitConfig.value = {
          valueType: 'reference',
          value: '',
        };
      }

      // Preserve split config when any split settings were provided.
      if (
        splitConfig.value ||
        splitConfig.variables ||
        splitConfig.parallelism !== undefined ||
        splitConfig.sequential !== undefined ||
        splitConfig.dontStopOnFailed !== undefined ||
        splitConfig.maxRetries !== undefined ||
        splitConfig.retryDelay !== undefined ||
        splitConfig.timeout !== undefined ||
        splitConfig.allowNull !== undefined ||
        splitConfig.convertSingleValue !== undefined ||
        splitConfig.batchSize !== undefined
      ) {
        cleaned[id].config = splitConfig;
      }

      // Add outputSchema if defined
      if (
        outputSchema &&
        typeof outputSchema === 'object' &&
        Object.keys(outputSchema).length > 0
      ) {
        cleaned[id].outputSchema = outputSchema;
      }
    }

    // Switch step: use config instead of inputMapping
    if (restData.stepType === 'Switch') {
      // Remove inputMapping for Switch steps - we use config instead
      delete cleaned[id].inputMapping;
      delete cleaned[id].switchRoutingMode;

      const switchConfig: {
        value?: {
          valueType: string;
          value: unknown;
          type?: string;
          default?: unknown;
        };
        cases?: Array<{
          match: any;
          matchType: string;
          output: any;
          route?: string;
        }>;
        default?: any;
      } = {};

      if (Array.isArray(inputMapping)) {
        // Serialize the switch value through the shared mapping path (same as
        // Split/Filter/GroupBy) so reference type hints, fallback defaults,
        // and composite values round-trip.
        const sourceValue = serializeSourceMappingValue(inputMapping);
        if (sourceValue) {
          switchConfig.value = sourceValue;
        }

        // Extract cases
        const casesItem = inputMapping.find(
          (item: any) => item.type === 'cases'
        );
        if (
          casesItem?.value &&
          Array.isArray(casesItem.value) &&
          casesItem.value.length > 0
        ) {
          switchConfig.cases = casesItem.value.map((c: any) => ({
            match: c.match,
            matchType: mapMatchTypeToAPI(c.matchType),
            output: c.output,
            ...(c.route ? { route: c.route } : {}),
          }));
        }

        // Extract default — only when the entry was authored. An absent
        // default means "no match is an error" at runtime, so fabricating
        // one (or persisting a cleared '') would change semantics.
        const defaultItem = inputMapping.find(
          (item: any) => item.type === 'default'
        );
        if (
          defaultItem !== undefined &&
          defaultItem.value !== undefined &&
          defaultItem.value !== ''
        ) {
          switchConfig.default = defaultItem.value;
        }
      }

      // SwitchConfig requires `value` (deny_unknown_fields + mandatory field
      // on the backend); never emit a config object lacking it.
      if (switchConfig.value) {
        cleaned[id].config = switchConfig;
      }
    }

    // Filter step: use config instead of inputMapping
    if (restData.stepType === 'Filter') {
      delete cleaned[id].inputMapping;
      delete cleaned[id].filterCondition;
      const existingFilterConfig = (restData as any).config;

      const filterConfig: {
        value?: any;
        condition?: any;
      } = {};

      const sourceValue = serializeSourceMappingValue(inputMapping);
      if (sourceValue) {
        filterConfig.value = sourceValue;
      } else if (hasMappingValuePayload(existingFilterConfig?.value)) {
        filterConfig.value = existingFilterConfig.value;
      }

      // Add condition from form data
      if (filterCondition) {
        filterConfig.condition = normalizeConditionExpression(filterCondition);
      } else if (existingFilterConfig?.condition) {
        filterConfig.condition = normalizeConditionExpression(
          existingFilterConfig.condition
        );
      }

      // Only add config if it has the required fields
      if (filterConfig.value && filterConfig.condition) {
        cleaned[id].config = filterConfig;
      }
    }

    // While step: serialize condition and config
    if (restData.stepType === 'While') {
      delete cleaned[id].inputMapping;
      delete cleaned[id].whileCondition;
      delete cleaned[id].whileMaxIterations;
      delete cleaned[id].whileTimeout;

      // Set condition at root level (API expects WhileStep.condition)
      if (whileCondition) {
        cleaned[id].condition = normalizeConditionExpression(whileCondition);
      }

      // Build config object
      const whileConfig: { maxIterations?: number; timeout?: number | null } =
        {};
      if (whileMaxIterations !== undefined && whileMaxIterations !== null) {
        whileConfig.maxIterations = whileMaxIterations;
      }
      if (whileTimeout !== undefined && whileTimeout !== null) {
        whileConfig.timeout = whileTimeout;
      }

      if (Object.keys(whileConfig).length > 0) {
        cleaned[id].config = whileConfig;
      }
    }

    // GroupBy step: use config instead of inputMapping
    if (restData.stepType === 'GroupBy') {
      delete cleaned[id].inputMapping;
      delete cleaned[id].groupByKey;
      delete cleaned[id].groupByExpectedKeys;
      const existingGroupByConfig = (restData as any).config;

      const groupByConfig: {
        value?: any;
        key?: string;
        expectedKeys?: unknown[];
      } = {};

      const sourceValue = serializeSourceMappingValue(inputMapping);
      if (sourceValue) {
        groupByConfig.value = sourceValue;
      } else if (hasMappingValuePayload(existingGroupByConfig?.value)) {
        groupByConfig.value = existingGroupByConfig.value;
      }

      // Add group key from form data
      if (groupByKey) {
        groupByConfig.key = groupByKey;
      } else if (existingGroupByConfig?.key) {
        groupByConfig.key = existingGroupByConfig.key;
      }

      // Add expected keys from form data (already an array)
      if (
        Array.isArray(groupByExpectedKeys) &&
        groupByExpectedKeys.length > 0
      ) {
        groupByConfig.expectedKeys = groupByExpectedKeys;
      } else if (Array.isArray(existingGroupByConfig?.expectedKeys)) {
        groupByConfig.expectedKeys = existingGroupByConfig.expectedKeys;
      }

      // Only add config if it has the required fields
      if (groupByConfig.value && groupByConfig.key) {
        cleaned[id].config = groupByConfig;
      }
    }

    // AiAgent step: use config instead of inputMapping
    if (restData.stepType === 'AiAgent') {
      delete cleaned[id].inputMapping;

      const aiAgentConfig: {
        systemPrompt?: { valueType: string; value: unknown };
        userPrompt?: { valueType: string; value: unknown };
        provider?: string;
        model?: string | null;
        maxIterations?: number | null;
        temperature?: number | null;
        maxTokens?: number | null;
        maxRetries?: number | null;
        retryDelay?: number | null;
      } = {};

      if (Array.isArray(inputMapping)) {
        const systemPromptItem = inputMapping.find(
          (item: any) => item.type === 'systemPrompt'
        );
        if (
          systemPromptItem?.value !== undefined &&
          systemPromptItem.value !== ''
        ) {
          aiAgentConfig.systemPrompt = {
            valueType: systemPromptItem.valueType || 'immediate',
            value: systemPromptItem.value,
          };
        }

        const userPromptItem = inputMapping.find(
          (item: any) => item.type === 'userPrompt'
        );
        if (
          userPromptItem?.value !== undefined &&
          userPromptItem.value !== ''
        ) {
          aiAgentConfig.userPrompt = {
            valueType: userPromptItem.valueType || 'immediate',
            value: userPromptItem.value,
          };
        }

        const providerItem = inputMapping.find(
          (item: any) => item.type === 'provider'
        );
        if (providerItem?.value) {
          aiAgentConfig.provider = String(providerItem.value);
        }

        const modelItem = inputMapping.find(
          (item: any) => item.type === 'model'
        );
        if (modelItem?.value) {
          aiAgentConfig.model = String(modelItem.value);
        }

        const maxIterationsItem = inputMapping.find(
          (item: any) => item.type === 'maxIterations'
        );
        if (
          maxIterationsItem?.value !== undefined &&
          maxIterationsItem.value !== ''
        ) {
          aiAgentConfig.maxIterations = Number(maxIterationsItem.value);
        }

        const temperatureItem = inputMapping.find(
          (item: any) => item.type === 'temperature'
        );
        if (
          temperatureItem?.value !== undefined &&
          temperatureItem.value !== ''
        ) {
          aiAgentConfig.temperature = Number(temperatureItem.value);
        }

        const maxTokensItem = inputMapping.find(
          (item: any) => item.type === 'maxTokens'
        );
        if (maxTokensItem?.value !== undefined && maxTokensItem.value !== '') {
          aiAgentConfig.maxTokens = Number(maxTokensItem.value);
        }

        const maxRetriesItem = inputMapping.find(
          (item: any) => item.type === 'maxRetries'
        );
        if (
          maxRetriesItem?.value !== undefined &&
          maxRetriesItem.value !== ''
        ) {
          aiAgentConfig.maxRetries = Number(maxRetriesItem.value);
        }

        const retryDelayItem = inputMapping.find(
          (item: any) => item.type === 'retryDelay'
        );
        if (
          retryDelayItem?.value !== undefined &&
          retryDelayItem.value !== ''
        ) {
          aiAgentConfig.retryDelay = Number(retryDelayItem.value);
        }
      }

      // Memory config: serialize from inputMapping entries into config.memory
      const memoryEnabledItem = inputMapping.find(
        (item: any) => item.type === 'memoryEnabled'
      );
      if (memoryEnabledItem?.value === true) {
        const memoryConfig: {
          conversationId?: { valueType: string; value: unknown };
          compaction?: { maxMessages?: number; strategy?: string };
        } = {};

        const conversationIdItem = inputMapping.find(
          (item: any) => item.type === 'memoryConversationId'
        );
        // Always include conversationId when memory is enabled — backend requires it
        memoryConfig.conversationId = {
          valueType: conversationIdItem?.valueType || 'reference',
          value: conversationIdItem?.value ?? '',
        };

        const maxMessagesItem = inputMapping.find(
          (item: any) => item.type === 'memoryMaxMessages'
        );
        const strategyItem = inputMapping.find(
          (item: any) => item.type === 'memoryStrategy'
        );
        if (
          (maxMessagesItem?.value !== undefined &&
            maxMessagesItem.value !== '') ||
          (strategyItem?.value !== undefined && strategyItem.value !== '')
        ) {
          memoryConfig.compaction = {};
          if (
            maxMessagesItem?.value !== undefined &&
            maxMessagesItem.value !== ''
          ) {
            memoryConfig.compaction.maxMessages = Number(maxMessagesItem.value);
          }
          if (strategyItem?.value) {
            memoryConfig.compaction.strategy = String(strategyItem.value);
          }
        }

        (aiAgentConfig as any).memory = memoryConfig;
      }

      // Output schema: convert SchemaField[] → Record<string, SchemaField>
      const outputSchemaItem = inputMapping.find(
        (item: any) => item.type === 'outputSchema'
      );
      if (
        outputSchemaItem?.value &&
        Array.isArray(outputSchemaItem.value) &&
        outputSchemaItem.value.length > 0
      ) {
        (aiAgentConfig as any).outputSchema = buildSchemaFromFields(
          outputSchemaItem.value
        );
      }

      cleaned[id].config = aiAgentConfig;
    }

    // WaitForSignal step: fields are top-level (not nested under config).
    // Cleared form fields must DELETE the stale top-level key — loaded step
    // data is spread into node.data and would otherwise resurrect on save.
    // Semantics of absent keys: timeoutMs = wait indefinitely; pollIntervalMs =
    // runtime default (1000ms); responseSchema = no validation; action = none.
    if (restData.stepType === 'WaitForSignal') {
      delete cleaned[id].inputMapping;

      if (Array.isArray(inputMapping)) {
        // responseSchema: convert SchemaField[] → Record<string, SchemaField>
        const responseSchemaItem = inputMapping.find(
          (item: any) => item.type === 'responseSchema'
        );
        if (
          responseSchemaItem?.value &&
          Array.isArray(responseSchemaItem.value) &&
          responseSchemaItem.value.length > 0
        ) {
          cleaned[id].responseSchema = buildSchemaFromFields(
            responseSchemaItem.value
          );
        } else {
          delete cleaned[id].responseSchema;
        }

        // timeoutMs: serialize as MappingValue if present. The runtime
        // (wait_timeout_ms in runtara-workflow-stdlib) requires the resolved
        // value to be a number, so only immediate (numeric) and reference
        // modes are valid — template renders to a string and composite to an
        // object, both rejected at runtime. Non-numeric immediates (e.g.
        // legacy template strings that would serialize as NaN → null) are
        // dropped instead of emitting invalid JSON.
        const timeoutItem = inputMapping.find(
          (item: any) => item.type === 'timeoutMs'
        );
        delete cleaned[id].timeoutMs;
        if (timeoutItem?.value !== undefined && timeoutItem.value !== '') {
          if (timeoutItem.valueType === 'reference') {
            cleaned[id].timeoutMs = {
              valueType: 'reference',
              value: timeoutItem.value,
            };
          } else {
            const timeoutNumber = Number(timeoutItem.value);
            if (Number.isFinite(timeoutNumber)) {
              cleaned[id].timeoutMs = {
                valueType: 'immediate',
                value: timeoutNumber,
              };
            }
          }
        }

        // pollIntervalMs: serialize as plain integer (backend type is u64;
        // serde rejects decimals)
        const pollItem = inputMapping.find(
          (item: any) => item.type === 'pollIntervalMs'
        );
        delete cleaned[id].pollIntervalMs;
        if (pollItem?.value !== undefined && pollItem.value !== '') {
          const pollNumber = Number(pollItem.value);
          if (Number.isFinite(pollNumber)) {
            cleaned[id].pollIntervalMs = Math.round(pollNumber);
          }
        }

        const actionKeyItem = inputMapping.find(
          (item: any) => item.type === 'actionKey'
        );
        const actionCorrelationItem = inputMapping.find(
          (item: any) => item.type === 'actionCorrelation'
        );
        const actionContextItem = inputMapping.find(
          (item: any) => item.type === 'actionContext'
        );
        const action: Record<string, unknown> = {};
        if (actionKeyItem?.value) {
          action.key = String(actionKeyItem.value);
        }
        const correlation = serializeMappingObject(
          actionCorrelationItem?.value,
          actionCorrelationItem?.valueType
        );
        if (correlation && Object.keys(correlation).length > 0) {
          action.correlation = correlation;
        }
        const context = serializeMappingObject(
          actionContextItem?.value,
          actionContextItem?.valueType
        );
        if (context && Object.keys(context).length > 0) {
          action.context = context;
        }
        if (Object.keys(action).length > 0) {
          cleaned[id].action = action;
        } else {
          delete cleaned[id].action;
        }

        // onWait: the form editor sets `onWait` to undefined when blanked;
        // drop null/undefined leftovers so the key never resurrects.
        if (cleaned[id].onWait == null) {
          delete cleaned[id].onWait;
        }
      }
    }
  }

  return cleaned;
}

export function executionGraphToReactFlow(
  executionGraph: ExecutionGraphDto & { notes?: Note[]; nodes?: unknown }
) {
  // Migrate legacy Start steps if present
  let graphToProcess = executionGraph;
  if (needsStartStepMigration(executionGraph)) {
    const migrationResult = migrateStartStep(executionGraph);
    graphToProcess = migrationResult.executionGraph as ExecutionGraphDto & {
      notes?: Note[];
    };

    // Log migration for debugging
    if (migrationResult.wasMigrated) {
      console.info(
        'Migrated legacy Start step from execution graph.',
        migrationResult.extractedInputSchema ? 'Extracted inputSchema.' : '',
        migrationResult.extractedVariables ? 'Extracted variables.' : ''
      );
    }
  }

  const { steps = {}, executionPlan = [], notes = [] } = graphToProcess;
  const { nodes: parsedNodes, edges } = normalizeNodesAndEdges(
    steps,
    executionPlan || []
  );
  const nodes = applyStoredNodeVisualState(parsedNodes, graphToProcess.nodes);

  // Convert notes to React Flow nodes
  // New format uses: { text, position: {x, y} }
  const noteNodes: Node[] = (notes || []).map((note: any) => {
    const defaultSize = NODE_TYPE_SIZES[NODE_TYPES.NoteNode] || {
      width: 240,
      height: 120,
    };
    const width = snapToGrid(note.metadata?.width ?? defaultSize.width);
    const height = snapToGrid(note.metadata?.height ?? defaultSize.height);

    // Handle position - new format uses position.x/y
    const x = note.position?.x ?? note.x ?? 0;
    const y = note.position?.y ?? note.y ?? 0;

    // Handle content - new format uses "text" field
    const content = note.text ?? note.content ?? '';

    return {
      id: note.id,
      type: NODE_TYPES.NoteNode,
      position: snapPositionToGrid({ x, y }),
      data: {
        id: note.id,
        content,
      },
      width,
      height,
      style: {
        width,
        height,
      },
    };
  });

  return { nodes: [...nodes, ...noteNodes], edges };
}

function normalizeNodesAndEdges(
  steps: Record<string, ExecutionGraphStepDto>,
  executionPlan: ExecutionGraphTransitionDto[],
  parentId?: string
) {
  const nodes: Node[] = [];
  const edges: Edge[] = [];

  // nodes
  for (const [id, step] of Object.entries(steps)) {
    const { subgraph, ...data } = step;
    const { inputMapping = {} } = data;

    // Preserve subgraph-level ExecutionGraph fields (variables, schemas,
    // name, description, notes, entryPoint, ...). Child steps/edges become
    // React Flow nodes and the subgraph is rebuilt from them on save, so
    // anything not carried here would be silently dropped by a save.
    if (subgraph) {
      const {
        steps: _subgraphSteps,
        executionPlan: _subgraphPlan,
        ...subgraphMeta
      } = subgraph as Record<string, unknown>;
      void _subgraphSteps;
      void _subgraphPlan;
      if (Object.keys(subgraphMeta).length > 0) {
        (data as Record<string, unknown>).subgraphMeta = subgraphMeta;
      }
    }

    const nodeType = step.stepType
      ? STEP_TYPES[step.stepType] || NODE_TYPES.BasicNode
      : NODE_TYPES.BasicNode;

    // Helper function to safely parse potentially double-stringified values
    const safeParseValue = (value: any): any => {
      if (typeof value !== 'string') return value;
      try {
        const parsed = JSON.parse(value);
        // If it's still a string after parsing, it might be double-stringified
        if (typeof parsed === 'string') {
          return parsed;
        }
        return parsed;
      } catch {
        return value;
      }
    };

    // Helper function to convert composite values from API format (type) to UI format (typeHint)
    const convertCompositeToUIFormat = (compositeVal: any): any => {
      const convertEntry = (val: any) => {
        const typedVal = val as {
          valueType: 'reference' | 'immediate' | 'composite' | 'template';
          value: any;
          type?: string;
          default?: any;
        };
        if (typedVal.valueType === 'composite') {
          return {
            valueType: 'composite',
            value: convertCompositeToUIFormat(typedVal.value),
            ...(typedVal.type ? { typeHint: typedVal.type } : {}),
          };
        }
        const out: Record<string, any> = {
          valueType: typedVal.valueType,
          value: typedVal.value,
        };
        // Convert backend `type` → UI `typeHint` for every non-composite variant,
        // not only `immediate` — references/templates can carry type hints too.
        if (typedVal.type !== undefined) {
          out.typeHint = typedVal.type;
        }
        // Preserve ReferenceValue.default so it survives the UI round-trip.
        if (
          typedVal.valueType === 'reference' &&
          typedVal.default !== undefined
        ) {
          out.defaultValue = typedVal.default;
        }
        return out;
      };

      // Handle composite object
      if (
        compositeVal &&
        typeof compositeVal === 'object' &&
        !Array.isArray(compositeVal)
      ) {
        const convertedObject: Record<string, any> = {};
        for (const [key, val] of Object.entries(compositeVal)) {
          convertedObject[key] = convertEntry(val);
        }
        return convertedObject;
      }

      // Handle composite array
      if (Array.isArray(compositeVal)) {
        return compositeVal.map(convertEntry);
      }

      // Return as-is if not a composite structure
      return compositeVal;
    };

    // Get correct size for this node type
    const nodeSize = NODE_TYPE_SIZES[nodeType] || {
      width: BASE_WIDTH,
      height: BASE_HEIGHT,
    };

    // Type assertion for extended properties that may exist at runtime
    const extendedData = data as any;
    const parsedInputSchema = safeParseValue((data as any).inputSchema);

    const node: Node = {
      id,
      type: nodeType,
      data: {
        ...data,
        ...(parsedInputSchema ? { inputSchema: parsedInputSchema } : {}),
        // Fix potentially double-stringified values for EmbedWorkflow steps
        ...(extendedData.childWorkflowId && {
          childWorkflowId: safeParseValue(extendedData.childWorkflowId),
        }),
        ...(extendedData.childVersion && {
          childVersion: safeParseValue(extendedData.childVersion),
        }),
        // Handle inputMapping conversion - flat object format
        inputMapping: Object.keys(inputMapping).map((input) => {
          const mappingValue = inputMapping[input];

          // Handle new format: { valueType, value, type?, default? }
          if (
            typeof mappingValue === 'object' &&
            mappingValue !== null &&
            'value' in mappingValue
          ) {
            const typedValue = mappingValue as {
              value: any;
              type?: string;
              default?: any;
              valueType?: 'reference' | 'immediate' | 'composite' | 'template';
            };

            // For composite values, convert nested 'type' fields to 'typeHint'
            if (typedValue.valueType === 'composite') {
              return {
                type: input,
                value: convertCompositeToUIFormat(typedValue.value),
                typeHint: typedValue.type as ValueType | undefined,
                valueType: 'composite' as const,
              };
            }

            const resolvedValueType = typedValue.valueType || 'immediate';
            const entry: {
              type: string;
              value: any;
              typeHint: ValueType | undefined;
              valueType: 'reference' | 'immediate' | 'template';
              defaultValue?: any;
            } = {
              type: input,
              value: typedValue.value, // Can be string, array, object, or composite structure
              typeHint: typedValue.type as ValueType | undefined,
              valueType: resolvedValueType as
                | 'reference'
                | 'immediate'
                | 'template',
            };
            // Preserve ReferenceValue.default so a subsequent save doesn't drop it.
            if (
              resolvedValueType === 'reference' &&
              typedValue.default !== undefined
            ) {
              entry.defaultValue = typedValue.default;
            }
            return entry;
          }

          // Handle legacy format: string value
          return {
            type: input,
            value: mappingValue,
            typeHint: undefined,
            valueType: 'immediate' as const,
          };
        }),
        // For Split steps, parse config, inputSchema and outputSchema into UI fields
        ...(step.stepType === 'Split'
          ? (() => {
              const config = (data as any).config;
              // Convert config.value to inputMapping format for the UI.
              // Carry the backend `type` through so the save path can round-trip it.
              const splitInputMapping = config?.value
                ? [
                    {
                      type: 'value',
                      value: config.value.value,
                      typeHint: config.value.type ?? 'auto',
                      valueType: config.value.valueType || 'reference',
                      ...(config.value.default !== undefined
                        ? { defaultValue: config.value.default }
                        : {}),
                    },
                  ]
                : [];

              // Convert config.variables to splitVariablesFields format
              const splitVariablesFields = config?.variables
                ? Object.entries(config.variables).map(([name, varDef]) => {
                    const typedVarDef = varDef as {
                      valueType?:
                        | 'reference'
                        | 'immediate'
                        | 'composite'
                        | 'template';
                      value: unknown;
                      type?: string;
                    };
                    const resolvedValueType:
                      | 'reference'
                      | 'immediate'
                      | 'composite'
                      | 'template' =
                      typedVarDef.valueType ||
                      (typeof typedVarDef.value === 'object' &&
                      typedVarDef.value !== null
                        ? 'composite'
                        : 'reference');

                    // Keep immediates/references as-is so round-trip is lossless.
                    // Composites: arrays stay arrays; object-shaped values stay objects.
                    // SplitStepField renders scalars via JSON.stringify when needed, so
                    // we don't need to coerce at load time.
                    const resolvedValue =
                      resolvedValueType === 'composite'
                        ? Array.isArray(typedVarDef.value)
                          ? typedVarDef.value
                          : typeof typedVarDef.value === 'object' &&
                              typedVarDef.value !== null
                            ? typedVarDef.value
                            : {}
                        : typedVarDef.value === undefined
                          ? ''
                          : typedVarDef.value;

                    return {
                      name,
                      value: resolvedValue,
                      // Only forward `type` when backend had it; don't synthesize a default
                      // or we'll asymmetrically inject a field that wasn't there on load.
                      ...(typedVarDef.type !== undefined
                        ? { type: typedVarDef.type }
                        : {}),
                      valueType: resolvedValueType,
                    };
                  })
                : [];

              return {
                // Override inputMapping with config.value for Split steps
                inputMapping: splitInputMapping,
                splitInputSchemaFields: parsedInputSchema
                  ? parseSchema(parsedInputSchema).map(
                      normalizeSchemaFieldForEditor
                    )
                  : [],
                splitOutputSchemaFields: (data as any).outputSchema
                  ? parseSchema((data as any).outputSchema).map(
                      normalizeSchemaFieldForEditor
                    )
                  : [],
                outputSchema: safeParseValue((data as any).outputSchema),
                // Config fields
                splitVariablesFields,
                splitParallelism: config?.parallelism ?? 0,
                splitSequential: config?.sequential ?? false,
                splitDontStopOnFailed: config?.dontStopOnFailed ?? false,
                splitMaxRetries: config?.maxRetries ?? undefined,
                splitRetryDelay: config?.retryDelay ?? undefined,
                splitTimeout: config?.timeout ?? undefined,
                splitAllowNull: config?.allowNull ?? false,
                splitConvertSingleValue: config?.convertSingleValue ?? false,
                splitBatchSize: config?.batchSize ?? undefined,
              };
            })()
          : {}),
        // For Switch steps, parse config into inputMapping format for the UI
        ...((step.stepType as string) === 'Switch'
          ? (() => {
              const config = (data as any).config;
              const switchInputMapping: any[] = [];

              // Convert config.value to value field. Carry the backend `type`
              // and `default` through so the save path can round-trip them.
              if (config?.value) {
                switchInputMapping.push({
                  type: 'value',
                  value: config.value.value,
                  typeHint: config.value.type ?? 'auto',
                  valueType: config.value.valueType || 'reference',
                  ...(config.value.default !== undefined
                    ? { defaultValue: config.value.default }
                    : {}),
                });
              } else {
                switchInputMapping.push({
                  type: 'value',
                  value: '',
                  typeHint: 'auto',
                  valueType: 'reference',
                });
              }

              // Convert config.cases to cases field with UI match types
              const uiCases = (config?.cases || []).map((c: any) => ({
                match: c.match,
                matchType: mapMatchTypeFromAPI(c.matchType),
                output: c.output,
                ...(c.route ? { route: c.route } : {}),
              }));
              switchInputMapping.push({
                type: 'cases',
                value: uiCases,
                typeHint: 'json',
              });

              // Convert config.default — only when the workflow authored one.
              // An absent default means "no match fails the step"; fabricating
              // a {} here would silently change semantics on the next save.
              if (config?.default !== undefined) {
                switchInputMapping.push({
                  type: 'default',
                  value: config.default,
                  typeHint: 'json',
                });
              }

              // Detect routing mode: any case with a route field
              const hasRoutes = uiCases.some(
                (c: any) => c.route && c.route !== ''
              );

              return {
                inputMapping: switchInputMapping,
                ...(hasRoutes
                  ? {
                      switchRoutingMode: true,
                    }
                  : {}),
              };
            })()
          : {}),
        // For Filter steps, parse config into form fields
        ...((step.stepType as string) === 'Filter'
          ? (() => {
              const config = (data as any).config;
              const filterInputMapping = config?.value
                ? [
                    {
                      type: 'value',
                      value: config.value.value,
                      typeHint: config.value.type ?? 'auto',
                      valueType: config.value.valueType || 'reference',
                      ...(config.value.default !== undefined
                        ? { defaultValue: config.value.default }
                        : {}),
                    },
                  ]
                : [];

              return {
                inputMapping: filterInputMapping,
                filterCondition: config?.condition,
              };
            })()
          : {}),
        // For While steps, parse condition and config into form fields
        ...((step.stepType as string) === 'While'
          ? (() => {
              const config = (data as any).config;
              return {
                inputMapping: [],
                whileCondition: (data as any).condition,
                whileMaxIterations: config?.maxIterations ?? 10,
                whileTimeout: config?.timeout ?? null,
              };
            })()
          : {}),
        // For GroupBy steps, parse config into form fields
        ...((step.stepType as string) === 'GroupBy'
          ? (() => {
              const config = (data as any).config;
              const groupByInputMapping = config?.value
                ? [
                    {
                      type: 'value',
                      value: config.value.value,
                      typeHint: config.value.type ?? 'auto',
                      valueType: config.value.valueType || 'reference',
                      ...(config.value.default !== undefined
                        ? { defaultValue: config.value.default }
                        : {}),
                    },
                  ]
                : [];

              return {
                inputMapping: groupByInputMapping,
                groupByKey: config?.key || '',
                groupByExpectedKeys: Array.isArray(config?.expectedKeys)
                  ? config.expectedKeys
                  : [],
              };
            })()
          : {}),
        // For AiAgent steps, parse config into form fields
        ...((step.stepType as string) === 'AiAgent'
          ? (() => {
              const config = (data as any).config;
              const aiInputMapping: any[] = [];

              if (config?.systemPrompt) {
                aiInputMapping.push({
                  type: 'systemPrompt',
                  value: config.systemPrompt.value ?? '',
                  valueType: config.systemPrompt.valueType || 'immediate',
                  typeHint: 'string',
                });
              }

              if (config?.userPrompt) {
                aiInputMapping.push({
                  type: 'userPrompt',
                  value: config.userPrompt.value ?? '',
                  valueType: config.userPrompt.valueType || 'immediate',
                  typeHint: 'string',
                });
              }

              if (config?.provider) {
                aiInputMapping.push({
                  type: 'provider',
                  value: config.provider,
                  valueType: 'immediate',
                  typeHint: 'string',
                });
              }

              if (config?.model) {
                aiInputMapping.push({
                  type: 'model',
                  value: config.model,
                  valueType: 'immediate',
                  typeHint: 'string',
                });
              }

              if (
                config?.maxIterations !== undefined &&
                config.maxIterations !== null
              ) {
                aiInputMapping.push({
                  type: 'maxIterations',
                  value: config.maxIterations,
                  valueType: 'immediate',
                  typeHint: 'integer',
                });
              }

              if (
                config?.temperature !== undefined &&
                config.temperature !== null
              ) {
                aiInputMapping.push({
                  type: 'temperature',
                  value: config.temperature,
                  valueType: 'immediate',
                  typeHint: 'number',
                });
              }

              if (
                config?.maxTokens !== undefined &&
                config.maxTokens !== null
              ) {
                aiInputMapping.push({
                  type: 'maxTokens',
                  value: config.maxTokens,
                  valueType: 'immediate',
                  typeHint: 'integer',
                });
              }

              if (
                config?.maxRetries !== undefined &&
                config.maxRetries !== null
              ) {
                aiInputMapping.push({
                  type: 'maxRetries',
                  value: config.maxRetries,
                  valueType: 'immediate',
                  typeHint: 'integer',
                });
              }

              if (
                config?.retryDelay !== undefined &&
                config.retryDelay !== null
              ) {
                aiInputMapping.push({
                  type: 'retryDelay',
                  value: config.retryDelay,
                  valueType: 'immediate',
                  typeHint: 'integer',
                });
              }

              // Memory config: deserialize config.memory into form fields
              if (config?.memory) {
                aiInputMapping.push({
                  type: 'memoryEnabled',
                  value: true,
                  valueType: 'immediate',
                  typeHint: 'boolean',
                });

                if (config.memory.conversationId) {
                  aiInputMapping.push({
                    type: 'memoryConversationId',
                    value: config.memory.conversationId.value ?? '',
                    valueType:
                      config.memory.conversationId.valueType || 'reference',
                    typeHint: 'string',
                  });
                }

                if (config.memory.compaction) {
                  if (config.memory.compaction.maxMessages !== undefined) {
                    aiInputMapping.push({
                      type: 'memoryMaxMessages',
                      value: config.memory.compaction.maxMessages,
                      valueType: 'immediate',
                      typeHint: 'integer',
                    });
                  }
                  if (config.memory.compaction.strategy) {
                    aiInputMapping.push({
                      type: 'memoryStrategy',
                      value: config.memory.compaction.strategy,
                      valueType: 'immediate',
                      typeHint: 'string',
                    });
                  }
                }

                // Find the memory provider step from execution plan
                const memoryEdge = (executionPlan || []).find(
                  (e) => e.fromStep === id && e.label === 'memory'
                );
                if (memoryEdge?.toStep) {
                  aiInputMapping.push({
                    type: 'memoryProviderStepId',
                    value: memoryEdge.toStep,
                    valueType: 'immediate',
                    typeHint: 'string',
                  });
                }
              }

              // Build tools array from executionPlan edges with labels
              // Filter out 'memory' (memory provider) and 'onError' (error
              // route) labels — they are not tools
              const toolNames = (executionPlan || [])
                .filter(
                  (e) =>
                    e.fromStep === id &&
                    e.label &&
                    e.label !== 'next' &&
                    e.label !== 'default' &&
                    e.label !== 'memory' &&
                    e.label !== 'onError'
                )
                .map((e) => e.label as string);

              if (toolNames.length > 0) {
                aiInputMapping.push({
                  type: 'tools',
                  value: toolNames,
                  valueType: 'immediate',
                  typeHint: 'json',
                });
              }

              // Output schema: convert Record<string, SchemaField> → SchemaField[]
              if (
                config?.outputSchema &&
                typeof config.outputSchema === 'object'
              ) {
                const schemaFields = parseSchema(config.outputSchema);
                aiInputMapping.push({
                  type: 'outputSchema',
                  value: schemaFields.map(normalizeSchemaFieldForEditor),
                  valueType: 'immediate',
                  typeHint: 'json',
                });
              }

              return { inputMapping: aiInputMapping };
            })()
          : {}),
        // For Delay steps, parse durationMs into inputMapping format
        ...((step.stepType as string) === 'Delay'
          ? (() => {
              const delayStep = data as any;
              const duration = delayStep.durationMs;
              return {
                inputMapping: [
                  {
                    type: 'durationMs',
                    value: duration?.value ?? '',
                    valueType: duration?.valueType || 'immediate',
                    typeHint: duration?.type || 'number',
                    ...(duration?.default !== undefined
                      ? { defaultValue: duration.default }
                      : {}),
                  },
                ],
              };
            })()
          : {}),
        // For WaitForSignal steps, parse top-level fields into inputMapping
        ...((step.stepType as string) === 'WaitForSignal'
          ? (() => {
              const waitStep = data as any;
              const waitInputMapping: any[] = [];

              // Convert responseSchema (Record<string,SchemaField>) → SchemaField[]
              const schemaFields = parseSchema(waitStep.responseSchema);
              waitInputMapping.push({
                type: 'responseSchema',
                value: schemaFields.map(normalizeSchemaFieldForEditor),
                valueType: 'immediate',
                typeHint: 'json',
              });

              // Convert timeoutMs (MappingValue)
              if (waitStep.timeoutMs) {
                waitInputMapping.push({
                  type: 'timeoutMs',
                  value: waitStep.timeoutMs.value ?? '',
                  valueType: waitStep.timeoutMs.valueType || 'immediate',
                  typeHint: 'number',
                });
              } else {
                waitInputMapping.push({
                  type: 'timeoutMs',
                  value: '',
                  valueType: 'immediate',
                  typeHint: 'number',
                });
              }

              // Convert pollIntervalMs (number)
              waitInputMapping.push({
                type: 'pollIntervalMs',
                value:
                  waitStep.pollIntervalMs !== undefined &&
                  waitStep.pollIntervalMs !== null
                    ? String(waitStep.pollIntervalMs)
                    : '',
                valueType: 'immediate',
                typeHint: 'number',
              });

              if (waitStep.action?.key) {
                waitInputMapping.push({
                  type: 'actionKey',
                  value: waitStep.action.key,
                  valueType: 'immediate',
                  typeHint: 'string',
                });
              }
              if (waitStep.action?.correlation) {
                waitInputMapping.push({
                  type: 'actionCorrelation',
                  value: convertCompositeToUIFormat(
                    waitStep.action.correlation
                  ),
                  valueType: 'composite',
                  typeHint: 'json',
                });
              }
              if (waitStep.action?.context) {
                waitInputMapping.push({
                  type: 'actionContext',
                  value: convertCompositeToUIFormat(waitStep.action.context),
                  valueType: 'composite',
                  typeHint: 'json',
                });
              }

              return { inputMapping: waitInputMapping };
            })()
          : {}),
        // For Log steps, parse top-level fields into inputMapping
        ...((step.stepType as string) === 'Log'
          ? (() => {
              const logStep = data as any;
              const logInputMapping: any[] = [
                {
                  type: 'message',
                  value: logStep.message || '',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
                {
                  type: 'level',
                  value: logStep.level || 'info',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
              ];
              if (logStep.context) {
                logInputMapping.push({
                  type: 'context',
                  value: convertCompositeToUIFormat(logStep.context),
                  typeHint: 'json',
                  valueType: 'composite',
                });
              }

              return { inputMapping: logInputMapping };
            })()
          : {}),
        // For Error steps, parse top-level fields into inputMapping
        ...((step.stepType as string) === 'Error'
          ? (() => {
              const errorStep = data as any;
              const errorInputMapping: any[] = [
                {
                  type: 'code',
                  value: errorStep.code || '',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
                {
                  type: 'message',
                  value: errorStep.message || '',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
                {
                  type: 'category',
                  value: errorStep.category || 'permanent',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
                {
                  type: 'severity',
                  value: errorStep.severity || 'error',
                  typeHint: 'string',
                  valueType: 'immediate',
                },
              ];
              if (errorStep.context) {
                errorInputMapping.push({
                  type: 'context',
                  value: convertCompositeToUIFormat(errorStep.context),
                  typeHint: 'json',
                  valueType: 'composite',
                });
              }

              return { inputMapping: errorInputMapping };
            })()
          : {}),
      },
      // Resizable nodes (Container, Note) use saved dimensions; others always use config size
      width: snapToGrid(
        nodeType === NODE_TYPES.ContainerNode ||
          nodeType === NODE_TYPES.NoteNode
          ? (data.renderingParameters?.width ?? nodeSize.width)
          : nodeSize.width
      ),
      height: snapToGrid(
        nodeType === NODE_TYPES.ContainerNode ||
          nodeType === NODE_TYPES.NoteNode
          ? (data.renderingParameters?.height ?? nodeSize.height)
          : nodeSize.height
      ),
      style: {
        width: snapToGrid(
          nodeType === NODE_TYPES.ContainerNode ||
            nodeType === NODE_TYPES.NoteNode
            ? (data.renderingParameters?.width ?? nodeSize.width)
            : nodeSize.width
        ),
        height: snapToGrid(
          nodeType === NODE_TYPES.ContainerNode ||
            nodeType === NODE_TYPES.NoteNode
            ? (data.renderingParameters?.height ?? nodeSize.height)
            : nodeSize.height
        ),
      },
      position: snapPositionToGrid({
        x: data.renderingParameters?.x ?? 0,
        y: data.renderingParameters?.y ?? 0,
      }),
    };

    if (parentId) {
      node.parentId = parentId;
      node.extent = 'parent';
      node.expandParent = true;
    }

    nodes.push(node);

    if (subgraph) {
      const { nodes: childNodes, edges: childEdges } = normalizeNodesAndEdges(
        subgraph.steps || {},
        subgraph.executionPlan || [],
        id
      );
      nodes.push(...childNodes);
      edges.push(...childEdges);
    }
  }

  // edges
  for (let i = 0; i < executionPlan.length; i++) {
    const edge = executionPlan[i];
    const sourceStep = steps[edge.fromStep ?? ''];
    const isSwitchSource = (sourceStep?.stepType as string) === 'Switch';

    let sourceHandle: string;

    if (isSwitchSource) {
      // Switch nodes: map edge labels to the correct sourceHandle
      const label = edge.label || 'next';
      if (label === 'default') {
        sourceHandle = 'default';
      } else if (label === 'next') {
        // Value mode: single output
        sourceHandle = 'source';
      } else if (label.startsWith('case-')) {
        // Direct case index label
        sourceHandle = label;
      } else {
        // Route label: find matching case index by route name
        const config = (sourceStep as any).config;
        const cases = config?.cases || [];
        const caseIndex = cases.findIndex((c: any) => c.route === label);
        sourceHandle = caseIndex >= 0 ? `case-${caseIndex}` : label;
      }
    } else {
      // Non-Switch: convert spec labels back to React Flow sourceHandle
      // "next" -> "source" for sequential edges, keep "true"/"false" for Conditional
      sourceHandle =
        !edge.label || edge.label === 'default' || edge.label === 'next'
          ? 'source'
          : edge.label;
    }

    edges.push({
      id: `${edge.fromStep}-${edge.toStep}-${edge.label || 'default'}-${i}`,
      source: edge.fromStep ?? '',
      target: edge.toStep ?? '',
      sourceHandle,
      data: {
        ...(edge.condition !== undefined ? { condition: edge.condition } : {}),
        ...(edge.priority !== undefined ? { priority: edge.priority } : {}),
      },
    });
  }

  return { nodes: ensureContainersContainChildren(nodes), edges };
}
