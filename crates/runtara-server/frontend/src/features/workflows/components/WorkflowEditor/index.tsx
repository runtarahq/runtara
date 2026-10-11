import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Background,
  Connection,
  Controls,
  Edge,
  EdgeTypes,
  Node,
  NodeTypes,
  ReactFlow,
  ReactFlowProvider,
  getViewportForBounds,
  Panel,
  useNodesInitialized,
  useReactFlow,
  useStoreApi,
  OnConnectEnd,
} from '@xyflow/react';
import { RectangleHorizontal, Rows3 } from 'lucide-react';
import {
  generateStepId,
  useWorkflowStore,
} from '@/features/workflows/stores/workflowStore.ts';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { useValidationStore } from '@/features/workflows/stores/validationStore';
import { toast } from 'sonner';
import { useIsDarkTheme } from '@/shared/stores/themeStore';
import { Tabs, TabsList, TabsTrigger } from '@/shared/components/ui/tabs';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
  STEP_TYPES,
} from '@/features/workflows/config/workflow.ts';
import {
  SNAP_GRID_SIZE,
  snapPositionToGrid,
} from '@/features/workflows/config/workflow-editor.ts';
import { validateConnection } from '@/features/workflows/utils/graph-validation';
import { ExecutionGraphStepDto } from '@/features/workflows/types/execution-graph';

import * as form from './NodeForm/NodeFormItem.tsx';
import {
  AnimatedSVGEdge,
  ImprovedEdge,
  EdgeContextProvider,
} from './CustomEdges';
import {
  BasicNode,
  ConditionalNode,
  ContainerNode,
  CreateNode,
  SwitchNode,
  AiAgentNode,
  NoteNode,
  StartIndicatorNode,
} from './CustomNodes';
import {
  getNodePositionInsideParent,
  getLayoutedElements,
} from './CustomNodes/utils.tsx';
import { NodeConfigPanel } from './NodeConfigPanel';
import { NodeFormProvider } from './NodeForm/NodeFormProvider';
import { StepPickerModal, StepPickerResult } from './NodeForm/StepPickerModal';
import { NodeConfigProvider } from './NodeConfigContext';
import { CanvasDisplayProvider } from './CanvasDisplayContext';
import type { EdgeRouteActions } from './CustomEdges/EdgeContext';
import { composeConditionSuggestions } from './NodeForm/InputMappingValueField/VariableSuggestions';
import {
  resolveContainerScope,
  splitItemSchemaFieldsFromScope,
  type ScopeNode,
} from '@/features/workflows/utils/container-scope';
import type { CanvasDetailLevel } from './CustomNodes/node-size';
import { readCanvasDetail, writeCanvasDetail } from './editor-preferences';
import { CANVAS_Z } from '@/features/workflows/components/WorkflowEditor/canvas-z';
import {
  validateNodeEditWithRust,
  type PendingNodeCandidate,
} from './node-edit-rust-validation';

// Re-export CreateStepContext type for external use
export interface CreateStepContext {
  position: { x: number; y: number };
  parentId?: string;
  insertionEdge?: {
    source: string;
    target: string;
    sourceHandle: string;
  };
  connection?: {
    source: string;
    sourceHandle: string;
  };
  /**
   * Parallel fan-out over an existing source->target edge: the new step N is
   * wired source->N and N->target while the original edge is kept, so the
   * unconditional branches re-converge at the target (validator E073).
   */
  parallelBranch?: {
    source: string;
    target: string;
    sourceHandle: string;
  };
}

const nodeTypes: NodeTypes = {
  [NODE_TYPES.BasicNode]: BasicNode,
  [NODE_TYPES.CreateNode]: CreateNode,
  [NODE_TYPES.ConditionalNode]: ConditionalNode,
  [NODE_TYPES.SwitchNode]: SwitchNode,
  [NODE_TYPES.ContainerNode]: ContainerNode,
  [NODE_TYPES.AiAgentNode]: AiAgentNode,
  [NODE_TYPES.NoteNode]: NoteNode,
  [NODE_TYPES.StartIndicatorNode]: StartIndicatorNode,
};

const edgeTypes: EdgeTypes = {
  default: ImprovedEdge,
  animated: AnimatedSVGEdge,
};

/**
 * Generates a unique step name by appending a number if the base name already exists.
 * E.g., "Random Double" -> "Random Double 2" -> "Random Double 3"
 */
function generateUniqueStepName(
  baseName: string,
  existingNodes: Node[]
): string {
  const existingNames = new Set(
    existingNodes
      .filter((node) => node.data?.name)
      .map((node) => node.data.name as string)
  );

  if (!existingNames.has(baseName)) {
    return baseName;
  }

  // Find the next available number
  let counter = 2;
  while (existingNames.has(`${baseName} ${counter}`)) {
    counter++;
  }

  return `${baseName} ${counter}`;
}

type StagedNodeChanges = Record<string, form.SchemaType>;

type WorkflowEditorProps = {
  nodes: Node[];
  edges: Edge[];
  readOnly?: boolean;
  /** When true, allows selecting executed nodes for inspection (but not editing) */
  debugInspectMode?: boolean;
  workflow?: {
    id: string;
    name: string;
    description?: string;
    variables?: Array<{
      name: string;
      value: unknown;
      type: string;
      description?: string | null;
    }>;
    inputSchemaFields?: Array<{
      name: string;
      type: string;
      required: boolean;
      description: string;
      nullable?: boolean;
    }>;
    outputSchemaFields?: Array<{
      name: string;
      type: string;
      required: boolean;
      description: string;
      nullable?: boolean;
    }>;
    stateSchemaFields?: Array<{
      name: string;
      type: string;
      required: boolean;
      description: string;
      nullable?: boolean;
      label?: string;
      format?: string;
      enum?: unknown[];
    }>;
    executionTimeoutSeconds?: number;
    rateLimitBudgetMs?: number;
    durable?: boolean | null;
    entryPoint?: string;
  };
  stagedNodeChanges?: StagedNodeChanges;
  onStagedNodeChange?: (nodeId: string, data: form.SchemaType) => void;
  onResetNodeChanges?: (nodeId: string) => void;
  /** Stages workflow-level changes the canvas makes, such as a new entry point. */
  onWorkflowChange?: (changes: { entryPoint?: string }) => void;
};

// Long workflows are several thousand pixels wide; React Flow's default
// minimum zoom (0.5) cannot fit them, so fit-to-view silently clipped both
// ends. Fitting never zooms past 100%: a two-step workflow blown up to 2x
// reads as broken, not as "fitted".
const CANVAS_MIN_ZOOM = 0.1;
const CANVAS_FIT_VIEW_OPTIONS = { padding: 0.15, maxZoom: 1 };
// Below this zoom card text is unreadable. A workflow too long to fit above it
// opens at this zoom, anchored on its start, and the Fit button shows it all.
const CANVAS_READABLE_ZOOM = 0.8;
const CANVAS_VIEW_MARGIN = 48;

