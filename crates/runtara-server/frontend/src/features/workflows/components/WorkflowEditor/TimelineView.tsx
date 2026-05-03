import {
  Fragment,
  type ReactNode,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  useCallback,
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
  Pause,
  PenLine,
  Plus,
  Repeat,
  Split,
  Workflow,
  XCircle,
  Zap,
} from 'lucide-react';

import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import { cn } from '@/lib/utils.ts';
import { NODE_TYPES } from '@/features/workflows/config/workflow.ts';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
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
};

export type TimelineAddStepRequest = {
  sourceNodeId?: string;
  sourceHandle?: string;
  targetNodeId?: string;
  parentId?: string;
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
    first?.parentId === second?.parentId
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
    case ExecutionStatus.Completed:
      return {
        label: executionStatus.executionTime
          ? formatExecutionTime(executionStatus.executionTime)
          : 'Completed',
        variant: 'success' as const,
        icon: CheckCircle2,
      };
    case ExecutionStatus.Running:
    case ExecutionStatus.Compiling:
      return {
        label: status === ExecutionStatus.Running ? 'Running' : 'Compiling',
        variant: 'default' as const,
        icon: Loader2,
        iconClassName: 'animate-spin',
      };
    case ExecutionStatus.Queued:
      return {
        label: 'Queued',
        variant: 'warning' as const,
        icon: Pause,
      };
    case ExecutionStatus.Failed:
    case ExecutionStatus.Timeout:
      return {
        label: status === ExecutionStatus.Timeout ? 'Timeout' : 'Failed',
        variant: 'destructive' as const,
        icon: AlertCircle,
      };
    case ExecutionStatus.Cancelled:
      return {
        label: 'Cancelled',
        variant: 'muted' as const,
        icon: XCircle,
      };
    case ExecutionStatus.Suspended:
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
    case ExecutionStatus.Running:
    case ExecutionStatus.Compiling:
      return 'border-blue-500';
    case ExecutionStatus.Completed:
      return 'border-green-500';
    case ExecutionStatus.Failed:
    case ExecutionStatus.Timeout:
      return 'border-red-500';
    case ExecutionStatus.Queued:
      return 'border-yellow-500';
    case ExecutionStatus.Suspended:
      return 'border-blue-400';
    case ExecutionStatus.Cancelled:
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
    case ExecutionStatus.Running:
    case ExecutionStatus.Compiling:
      return 'border-blue-500 bg-blue-50 text-blue-700 dark:bg-blue-950 dark:text-blue-300';
    case ExecutionStatus.Completed:
      return 'border-green-500 bg-green-50 text-green-700 dark:bg-green-950 dark:text-green-300';
    case ExecutionStatus.Failed:
    case ExecutionStatus.Timeout:
      return 'border-red-500 bg-red-50 text-red-700 dark:bg-red-950 dark:text-red-300';
    case ExecutionStatus.Queued:
      return 'border-yellow-500 bg-yellow-50 text-yellow-700 dark:bg-yellow-950 dark:text-yellow-300';
    case ExecutionStatus.Suspended:
      return 'border-blue-400 bg-slate-50 text-slate-700 dark:bg-slate-900 dark:text-slate-300';
    case ExecutionStatus.Cancelled:
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
  const inputMapping = node.data?.inputMapping;
  if (!Array.isArray(inputMapping)) return 0;

  const casesField = inputMapping.find(
    (item) =>
      typeof item === 'object' &&
      item !== null &&
      (item as { type?: unknown }).type === 'cases'
  );

  const cases = (casesField as { value?: unknown } | undefined)?.value;
  return Array.isArray(cases) ? cases.length : 0;
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
        executionStatus?.status === ExecutionStatus.Suspended &&
          'border-2 animate-glow-pulse',
        isSuspendedExecution &&
          executionStatus?.status === ExecutionStatus.Queued &&
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

function getHiddenNodeIds(nodes: Node[], edges: Edge[]): Set<string> {
  const hiddenNodes = new Set<string>();
  const aiAgentNodes = nodes.filter(
    (node) => node.type === NODE_TYPES.AiAgentNode
  );

  for (const agentNode of aiAgentNodes) {
    for (const edge of edges) {
      if (
        edge.source === agentNode.id &&
        edge.sourceHandle &&
        edge.sourceHandle !== 'source'
      ) {
        hiddenNodes.add(edge.target);
      }
    }
  }

  return hiddenNodes;
}

function isRenderableNode(node: Node, hiddenNodeIds: Set<string>): boolean {
  if (hiddenNodeIds.has(node.id)) return false;
  if (excludedNodeTypes.has(node.type || '')) return false;
  return true;
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

function buildTimelineItems(
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
}: {
  request: TimelineAddStepRequest | null;
  depth: number;
  activeAddStepRequest?: TimelineAddStepRequest | null;
  renderInlineAddStep?: (request: TimelineAddStepRequest) => ReactNode;
  onAddStep?: (request: TimelineAddStepRequest) => void;
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
        />
      )}
      {items.map((item, index) => (
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
            />
          )}
        </Fragment>
      ))}
    </div>
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
          executionStatus?.status === ExecutionStatus.Suspended &&
            'border-2 animate-glow-pulse',
          !canInspectInDebug && 'opacity-60',
          isSuspendedExecution &&
            executionStatus?.status === ExecutionStatus.Queued &&
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
}: WorkflowTimelineViewProps) {
  const nodes = useWorkflowStore((state) => state.nodes);
  const edges = useWorkflowStore((state) => state.edges);
  const moveTimelineItem = useWorkflowStore((state) => state.moveTimelineItem);
  const flipConditionalBranches = useWorkflowStore(
    (state) => state.flipConditionalBranches
  );
  const moveSwitchCase = useWorkflowStore((state) => state.moveSwitchCase);
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
    }),
    [flipConditionalBranches, moveSwitchCase]
  );

  const rootListContext = useMemo(
    () => createScopeListContext(undefined, items),
    [items]
  );

  if (items.length === 0) {
    const inlineAddStep = activeAddStepRequest
      ? renderInlineAddStep?.(activeAddStepRequest)
      : null;

    return (
      <div
        className="flex h-full items-center justify-center bg-background p-6"
        data-testid="workflow-timeline-empty"
      >
        {inlineAddStep ? (
          <div className="w-full max-w-3xl overflow-hidden rounded-md border border-dashed border-primary/50 bg-card shadow-sm">
            {inlineAddStep}
          </div>
        ) : (
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
        )}
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
