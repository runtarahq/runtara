import {
  Fragment,
  type ReactNode,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { Edge, Node } from '@xyflow/react';
import {
  AlertCircle,
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Cpu,
  Flag,
  GitBranch,
  GripVertical,
  ListTree,
  Loader2,
  MemoryStick,
  Merge,
  Pause,
  PenLine,
  Plug,
  Plus,
  Repeat,
  Settings2,
  Split,
  Trash2,
  Workflow,
  XCircle,
  Zap,
  type LucideIcon,
} from 'lucide-react';

import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  ConditionEditor,
  type ConditionSchemaFieldInfo,
  type ConditionVariableInfo,
} from '@/shared/components/ui/condition-editor';
import { cn } from '@/lib/utils.ts';
import { NODE_TYPES } from '@/features/workflows/config/workflow.ts';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { canStepHaveErrorHandler } from '@/features/workflows/utils/step-error-support';
import { wouldCreateLoop } from '@/features/workflows/utils/graph-validation';
import {
  type NodeExecutionStatus,
  useExecutionStore,
} from '@/features/workflows/stores/executionStore';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { ExecutionGraphStepDto } from '@/features/workflows/types/execution-graph';

type WorkflowTimelineViewProps = {
  readOnly?: boolean;
  debugInspectMode?: boolean;
  onEditNode?: (nodeId: string) => void;
  editingNodeId?: string | null;
  renderInlineEditor?: (nodeId: string) => ReactNode;
  activeAddStepRequest?: TimelineAddStepRequest | null;
  renderInlineAddStep?: (request: TimelineAddStepRequest) => ReactNode;
  onAddStep?: (request: TimelineAddStepRequest) => void;
  /** Workflow input schema fields for the route condition editor picker. */
  inputSchemaFields?: ConditionSchemaFieldInfo[];
  /** Workflow variables for the route condition editor picker. */
  variables?: ConditionVariableInfo[];
};

export type TimelineAddStepRequest = {
  sourceNodeId?: string;
  sourceHandle?: string;
  targetNodeId?: string;
  parentId?: string;
  pickerMode?: 'all' | 'tool' | 'memory';
  directStep?: {
    stepType: string;
    name: string;
    agentId?: string;
    capabilityId?: string;
    inputMapping?: Record<string, unknown>[];
  };
  aiAgentTool?: boolean;
  aiAgentMemory?: boolean;
  /**
   * Parallel fan-out: create the new step N alongside the existing
   * source->target edge (S->N plus N->T, keeping S->T) instead of splicing it
   * into the edge. The resulting diamond re-converges at the target, which is
   * exactly what validation.rs E073 requires of unconditional fan-out.
   */
  parallelBranch?: boolean;
};

type TimelineItem = {
  node: Node;
  children: TimelineItem[];
  outgoingEdges: Edge[];
  lanes: BranchLane[];
};

type BranchLane = {
  edge: Edge;
  items: TimelineItem[];
  continuationNode?: Node;
};

type BranchLanePlan = {
  edge: Edge;
  nodeIds: Set<string>;
  continuationNode?: Node;
};

type DropPlacement = 'before' | 'after';

type TimelineListContext = {
  key: string;
  type: 'scope' | 'lane';
  parentId?: string;
  branchSourceId?: string;
  branchSourceHandle?: string;
  orderedNodeIds: string[];
  subtreeNodeIdsByRoot: Record<string, string[]>;
};

type DragState = {
  nodeId: string;
  contextKey: string;
} | null;

type DropTarget = {
  nodeId: string;
  contextKey: string;
  placement: DropPlacement;
} | null;

type TimelineDragController = {
  dragging: DragState;
  dropTarget: DropTarget;
  onPointerDown: (
    event: ReactPointerEvent<HTMLElement>,
    item: TimelineItem,
    context: TimelineListContext
  ) => void;
  onMouseDown: (
    event: ReactMouseEvent<HTMLElement>,
    item: TimelineItem,
    context: TimelineListContext
  ) => void;
};

type TimelineRouteController = {
  onFlipConditionalBranches: (nodeId: string) => void;
  onMoveSwitchCase: (
    nodeId: string,
    caseIndex: number,
    direction: 'up' | 'down'
  ) => void;
  onUpdateRouteData: (
    edgeId: string,
    updates: { condition?: unknown; priority?: number }
  ) => void;
  /** Removes an edge — same store path the canvas remove uses. */
  onDeleteRoute: (edgeId: string) => void;
  /** Creates an unconditional edge between two existing steps (join). */
  onConnectSteps: (sourceNodeId: string, targetNodeId: string) => void;
  conditionInputSchemaFields?: ConditionSchemaFieldInfo[];
  conditionVariables?: ConditionVariableInfo[];
};

function areTimelineAddRequestsEqual(
  first: TimelineAddStepRequest | null | undefined,
  second: TimelineAddStepRequest | null | undefined
) {
  return (
    Boolean(first && second) &&
    first?.sourceNodeId === second?.sourceNodeId &&
    first?.sourceHandle === second?.sourceHandle &&
    first?.targetNodeId === second?.targetNodeId &&
    first?.parentId === second?.parentId &&
    first?.pickerMode === second?.pickerMode &&
    first?.directStep?.stepType === second?.directStep?.stepType &&
    first?.directStep?.name === second?.directStep?.name &&
    first?.aiAgentTool === second?.aiAgentTool &&
    first?.aiAgentMemory === second?.aiAgentMemory &&
    first?.parallelBranch === second?.parallelBranch
  );
}

type StepData = Partial<ExecutionGraphStepDto> & {
  content?: string;
};

const excludedNodeTypes = new Set([
  NODE_TYPES.CreateNode,
  NODE_TYPES.NoteNode,
  NODE_TYPES.StartIndicatorNode,
]);

function getStepData(node: Node): StepData {
  return (node.data ?? {}) as StepData;
}

function getStepName(node: Node): string {
  const data = getStepData(node);
  return data.name || data.id || node.id;
}

function getStepType(node: Node): string {
  const data = getStepData(node);
  return data.stepType || 'Step';
}

function isScopeStepType(stepType: string): boolean {
  return (
    stepType === 'Split' || stepType === 'While' || stepType === 'RepeatUntil'
  );
}

function getStepDescription(node: Node): string {
  const data = getStepData(node);

  if (data.description) return data.description;
  if (data.stepType === 'Agent') {
    const agent = data.agentId || 'agent';
    const capability = data.capabilityId || 'capability';
    return `${agent} / ${capability}`;
  }
  if (data.stepType === 'Conditional') return 'Routes execution by condition.';
  if (data.stepType === 'Split') return 'Runs a subgraph for each item.';
  if (data.stepType === 'While')
    return 'Repeats its subgraph while the condition is true.';
  if (data.stepType === 'EmbedWorkflow') return 'Calls another workflow.';
  if (data.stepType === 'Finish') return 'Completes this path.';

  return node.id;
}

function getStepIcon(stepType: string) {
  switch (stepType) {
    case 'Conditional':
      return GitBranch;
    case 'Split':
      return Split;
    case 'While':
    case 'RepeatUntil':
      return Repeat;
    case 'EmbedWorkflow':
      return Workflow;
    case 'Finish':
      return Flag;
    case 'Agent':
      return Zap;
    case 'AiAgent':
    case 'AI Agent':
      return Bot;
    case 'Delay':
    case 'WaitForSignal':
      return Pause;
    case 'Log':
      return PenLine;
    case 'Error':
      return AlertCircle;
    default:
      return Cpu;
  }
}

function getStepBadgeVariant(stepType: string) {
  switch (stepType) {
    case 'Conditional':
    case 'Switch':
      return 'warning' as const;
    case 'Split':
    case 'While':
    case 'RepeatUntil':
      return 'default' as const;
    case 'EmbedWorkflow':
      return 'secondary' as const;
    case 'Finish':
      return 'success' as const;
    default:
      return 'muted' as const;
  }
}