function CanvasDetailToggle({
  value,
  onChange,
}: {
  value: CanvasDetailLevel;
  onChange: (value: CanvasDetailLevel) => void;
}) {
  return (
    <Tabs
      value={value}
      onValueChange={(next) => onChange(next as CanvasDetailLevel)}
    >
      <TabsList
        className="pointer-events-auto shadow-sm"
        aria-label="Step card detail"
      >
        <TabsTrigger
          value="detailed"
          className="gap-2"
          data-testid="canvas-detail-detailed"
          title="Show each step's configured inputs on its card"
        >
          <Rows3 className="size-4" aria-hidden="true" />
          Details
        </TabsTrigger>
        <TabsTrigger
          value="compact"
          className="gap-2"
          data-testid="canvas-detail-compact"
          title="Show step names only; hover a step for its inputs"
        >
          <RectangleHorizontal className="size-4" aria-hidden="true" />
          Compact
        </TabsTrigger>
      </TabsList>
    </Tabs>
  );
}

export function WorkflowEditor(props: WorkflowEditorProps) {
  return (
    <ReactFlowProvider>
      <WorkflowEditorContent {...props} />
    </ReactFlowProvider>
  );
}

function WorkflowEditorContent({
  nodes: initialNodes,
  edges: initialEdges,
  readOnly = false,
  debugInspectMode = false,
  workflow,
  stagedNodeChanges = {},
  onStagedNodeChange,
  onResetNodeChanges,
  onWorkflowChange,
}: WorkflowEditorProps) {
  const isDark = useIsDarkTheme();
  // Context for creating new steps
  const [createStepContext, setCreateStepContext] =
    useState<CreateStepContext | null>(null);

  // Track selected edge ID for showing "+" button
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);

  // Dialog states
  const [editingNodeId, setEditingNodeId] = useState<string | null>(null);
  const [showStepPicker, setShowStepPicker] = useState(false);
  // Detailed cards list each step's configured inputs, so the workflow reads
  // without opening every step's editor.
  const [canvasDetail, setCanvasDetailState] =
    useState<CanvasDetailLevel>(readCanvasDetail);
  const setCanvasDetail = useCallback((detail: CanvasDetailLevel) => {
    setCanvasDetailState(detail);
    writeCanvasDetail(detail);
  }, []);

  // Pending new node state from store (for deferred creation until user confirms)
  const pendingNewNode = useWorkflowStore((state) => state.pendingNewNode);
  const setPendingNewNode = useWorkflowStore(
    (state) => state.setPendingNewNode
  );

  // Callback for child node components to open config dialogs (including hidden nodes)
  // Opening an editor abandons any half-finished create. The pending node has
  // to go with the rest of that state: it is what the create panel renders
  // from, and leaving it set puts two config panels on screen at once.
  const openNodeConfig = useCallback(
    (nodeId: string) => {
      if (workflow && !readOnly) {
        setCreateStepContext(null);
        setPendingNewNode(null);
        setEditingNodeId(nodeId);
      }
    },
    [workflow, readOnly, setPendingNewNode]
  );
  const nodeConfigContextValue = useMemo(
    () => ({ openNodeConfig }),
    [openNodeConfig]
  );

  const ref = useRef(null);

  // Get state from Zustand store
  const nodes = useWorkflowStore((state) => state.nodes);
  // Rebuilt only when a step is added, removed, renamed or retyped, so cards
  // do not all re-render on every selection or position change.
  const stepIndexKey = useMemo(
    () =>
      JSON.stringify(
        nodes.map((node) => [
          node.id,
          (node.data as { name?: string })?.name ?? '',
          (node.data as { stepType?: string })?.stepType ?? '',
        ])
      ),
    [nodes]
  );
  const canvasDisplayValue = useMemo(() => {
    const entries = JSON.parse(stepIndexKey) as Array<[string, string, string]>;
    return {
      detailLevel: canvasDetail,
      stepPreviews: true,
      editable: !readOnly && !!workflow,
      stepNames: new Map(
        entries.filter(([, name]) => name).map(([id, name]) => [id, name])
      ),
      stepTypes: new Map(entries.map(([id, , stepType]) => [id, stepType])),
    };
  }, [canvasDetail, readOnly, stepIndexKey, workflow]);
  const edges = useWorkflowStore((state) => state.edges);
  const pendingCenterNodeId = useWorkflowStore(
    (state) => state.pendingCenterNodeId
  );

  // Get actions from store
  const onNodesChange = useWorkflowStore((state) => state.onNodesChange);
  const onEdgesChange = useWorkflowStore((state) => state.onEdgesChange);
  const addNode = useWorkflowStore((state) => state.addNode);
  const addStoreEdge = useWorkflowStore((state) => state.addEdge);
  const addStoreEdges = useWorkflowStore((state) => state.addEdges);
  const insertNodeBetween = useWorkflowStore(
    (state) => state.insertNodeBetween
  );
  const syncFromReactFlow = useWorkflowStore(
    (state) => state.syncFromReactFlow
  );
  const setSelectedNodeId = useWorkflowStore(
    (state) => state.setSelectedNodeId
  );
  const setPendingCenterNodeId = useWorkflowStore(
    (state) => state.setPendingCenterNodeId
  );
  const removeNode = useWorkflowStore((state) => state.removeNode);
  const updateNode = useWorkflowStore((state) => state.updateNode);

  const {
    getIntersectingNodes,
    getNodesBounds,
    screenToFlowPosition,
    fitView,
    setCenter,
    setViewport,
    getZoom,
  } = useReactFlow();
  const reactFlowStore = useStoreApi();

  // Show step picker when createStepContext changes
  useEffect(() => {
    if (createStepContext && !readOnly) {
      setShowStepPicker(true);
    } else {
      setShowStepPicker(false);
    }
  }, [createStepContext, readOnly]);

  // Get editing node data
  const editingNodeData = useMemo(() => {
    if (!editingNodeId) return null;
    const node = nodes.find((n) => n.id === editingNodeId);
    if (!node) return null;
    return {
      id: editingNodeId,
      parentId: node.parentId,
      data: (stagedNodeChanges[editingNodeId] || node.data) as form.SchemaType,
      originalData: node.data as form.SchemaType,
    };
  }, [editingNodeId, nodes, stagedNodeChanges]);

  const closeNodeConfig = useCallback(() => {
    setEditingNodeId(null);
  }, []);

  // Keep the config dialog/panel bound to the step being edited when its id
  // is renamed (renameStep changes the node id in the store immediately).
  const lastStepRename = useWorkflowStore((state) => state.lastStepRename);
  useEffect(() => {
    if (!lastStepRename) return;
    setEditingNodeId((current) =>
      current === lastStepRename.oldId ? lastStepRename.newId : current
    );
    // Keyed on the rename event only so a stale rename is never re-applied
    // to a step that later reuses the freed id.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lastStepRename]);

  const closeCreateStep = useCallback(() => {
    setCreateStepContext(null);
    setSelectedEdgeId(null);
    setShowStepPicker(false);
  }, []);

  // Handle backspace/delete key to remove selected node
  useEffect(() => {
    if (readOnly) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Backspace' && e.key !== 'Delete') return;

      const target = e.target as HTMLElement;
      const tagName = target.tagName.toLowerCase();

      if (
        tagName === 'input' ||
        tagName === 'textarea' ||
        tagName === 'select' ||
        tagName === 'button' ||
        target.isContentEditable ||
        target.closest('[role="textbox"]') ||
        target.closest('[role="combobox"]') ||
        target.closest('[role="listbox"]') ||
        target.closest('[role="menu"]') ||
        target.closest('[role="dialog"]')
      ) {
        return;
      }

      const { selectedNodeId: currentSelectedNodeId, nodes: currentNodes } =
        useWorkflowStore.getState();

      if (!currentSelectedNodeId) return;

      const selectedNode = currentNodes.find(
        (n) => n.id === currentSelectedNodeId
      );
      if (!selectedNode) return;

      if (
        selectedNode.type === NODE_TYPES.StartIndicatorNode ||
        selectedNode.type === NODE_TYPES.CreateNode
      ) {
        return;
      }

      e.preventDefault();
      removeNode(currentSelectedNodeId);
      setSelectedNodeId(null);
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [readOnly, removeNode, setSelectedNodeId]);

  // Center viewport on newly created node
  useEffect(() => {
    if (!pendingCenterNodeId) return;

    const { selectedNodeId: currentSelectedNodeId, nodes: currentNodes } =
      useWorkflowStore.getState();

    if (currentSelectedNodeId !== pendingCenterNodeId) return;

    const targetNode = currentNodes.find((n) => n.id === pendingCenterNodeId);
    if (!targetNode) return;

    setPendingCenterNodeId(null);

    const nodeType = targetNode.type || NODE_TYPES.BasicNode;
    const nodeSize = NODE_TYPE_SIZES[nodeType] || { width: 180, height: 48 };

    let absoluteX = targetNode.position.x;
    let absoluteY = targetNode.position.y;

    if (targetNode.parentId) {
      const parent = currentNodes.find((n) => n.id === targetNode.parentId);
      if (parent) {
        absoluteX += parent.position.x;
        absoluteY += parent.position.y;
      }
    }

    const nodeCenterX = absoluteX + nodeSize.width / 2;
    const nodeCenterY = absoluteY + nodeSize.height / 2;

    const zoom = getZoom();

    setTimeout(() => {
      setCenter(nodeCenterX, nodeCenterY, { zoom, duration: 300 });
    }, 100);
  }, [pendingCenterNodeId, getZoom, setCenter, setPendingCenterNodeId]);

  // Virtual Start Indicator
  const entryPointId = useMemo(() => {
    const workflowNodes = nodes.filter(
      (n) =>
        n.type !== NODE_TYPES.NoteNode &&
        n.type !== NODE_TYPES.CreateNode &&
        n.type !== NODE_TYPES.StartIndicatorNode &&
        !n.parentId
    );

    if (workflowNodes.length === 0) return null;
    if (workflow?.entryPoint) {
      const explicitEntryPoint = workflowNodes.find(
        (node) => node.id === workflow.entryPoint
      );
      if (explicitEntryPoint) return explicitEntryPoint.id;
    }

    const nodesWithIncomingEdges = new Set(
      edges
        .filter((e) => e.source !== '__start_indicator__')
        .map((e) => e.target)
    );

    // Find all nodes without incoming edges (potential entry points)
    const candidateNodes = workflowNodes.filter(
      (n) => !nodesWithIncomingEdges.has(n.id)
    );

    if (candidateNodes.length === 0) return null;

    // If multiple candidates, pick the leftmost one (smallest x position)
    // This ensures the Start indicator stays stable when edges are deleted
    const entryPointNode = candidateNodes.reduce((leftmost, node) =>
      node.position.x < leftmost.position.x ? node : leftmost
    );

    return entryPointNode?.id || null;
  }, [nodes, edges, workflow?.entryPoint]);

  // Identify AI Agent tool/memory nodes and edges to hide from canvas.
  // Tool steps are rendered inline in the AI Agent card, so their separate
  // nodes and connecting edges should not appear on the canvas.
  const { hiddenNodeIds, hiddenEdgeIds } = useMemo(() => {
    const hiddenNodes = new Set<string>();
    const hiddenEdges = new Set<string>();

    // Find all AI Agent nodes
    const aiAgentNodes = nodes.filter((n) => n.type === NODE_TYPES.AiAgentNode);

    for (const agentNode of aiAgentNodes) {
      // Find edges from this AI Agent whose sourceHandle is a tool name,
      // 'memory' or an mcp.<toolset> route (any handle that is NOT the
      // standard 'source' output). 'onError' is a normal error route — the
      // handler stays visible on the canvas like any other error branch.
      for (const edge of edges) {
        if (
          edge.source === agentNode.id &&
          edge.sourceHandle &&
          edge.sourceHandle !== 'source' &&
          edge.sourceHandle !== 'onError'
        ) {
          hiddenEdges.add(edge.id);
          hiddenNodes.add(edge.target);
        }
      }
    }

    return { hiddenNodeIds: hiddenNodes, hiddenEdgeIds: hiddenEdges };
  }, [nodes, edges]);

  const visibleCanvasNodes = useMemo(
    () =>
      hiddenNodeIds.size > 0
        ? nodes.filter((node) => !hiddenNodeIds.has(node.id))
        : nodes,
    [hiddenNodeIds, nodes]
  );

  const visibleCanvasEdges = useMemo(
    () => edges.filter((edge) => !hiddenEdgeIds.has(edge.id)),
    [edges, hiddenEdgeIds]
  );

  const layoutedCanvas = useMemo(() => {
    const layout = getLayoutedElements(visibleCanvasNodes, visibleCanvasEdges, {
      detailLevel: canvasDetail,
    });

    return {
      nodes: layout.nodes.map((node) => ({
        ...node,
        draggable: false,
      })),
      edgeRoutes: layout.edgeRoutes ?? {},
    };
  }, [canvasDetail, visibleCanvasEdges, visibleCanvasNodes]);
  const layoutedCanvasNodes = layoutedCanvas.nodes;
  const layoutedCanvasEdgeRoutes = layoutedCanvas.edgeRoutes;

  const entryPointNode = entryPointId
    ? layoutedCanvasNodes.find((n) => n.id === entryPointId)
    : null;

  const startIndicatorNode: Node | null = useMemo(() => {
    const startIndicatorSize = NODE_TYPE_SIZES[NODE_TYPES.StartIndicatorNode];

    if (entryPointNode) {
      // Use the node's actual rendered dimensions, not config defaults.
      // Containers/resizable nodes can be much larger than their default config size.
      const entryPointHeight =
        (entryPointNode.style?.height as number) ||
        (entryPointNode.height as number) ||
        NODE_TYPE_SIZES[entryPointNode.type || NODE_TYPES.BasicNode]?.height ||
        48;

      return {
        id: '__start_indicator__',
        type: NODE_TYPES.StartIndicatorNode,
        position: {
          x: entryPointNode.position.x - startIndicatorSize.width - 60,
          y:
            entryPointNode.position.y +
            entryPointHeight / 2 -
            startIndicatorSize.height / 2,
        },
        data: { hasEntryPoint: true },
        selectable: false,
        draggable: false,
        connectable: false,
        deletable: false,
        style: {
          width: startIndicatorSize.width,
          height: startIndicatorSize.height,
        },
      };
    }

    return {
      id: '__start_indicator__',
      type: NODE_TYPES.StartIndicatorNode,
      position: { x: 0, y: 0 },
      data: {
        hasEntryPoint: false,
        onAddFirstStep: () => {
          const defaultNodeHeight = 48;
          setCreateStepContext({
            position: {
              x: startIndicatorSize.width + 60,
              y: (startIndicatorSize.height - defaultNodeHeight) / 2,
            },
          });
        },
      },
      selectable: false,
      draggable: false,
      connectable: false,
      deletable: false,
      style: {
        width: startIndicatorSize.width,
        height: startIndicatorSize.height,
      },
    };
  }, [entryPointNode]);

  const startIndicatorEdge: Edge | null = useMemo(() => {
    if (!entryPointId) return null;

    return {
      id: '__start_indicator_edge__',
      type: 'default',
      source: '__start_indicator__',
      target: entryPointId,
      sourceHandle: 'source',
      targetHandle: 'target',
      selectable: true,
      deletable: false,
      data: {},
    };
  }, [entryPointId]);

  // Every canvas node renders at exactly its layout size (see node-size.ts),
  // so React Flow is told that size up front instead of measuring it. This
  // canvas drops React Flow's dimension changes (the measurements must not
  // reach the workflow store), so without `measured` React Flow never
  // considers the nodes initialized and the viewport is never fitted.
  const nodesWithStartIndicator = useMemo(() => {
    const canvasNodes = startIndicatorNode
      ? [startIndicatorNode, ...layoutedCanvasNodes]
      : layoutedCanvasNodes;
    return canvasNodes.map((node) => {
      const width = node.style?.width;
      const height = node.style?.height;
      return typeof width === 'number' && typeof height === 'number'
        ? { ...node, measured: { width, height } }
        : node;
    });
  }, [layoutedCanvasNodes, startIndicatorNode]);

  const edgesWithStartIndicator = useMemo(() => {
    const visibleNodeIds = new Set(layoutedCanvasNodes.map((node) => node.id));
    // Create a set of node IDs that are inside a container (have parentId)
    const nodesInsideContainer = new Set(
      layoutedCanvasNodes.filter((n) => n.parentId).map((n) => n.id)
    );

    // Filter out hidden tool/memory edges and apply selection
    const edgesWithSelection = visibleCanvasEdges
      .filter(
        (edge) =>
          visibleNodeIds.has(edge.source) && visibleNodeIds.has(edge.target)
      )
      .map((edge) => {
        // Check if this edge connects nodes inside a container
        const isInsideContainer =
          nodesInsideContainer.has(edge.source) ||
          nodesInsideContainer.has(edge.target);

        return {
          ...edge,
          type: edge.type || 'default',
          targetHandle: edge.targetHandle || 'target',
          selected: edge.id === selectedEdgeId,
          // Edges inside containers need higher zIndex to appear above the container background
          zIndex: isInsideContainer ? CANVAS_Z.edgeInContainer : CANVAS_Z.edge,
        };
      });

    if (!startIndicatorEdge) return edgesWithSelection;

    return [
      {
        ...startIndicatorEdge,
        selected: selectedEdgeId === '__start_indicator_edge__',
      },
      ...edgesWithSelection,
    ];
  }, [
    layoutedCanvasNodes,
    selectedEdgeId,
    startIndicatorEdge,
    visibleCanvasEdges,
  ]);

  // Initialize store with provided nodes and edges
  const lastSyncedDataRef = useRef<string>('');
  const lastFittedCanvasLayoutRef = useRef<string | null>(null);

  useEffect(() => {
    const dataHash = JSON.stringify({
      nodeIds: initialNodes.map((n) => n.id).sort(),
      edgeIds: initialEdges.map((e) => e.id).sort(),
      nodeCount: initialNodes.length,
      edgeCount: initialEdges.length,
      // Include node data to detect content changes when switching versions
      // (nodes may have same IDs but different step configurations)
      nodeData: initialNodes.map((n) => ({ id: n.id, data: n.data })),
    });

    if (dataHash !== lastSyncedDataRef.current) {
      syncFromReactFlow(initialNodes, initialEdges);
      lastSyncedDataRef.current = dataHash;
    }
  }, [initialNodes, initialEdges, syncFromReactFlow]);

  const canvasLayoutKey = useMemo(
    () =>
      JSON.stringify({
        nodes: nodesWithStartIndicator.map((node) => ({
          id: node.id,
          type: node.type,
          parentId: node.parentId,
          position: node.position,
          width: node.width ?? node.style?.width,
          height: node.height ?? node.style?.height,
        })),
        edges: edgesWithStartIndicator.map((edge) => ({
          id: edge.id,
          source: edge.source,
          target: edge.target,
          sourceHandle: edge.sourceHandle,
          targetHandle: edge.targetHandle,
        })),
      }),
    [edgesWithStartIndicator, nodesWithStartIndicator]
  );

  // What the viewport is fitted to: which steps and edges exist, and the
  // detail level. Deliberately not sizes or positions — editing a step can
  // grow its card, and re-fitting on that would yank the view mid-edit.
  const canvasFitKey = useMemo(
    () =>
      JSON.stringify({
        detail: canvasDetail,
        nodes: nodesWithStartIndicator.map((node) => node.id),
        edges: edgesWithStartIndicator.map((edge) => edge.id),
      }),
    [canvasDetail, edgesWithStartIndicator, nodesWithStartIndicator]
  );

  const fitCanvasReadable = useCallback(() => {
    const { width, height } = reactFlowStore.getState();
    const bounds = getNodesBounds(nodesWithStartIndicator);
    if (!width || !height || bounds.width === 0 || bounds.height === 0) {
      fitView({ duration: 200, ...CANVAS_FIT_VIEW_OPTIONS });
      return;
    }
    const fitted = getViewportForBounds(
      bounds,
      width,
      height,
      CANVAS_MIN_ZOOM,
      CANVAS_FIT_VIEW_OPTIONS.maxZoom,
      CANVAS_FIT_VIEW_OPTIONS.padding
    );
    if (fitted.zoom >= CANVAS_READABLE_ZOOM) {
      setViewport(fitted, { duration: 200 });
      return;
    }
    const zoom = CANVAS_READABLE_ZOOM;
    const contentHeight = bounds.height * zoom;
    setViewport(
      {
        zoom,
        x: CANVAS_VIEW_MARGIN - bounds.x * zoom,
        y:
          contentHeight + CANVAS_VIEW_MARGIN * 2 <= height
            ? (height - contentHeight) / 2 - bounds.y * zoom
            : CANVAS_VIEW_MARGIN - bounds.y * zoom,
      },
      { duration: 200 }
    );
  }, [
    fitView,
    getNodesBounds,
    nodesWithStartIndicator,
    reactFlowStore,
    setViewport,
  ]);

  // Fit only once React Flow has measured every node: fitting earlier uses
  // partial bounds and leaves part of the workflow off-screen.
  const nodesInitialized = useNodesInitialized();
  useEffect(() => {
    if (
      lastFittedCanvasLayoutRef.current === canvasFitKey ||
      (visibleCanvasNodes.length > 0 && !nodesInitialized)
    ) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      if (visibleCanvasNodes.length > 0) {
        fitCanvasReadable();
      } else {
        const startIndicatorSize =
          NODE_TYPE_SIZES[NODE_TYPES.StartIndicatorNode];
        const startCenterX = startIndicatorSize.width / 2;
        const startCenterY = startIndicatorSize.height / 2;
        setCenter(startCenterX, startCenterY, { zoom: 1, duration: 200 });
      }

      lastFittedCanvasLayoutRef.current = canvasFitKey;
    }, 100);

    return () => window.clearTimeout(timeoutId);
  }, [
    canvasFitKey,
    fitCanvasReadable,
    nodesInitialized,
    setCenter,
    visibleCanvasNodes.length,
  ]);

  useEffect(() => {
    // React Flow needs handle bounds before it can draw edges. In the locked
    // auto-layout canvas, those measurements are render-only and must not flow
    // back into the workflow store.
    const refreshNodeInternals = () => {
      const { domNode, updateNodeInternals } = reactFlowStore.getState();
      if (!domNode) return;

      const nodeElements = new Map<string, HTMLDivElement>();
      domNode
        .querySelectorAll<HTMLDivElement>('.react-flow__node[data-id]')
        .forEach((nodeElement) => {
          const nodeId = nodeElement.getAttribute('data-id');
          if (nodeId) {
            nodeElements.set(nodeId, nodeElement);
          }
        });

      const updates = new Map();
      nodesWithStartIndicator.forEach((node) => {
        const nodeElement = nodeElements.get(node.id);
        if (nodeElement) {
          updates.set(node.id, {
            id: node.id,
            nodeElement,
            force: true,
          });
        }
      });

      if (updates.size > 0) {
        updateNodeInternals(updates, { triggerFitView: false });
      }
    };

    const animationFrameId = window.requestAnimationFrame(refreshNodeInternals);
    const timeoutIds = [50, 250, 750].map((delay) =>
      window.setTimeout(refreshNodeInternals, delay)
    );

    return () => {
      window.cancelAnimationFrame(animationFrameId);
      timeoutIds.forEach((timeoutId) => window.clearTimeout(timeoutId));
    };
  }, [canvasLayoutKey, nodesWithStartIndicator, reactFlowStore]);

  const onConnect = useCallback(
    (connection: Edge | Connection) => {
      if ('source' in connection && 'target' in connection) {
        const validation = validateConnection(
          edges,
          nodes,
          connection.source!,
          connection.target!
        );

        if (!validation.isValid) {
          toast.error('Invalid Connection', {
            description: validation.errorMessage,
          });
          return;
        }

        addStoreEdge(
          connection.source!,
          connection.target!,
          connection.sourceHandle || undefined
        );
      }
    },
    [addStoreEdge, edges, nodes]
  );

  const isValidConnection = useCallback(
    (connection: Edge | Connection) => {
      if (!connection.source || !connection.target) return false;

      const validation = validateConnection(
        edges,
        nodes,
        connection.source,
        connection.target
      );

      return validation.isValid;
    },
    [edges, nodes]
  );

  const onConnectEnd: OnConnectEnd = useCallback(
    (event, connectionState) => {
      if (!connectionState.toNode && ref.current) {
        const { fromNode, fromHandle } = connectionState;

        if (fromNode) {
          const sourceNode = nodes.find((n) => n.id === fromNode.id);
          if (!sourceNode) return;

          const clientX =
            'clientX' in event ? event.clientX : event.touches?.[0]?.clientX;
          const clientY =
            'clientY' in event ? event.clientY : event.touches?.[0]?.clientY;

          if (clientX === undefined || clientY === undefined) return;

          const mousePosition = screenToFlowPosition({
            x: clientX,
            y: clientY,
          });
          const draggedUp = mousePosition.y < sourceNode.position.y;
          const horizontalOffset = 216;
          const verticalSpacing = 120;

          const existingTargets = edges
            .filter((e) => e.source === fromNode.id)
            .map((e) => nodes.find((n) => n.id === e.target))
            .filter((n) => n !== undefined);

          let verticalPosition = sourceNode.position.y;

          if (existingTargets.length > 0) {
            if (draggedUp) {
              const firstSibling = existingTargets[0];
              verticalPosition = firstSibling.position.y - verticalSpacing;
            } else {
              const lastSibling = existingTargets[existingTargets.length - 1];
              verticalPosition = lastSibling.position.y + verticalSpacing;
            }
          }

          const calculatedPosition = snapPositionToGrid({
            x: sourceNode.position.x + horizontalOffset,
            y: verticalPosition,
          });

          setCreateStepContext({
            position: calculatedPosition,
            connection: {
              source: fromNode.id,
              sourceHandle: fromHandle?.id || 'source',
            },
          });
        }
      }
    },
    [screenToFlowPosition, nodes, edges]
  );

  const handleEdgeInsertClick = useCallback(
    (edgeData: {
      id: string;
      source: string;
      target: string;
      sourceHandle: string;
      position: { x: number; y: number };
    }) => {
      setCreateStepContext({
        position: edgeData.position,
        insertionEdge: {
          source: edgeData.source,
          target: edgeData.target,
          sourceHandle: edgeData.sourceHandle,
        },
      });
    },
    []
  );

  const handleEdgeParallelClick = useCallback(
    (edgeData: {
      source: string;
      target: string;
      sourceHandle: string;
      position: { x: number; y: number };
    }) => {
      setCreateStepContext({
        position: edgeData.position,
        parallelBranch: {
          source: edgeData.source,
          target: edgeData.target,
          sourceHandle: edgeData.sourceHandle,
        },
      });
    },
    []
  );

  // Route condition/priority editing on a selected edge. Conditions share the
  // canonical suggestion pipeline, scoped to the container the edge is in.
  const updateEdgeData = useWorkflowStore((state) => state.updateEdgeData);
  const edgeRouteActions = useMemo<EdgeRouteActions>(
    () => ({
      onUpdateRouteData: (edgeId, updates) => updateEdgeData(edgeId, updates),
      onDeleteRoute: (edgeId) =>
        onEdgesChange([{ id: edgeId, type: 'remove' }]),
      stepName: (stepId) => canvasDisplayValue.stepNames.get(stepId) || stepId,
      conditionSuggestionsForEdge: (edge) => {
        const container = nodes.find((n) => n.id === edge.source)?.parentId;
        const scope = resolveContainerScope(nodes as ScopeNode[], container);
        return composeConditionSuggestions({
          previousSteps: [],
          inputSchemaFields: workflow?.inputSchemaFields,
          variables: workflow?.variables,
          isInsideWhileLoop: scope.isInsideWhileLoop,
          isInsideSplit: scope.isInsideSplit,
          isInsideWaitScope: scope.isInsideWaitScope,
          splitItemSchemaFields: splitItemSchemaFieldsFromScope(scope),
        });
      },
    }),
    [
      canvasDisplayValue.stepNames,
      nodes,
      onEdgesChange,
      updateEdgeData,
      workflow?.inputSchemaFields,
      workflow?.variables,
    ]
  );

  // Handle step picker selection - stores pending node for user confirmation
  const handleStepPickerSelect = useCallback(
    (result: StepPickerResult) => {
      if (!createStepContext) return;

      const uniqueName = generateUniqueStepName(result.name, nodes);
      const newNodeId = generateStepId(
        uniqueName,
        nodes.map((node) => node.id)
      );
      const data = {
        ...form.initialValues,
        stepType: result.stepType,
        name: uniqueName,
        agentId: result.agentId || '',
        capabilityId: result.capabilityId || '',
      } as form.SchemaType;

      // Parallel branch over an existing edge: defer creation like the other
      // flows; the diamond edges are wired in handlePendingNodeSave.
      if (createStepContext.parallelBranch) {
        const sourceNode = nodes.find(
          (n) => n.id === createStepContext.parallelBranch!.source
        );

        let finalPosition = createStepContext.position;
        let parentId: string | undefined;

        if (sourceNode?.parentId) {
          parentId = sourceNode.parentId;

          const getAbsolutePosition = (
            node: Node
          ): { x: number; y: number } => {
            if (!node.parentId) return node.position;
            const parent = nodes.find((n) => n.id === node.parentId);
            if (!parent) return node.position;
            const parentAbsolute = getAbsolutePosition(parent);
            return {
              x: parentAbsolute.x + node.position.x,
              y: parentAbsolute.y + node.position.y,
            };
          };

          const parentNode = nodes.find((n) => n.id === parentId);
          if (parentNode) {
            const parentAbsolute = getAbsolutePosition(parentNode);
            finalPosition = {
              x: createStepContext.position.x - parentAbsolute.x,
              y: createStepContext.position.y - parentAbsolute.y,
            };
          }
        }

        setPendingNewNode({
          id: newNodeId,
          data: data as any,
          position: finalPosition,
          parentId,
          sourceNodeId: createStepContext.parallelBranch.source,
          sourceHandle: createStepContext.parallelBranch.sourceHandle,
          targetNodeId: createStepContext.parallelBranch.target,
        });

        setCreateStepContext(null);
        setSelectedEdgeId(null);
        setShowStepPicker(false);
        return;
      }

      // Check if we're inserting between nodes via edge click
      if (createStepContext.insertionEdge) {
        if (createStepContext.insertionEdge.source === '__start_indicator__') {
          const targetNode = nodes.find(
            (n) => n.id === createStepContext.insertionEdge!.target
          );
          if (targetNode) {
            const nodeType = STEP_TYPES[data.stepType] || NODE_TYPES.BasicNode;
            const newNodeSize = NODE_TYPE_SIZES[nodeType] || {
              width: 180,
              height: 48,
            };
            const horizontalSpacing = 108;

            const newPosition = snapPositionToGrid({
              x: targetNode.position.x - newNodeSize.width - horizontalSpacing,
              y:
                targetNode.position.y +
                (targetNode.height || 48) / 2 -
                newNodeSize.height / 2,
            });

            // Store as pending node instead of creating immediately
            setPendingNewNode({
              id: newNodeId,
              data: data as any,
              position: newPosition,
              targetNodeId: createStepContext.insertionEdge.target,
              becomesEntryPoint: true,
            });

            setCreateStepContext(null);
            setSelectedEdgeId(null);
            setShowStepPicker(false);
            return;
          }
        }

        // Regular insertion between nodes
        setPendingNewNode({
          id: newNodeId,
          data: data as any,
          position: createStepContext.position,
          insertionEdge: createStepContext.insertionEdge,
        });
        setCreateStepContext(null);
        setSelectedEdgeId(null);
        setShowStepPicker(false);
        return;
      }

      // Check if we're creating a node from a dragged connection
      if (createStepContext.connection) {
        const sourceNode = nodes.find(
          (n) => n.id === createStepContext.connection!.source
        );
        const type = STEP_TYPES[data.stepType] || NODE_TYPES.BasicNode;
        const newNodeSize = NODE_TYPE_SIZES[type];

        let finalPosition = createStepContext.position;
        let parentId: string | undefined;

        if (sourceNode) {
          const getAbsolutePosition = (
            node: Node
          ): { x: number; y: number } => {
            if (!node.parentId) return node.position;
            const parent = nodes.find((n) => n.id === node.parentId);
            if (!parent) return node.position;
            const parentAbsolute = getAbsolutePosition(parent);
            return {
              x: parentAbsolute.x + node.position.x,
              y: parentAbsolute.y + node.position.y,
            };
          };

          const sourceAbsolute = getAbsolutePosition(sourceNode);
          const sourceHandleY =
            sourceAbsolute.y + (sourceNode.height || 48) / 2;
          const newNodeHandleOffset = newNodeSize.height / 2;

          const absolutePosition = {
            x: createStepContext.position.x,
            y: sourceHandleY - newNodeHandleOffset,
          };

          if (sourceNode.parentId) {
            parentId = sourceNode.parentId;
            const parentNode = nodes.find((n) => n.id === parentId);
            if (parentNode) {
              const parentAbsolute = getAbsolutePosition(parentNode);
              finalPosition = {
                x: absolutePosition.x - parentAbsolute.x,
                y: absolutePosition.y - parentAbsolute.y,
              };
            }
          } else {
            finalPosition = absolutePosition;
          }
        }

        // Store as pending node instead of creating immediately
        setPendingNewNode({
          id: newNodeId,
          data: data as any,
          position: finalPosition,
          parentId,
          sourceNodeId: createStepContext.connection.source,
          sourceHandle: createStepContext.connection.sourceHandle,
        });

        setCreateStepContext(null);
        setSelectedEdgeId(null);
        setShowStepPicker(false);
        return;
      }

      // Regular node creation flow
      const type = STEP_TYPES[data.stepType] || NODE_TYPES.BasicNode;
      const style = NODE_TYPE_SIZES[type];

      let parentId: string | undefined = createStepContext.parentId;
      let finalPosition = createStepContext.position;

      const tempNode: Node = {
        id: 'temp',
        type,
        position: createStepContext.position,
        data: {},
        style,
        width: style.width,
        height: style.height,
      };

      const intersections = getIntersectingNodes(tempNode).filter(
        (n) => n.type === NODE_TYPES.ContainerNode
      );

      const groupNode = intersections[intersections.length - 1];

      if (!parentId && groupNode) {
        finalPosition = getNodePositionInsideParent(tempNode, groupNode) ?? {
          x: 0,
          y: 0,
        };
        parentId = groupNode?.id;
      }

      // Store as pending node instead of creating immediately
      setPendingNewNode({
        id: newNodeId,
        data: data as any,
        position: finalPosition,
        parentId,
      });

      setCreateStepContext(null);
      setSelectedEdgeId(null);
      setShowStepPicker(false);
    },
    [createStepContext, getIntersectingNodes, nodes, setPendingNewNode]
  );

  const handleCancelCreate = useCallback(() => {
    closeCreateStep();
    setPendingNewNode(null);
    setSelectedNodeId(null);
  }, [closeCreateStep, setPendingNewNode, setSelectedNodeId]);

  const validateNodeEditCandidate = useCallback(
    async (
      nodeId: string,
      data: form.SchemaType,
      pendingNode?: PendingNodeCandidate | null
    ) => {
      const currentState = useWorkflowStore.getState();
      const nodesWithStagedChanges = currentState.nodes.map((node) => {
        const stagedData = stagedNodeChanges[node.id];
        return stagedData
          ? {
              ...node,
              data: {
                ...node.data,
                ...stagedData,
              },
            }
          : node;
      });
      const validation = await validateNodeEditWithRust({
        nodes: nodesWithStagedChanges,
        edges: currentState.edges,
        nodeId,
        data,
        workflow,
        pendingNode,
      });

      const validationStore = useValidationStore.getState();
      validationStore.clearMessages('client');
      if (validation.messages.length > 0) {
        validationStore.addMessages(validation.messages);
      }

      if (!validation.canApply) {
        toast.error('Step validation failed', {
          description:
            validation.rustValidation.errors[0] ||
            validation.rustValidation.message,
        });
        return false;
      }

      if (validation.rustValidation.status === 'unavailable') {
        toast.warning('Rust validation unavailable', {
          description: validation.rustValidation.message,
        });
      }

      return true;
    },
    [stagedNodeChanges, workflow]
  );

  // Node config dialog handlers
  const handleNodeSave = useCallback(
    async (nodeId: string, data: form.SchemaType) => {
      const canApply = await validateNodeEditCandidate(nodeId, data);
      if (!canApply) return false;

      updateNode(nodeId, data as unknown as Partial<ExecutionGraphStepDto>);
      onResetNodeChanges?.(nodeId);
      closeNodeConfig();
      return true;
    },
    [validateNodeEditCandidate, updateNode, onResetNodeChanges, closeNodeConfig]
  );

  const handleNodeDelete = useCallback(
    (nodeId: string) => {
      removeNode(nodeId);
      setSelectedNodeId(null);
      closeNodeConfig();
    },
    [removeNode, setSelectedNodeId, closeNodeConfig]
  );

  // Pending new node handlers
  const handlePendingNodeSave = useCallback(
    async (nodeId: string, data: form.SchemaType) => {
      if (!pendingNewNode || pendingNewNode.id !== nodeId) return false;

      const canApply = await validateNodeEditCandidate(
        nodeId,
        data,
        pendingNewNode
      );
      if (!canApply) return false;

      const nodeType = STEP_TYPES[data.stepType] || NODE_TYPES.BasicNode;
      const isConditional =
        nodeType === NODE_TYPES.ConditionalNode ||
        data.stepType === 'Conditional';

      // Handle edge connections based on creation context
      if (pendingNewNode.insertionEdge) {
        // Inserting between existing nodes - insertNodeBetween handles everything
        insertNodeBetween(
          pendingNewNode.insertionEdge.source,
          pendingNewNode.insertionEdge.target,
          pendingNewNode.insertionEdge.sourceHandle,
          { ...data, id: nodeId } as any,
          pendingNewNode.position
        );
        setPendingCenterNodeId(nodeId);
        setSelectedNodeId(nodeId);
      } else {
        // Create the node
        const createdNodeId = addNode(
          { ...data, id: nodeId } as any,
          pendingNewNode.position,
          pendingNewNode.parentId
        );

        if (createdNodeId) {
          if (pendingNewNode.sourceNodeId && pendingNewNode.targetNodeId) {
            // Parallel branch: the existing source->target edge is kept; the
            // new step gets the diamond legs source->new and new->target so
            // the unconditional fan-out re-converges (validator E073).
            const continuationEdges = isConditional
              ? [
                  {
                    from: createdNodeId,
                    to: pendingNewNode.targetNodeId,
                    label: 'true',
                  },
                  {
                    from: createdNodeId,
                    to: pendingNewNode.targetNodeId,
                    label: 'false',
                  },
                ]
              : [
                  {
                    from: createdNodeId,
                    to: pendingNewNode.targetNodeId,
                    label: 'source',
                  },
                ];

            addStoreEdges([
              {
                from: pendingNewNode.sourceNodeId,
                to: createdNodeId,
                label: pendingNewNode.sourceHandle || 'source',
              },
              ...continuationEdges,
            ]);
          } else if (pendingNewNode.targetNodeId) {
            // Connecting to a target node (e.g., inserting before first node from start indicator)
            if (isConditional) {
              addStoreEdges([
                {
                  from: createdNodeId,
                  to: pendingNewNode.targetNodeId,
                  label: 'true',
                },
                {
                  from: createdNodeId,
                  to: pendingNewNode.targetNodeId,
                  label: 'false',
                },
              ]);
            } else {
              addStoreEdge(
                createdNodeId,
                pendingNewNode.targetNodeId,
                'source'
              );
            }
            // A pinned entry point would leave the new first step unreachable
            // (E002). In auto mode the entry is re-derived from the edges.
            if (pendingNewNode.becomesEntryPoint && workflow?.entryPoint) {
              onWorkflowChange?.({ entryPoint: createdNodeId });
            }
          } else if (pendingNewNode.sourceNodeId) {
            // Connecting from a source node (dragged connection)
            addStoreEdge(
              pendingNewNode.sourceNodeId,
              createdNodeId,
              pendingNewNode.sourceHandle || 'source'
            );
          }

          setPendingCenterNodeId(createdNodeId);
          setSelectedNodeId(createdNodeId);
        }
      }

      setPendingNewNode(null);
      return true;
    },
    [
      pendingNewNode,
      validateNodeEditCandidate,
      addNode,
      addStoreEdge,
      addStoreEdges,
      insertNodeBetween,
      setPendingCenterNodeId,
      setSelectedNodeId,
      setPendingNewNode,
      workflow?.entryPoint,
      onWorkflowChange,
    ]
  );

  const handlePendingNodeCancel = useCallback(() => {
    setPendingNewNode(null);
  }, [setPendingNewNode]);

  const handleNodesChange = useCallback(
    (changes: import('@xyflow/react').NodeChange[]) => {
      const structuralChanges = changes.filter(
        (change) => change.type !== 'position' && change.type !== 'dimensions'
      );
      if (structuralChanges.length > 0) {
        onNodesChange(structuralChanges);
      }
    },
    [onNodesChange]
  );

  return (
    <>
      <div data-workflow-editor-root="" className="relative h-full w-full">
        <NodeConfigProvider value={nodeConfigContextValue}>
          <CanvasDisplayProvider value={canvasDisplayValue}>
            <EdgeContextProvider
              onInsertClick={readOnly ? undefined : handleEdgeInsertClick}
              onParallelClick={readOnly ? undefined : handleEdgeParallelClick}
              routeActions={readOnly ? undefined : edgeRouteActions}
              allEdges={edges}
              edgeRoutes={layoutedCanvasEdgeRoutes}
            >
              <ReactFlow
                ref={ref}
                nodes={nodesWithStartIndicator}
                edges={edgesWithStartIndicator}
                edgeTypes={edgeTypes}
                nodeTypes={nodeTypes}
                onNodesChange={handleNodesChange}
                onEdgesChange={onEdgesChange}
                onConnect={readOnly ? undefined : onConnect}
                onConnectEnd={readOnly ? undefined : onConnectEnd}
                isValidConnection={readOnly ? undefined : isValidConnection}
                proOptions={{ hideAttribution: true }}
                minZoom={CANVAS_MIN_ZOOM}
                maxZoom={2}
                // Double-click opens a step's editor. React Flow's zoom-on-
                // double-click (d3-zoom) stops the event at the pane, and with
                // nodes not draggable they lack the class that exempts them,
                // so onNodeDoubleClick never fired.
                zoomOnDoubleClick={false}
                defaultEdgeOptions={{ zIndex: CANVAS_Z.edge }}
                onNodeDoubleClick={(_event, node) => {
                  // Don't open dialog for special nodes
                  if (
                    node.type === NODE_TYPES.CreateNode ||
                    node.type === NODE_TYPES.NoteNode ||
                    node.type === NODE_TYPES.StartIndicatorNode
                  ) {
                    return;
                  }
                  // Open dialog for editing on double-click
                  if (workflow && !readOnly) {
                    setEditingNodeId(node.id);
                  }
                }}
                onNodeClick={(event, node) => {
                  // Don't select special nodes
                  if (
                    node.type === NODE_TYPES.CreateNode ||
                    node.type === NODE_TYPES.NoteNode ||
                    node.type === NODE_TYPES.StartIndicatorNode
                  ) {
                    setSelectedNodeId(null);
                    setSelectedEdgeId(null);
                    return;
                  }
                  // Don't select if clicking on a button
                  const target = event.target as HTMLElement;
                  if (target.closest('button')) {
                    return;
                  }
                  // In debug inspect mode, allow selecting executed nodes only
                  if (debugInspectMode) {
                    const nodeStatus = useExecutionStore
                      .getState()
                      .nodeExecutionStatus.get(node.id);
                    if (nodeStatus) {
                      setSelectedNodeId(node.id);
                      setSelectedEdgeId(null);
                    }
                    return;
                  }
                  // Select node on single click (for moving, keyboard delete, etc.)
                  if (workflow && !readOnly) {
                    setSelectedNodeId(node.id);
                    setSelectedEdgeId(null);
                    // With a step's editor already open, a click moves it to
                    // the clicked step: reading through steps should not
                    // need a close and a double-click per step. Unsaved
                    // edits stay staged on the step they were made on.
                    if (editingNodeId && editingNodeId !== node.id) {
                      openNodeConfig(node.id);
                    }
                  }
                }}
                onEdgeClick={(_event, edge) => {
                  setSelectedEdgeId(edge.id);
                  setSelectedNodeId(null);
                }}
                onPaneClick={() => {
                  const { selectedNodeId: currentSelectedNodeId } =
                    useWorkflowStore.getState();
                  if (currentSelectedNodeId) {
                    setSelectedNodeId(null);
                  }
                  if (selectedEdgeId) {
                    setSelectedEdgeId(null);
                  }
                }}
                snapToGrid={true}
                snapGrid={[SNAP_GRID_SIZE, SNAP_GRID_SIZE]}
                elevateEdgesOnSelect={true}
                edgesReconnectable={false}
                disableKeyboardA11y={true}
                elementsSelectable={!readOnly || debugInspectMode}
                nodesDraggable={false}
                nodesConnectable={!readOnly}
                nodesFocusable={!readOnly}
                edgesFocusable={!readOnly}
              >
                <Controls fitViewOptions={CANVAS_FIT_VIEW_OPTIONS} />
                <Panel position="bottom-right">
                  <CanvasDetailToggle
                    value={canvasDetail}
                    onChange={setCanvasDetail}
                  />
                </Panel>
                <Background
                  gap={SNAP_GRID_SIZE}
                  size={1}
                  color={isDark ? '#262626' : undefined}
                />
              </ReactFlow>
            </EdgeContextProvider>
          </CanvasDisplayProvider>
        </NodeConfigProvider>
      </div>

      {/* Step Picker Modal */}
      <NodeFormProvider
        isAddingBefore={nodes.length === 0}
        parentNodeId={
          createStepContext?.connection?.source ||
          createStepContext?.insertionEdge?.source ||
          createStepContext?.parallelBranch?.source
        }
        createContainerId={createStepContext?.parentId ?? null}
      >
        <StepPickerModal
          open={showStepPicker}
          onOpenChange={(open) => {
            if (!open) {
              handleCancelCreate();
            }
          }}
          onSelect={handleStepPickerSelect}
          allowFinish={
            !createStepContext?.insertionEdge &&
            !createStepContext?.parallelBranch
          }
          closeOnSelect={false}
        />
      </NodeFormProvider>

      {/* Docked step inspector - for editing existing nodes */}
      {editingNodeData && workflow && (
        <NodeConfigPanel
          open={!!editingNodeId}
          onOpenChange={(open) => {
            if (!open) closeNodeConfig();
          }}
          nodeId={editingNodeData.id}
          parentNodeId={editingNodeData.parentId}
          nodeData={editingNodeData.data}
          originalNodeData={editingNodeData.originalData}
          outputSchemaFields={workflow.outputSchemaFields}
          inputSchemaFields={workflow.inputSchemaFields}
          stateSchemaFields={workflow.stateSchemaFields}
          variables={workflow.variables}
          onSave={handleNodeSave}
          onStagedChange={onStagedNodeChange}
          onReset={onResetNodeChanges}
          onDelete={handleNodeDelete}
        />
      )}

      {/* Docked step inspector - for creating new nodes */}
      {pendingNewNode && workflow && (
        <NodeConfigPanel
          open={!!pendingNewNode}
          onOpenChange={(open) => {
            if (!open) handlePendingNodeCancel();
          }}
          nodeId={pendingNewNode.id}
          nodeData={pendingNewNode.data as unknown as form.SchemaType}
          originalNodeData={pendingNewNode.data as unknown as form.SchemaType}
          outputSchemaFields={workflow.outputSchemaFields}
          inputSchemaFields={workflow.inputSchemaFields}
          stateSchemaFields={workflow.stateSchemaFields}
          variables={workflow.variables}
          onSave={handlePendingNodeSave}
          isCreate
          parentNodeId={
            pendingNewNode.sourceNodeId || pendingNewNode.insertionEdge?.source
          }
          createContainerId={pendingNewNode.parentId ?? null}
        />
      )}
    </>
  );
}