function formatExecutionTime(ms?: number) {
  if (ms === undefined || ms === null) return '';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

function getExecutionBadge(executionStatus?: NodeExecutionStatus) {
  if (!executionStatus) return null;

  const status = executionStatus.status;

  switch (status) {
    case 'completed':
      return {
        label: executionStatus.executionTime
          ? formatExecutionTime(executionStatus.executionTime)
          : 'Completed',
        variant: 'success' as const,
        icon: CheckCircle2,
      };
    case 'running':
    case 'compiling':
      return {
        label: status === 'running' ? 'Running' : 'Compiling',
        variant: 'default' as const,
        icon: Loader2,
        iconClassName: 'animate-spin',
      };
    case 'queued':
      return {
        label: 'Queued',
        variant: 'warning' as const,
        icon: Pause,
      };
    case 'failed':
    case 'timeout':
      return {
        label: status === 'timeout' ? 'Timeout' : 'Failed',
        variant: 'destructive' as const,
        icon: AlertCircle,
      };
    case 'cancelled':
      return {
        label: 'Cancelled',
        variant: 'muted' as const,
        icon: XCircle,
      };
    case 'suspended':
      return {
        label: 'Suspended',
        variant: 'warning' as const,
        icon: Pause,
      };
    default:
      return null;
  }
}

function getExecutionBorderClass(status?: ExecutionStatus) {
  switch (status) {
    case 'running':
    case 'compiling':
      return 'border-blue-500';
    case 'completed':
      return 'border-green-500';
    case 'failed':
    case 'timeout':
      return 'border-red-500';
    case 'queued':
      return 'border-yellow-500';
    case 'suspended':
      return 'border-blue-400';
    case 'cancelled':
      return 'border-gray-400';
    default:
      return '';
  }
}

function getExecutionIconClass(
  status: ExecutionStatus | undefined,
  hasValidationError = false
) {
  if (hasValidationError) return 'border-destructive text-destructive';

  switch (status) {
    case 'running':
    case 'compiling':
      return 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-950 dark:text-blue-300';
    case 'completed':
      return 'border-green-500 bg-green-50 text-green-700 dark:bg-green-950 dark:text-green-300';
    case 'failed':
    case 'timeout':
      return 'border-red-500 bg-red-50 text-red-700 dark:bg-red-950 dark:text-red-300';
    case 'queued':
      return 'border-yellow-500 bg-yellow-50 text-yellow-700 dark:bg-yellow-950 dark:text-yellow-300';
    case 'suspended':
      return 'border-blue-400 bg-slate-50 text-slate-700 dark:bg-slate-900 dark:text-slate-300';
    case 'cancelled':
      return 'border-gray-400 bg-gray-50 text-gray-600 dark:bg-gray-900 dark:text-gray-300';
    default:
      return '';
  }
}

function useTimelineNodeExecutionStatus(
  node: Node
): NodeExecutionStatus | undefined {
  const stepId = getStepData(node).id;

  return useExecutionStore((state) => {
    const statusByNodeId = state.nodeExecutionStatus.get(node.id);
    if (statusByNodeId) return statusByNodeId;

    return typeof stepId === 'string'
      ? state.nodeExecutionStatus.get(stepId)
      : undefined;
  });
}

function getEdgeLabel(edge: Edge): string {
  const explicitLabel =
    typeof edge.label === 'string'
      ? edge.label
      : typeof edge.data?.label === 'string'
        ? edge.data.label
        : undefined;

  if (explicitLabel) return explicitLabel;
  if (!edge.sourceHandle || edge.sourceHandle === 'source') return 'next';
  return edge.sourceHandle;
}

function getSwitchCaseCount(node: Node): number {
  return getSwitchCases(node).length;
}

function getSwitchCases(node: Node): unknown[] {
  const inputMapping = node.data?.inputMapping;
  if (!Array.isArray(inputMapping)) return [];

  const casesField = inputMapping.find(
    (item) =>
      typeof item === 'object' &&
      item !== null &&
      (item as { type?: unknown }).type === 'cases'
  );

  const cases = (casesField as { value?: unknown } | undefined)?.value;
  return Array.isArray(cases) ? cases : [];
}

function isSwitchRoutingMode(node: Node): boolean {
  const inputMapping = node.data?.inputMapping;
  if (!Array.isArray(inputMapping)) return false;

  const routingModeField = inputMapping.find(
    (item) =>
      typeof item === 'object' &&
      item !== null &&
      (item as { type?: unknown }).type === 'routingMode'
  );
  if ((routingModeField as { value?: unknown } | undefined)?.value === true) {
    return true;
  }

  return getSwitchCases(node).some(
    (caseItem) =>
      typeof caseItem === 'object' &&
      caseItem !== null &&
      Boolean((caseItem as { route?: unknown }).route)
  );
}

function getInputMappingValue(node: Node, fieldType: string): unknown {
  const inputMapping = node.data?.inputMapping;
  if (!Array.isArray(inputMapping)) return undefined;

  return (
    inputMapping.find(
      (item) =>
        typeof item === 'object' &&
        item !== null &&
        (item as { type?: unknown }).type === fieldType
    ) as { value?: unknown } | undefined
  )?.value;
}

function isAiAgentStep(node: Node): boolean {
  const stepType = getStepType(node);
  return stepType === 'AiAgent' || stepType === 'AI Agent';
}

function hasAiAgentMemory(node: Node): boolean {
  return getInputMappingValue(node, 'memoryEnabled') === true;
}

function getCaseIndex(label: string): number | null {
  const normalizedLabel = label.trim().toLowerCase();
  const handleMatch = /^case-(\d+)$/.exec(normalizedLabel);
  if (handleMatch) return Number(handleMatch[1]);

  const labelMatch = /^case\s+(\d+)$/.exec(normalizedLabel);
  if (labelMatch) return Number(labelMatch[1]) - 1;

  return null;
}

function getRouteSourceHandle(edge: Edge): string {
  if (edge.sourceHandle) return edge.sourceHandle;

  const label = getEdgeLabel(edge);
  const normalizedLabel = label.trim().toLowerCase();
  const caseIndex = getCaseIndex(label);

  if (normalizedLabel === 'true' || normalizedLabel === 'false') {
    return normalizedLabel;
  }
  if (caseIndex !== null) return `case-${caseIndex}`;
  if (normalizedLabel === 'default') return 'default';
  if (normalizedLabel === 'next') return 'source';

  return 'source';
}

function getBranchEdgeOrder(edge: Edge): [number, number, string] {
  const label = getEdgeLabel(edge);
  const normalizedLabel = label.trim().toLowerCase();
  const caseIndex = getCaseIndex(label);

  if (normalizedLabel === 'true') return [0, 0, label];
  if (normalizedLabel === 'false') return [0, 1, label];
  if (caseIndex !== null) return [1, caseIndex, label];
  if (normalizedLabel === 'default') return [2, 0, label];
  if (normalizedLabel === 'onerror') return [3, 0, label];
  return [4, 0, label];
}

function compareBranchEdges(a: Edge, b: Edge): number {
  const aOrder = getBranchEdgeOrder(a);
  const bOrder = getBranchEdgeOrder(b);

  if (aOrder[0] !== bOrder[0]) return aOrder[0] - bOrder[0];
  if (aOrder[1] !== bOrder[1]) return aOrder[1] - bOrder[1];
  return aOrder[2].localeCompare(bOrder[2]);
}

function isBranchEdge(edge: Edge, outgoingEdges: Edge[]): boolean {
  const label = getEdgeLabel(edge);
  return label !== 'next' || outgoingEdges.length > 1;
}

function createInsertionRequest(
  items: TimelineItem[],
  slotIndex: number,
  listContext: TimelineListContext,
  endTargetNode?: Node
): TimelineAddStepRequest | null {
  const previousItem = items[slotIndex - 1];
  const nextItem = items[slotIndex];
  const targetNode =
    nextItem?.node ?? (!previousItem ? endTargetNode : undefined);

  if (!previousItem && targetNode) {
    if (listContext.type === 'lane' && listContext.branchSourceId) {
      return {
        sourceNodeId: listContext.branchSourceId,
        sourceHandle: listContext.branchSourceHandle || 'source',
        targetNodeId: targetNode.id,
        parentId: listContext.parentId,
      };
    }

    return {
      targetNodeId: targetNode.id,
      parentId: listContext.parentId,
    };
  }

  if (previousItem) {
    if (getStepType(previousItem.node) === 'Finish') return null;

    const directTargetNode = nextItem?.node ?? endTargetNode;
    const nextEdge = directTargetNode
      ? previousItem.outgoingEdges.find(
          (edge) => edge.target === directTargetNode.id
        )
      : undefined;

    if (directTargetNode && !nextEdge) return null;

    return {
      sourceNodeId: previousItem.node.id,
      sourceHandle: nextEdge ? getRouteSourceHandle(nextEdge) : 'source',
      targetNodeId: nextEdge?.target,
      parentId: listContext.parentId,
    };
  }

  if (listContext.type === 'lane' && listContext.branchSourceId) {
    return {
      sourceNodeId: listContext.branchSourceId,
      sourceHandle: listContext.branchSourceHandle || 'source',
      parentId: listContext.parentId,
    };
  }

  return {
    parentId: listContext.parentId,
  };
}

function collectReachableNodeIds(
  startNodeId: string,
  scopedIds: Set<string>,
  outgoingBySource: Map<string, Edge[]>,
  sourceNodeId: string
): Set<string> {
  const reachable = new Set<string>();
  const stack = [startNodeId];

  while (stack.length > 0) {
    const nodeId = stack.pop()!;
    if (nodeId === sourceNodeId || reachable.has(nodeId)) continue;
    if (!scopedIds.has(nodeId)) continue;

    reachable.add(nodeId);

    for (const edge of outgoingBySource.get(nodeId) ?? []) {
      stack.push(edge.target);
    }
  }

  return reachable;
}

function LaneContinuationNode({ node }: { node: Node }) {
  const stepType = getStepType(node);
  const StepIcon = getStepIcon(stepType);
  const executionStatus = useTimelineNodeExecutionStatus(node);
  const isSuspendedExecution = useExecutionStore((s) => s.isSuspended);
  const executionBadge = getExecutionBadge(executionStatus);
  const ExecutionIcon = executionBadge?.icon;

  return (
    <div
      className={cn(
        'flex min-w-0 items-center gap-2 rounded-md border border-dashed bg-muted/30 px-3 py-2 text-sm transition-colors',
        executionStatus && getExecutionBorderClass(executionStatus.status),
        executionStatus?.status === 'suspended' &&
          'border-2 animate-glow-pulse',
        isSuspendedExecution &&
          executionStatus?.status === 'queued' &&
          'opacity-25'
      )}
    >
      <div
        className={cn(
          'flex size-7 shrink-0 items-center justify-center rounded-full border bg-background text-muted-foreground',
          getExecutionIconClass(executionStatus?.status)
        )}
      >
        <StepIcon className="size-3.5" aria-hidden="true" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <span className="truncate font-medium text-foreground">
            {getStepName(node)}
          </span>
          <Badge variant={getStepBadgeVariant(stepType)}>{stepType}</Badge>
          {executionBadge && ExecutionIcon && (
            <Badge variant={executionBadge.variant}>
              <ExecutionIcon
                className={cn('mr-1 size-3', executionBadge.iconClassName)}
                aria-hidden="true"
              />
              {executionBadge.label}
            </Badge>
          )}
          <Badge variant="outline">shared path</Badge>
        </div>
        <p
          className={cn(
            'mt-0.5 truncate text-xs text-muted-foreground',
            executionStatus?.error && 'text-destructive'
          )}
        >
          {executionStatus?.error || getStepDescription(node)}
        </p>
      </div>
    </div>
  );
}

function compareByPosition(a: Node, b: Node): number {
  if (a.position.x !== b.position.x) return a.position.x - b.position.x;
  if (a.position.y !== b.position.y) return a.position.y - b.position.y;
  return getStepName(a).localeCompare(getStepName(b));
}

// Exported for tests.
export function getHiddenNodeIds(nodes: Node[], edges: Edge[]): Set<string> {
  const hiddenNodes = new Set<string>();
  const aiAgentNodes = nodes.filter(
    (node) => node.type === NODE_TYPES.AiAgentNode
  );

  for (const agentNode of aiAgentNodes) {
    for (const edge of edges) {
      // Tool / memory / mcp.<toolset> attachments render inline in the AI
      // Agent card, so their targets are hidden from the timeline. `onError`
      // is a normal error route (compiler: AiAgent error_plan) and must stay
      // visible as a timeline branch.
      if (
        edge.source === agentNode.id &&
        edge.sourceHandle &&
        edge.sourceHandle !== 'source' &&
        edge.sourceHandle !== 'onError'
      ) {
        hiddenNodes.add(edge.target);
      }
    }
  }

  return hiddenNodes;
}

function isRenderableNode(node: Node, hiddenNodeIds: Set<string>): boolean {
  return !hiddenNodeIds.has(node.id) && !excludedNodeTypes.has(node.type || '');
}

function orderNodesInScope(
  allNodes: Node[],
  allEdges: Edge[],
  parentId: string | undefined,
  hiddenNodeIds: Set<string>
): Node[] {
  const scopedNodes = allNodes
    .filter(
      (node) =>
        node.parentId === parentId && isRenderableNode(node, hiddenNodeIds)
    )
    .sort(compareByPosition);
  const scopedIds = new Set(scopedNodes.map((node) => node.id));
  const scopedEdges = allEdges.filter(
    (edge) => scopedIds.has(edge.source) && scopedIds.has(edge.target)
  );

  const indegree = new Map(scopedNodes.map((node) => [node.id, 0]));
  for (const edge of scopedEdges) {
    indegree.set(edge.target, (indegree.get(edge.target) || 0) + 1);
  }

  const nodeById = new Map(scopedNodes.map((node) => [node.id, node]));
  const outgoing = new Map<string, Edge[]>();
  for (const edge of scopedEdges) {
    const edges = outgoing.get(edge.source) ?? [];
    edges.push(edge);
    outgoing.set(edge.source, edges);
  }

  const queue = scopedNodes.filter((node) => indegree.get(node.id) === 0);
  const ordered: Node[] = [];
  const visited = new Set<string>();

  while (queue.length > 0) {
    queue.sort(compareByPosition);
    const current = queue.shift()!;
    if (visited.has(current.id)) continue;

    visited.add(current.id);
    ordered.push(current);

    const nextEdges = outgoing.get(current.id) ?? [];
    for (const edge of nextEdges) {
      const nextInDegree = (indegree.get(edge.target) || 0) - 1;
      indegree.set(edge.target, nextInDegree);
      if (nextInDegree <= 0) {
        const targetNode = nodeById.get(edge.target);
        if (targetNode) queue.push(targetNode);
      }
    }
  }

  for (const node of scopedNodes) {
    if (!visited.has(node.id)) ordered.push(node);
  }

  return ordered;
}

// Exported for tests.
export function buildTimelineItems(
  allNodes: Node[],
  allEdges: Edge[],
  parentId: string | undefined,
  hiddenNodeIds: Set<string>
): TimelineItem[] {
  const scopedNodes = orderNodesInScope(
    allNodes,
    allEdges,
    parentId,
    hiddenNodeIds
  );
  const scopedIds = new Set(scopedNodes.map((node) => node.id));
  const nodeById = new Map(scopedNodes.map((node) => [node.id, node]));
  const scopedEdges = allEdges.filter(
    (edge) => scopedIds.has(edge.source) && scopedIds.has(edge.target)
  );
  const outgoingBySource = new Map<string, Edge[]>();

  for (const edge of scopedEdges) {
    const outgoingEdges = outgoingBySource.get(edge.source) ?? [];
    outgoingEdges.push(edge);
    outgoingBySource.set(edge.source, outgoingEdges);
  }

  const branchReachabilityBySource = new Map<
    string,
    {
      edge: Edge;
      reachable: Set<string>;
      reachCount: Map<string, number>;
    }[]
  >();
  const lanePlansBySource = new Map<string, BranchLanePlan[]>();
  const laneOwnerByNodeId = new Map<string, string>();
  const protectedContinuationNodeIds = new Set<string>();

  for (const node of scopedNodes) {
    const outgoingEdges = outgoingBySource.get(node.id) ?? [];
    const branchEdges = outgoingEdges
      .filter((edge) => isBranchEdge(edge, outgoingEdges))
      .sort(compareBranchEdges);

    if (branchEdges.length === 0) continue;

    const reachableByEdge = branchEdges.map((edge) => ({
      edge,
      reachable: collectReachableNodeIds(
        edge.target,
        scopedIds,
        outgoingBySource,
        node.id
      ),
    }));
    const reachCount = new Map<string, number>();

    for (const { reachable } of reachableByEdge) {
      for (const nodeId of reachable) {
        reachCount.set(nodeId, (reachCount.get(nodeId) ?? 0) + 1);
      }
    }

    for (const [nodeId, count] of reachCount) {
      if (count > 1) {
        protectedContinuationNodeIds.add(nodeId);
      }
    }

    branchReachabilityBySource.set(
      node.id,
      reachableByEdge.map(({ edge, reachable }) => ({
        edge,
        reachable,
        reachCount,
      }))
    );
  }

  for (const node of scopedNodes) {
    const reachableByEdge = branchReachabilityBySource.get(node.id);
    if (!reachableByEdge) continue;

    const lanePlans = reachableByEdge.map(({ edge, reachable, reachCount }) => {
      const nodeIds = new Set<string>();

      for (const nodeId of reachable) {
        if (
          reachCount.get(nodeId) === 1 &&
          !protectedContinuationNodeIds.has(nodeId)
        ) {
          nodeIds.add(nodeId);
          laneOwnerByNodeId.set(nodeId, node.id);
        }
      }

      return {
        edge,
        nodeIds,
        continuationNode: nodeIds.has(edge.target)
          ? undefined
          : nodeById.get(edge.target),
      };
    });

    lanePlansBySource.set(node.id, lanePlans);
  }

  function createItemsFromNodeIds(
    nodeIds: Set<string>,
    ownerNodeId: string
  ): TimelineItem[] {
    return scopedNodes
      .filter((node) => nodeIds.has(node.id))
      .filter((node) => {
        const owner = laneOwnerByNodeId.get(node.id);
        return !owner || owner === ownerNodeId;
      })
      .map((node) => createItem(node));
  }

  function createItem(node: Node): TimelineItem {
    const lanePlans = lanePlansBySource.get(node.id) ?? [];

    return {
      node,
      children: buildTimelineItems(allNodes, allEdges, node.id, hiddenNodeIds),
      outgoingEdges: outgoingBySource.get(node.id) ?? [],
      lanes: lanePlans.map((lane) => ({
        edge: lane.edge,
        continuationNode: lane.continuationNode,
        items: createItemsFromNodeIds(lane.nodeIds, node.id),
      })),
    };
  }

  return scopedNodes
    .filter((node) => !laneOwnerByNodeId.has(node.id))
    .map((node) => createItem(node));
}

function countItems(items: TimelineItem[]): number {
  return items.reduce(
    (sum, item) =>
      sum +
      1 +
      countItems(item.children) +
      item.lanes.reduce((laneSum, lane) => laneSum + countItems(lane.items), 0),
    0
  );
}

function collectTimelineItemNodeIds(item: TimelineItem): Set<string> {
  const nodeIds = new Set<string>([item.node.id]);

  for (const child of item.children) {
    for (const nodeId of collectTimelineItemNodeIds(child)) {
      nodeIds.add(nodeId);
    }
  }

  for (const lane of item.lanes) {
    for (const laneItem of lane.items) {
      for (const nodeId of collectTimelineItemNodeIds(laneItem)) {
        nodeIds.add(nodeId);
      }
    }
  }

  return nodeIds;
}

function createSubtreeNodeIdsByRoot(
  items: TimelineItem[]
): Record<string, string[]> {
  return Object.fromEntries(
    items.map((item) => {
      const nodeIds = collectTimelineItemNodeIds(item);
      nodeIds.delete(item.node.id);
      return [item.node.id, [...nodeIds]];
    })
  );
}

function createScopeListContext(
  parentId: string | undefined,
  items: TimelineItem[]
): TimelineListContext {
  return {
    key: `scope:${parentId ?? 'root'}`,
    type: 'scope',
    parentId,
    orderedNodeIds: items.map((item) => item.node.id),
    subtreeNodeIdsByRoot: createSubtreeNodeIdsByRoot(items),
  };
}

function createLaneListContext(
  parentId: string | undefined,
  lane: BranchLane
): TimelineListContext {
  const branchSourceHandle = getRouteSourceHandle(lane.edge);

  return {
    key: `lane:${parentId ?? 'root'}:${lane.edge.source}:${branchSourceHandle}`,
    type: 'lane',
    parentId,
    branchSourceId: lane.edge.source,
    branchSourceHandle,
    orderedNodeIds: lane.items.map((item) => item.node.id),
    subtreeNodeIdsByRoot: createSubtreeNodeIdsByRoot(lane.items),
  };
}

const defaultTimelineErrorInputMapping: Record<string, unknown>[] = [
  {
    type: 'code',
    value: 'HANDLED_ERROR',
    typeHint: 'string',
    valueType: 'immediate',
  },
  {
    type: 'message',
    value: 'Handled by workflow error route',
    typeHint: 'string',
    valueType: 'immediate',
  },
  {
    type: 'category',
    value: 'permanent',
    typeHint: 'string',
    valueType: 'immediate',
  },
  {
    type: 'severity',
    value: 'error',
    typeHint: 'string',
    valueType: 'immediate',
  },
];

type TimelineRouteAddAction = {
  key: string;
  label: string;
  ariaLabel: string;
  sourceHandle?: string;
  icon: LucideIcon;
  request: TimelineAddStepRequest;
};

function getOutgoingSourceHandles(item: TimelineItem): Set<string> {
  return new Set(item.outgoingEdges.map((edge) => getRouteSourceHandle(edge)));
}

// Exported for tests.
export function getTimelineRouteAddActions(
  item: TimelineItem
): TimelineRouteAddAction[] {
  const node = item.node;
  const sourceNodeId = node.id;
  const parentId = node.parentId;
  const stepName = getStepName(node);
  const stepType = getStepType(node);
  const existingHandles = getOutgoingSourceHandles(item);
  const actions: TimelineRouteAddAction[] = [];

  if (stepType === 'Conditional') {
    for (const sourceHandle of ['true', 'false']) {
      if (existingHandles.has(sourceHandle)) continue;
      actions.push({
        key: `conditional-${sourceHandle}`,
        label: sourceHandle,
        ariaLabel: `Add ${sourceHandle} branch from ${stepName}`,
        sourceHandle,
        icon: GitBranch,
        request: { sourceNodeId, sourceHandle, parentId },
      });
    }
  }

  if (stepType === 'Switch' && isSwitchRoutingMode(node)) {
    getSwitchCases(node).forEach((_caseItem, index) => {
      const sourceHandle = `case-${index}`;
      if (existingHandles.has(sourceHandle)) return;
      actions.push({
        key: sourceHandle,
        label: `case ${index + 1}`,
        ariaLabel: `Add case ${index + 1} route from ${stepName}`,
        sourceHandle,
        icon: GitBranch,
        request: { sourceNodeId, sourceHandle, parentId },
      });
    });

    if (!existingHandles.has('default')) {
      actions.push({
        key: 'default',
        label: 'default',
        ariaLabel: `Add default route from ${stepName}`,
        sourceHandle: 'default',
        icon: GitBranch,
        request: { sourceNodeId, sourceHandle: 'default', parentId },
      });
    }
  }

  // Parallel branch: offered exactly when the step has a single unconditional
  // outgoing edge S->T (handle "source", no edge condition). The new step N is
  // wired S->N and N->T while S->T is kept, forming a diamond that re-converges
  // at T — the shape validation.rs E073 requires of unconditional fan-out.
  const unconditionalEdges = item.outgoingEdges.filter(
    (edge) => getRouteSourceHandle(edge) === 'source'
  );
  if (
    unconditionalEdges.length === 1 &&
    getRouteCondition(unconditionalEdges[0]) === undefined &&
    stepType !== 'Finish'
  ) {
    actions.push({
      key: 'parallel-branch',
      label: 'parallel branch',
      ariaLabel: `Add parallel branch from ${stepName}`,
      sourceHandle: 'source',
      icon: Split,
      request: {
        sourceNodeId,
        sourceHandle: 'source',
        targetNodeId: unconditionalEdges[0].target,
        parentId,
        parallelBranch: true,
      },
    });
  }

  if (!existingHandles.has('onError') && canStepHaveErrorHandler(stepType)) {
    actions.push({
      key: 'onError',
      label: 'error',
      ariaLabel: `Add error handler from ${stepName}`,
      sourceHandle: 'onError',
      icon: AlertCircle,
      request: {
        sourceNodeId,
        sourceHandle: 'onError',
        parentId,
        directStep: {
          stepType: 'Error',
          name: 'Error handler',
          inputMapping: defaultTimelineErrorInputMapping,
        },
      },
    });
  }

  if (isAiAgentStep(node)) {
    actions.push({
      key: 'ai-tool',
      label: 'tool',
      ariaLabel: `Add tool to ${stepName}`,
      icon: Zap,
      request: {
        sourceNodeId,
        parentId,
        pickerMode: 'tool',
        aiAgentTool: true,
      },
    });

    if (!hasAiAgentMemory(node)) {
      actions.push({
        key: 'ai-memory',
        label: 'memory',
        ariaLabel: `Add memory to ${stepName}`,
        icon: MemoryStick,
        request: {
          sourceNodeId,
          parentId,
          pickerMode: 'memory',
          aiAgentMemory: true,
        },
      });
    }
  }

  return actions;
}

/**
 * Creates the add-step request for an `mcp.<toolset>` edge from an AiAgent
 * step. The target mirrors what the validator expects (validation.rs MCP edge
 * rules): an Agent step whose agentId is "mcp".
 * Exported for tests.
 */
export function createMcpToolsetAddRequest(
  node: Node,
  toolset: string
): TimelineAddStepRequest {
  return {
    sourceNodeId: node.id,
    sourceHandle: `mcp.${toolset}`,
    parentId: node.parentId,
    directStep: {
      stepType: 'Agent',
      name: `${toolset} MCP toolset`,
      agentId: 'mcp',
      capabilityId: 'mcp-tool-invoke',
    },
  };
}

/**
 * Whether the timeline offers a "connect to existing step" (join) action on
 * this step: a branch/lane end with no unconditional continuation. Steps whose
 * outgoing edges are inherently labeled (Conditional true/false, routing-mode
 * Switch case/default) and terminal Finish steps are excluded.
 * Exported for tests.
 */
export function canOfferTimelineJoin(item: TimelineItem): boolean {
  const stepType = getStepType(item.node);
  if (stepType === 'Finish' || stepType === 'Conditional') return false;
  if (stepType === 'Switch' && isSwitchRoutingMode(item.node)) return false;
  return !getOutgoingSourceHandles(item).has('source');
}

/**
 * Valid join targets for a lane-end step: renderable steps in the same scope
 * (same parentId), excluding the step itself, steps it already routes to, and
 * any target that would create a cycle (graph-validation.ts check).
 * Exported for tests.
 */
export function getTimelineJoinTargets(
  sourceNode: Node,
  nodes: Node[],
  edges: Edge[]
): Node[] {
  const hiddenNodeIds = getHiddenNodeIds(nodes, edges);

  return nodes
    .filter(
      (candidate) =>
        candidate.id !== sourceNode.id &&
        (candidate.parentId ?? undefined) ===
          (sourceNode.parentId ?? undefined) &&
        isRenderableNode(candidate, hiddenNodeIds) &&
        !edges.some(
          (edge) =>
            edge.source === sourceNode.id && edge.target === candidate.id
        ) &&
        !wouldCreateLoop(edges, sourceNode.id, candidate.id)
    )
    .sort(compareByPosition);
}

/**
 * The edge-only request a timeline join produces: an unconditional edge from
 * the lane-end step to an existing step — no new step is created.
 * Exported for tests.
 */
export function createTimelineJoinRequest(
  sourceNode: Node,
  targetNode: Node
): { sourceNodeId: string; targetNodeId: string; sourceHandle: 'source' } {
  return {
    sourceNodeId: sourceNode.id,
    targetNodeId: targetNode.id,
    sourceHandle: 'source',
  };
}

function TimelineConnectToStepButton({
  node,
  targets,
  onConnectSteps,
}: {
  node: Node;
  targets: Node[];
  onConnectSteps: TimelineRouteController['onConnectSteps'];
}) {
  const [open, setOpen] = useState(false);
  const stepName = getStepName(node);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7 gap-1.5 border-dashed px-2 text-xs text-muted-foreground shadow-none hover:text-foreground"
          aria-label={`Connect ${stepName} to an existing step`}
          data-testid="timeline-join-step"
          data-source-node-id={node.id}
        >
          <Merge className="size-3.5" aria-hidden="true" />
          Connect to step
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" side="bottom" className="w-72 p-2">
        <p className="px-1 pb-2 text-xs text-muted-foreground">
          Continue {stepName} at an existing step in this scope.
        </p>
        <div className="flex max-h-64 flex-col gap-1 overflow-auto">
          {targets.map((target) => {
            const targetStepType = getStepType(target);
            return (
              <Button
                key={target.id}
                type="button"
                variant="ghost"
                size="sm"
                className="h-auto justify-start gap-2 px-2 py-1.5 text-left"
                onClick={() => {
                  const request = createTimelineJoinRequest(node, target);
                  onConnectSteps(request.sourceNodeId, request.targetNodeId);
                  setOpen(false);
                }}
                data-testid="timeline-join-target"
                data-target-node-id={target.id}
              >
                <span className="truncate text-xs font-medium">
                  {getStepName(target)}
                </span>
                <Badge variant={getStepBadgeVariant(targetStepType)}>
                  {targetStepType}
                </Badge>
              </Button>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function isMcpToolsetRequestForNode(
  request: TimelineAddStepRequest | null | undefined,
  nodeId: string
): request is TimelineAddStepRequest {
  return Boolean(
    request &&
      request.sourceNodeId === nodeId &&
      typeof request.sourceHandle === 'string' &&
      request.sourceHandle.startsWith('mcp.')
  );
}

function TimelineAddMcpToolsetButton({
  node,
  onAddStep,
}: {
  node: Node;
  onAddStep: (request: TimelineAddStepRequest) => void;
}) {
  const [open, setOpen] = useState(false);
  const [toolsetName, setToolsetName] = useState('');
  const [error, setError] = useState<string | null>(null);
  // Read all edges from the store: tool/memory/mcp targets are hidden from
  // the timeline, so the item's scoped outgoingEdges miss them.
  const edges = useWorkflowStore((state) => state.edges);
  const stepName = getStepName(node);

  const handleAdd = () => {
    // Server rules (validation.rs AiAgentMcpEdge*): the suffix after "mcp."
    // must be non-empty and unique per AiAgent step — no charset restriction.
    const toolset = toolsetName.trim();
    if (!toolset) {
      setError('Toolset name is required');
      return;
    }
    const duplicate = edges.some(
      (edge) =>
        edge.source === node.id && edge.sourceHandle === `mcp.${toolset}`
    );
    if (duplicate) {
      setError(`Toolset "${toolset}" is already connected`);
      return;
    }

    onAddStep(createMcpToolsetAddRequest(node, toolset));
    setOpen(false);
    setToolsetName('');
    setError(null);
  };

  return (
    <Popover
      open={open}
      onOpenChange={(nextOpen) => {
        setOpen(nextOpen);
        if (!nextOpen) {
          setToolsetName('');
          setError(null);
        }
      }}
    >
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7 gap-1.5 border-dashed px-2 text-xs text-muted-foreground shadow-none hover:text-foreground"
          aria-label={`Add MCP toolset to ${stepName}`}
          data-testid="timeline-add-mcp-toolset"
          data-source-node-id={node.id}
        >
          <Plug className="size-3.5" aria-hidden="true" />
          Add MCP toolset
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" side="bottom" className="w-72 space-y-3">
        <div>
          <p className="text-sm font-semibold text-foreground">MCP toolset</p>
          <p className="text-xs text-muted-foreground">
            Exposes {'<toolset>'}_search and {'<toolset>'}_invoke tools to the
            agent via an mcp.{'<toolset>'} route.
          </p>
        </div>
        <div className="space-y-2">
          <Label htmlFor={`mcp-toolset-name-${node.id}`}>Toolset name</Label>
          <Input
            id={`mcp-toolset-name-${node.id}`}
            value={toolsetName}
            onChange={(event) => {
              setToolsetName(event.target.value);
              setError(null);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                handleAdd();
              }
            }}
            placeholder="e.g. linear"
            data-testid="timeline-mcp-toolset-name"
          />
          {error && <p className="text-xs text-destructive">{error}</p>}
        </div>
        <div className="flex justify-end gap-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => setOpen(false)}
          >
            Cancel
          </Button>
          <Button
            type="button"
            size="sm"
            onClick={handleAdd}
            data-testid="timeline-mcp-toolset-confirm"
          >
            Add
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function TimelineRouteAddControls({
  item,
  depth,
  readOnly,
  debugInspectMode,
  activeAddStepRequest,
  renderInlineAddStep,
  onAddStep,
  routeController,
}: {
  item: TimelineItem;
  depth: number;
  readOnly?: boolean;
  debugInspectMode?: boolean;
  activeAddStepRequest?: TimelineAddStepRequest | null;
  renderInlineAddStep?: (request: TimelineAddStepRequest) => ReactNode;
  onAddStep?: (request: TimelineAddStepRequest) => void;
  routeController: TimelineRouteController;
}) {
  // Join candidates need the full graph: scoped outgoingEdges miss edges into
  // other scopes and the cycle check must see every edge.
  const nodes = useWorkflowStore((state) => state.nodes);
  const edges = useWorkflowStore((state) => state.edges);

  if (readOnly || debugInspectMode || !onAddStep) return null;

  const showMcpToolsetAction = isAiAgentStep(item.node);
  const actions = getTimelineRouteAddActions(item);
  const joinTargets = canOfferTimelineJoin(item)
    ? getTimelineJoinTargets(item.node, nodes, edges)
    : [];
  if (actions.length === 0 && !showMcpToolsetAction && joinTargets.length === 0)
    return null;

  const activeAction = actions.find((action) =>
    areTimelineAddRequestsEqual(action.request, activeAddStepRequest)
  );
  const activeMcpRequest =
    !activeAction &&
    showMcpToolsetAction &&
    isMcpToolsetRequestForNode(activeAddStepRequest, item.node.id)
      ? activeAddStepRequest
      : null;
  const inlineAddStep = activeAction
    ? renderInlineAddStep?.(activeAction.request)
    : activeMcpRequest
      ? renderInlineAddStep?.(activeMcpRequest)
      : null;

  return (
    <div className="mt-2 space-y-2" style={{ marginLeft: depth * 24 + 40 }}>
      <div className="flex min-w-0 flex-wrap items-center gap-1.5">
        {actions.map((action) => {
          const ActionIcon = action.icon;
          return (
            <Button
              key={action.key}
              type="button"
              variant="outline"
              size="sm"
              className="h-7 gap-1.5 border-dashed px-2 text-xs text-muted-foreground shadow-none hover:text-foreground"
              onClick={() => onAddStep(action.request)}
              aria-label={action.ariaLabel}
              data-testid="timeline-add-route"
              data-source-node-id={item.node.id}
              data-source-handle={action.sourceHandle}
            >
              <ActionIcon className="size-3.5" aria-hidden="true" />
              Add {action.label}
            </Button>
          );
        })}
        {showMcpToolsetAction && (
          <TimelineAddMcpToolsetButton node={item.node} onAddStep={onAddStep} />
        )}
        {joinTargets.length > 0 && (
          <TimelineConnectToStepButton
            node={item.node}
            targets={joinTargets}
            onConnectSteps={routeController.onConnectSteps}
          />
        )}
      </div>
      {inlineAddStep && (
        <div className="overflow-hidden rounded-md border border-dashed border-primary/50 bg-card shadow-sm">
          {inlineAddStep}
        </div>
      )}
    </div>
  );
}

function collectTimelineListContexts(
  items: TimelineItem[]
): Map<string, TimelineListContext> {
  const contexts = new Map<string, TimelineListContext>();

  function collectScope(itemsInScope: TimelineItem[], parentId?: string) {
    const scopeContext = createScopeListContext(parentId, itemsInScope);
    contexts.set(scopeContext.key, scopeContext);
    collectNestedContexts(itemsInScope);
  }

  function collectNestedContexts(itemsInScope: TimelineItem[]) {
    for (const item of itemsInScope) {
      for (const lane of item.lanes) {
        const laneContext = createLaneListContext(item.node.parentId, lane);
        contexts.set(laneContext.key, laneContext);
        collectNestedContexts(lane.items);
      }

      if (item.children.length > 0) {
        collectScope(item.children, item.node.id);
      }
    }
  }

  collectScope(items);
  return contexts;
}

function TimelineInsertionPoint({
  request,
  depth,
  activeAddStepRequest,
  renderInlineAddStep,
  onAddStep,
  routeEdge,
  routeSettingsDisabled,
  routeController,
}: {
  request: TimelineAddStepRequest | null;
  depth: number;
  activeAddStepRequest?: TimelineAddStepRequest | null;
  renderInlineAddStep?: (request: TimelineAddStepRequest) => ReactNode;
  onAddStep?: (request: TimelineAddStepRequest) => void;
  routeEdge?: Edge;
  routeSettingsDisabled?: boolean;
  routeController?: TimelineRouteController;
}) {
  if (!request || !onAddStep) return null;

  const inlineAddStep = areTimelineAddRequestsEqual(
    request,
    activeAddStepRequest
  )
    ? renderInlineAddStep?.(request)
    : null;

  if (inlineAddStep) {
    return (
      <div className="py-1" style={{ marginLeft: depth * 24 }}>
        <div className="overflow-hidden rounded-md border border-dashed border-primary/50 bg-card shadow-sm">
          {inlineAddStep}
        </div>
      </div>
    );
  }

  return (
    <div
      className="group flex min-w-0 items-center gap-2 py-1"
      style={{ marginLeft: depth * 24 }}
    >
      <span
        className="h-px flex-1 border-t border-dashed border-border"
        aria-hidden="true"
      />
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="h-7 border-dashed bg-background px-2 text-xs text-muted-foreground shadow-none transition-colors group-hover:border-primary/60 group-hover:text-foreground"
        onClick={() => onAddStep(request)}
        aria-label="Add step here"
        data-testid="timeline-add-step"
        data-source-node-id={request.sourceNodeId}
        data-source-handle={request.sourceHandle}
        data-target-node-id={request.targetNodeId}
        data-parent-node-id={request.parentId}
      >
        <Plus className="size-3.5" aria-hidden="true" />
        Add step
      </Button>
      {routeEdge && routeController && (
        <TimelineRouteSettings
          edge={routeEdge}
          label={getEdgeLabel(routeEdge)}
          disabled={routeSettingsDisabled}
          className="mt-0"
          routeController={routeController}
        />
      )}
      <span
        className="h-px flex-1 border-t border-dashed border-border"
        aria-hidden="true"
      />
    </div>
  );
}

function TimelineItemList({
  items,
  depth,
  listContext,
  readOnly,
  debugInspectMode,
  expandedContainers,
  onToggleContainer,
  onEditNode,
  editingNodeId,
  renderInlineEditor,
  activeAddStepRequest,
  renderInlineAddStep,
  onAddStep,
  dragController,
  routeController,
  endTargetNode,
}: {
  items: TimelineItem[];
  depth: number;
  listContext: TimelineListContext;
  readOnly?: boolean;
  debugInspectMode?: boolean;
  expandedContainers: Record<string, boolean>;
  onToggleContainer: (nodeId: string) => void;
  onEditNode?: (nodeId: string) => void;
  editingNodeId?: string | null;
  renderInlineEditor?: (nodeId: string) => ReactNode;
  activeAddStepRequest?: TimelineAddStepRequest | null;
  renderInlineAddStep?: (request: TimelineAddStepRequest) => ReactNode;
  onAddStep?: (request: TimelineAddStepRequest) => void;
  dragController: TimelineDragController;
  routeController: TimelineRouteController;
  endTargetNode?: Node;
}) {
  const canAdd = !readOnly && !debugInspectMode && Boolean(onAddStep);

  return (
    <div className="flex flex-col gap-2">
      {canAdd && (
        <TimelineInsertionPoint
          request={createInsertionRequest(items, 0, listContext, endTargetNode)}
          depth={depth}
          activeAddStepRequest={activeAddStepRequest}
          renderInlineAddStep={renderInlineAddStep}
          onAddStep={onAddStep}
          routeController={routeController}
        />
      )}
      {items.map((item, index) => {
        const directTargetNode = items[index + 1]?.node ?? endTargetNode;
        const routeEdge = directTargetNode
          ? item.outgoingEdges.find(
              (edge) => edge.target === directTargetNode.id
            )
          : undefined;
        const routeSettingsDisabled =
          readOnly ||
          debugInspectMode ||
          getStepType(item.node) === 'Conditional';

        return (
          <Fragment key={item.node.id}>
            <WorkflowTimelineItem
              item={item}
              depth={depth}
              listContext={listContext}
              readOnly={readOnly}
              debugInspectMode={debugInspectMode}
              expandedContainers={expandedContainers}
              onToggleContainer={onToggleContainer}
              onEditNode={onEditNode}
              editingNodeId={editingNodeId}
              renderInlineEditor={renderInlineEditor}
              activeAddStepRequest={activeAddStepRequest}
              renderInlineAddStep={renderInlineAddStep}
              onAddStep={onAddStep}
              dragController={dragController}
              routeController={routeController}
            />
            {canAdd && (
              <TimelineInsertionPoint
                request={createInsertionRequest(
                  items,
                  index + 1,
                  listContext,
                  endTargetNode
                )}
                depth={depth}
                activeAddStepRequest={activeAddStepRequest}
                renderInlineAddStep={renderInlineAddStep}
                onAddStep={onAddStep}
                routeEdge={routeEdge}
                routeSettingsDisabled={routeSettingsDisabled}
                routeController={routeController}
              />
            )}
          </Fragment>
        );
      })}
    </div>
  );
}

function getRouteCondition(edge: Edge): unknown {
  return (edge.data as { condition?: unknown } | undefined)?.condition;
}

function getRoutePriority(edge: Edge): number | undefined {
  const priority = (edge.data as { priority?: unknown } | undefined)?.priority;
  return typeof priority === 'number' && Number.isFinite(priority)
    ? priority
    : undefined;
}

function formatRouteCondition(condition: unknown): string {
  if (condition === undefined) return '';
  return JSON.stringify(condition, null, 2);
}

function parseRouteCondition(
  value: string
): { ok: true; condition: unknown } | { ok: false; message: string } {
  if (!value.trim()) {
    return { ok: true, condition: undefined };
  }

  try {
    return { ok: true, condition: JSON.parse(value) };
  } catch (error) {
    return {
      ok: false,
      message: error instanceof Error ? error.message : 'Invalid JSON',
    };
  }
}

function parseRoutePriority(
  value: string
): { ok: true; priority: number | undefined } | { ok: false; message: string } {
  if (!value.trim()) {
    return { ok: true, priority: undefined };
  }

  const priority = Number(value);
  if (!Number.isFinite(priority)) {
    return { ok: false, message: 'Priority must be a number.' };
  }

  return { ok: true, priority };
}

/**
 * Whether the visual ConditionEditor can represent this condition. It needs an
 * operation object (`op` + `arguments`); anything else (or nothing) stays in
 * the Advanced JSON editor. Empty conditions are visually editable — the
 * builder starts from scratch.
 */
function isVisuallyEditableRouteCondition(condition: unknown): boolean {
  if (condition === undefined) return true;
  return (
    typeof condition === 'object' &&
    condition !== null &&
    'op' in condition &&
    (condition as { op?: unknown }).op !== undefined &&
    'arguments' in condition
  );
}

function TimelineRouteSettings({
  edge,
  label,
  disabled,
  className,
  routeController,
}: {
  edge: Edge;
  label: string;
  disabled?: boolean;
  className?: string;
  routeController: TimelineRouteController;
}) {
  const [open, setOpen] = useState(false);
  const [conditionText, setConditionText] = useState('');
  const [conditionMode, setConditionMode] = useState<'visual' | 'json'>(
    'visual'
  );
  const [conditionResetKey, setConditionResetKey] = useState(0);
  const [priorityText, setPriorityText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const hasMetadata =
    getRouteCondition(edge) !== undefined ||
    getRoutePriority(edge) !== undefined;

  useEffect(() => {
    const condition = getRouteCondition(edge);
    setConditionText(formatRouteCondition(condition));
    setConditionMode(
      isVisuallyEditableRouteCondition(condition) ? 'visual' : 'json'
    );
    setConditionResetKey((key) => key + 1);
    setPriorityText(
      getRoutePriority(edge) !== undefined ? String(getRoutePriority(edge)) : ''
    );
    setError(null);
  }, [edge.id, edge.data]);

  const handleApply = () => {
    const conditionResult = parseRouteCondition(conditionText);
    if (!conditionResult.ok) {
      setError(`Condition JSON: ${conditionResult.message}`);
      return;
    }

    const priorityResult = parseRoutePriority(priorityText);
    if (!priorityResult.ok) {
      setError(priorityResult.message);
      return;
    }

    routeController.onUpdateRouteData(edge.id, {
      condition: conditionResult.condition,
      priority: priorityResult.priority,
    });
    setError(null);
    setOpen(false);
  };

  const handleClear = () => {
    setConditionText('');
    setPriorityText('');
    setError(null);
    setConditionResetKey((key) => key + 1);
    routeController.onUpdateRouteData(edge.id, {
      condition: undefined,
      priority: undefined,
    });
  };

  const handleDelete = () => {
    routeController.onDeleteRoute(edge.id);
    setOpen(false);
  };

  const handleToggleConditionMode = () => {
    if (conditionMode === 'visual') {
      setConditionMode('json');
      return;
    }

    const conditionResult = parseRouteCondition(conditionText);
    if (!conditionResult.ok) {
      setError(`Condition JSON: ${conditionResult.message}`);
      return;
    }
    if (!isVisuallyEditableRouteCondition(conditionResult.condition)) {
      setError('This condition shape is only editable as JSON.');
      return;
    }

    setError(null);
    setConditionResetKey((key) => key + 1);
    setConditionMode('visual');
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className={cn(
            'ml-0.5 mt-1 size-6 rounded-sm bg-background text-muted-foreground',
            hasMetadata && 'text-primary',
            className
          )}
          disabled={disabled}
          aria-label={`Edit ${label} route condition and priority`}
          title="Route condition and priority"
        >
          <Settings2 className="size-3.5" aria-hidden="true" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        side="right"
        className="w-[26rem] space-y-4"
        onOpenAutoFocus={(event) => event.preventDefault()}
      >
        <div>
          <p className="text-sm font-semibold text-foreground">
            Route Settings
          </p>
          <p className="text-xs text-muted-foreground">
            {label} route from {edge.source} to {edge.target}
          </p>
        </div>

        <div className="space-y-2">
          <Label htmlFor={`route-priority-${edge.id}`}>Priority</Label>
          <Input
            id={`route-priority-${edge.id}`}
            type="number"
            inputMode="numeric"
            value={priorityText}
            onChange={(event) => setPriorityText(event.target.value)}
            placeholder="Optional"
          />
        </div>

        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <Label htmlFor={`route-condition-${edge.id}`}>Condition</Label>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-6 px-2 text-xs text-muted-foreground"
              onClick={handleToggleConditionMode}
              data-testid="timeline-route-condition-mode"
            >
              {conditionMode === 'visual' ? 'Advanced (JSON)' : 'Visual editor'}
            </Button>
          </div>
          {conditionMode === 'visual' ? (
            <ConditionEditor
              key={`${edge.id}-${conditionResetKey}`}
              value={conditionText || undefined}
              onChange={setConditionText}
              previousSteps={[]}
              inputSchemaFields={routeController.conditionInputSchemaFields}
              variables={routeController.conditionVariables}
            />
          ) : (
            <Textarea
              id={`route-condition-${edge.id}`}
              value={conditionText}
              onChange={(event) => setConditionText(event.target.value)}
              placeholder='{"type": "operation", "op": "EQ", "arguments": ["data.status", "ready"]}'
              className="min-h-32 font-mono text-xs"
            />
          )}
          {error && <p className="text-xs text-destructive">{error}</p>}
        </div>

        <div className="flex items-center justify-between gap-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="gap-1.5 text-destructive hover:text-destructive"
            onClick={handleDelete}
            disabled={disabled}
            aria-label={`Delete ${label} route from ${edge.source} to ${edge.target}`}
            data-testid="timeline-route-delete"
          >
            <Trash2 className="size-3.5" aria-hidden="true" />
            Delete route
          </Button>
          <div className="flex gap-2">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={handleClear}
            >
              Clear
            </Button>
            <Button type="button" size="sm" onClick={handleApply}>
              Apply
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function BranchLaneGroups({
  lanes,
  sourceNode,
  depth,
  parentId,
  readOnly,
  debugInspectMode,
  expandedContainers,
  onToggleContainer,
  onEditNode,
  editingNodeId,
  renderInlineEditor,
  activeAddStepRequest,
  renderInlineAddStep,
  onAddStep,
  dragController,
  routeController,
}: {
  lanes: BranchLane[];
  sourceNode: Node;
  depth: number;
  parentId?: string;
  readOnly?: boolean;
  debugInspectMode?: boolean;
  expandedContainers: Record<string, boolean>;
  onToggleContainer: (nodeId: string) => void;
  onEditNode?: (nodeId: string) => void;
  editingNodeId?: string | null;
  renderInlineEditor?: (nodeId: string) => ReactNode;
  activeAddStepRequest?: TimelineAddStepRequest | null;
  renderInlineAddStep?: (request: TimelineAddStepRequest) => ReactNode;
  onAddStep?: (request: TimelineAddStepRequest) => void;
  dragController: TimelineDragController;
  routeController: TimelineRouteController;
}) {
  if (lanes.length === 0) return null;

  const sourceStepType = getStepType(sourceNode);
  const sourceStepName = getStepName(sourceNode);
  const switchCaseCount = getSwitchCaseCount(sourceNode);
  const routeControlsDisabled = readOnly || debugInspectMode;
  const conditionalFlipLaneLabel =
    sourceStepType === 'Conditional'
      ? lanes
          .map((lane) => getEdgeLabel(lane.edge))
          .map((label) => label.trim().toLowerCase())
          .find((label) => label === 'true' || label === 'false')
      : undefined;

  return (
    <div
      className="relative mt-2 space-y-2"
      style={{ marginLeft: depth * 24 + 40 }}
      aria-label="Branch lanes"
    >
      <span
        className="absolute bottom-2 left-8 top-2 w-px bg-border"
        aria-hidden="true"
      />
      {lanes.map((lane) => {
        const edgeLabel = getEdgeLabel(lane.edge);
        const normalizedEdgeLabel = edgeLabel.trim().toLowerCase();
        const isErrorRoute = normalizedEdgeLabel === 'onerror';
        const caseIndex =
          sourceStepType === 'Switch' ? getCaseIndex(edgeLabel) : null;
        const showConditionalFlip =
          sourceStepType === 'Conditional' &&
          normalizedEdgeLabel === conditionalFlipLaneLabel;
        const laneContext = createLaneListContext(parentId, lane);

        return (
          <div
            key={lane.edge.id}
            className="grid min-w-0 grid-cols-[2.75rem_minmax(0,1fr)] gap-2"
          >
            <div className="relative flex min-h-10 flex-col items-start">
              <span
                className={cn(
                  'relative ml-1 mt-2 inline-flex items-center rounded-sm bg-background px-0.5 py-1 text-[10px] font-semibold uppercase leading-none text-muted-foreground',
                  isErrorRoute && 'text-destructive'
                )}
                style={{ writingMode: 'vertical-rl' }}
              >
                {edgeLabel}
              </span>
              {showConditionalFlip && (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="ml-0.5 mt-1 size-6 rounded-sm bg-background text-muted-foreground"
                  disabled={routeControlsDisabled}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    routeController.onFlipConditionalBranches(sourceNode.id);
                  }}
                  aria-label={`Flip true and false routes for ${sourceStepName}`}
                  title="Flip true and false routes"
                >
                  <ArrowUpDown className="size-3.5" aria-hidden="true" />
                </Button>
              )}
              {caseIndex !== null && switchCaseCount > 1 && (
                <div className="ml-0.5 mt-1 flex flex-col gap-0.5">
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-5 rounded-sm bg-background text-muted-foreground"
                    disabled={routeControlsDisabled || caseIndex === 0}
                    onClick={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      routeController.onMoveSwitchCase(
                        sourceNode.id,
                        caseIndex,
                        'up'
                      );
                    }}
                    aria-label={`Move case ${caseIndex} up for ${sourceStepName}`}
                    title="Move case up"
                  >
                    <ArrowUp className="size-3" aria-hidden="true" />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-5 rounded-sm bg-background text-muted-foreground"
                    disabled={
                      routeControlsDisabled || caseIndex >= switchCaseCount - 1
                    }
                    onClick={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      routeController.onMoveSwitchCase(
                        sourceNode.id,
                        caseIndex,
                        'down'
                      );
                    }}
                    aria-label={`Move case ${caseIndex} down for ${sourceStepName}`}
                    title="Move case down"
                  >
                    <ArrowDown className="size-3" aria-hidden="true" />
                  </Button>
                </div>
              )}
              {sourceStepType !== 'Conditional' && (
                <TimelineRouteSettings
                  edge={lane.edge}
                  label={edgeLabel}
                  disabled={routeControlsDisabled}
                  routeController={routeController}
                />
              )}
            </div>
            <div className="relative min-w-0 space-y-2 pb-2">
              <span
                className={cn(
                  'absolute -left-5 top-5 h-px w-5 bg-border',
                  isErrorRoute && 'bg-destructive/50'
                )}
                aria-hidden="true"
              />
              <TimelineItemList
                items={lane.items}
                depth={0}
                listContext={laneContext}
                readOnly={readOnly}
                debugInspectMode={debugInspectMode}
                expandedContainers={expandedContainers}
                onToggleContainer={onToggleContainer}
                onEditNode={onEditNode}
                editingNodeId={editingNodeId}
                renderInlineEditor={renderInlineEditor}
                activeAddStepRequest={activeAddStepRequest}
                renderInlineAddStep={renderInlineAddStep}
                onAddStep={onAddStep}
                dragController={dragController}
                routeController={routeController}
                endTargetNode={lane.continuationNode}
              />
              {lane.continuationNode && (
                <LaneContinuationNode node={lane.continuationNode} />
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

function WorkflowTimelineItem({
  item,
  depth,
  listContext,
  readOnly,
  debugInspectMode,
  expandedContainers,
  onToggleContainer,
  onEditNode,
  editingNodeId,
  renderInlineEditor,
  activeAddStepRequest,
  renderInlineAddStep,
  onAddStep,
  dragController,
  routeController,
}: {
  item: TimelineItem;
  depth: number;
  listContext: TimelineListContext;
  readOnly?: boolean;
  debugInspectMode?: boolean;
  expandedContainers: Record<string, boolean>;
  onToggleContainer: (nodeId: string) => void;
  onEditNode?: (nodeId: string) => void;
  editingNodeId?: string | null;
  renderInlineEditor?: (nodeId: string) => ReactNode;
  activeAddStepRequest?: TimelineAddStepRequest | null;
  renderInlineAddStep?: (request: TimelineAddStepRequest) => ReactNode;
  onAddStep?: (request: TimelineAddStepRequest) => void;
  dragController: TimelineDragController;
  routeController: TimelineRouteController;
}) {
  const selectedNodeId = useWorkflowStore((state) => state.selectedNodeId);
  const setSelectedNodeId = useWorkflowStore(
    (state) => state.setSelectedNodeId
  );
  const stepsWithErrors = useWorkflowStore((state) => state.stepsWithErrors);
  const isSuspendedExecution = useExecutionStore((s) => s.isSuspended);

  const node = item.node;
  const stepType = getStepType(node);
  const StepIcon = getStepIcon(stepType);
  const isSelected = selectedNodeId === node.id;
  const isEditingInline = editingNodeId === node.id;
  const isContainer = isScopeStepType(stepType) || item.children.length > 0;
  const isExpanded = expandedContainers[node.id] ?? true;
  const nestedItemCount = countItems(item.children);
  const executionStatus = useTimelineNodeExecutionStatus(node);
  const executionBadge = getExecutionBadge(executionStatus);
  const ExecutionIcon = executionBadge?.icon;
  const hasValidationError = stepsWithErrors.has(node.id);
  const canInspectInDebug = !debugInspectMode || Boolean(executionStatus);
  const childListContext = useMemo(
    () => createScopeListContext(node.id, item.children),
    [item.children, node.id]
  );
  const isDragging = dragController.dragging?.nodeId === node.id;
  const isDropBefore =
    dragController.dropTarget?.nodeId === node.id &&
    dragController.dropTarget?.contextKey === listContext.key &&
    dragController.dropTarget?.placement === 'before';
  const isDropAfter =
    dragController.dropTarget?.nodeId === node.id &&
    dragController.dropTarget?.contextKey === listContext.key &&
    dragController.dropTarget?.placement === 'after';

  const handleSelect = useCallback(() => {
    if (!canInspectInDebug) return;
    setSelectedNodeId(node.id);
  }, [canInspectInDebug, node.id, setSelectedNodeId]);

  const handleEdit = useCallback(() => {
    onEditNode?.(node.id);
  }, [node.id, onEditNode]);

  return (
    <div
      className="relative"
      data-timeline-context-key={listContext.key}
      data-timeline-node-id={node.id}
      data-testid="timeline-step"
      data-step-name={getStepName(node)}
      data-step-type={stepType}
      data-parent-node-id={node.parentId}
    >
      {isDropBefore && (
        <span
          className="pointer-events-none absolute -top-1 left-0 right-0 z-20 h-0.5 rounded-full bg-primary"
          aria-hidden="true"
        />
      )}
      <div
        className={cn(
          'relative overflow-hidden rounded-md border bg-background transition-colors',
          hasValidationError
            ? 'border-destructive ring-2 ring-destructive/30'
            : executionStatus
              ? getExecutionBorderClass(executionStatus.status)
              : (isSelected || isEditingInline) &&
                'border-primary bg-primary/5',
          executionStatus?.status === 'suspended' &&
            'border-2 animate-glow-pulse',
          !canInspectInDebug && 'opacity-60',
          isSuspendedExecution &&
            executionStatus?.status === 'queued' &&
            'opacity-25 pointer-events-none',
          isDragging && 'opacity-50'
        )}
        style={{ marginLeft: depth * 24 }}
      >
        <div className="flex gap-3 p-3">
          {!readOnly && !debugInspectMode && (
            <button
              type="button"
              className="mt-1 flex size-6 shrink-0 cursor-grab items-center justify-center rounded-sm text-muted-foreground hover:bg-muted active:cursor-grabbing"
              onClick={(event) => event.stopPropagation()}
              onPointerDown={(event) =>
                dragController.onPointerDown(event, item, listContext)
              }
              onMouseDown={(event) =>
                dragController.onMouseDown(event, item, listContext)
              }
              aria-label={`Move ${getStepName(node)} with nested steps`}
            >
              <GripVertical className="size-4" aria-hidden="true" />
            </button>
          )}
          <div className="flex flex-col items-center">
            <button
              type="button"
              className={cn(
                'flex size-8 items-center justify-center rounded-full border bg-muted text-muted-foreground',
                isSelected && 'border-primary text-primary',
                getExecutionIconClass(
                  executionStatus?.status,
                  hasValidationError
                )
              )}
              onClick={handleSelect}
              aria-label={`Select ${getStepName(node)}`}
            >
              <StepIcon className="size-4" aria-hidden="true" />
            </button>
            {isContainer && (
              <div
                className="mt-2 h-full min-h-6 w-px bg-border"
                aria-hidden="true"
              />
            )}
          </div>

          <div
            role="button"
            tabIndex={canInspectInDebug ? 0 : -1}
            className="min-w-0 flex-1 cursor-pointer text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
            onClick={handleSelect}
            onDoubleClick={readOnly ? undefined : handleEdit}
            onKeyDown={(event) => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                handleSelect();
              }
            }}
          >
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              {isContainer && (
                <button
                  type="button"
                  className="inline-flex size-5 items-center justify-center rounded-sm text-muted-foreground"
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onToggleContainer(node.id);
                  }}
                  aria-label={`${isExpanded ? 'Collapse' : 'Expand'} ${getStepName(node)}`}
                >
                  {isExpanded ? (
                    <ChevronDown className="size-4" aria-hidden="true" />
                  ) : (
                    <ChevronRight className="size-4" aria-hidden="true" />
                  )}
                </button>
              )}
              <h3 className="truncate text-sm font-semibold text-foreground">
                {getStepName(node)}
              </h3>
              <Badge variant={getStepBadgeVariant(stepType)}>{stepType}</Badge>
              {executionBadge && ExecutionIcon && (
                <Badge variant={executionBadge.variant}>
                  <ExecutionIcon
                    className={cn('mr-1 size-3', executionBadge.iconClassName)}
                    aria-hidden="true"
                  />
                  {executionBadge.label}
                </Badge>
              )}
              {hasValidationError && (
                <Badge variant="destructive">
                  <AlertCircle className="mr-1 size-3" aria-hidden="true" />
                  Problem
                </Badge>
              )}
            </div>

            <p
              className={cn(
                'mt-1 line-clamp-2 text-xs text-muted-foreground',
                executionStatus?.error && 'text-destructive'
              )}
            >
              {executionStatus?.error || getStepDescription(node)}
            </p>

            {item.children.length > 0 && (
              <div className="mt-3">
                <Badge variant="secondary">
                  <ListTree className="mr-1 size-3" aria-hidden="true" />
                  {nestedItemCount} nested
                </Badge>
              </div>
            )}
          </div>

          {!readOnly && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleEdit}
              aria-label={`Edit ${getStepName(node)}`}
              aria-expanded={isEditingInline}
            >
              <PenLine aria-hidden="true" />
              {isEditingInline ? 'Editing' : 'Edit'}
            </Button>
          )}
        </div>

        {isEditingInline && renderInlineEditor && (
          <div className="border-t bg-card/60">
            {renderInlineEditor(node.id)}
          </div>
        )}
      </div>

      <TimelineRouteAddControls
        item={item}
        depth={depth}
        readOnly={readOnly}
        debugInspectMode={debugInspectMode}
        activeAddStepRequest={activeAddStepRequest}
        renderInlineAddStep={renderInlineAddStep}
        onAddStep={onAddStep}
        routeController={routeController}
      />

      <BranchLaneGroups
        lanes={item.lanes}
        sourceNode={node}
        depth={depth}
        parentId={node.parentId}
        readOnly={readOnly}
        debugInspectMode={debugInspectMode}
        expandedContainers={expandedContainers}
        onToggleContainer={onToggleContainer}
        onEditNode={onEditNode}
        editingNodeId={editingNodeId}
        renderInlineEditor={renderInlineEditor}
        activeAddStepRequest={activeAddStepRequest}
        renderInlineAddStep={renderInlineAddStep}
        onAddStep={onAddStep}
        dragController={dragController}
        routeController={routeController}
      />

      {isContainer && isExpanded && (
        <div className="mt-2">
          <TimelineItemList
            items={item.children}
            depth={depth + 1}
            listContext={childListContext}
            readOnly={readOnly}
            debugInspectMode={debugInspectMode}
            expandedContainers={expandedContainers}
            onToggleContainer={onToggleContainer}
            onEditNode={onEditNode}
            editingNodeId={editingNodeId}
            renderInlineEditor={renderInlineEditor}
            activeAddStepRequest={activeAddStepRequest}
            renderInlineAddStep={renderInlineAddStep}
            onAddStep={onAddStep}
            dragController={dragController}
            routeController={routeController}
          />
        </div>
      )}
      {isDropAfter && (
        <span
          className="pointer-events-none absolute -bottom-1 left-0 right-0 z-20 h-0.5 rounded-full bg-primary"
          aria-hidden="true"
        />
      )}
    </div>
  );
}

export function WorkflowTimelineView({
  readOnly = false,
  debugInspectMode = false,
  onEditNode,
  editingNodeId,
  renderInlineEditor,
  activeAddStepRequest,
  renderInlineAddStep,
  onAddStep,
  inputSchemaFields,
  variables,
}: WorkflowTimelineViewProps) {
  const nodes = useWorkflowStore((state) => state.nodes);
  const edges = useWorkflowStore((state) => state.edges);
  const moveTimelineItem = useWorkflowStore((state) => state.moveTimelineItem);
  const flipConditionalBranches = useWorkflowStore(
    (state) => state.flipConditionalBranches
  );
  const moveSwitchCase = useWorkflowStore((state) => state.moveSwitchCase);
  const updateEdgeData = useWorkflowStore((state) => state.updateEdgeData);
  const addStoreEdge = useWorkflowStore((state) => state.addEdge);
  const onEdgesChange = useWorkflowStore((state) => state.onEdgesChange);
  const [expandedContainers, setExpandedContainers] = useState<
    Record<string, boolean>
  >({});
  const [dragging, setDragging] = useState<DragState>(null);
  const [dropTarget, setDropTarget] = useState<DropTarget>(null);
  const draggingRef = useRef<DragState>(null);
  const dropTargetRef = useRef<DropTarget>(null);

  const { items, totalSteps, branchCount, containerCount } = useMemo(() => {
    const hiddenNodeIds = getHiddenNodeIds(nodes, edges);
    const timelineItems = buildTimelineItems(
      nodes,
      edges,
      undefined,
      hiddenNodeIds
    );
    const renderableNodes = nodes.filter((node) =>
      isRenderableNode(node, hiddenNodeIds)
    );
    const renderableNodeIds = new Set(renderableNodes.map((node) => node.id));

    return {
      items: timelineItems,
      totalSteps: countItems(timelineItems),
      branchCount: edges.filter((edge) => {
        const label = getEdgeLabel(edge);
        return (
          renderableNodeIds.has(edge.source) &&
          renderableNodeIds.has(edge.target) &&
          label !== 'next' &&
          label !== 'source'
        );
      }).length,
      containerCount: renderableNodes.filter((node) =>
        renderableNodes.some((candidate) => candidate.parentId === node.id)
      ).length,
    };
  }, [nodes, edges]);

  const toggleContainer = useCallback((nodeId: string) => {
    setExpandedContainers((prev) => ({
      ...prev,
      [nodeId]: !(prev[nodeId] ?? true),
    }));
  }, []);

  const listContextsByKey = useMemo(
    () => collectTimelineListContexts(items),
    [items]
  );

  const setActiveDropTarget = useCallback((nextDropTarget: DropTarget) => {
    dropTargetRef.current = nextDropTarget;
    setDropTarget(nextDropTarget);
  }, []);

  const clearDragState = useCallback(() => {
    draggingRef.current = null;
    dropTargetRef.current = null;
    setDragging(null);
    setDropTarget(null);
  }, []);

  const getDropTargetFromPoint = useCallback((clientY: number): DropTarget => {
    const activeDragging = draggingRef.current;
    if (!activeDragging) return null;

    const candidates = Array.from(
      document.querySelectorAll<HTMLElement>(
        '[data-timeline-context-key][data-timeline-node-id]'
      )
    ).filter(
      (element) =>
        element.dataset.timelineContextKey === activeDragging.contextKey &&
        element.dataset.timelineNodeId &&
        element.dataset.timelineNodeId !== activeDragging.nodeId
    );

    let bestTarget:
      | { target: NonNullable<DropTarget>; distance: number }
      | undefined;

    for (const element of candidates) {
      const nodeId = element.dataset.timelineNodeId;
      if (!nodeId) continue;

      const bounds = element.getBoundingClientRect();
      const placement: DropPlacement =
        clientY < bounds.top + bounds.height / 2 ? 'before' : 'after';
      const distance =
        clientY < bounds.top
          ? bounds.top - clientY
          : clientY > bounds.bottom
            ? clientY - bounds.bottom
            : 0;

      if (!bestTarget || distance < bestTarget.distance) {
        bestTarget = {
          target: {
            nodeId,
            contextKey: activeDragging.contextKey,
            placement,
          },
          distance,
        };
      }
    }

    return bestTarget?.target ?? null;
  }, []);

  const updateDropTargetFromPoint = useCallback(
    (clientY: number) => {
      setActiveDropTarget(getDropTargetFromPoint(clientY));
    },
    [getDropTargetFromPoint, setActiveDropTarget]
  );

  const commitDragTarget = useCallback(
    (finalDropTarget?: DropTarget) => {
      const activeDragging = draggingRef.current;
      const activeDropTarget = finalDropTarget ?? dropTargetRef.current;

      if (!activeDragging || !activeDropTarget) {
        clearDragState();
        return;
      }

      const finalContext = listContextsByKey.get(activeDropTarget.contextKey);

      if (
        finalContext &&
        activeDropTarget.contextKey === activeDragging.contextKey &&
        activeDropTarget.nodeId !== activeDragging.nodeId
      ) {
        moveTimelineItem({
          draggedNodeId: activeDragging.nodeId,
          targetNodeId: activeDropTarget.nodeId,
          placement: activeDropTarget.placement,
          context: finalContext,
        });
      }

      clearDragState();
    },
    [clearDragState, listContextsByKey, moveTimelineItem]
  );

  const beginDrag = useCallback(
    (item: TimelineItem, context: TimelineListContext) => {
      const nextDragging = {
        nodeId: item.node.id,
        contextKey: context.key,
      };

      draggingRef.current = nextDragging;
      setDragging(nextDragging);
      setActiveDropTarget(null);
    },
    [setActiveDropTarget]
  );

  const handlePointerDown = useCallback(
    (
      event: ReactPointerEvent<HTMLElement>,
      item: TimelineItem,
      context: TimelineListContext
    ) => {
      if (event.button !== 0) return;

      event.preventDefault();
      event.stopPropagation();
      beginDrag(item, context);

      const handlePointerMove = (moveEvent: PointerEvent) => {
        moveEvent.preventDefault();
        updateDropTargetFromPoint(moveEvent.clientY);
      };

      const handlePointerUp = (upEvent: PointerEvent) => {
        upEvent.preventDefault();
        commitDragTarget(getDropTargetFromPoint(upEvent.clientY));
        window.removeEventListener('pointermove', handlePointerMove);
        window.removeEventListener('pointerup', handlePointerUp);
        window.removeEventListener('pointercancel', handlePointerCancel);
      };

      const handlePointerCancel = () => {
        clearDragState();
        window.removeEventListener('pointermove', handlePointerMove);
        window.removeEventListener('pointerup', handlePointerUp);
        window.removeEventListener('pointercancel', handlePointerCancel);
      };

      window.addEventListener('pointermove', handlePointerMove);
      window.addEventListener('pointerup', handlePointerUp);
      window.addEventListener('pointercancel', handlePointerCancel);
    },
    [
      beginDrag,
      clearDragState,
      commitDragTarget,
      getDropTargetFromPoint,
      updateDropTargetFromPoint,
    ]
  );

  const handleMouseDown = useCallback(
    (
      event: ReactMouseEvent<HTMLElement>,
      item: TimelineItem,
      context: TimelineListContext
    ) => {
      if (event.button !== 0 || draggingRef.current) return;

      event.preventDefault();
      event.stopPropagation();
      beginDrag(item, context);

      const handleMouseMove = (moveEvent: MouseEvent) => {
        moveEvent.preventDefault();
        updateDropTargetFromPoint(moveEvent.clientY);
      };

      const handleMouseUp = (upEvent: MouseEvent) => {
        upEvent.preventDefault();
        commitDragTarget(getDropTargetFromPoint(upEvent.clientY));
        window.removeEventListener('mousemove', handleMouseMove);
        window.removeEventListener('mouseup', handleMouseUp);
      };

      window.addEventListener('mousemove', handleMouseMove);
      window.addEventListener('mouseup', handleMouseUp);
    },
    [
      beginDrag,
      commitDragTarget,
      getDropTargetFromPoint,
      updateDropTargetFromPoint,
    ]
  );

  const dragController = useMemo<TimelineDragController>(
    () => ({
      dragging,
      dropTarget,
      onPointerDown: handlePointerDown,
      onMouseDown: handleMouseDown,
    }),
    [dragging, dropTarget, handlePointerDown, handleMouseDown]
  );

  const routeController = useMemo<TimelineRouteController>(
    () => ({
      onFlipConditionalBranches: flipConditionalBranches,
      onMoveSwitchCase: (nodeId, caseIndex, direction) =>
        moveSwitchCase({ nodeId, caseIndex, direction }),
      onUpdateRouteData: (edgeId, updates) => updateEdgeData(edgeId, updates),
      // Same store path the canvas uses for edge removal
      // (workflowStore.onEdgesChange with a 'remove' change).
      onDeleteRoute: (edgeId) =>
        onEdgesChange([{ id: edgeId, type: 'remove' }]),
      onConnectSteps: (sourceNodeId, targetNodeId) =>
        addStoreEdge(sourceNodeId, targetNodeId, 'source'),
      conditionInputSchemaFields: inputSchemaFields,
      conditionVariables: variables,
    }),
    [
      flipConditionalBranches,
      moveSwitchCase,
      updateEdgeData,
      onEdgesChange,
      addStoreEdge,
      inputSchemaFields,
      variables,
    ]
  );

  const rootListContext = useMemo(
    () => createScopeListContext(undefined, items),
    [items]
  );

  if (items.length === 0) {
    const inlineAddStep = activeAddStepRequest
      ? renderInlineAddStep?.(activeAddStepRequest)
      : null;

    if (inlineAddStep) {
      return (
        <div
          className="h-full overflow-auto bg-background p-6"
          data-testid="workflow-timeline-empty"
        >
          <div className="mx-auto w-full max-w-3xl overflow-hidden rounded-md border border-dashed border-primary/50 bg-card shadow-sm">
            {inlineAddStep}
          </div>
        </div>
      );
    }

    return (
      <div
        className="flex h-full items-center justify-center bg-background p-6"
        data-testid="workflow-timeline-empty"
      >
        <div className="max-w-sm rounded-md border bg-card p-6 text-center">
          <Workflow
            className="mx-auto size-10 text-muted-foreground"
            aria-hidden="true"
          />
          <h2 className="mt-4 text-base font-semibold">No workflow steps</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            Add the first step to see the automation timeline.
          </p>
          {!readOnly && !debugInspectMode && onAddStep && (
            <Button
              type="button"
              className="mt-4"
              onClick={() => onAddStep({})}
              data-testid="timeline-add-step"
            >
              <Plus aria-hidden="true" />
              Add step
            </Button>
          )}
        </div>
      </div>
    );
  }

  return (
    <div
      className="h-full overflow-auto bg-background"
      data-testid="workflow-timeline"
    >
      <div className="mx-auto flex max-w-5xl flex-col gap-4 p-6 pt-28">
        <div className="rounded-md border bg-card p-4">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <h2 className="text-lg font-semibold text-foreground">
                Workflow Timeline
              </h2>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant="secondary">{totalSteps} steps</Badge>
              <Badge variant="outline">{branchCount} branch edges</Badge>
              <Badge variant="outline">{containerCount} nested scopes</Badge>
            </div>
          </div>
        </div>

        <TimelineItemList
          items={items}
          depth={0}
          listContext={rootListContext}
          readOnly={readOnly}
          debugInspectMode={debugInspectMode}
          expandedContainers={expandedContainers}
          onToggleContainer={toggleContainer}
          onEditNode={onEditNode}
          editingNodeId={editingNodeId}
          renderInlineEditor={renderInlineEditor}
          activeAddStepRequest={activeAddStepRequest}
          renderInlineAddStep={renderInlineAddStep}
          onAddStep={onAddStep}
          dragController={dragController}
          routeController={routeController}
        />
      </div>
    </div>
  );
}
