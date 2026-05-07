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
  useReactFlow,
  useStoreApi,
  OnConnectEnd,
} from '@xyflow/react';
import { ListTree, Network } from 'lucide-react';
import { v4 as uuidv4 } from 'uuid';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { toast } from '@/shared/hooks/useToast';
import { cn } from '@/lib/utils.ts';
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/shared/components/ui/tabs';
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
  EventNode,
  SwitchNode,
  AiAgentNode,
  NoteNode,
  StartIndicatorNode,
} from './CustomNodes';
import {
  getNodePositionInsideParent,
  getLayoutedElements,
} from './CustomNodes/utils.tsx';
import { NodeConfigDialog } from './NodeConfigDialog';
import { NodeFormProvider } from './NodeForm/NodeFormProvider';
import {
  StepPickerModal,
  StepPickerPanel,
  StepPickerResult,
} from './NodeForm/StepPickerModal';
import { NodeConfigProvider } from './NodeConfigContext';
import {
  type TimelineAddStepRequest,
  WorkflowTimelineView,
} from './TimelineView';
import { TimelineNodeConfigPanel } from './TimelineNodeConfigPanel';

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
}

const nodeTypes: NodeTypes = {
  [NODE_TYPES.BasicNode]: BasicNode,
  [NODE_TYPES.CreateNode]: CreateNode,
  [NODE_TYPES.ConditionalNode]: ConditionalNode,
  [NODE_TYPES.SwitchNode]: SwitchNode,
  [NODE_TYPES.EventNode]: EventNode,
  [NODE_TYPES.WaitNode]: BasicNode,
  [NODE_TYPES.ContainerNode]: ContainerNode,
  [NODE_TYPES.GroupByNode]: BasicNode,
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

type WorkflowInputMappingItem = {
  type?: string;
  value?: unknown;
  typeHint?: string;
  valueType?: string;
  [key: string]: unknown;
};

function generateUniqueAiToolName(result: StepPickerResult, nodes: Node[]) {
  const allToolNames = new Set<string>();

  for (const node of nodes) {
    if (node.type !== NODE_TYPES.AiAgentNode) continue;
    const inputMapping = (node.data?.inputMapping || []) as
      | WorkflowInputMappingItem[]
      | undefined;
    const toolsField = inputMapping?.find((item) => item.type === 'tools');
    if (Array.isArray(toolsField?.value)) {
      for (const toolName of toolsField.value) {
        if (typeof toolName === 'string') allToolNames.add(toolName);
      }
    }
  }

  const baseName = (result.name || 'tool')
    .toLowerCase()
    .replace(/[^a-z0-9_]/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_|_$/g, '');
  const normalizedBaseName = baseName || 'tool';
  let toolName = normalizedBaseName;
  let index = 1;

  while (allToolNames.has(toolName)) {
    toolName = `${normalizedBaseName}_${index++}`;
  }

  return toolName;
}

function appendToolToInputMapping(
  inputMapping: WorkflowInputMappingItem[],
  toolName: string
) {
  const updatedMapping = [...inputMapping];
  const toolsIndex = updatedMapping.findIndex((item) => item.type === 'tools');

  if (toolsIndex >= 0) {
    const existingTools = Array.isArray(updatedMapping[toolsIndex].value)
      ? (updatedMapping[toolsIndex].value as unknown[])
      : [];
    updatedMapping[toolsIndex] = {
      ...updatedMapping[toolsIndex],
      value: [...existingTools, toolName],
    };
    return updatedMapping;
  }

  return [
    ...updatedMapping,
    {
      type: 'tools',
      value: [toolName],
      typeHint: 'json',
      valueType: 'immediate',
    },
  ];
}

function setOrAddInputMappingValue(
  inputMapping: WorkflowInputMappingItem[],
  type: string,
  value: unknown,
  typeHint: string,
  valueType = 'immediate'
) {
  const index = inputMapping.findIndex((item) => item.type === type);
  if (index >= 0) {
    inputMapping[index] = {
      ...inputMapping[index],
      value,
      valueType,
    };
    return;
  }

  inputMapping.push({ type, value, typeHint, valueType });
}

function createMemoryProviderInputMapping(): WorkflowInputMappingItem[] {
  return [
    {
      type: 'conversation_id',
      value: '',
      valueType: 'reference',
      typeHint: 'string',
    },
    {
      type: 'messages',
      value: '',
      valueType: 'reference',
      typeHint: 'json',
    },
  ];
}

// Type for staged node changes
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
    variables?: Array<{ name: string; value: string; type: string }>;
    inputSchemaFields?: Array<{
      name: string;
      type: string;
      required: boolean;
      description: string;
    }>;
    outputSchemaFields?: Array<{
      name: string;
      type: string;
      required: boolean;
      description: string;
    }>;
    executionTimeoutSeconds?: number;
  };
  stagedNodeChanges?: StagedNodeChanges;
  onStagedNodeChange?: (nodeId: string, data: form.SchemaType) => void;
  onResetNodeChanges?: (nodeId: string) => void;
};

type WorkflowEditorView = 'canvas' | 'timeline';
type NodeEditSurface = 'dialog' | 'timeline';

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
}: WorkflowEditorProps) {
  // Context for creating new steps
  const [createStepContext, setCreateStepContext] =
    useState<CreateStepContext | null>(null);
  const [createStepSurface, setCreateStepSurface] =
    useState<NodeEditSurface | null>(null);
  const [pendingNodeSurface, setPendingNodeSurface] =
    useState<NodeEditSurface | null>(null);
  const [timelineAddStepRequest, setTimelineAddStepRequest] =
    useState<TimelineAddStepRequest | null>(null);

  // Track selected edge ID for showing "+" button
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);

  // Dialog states
  const [editingNodeId, setEditingNodeId] = useState<string | null>(null);
  const [editingSurface, setEditingSurface] = useState<NodeEditSurface | null>(
    null
  );
  const [showStepPicker, setShowStepPicker] = useState(false);
  const [editorView, setEditorView] = useState<WorkflowEditorView>('timeline');

  // Callback for child node components to open config dialogs (including hidden nodes)
  const openNodeConfig = useCallback(
    (nodeId: string) => {
      if (workflow && !readOnly) {
        setCreateStepContext(null);
        setCreateStepSurface(null);
        setTimelineAddStepRequest(null);
        setPendingNodeSurface(null);
        setEditingNodeId(nodeId);
        setEditingSurface('dialog');
      }
    },
    [workflow, readOnly]
  );
  const openTimelineNodeConfig = useCallback(
    (nodeId: string) => {
      if (workflow && !readOnly && !debugInspectMode) {
        setCreateStepContext(null);
        setCreateStepSurface(null);
        setTimelineAddStepRequest(null);
        setPendingNodeSurface(null);
        setEditingNodeId(nodeId);
        setEditingSurface('timeline');
      }
    },
    [debugInspectMode, workflow, readOnly]
  );
  const nodeConfigContextValue = useMemo(
    () => ({ openNodeConfig }),
    [openNodeConfig]
  );
  const timelineNodeConfigContextValue = useMemo(
    () => ({ openNodeConfig: openTimelineNodeConfig }),
    [openTimelineNodeConfig]
  );

  // Pending new node state from store (for deferred creation until user confirms)
  const pendingNewNode = useWorkflowStore((state) => state.pendingNewNode);
  const setPendingNewNode = useWorkflowStore(
    (state) => state.setPendingNewNode
  );

  const ref = useRef(null);

  // Get state from Zustand store
  const nodes = useWorkflowStore((state) => state.nodes);
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
    screenToFlowPosition,
    fitView,
    setCenter,
    getZoom,
  } = useReactFlow();
  const reactFlowStore = useStoreApi();

  // Show step picker when createStepContext changes
  useEffect(() => {
    if (createStepContext && !readOnly && createStepSurface !== 'timeline') {
      setShowStepPicker(true);
    } else {
      setShowStepPicker(false);
    }
  }, [createStepContext, createStepSurface, readOnly]);

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
    setEditingSurface(null);
  }, []);

  const closeCreateStep = useCallback(() => {
    setCreateStepContext(null);
    setCreateStepSurface(null);
    setTimelineAddStepRequest(null);
    setPendingNodeSurface(null);
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
  }, [nodes, edges]);

  // Identify AI Agent tool/memory nodes and edges to hide from canvas.
  // Tool steps are rendered inline in the AI Agent card, so their separate
  // nodes and connecting edges should not appear on the canvas.
  const { hiddenNodeIds, hiddenEdgeIds } = useMemo(() => {
    const hiddenNodes = new Set<string>();
    const hiddenEdges = new Set<string>();

    // Find all AI Agent nodes
    const aiAgentNodes = nodes.filter((n) => n.type === NODE_TYPES.AiAgentNode);

    for (const agentNode of aiAgentNodes) {
      // Find edges from this AI Agent whose sourceHandle is a tool name or 'memory'
      // (any handle that is NOT the standard 'source' output)
      for (const edge of edges) {
        if (
          edge.source === agentNode.id &&
          edge.sourceHandle &&
          edge.sourceHandle !== 'source'
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
    const layout = getLayoutedElements(visibleCanvasNodes, visibleCanvasEdges);

    return {
      nodes: layout.nodes.map((node) => ({
        ...node,
        draggable: false,
      })),
      edgeRoutes: layout.edgeRoutes ?? {},
    };
  }, [visibleCanvasEdges, visibleCanvasNodes]);
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
          setCreateStepSurface('dialog');
          setTimelineAddStepRequest(null);
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

  const nodesWithStartIndicator = useMemo(() => {
    if (!startIndicatorNode) return layoutedCanvasNodes;
    return [startIndicatorNode, ...layoutedCanvasNodes];
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
          zIndex: isInsideContainer ? 1001 : 1,
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

  // Initialize the canvas viewport only while the canvas is visible. React Flow
  // measures a hidden canvas poorly, which leaves the workflow in a corner on
  // first switch from the timeline.
  useEffect(() => {
    if (editorView !== 'canvas') {
      lastFittedCanvasLayoutRef.current = null;
    }
  }, [editorView]);

  useEffect(() => {
    if (
      editorView !== 'canvas' ||
      lastFittedCanvasLayoutRef.current === canvasLayoutKey
    ) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      if (visibleCanvasNodes.length > 0) {
        fitView({ duration: 200, padding: 0.2 });
      } else {
        const startIndicatorSize =
          NODE_TYPE_SIZES[NODE_TYPES.StartIndicatorNode];
        const startCenterX = startIndicatorSize.width / 2;
        const startCenterY = startIndicatorSize.height / 2;
        setCenter(startCenterX, startCenterY, { zoom: 1, duration: 200 });
      }

      lastFittedCanvasLayoutRef.current = canvasLayoutKey;
    }, 100);

    return () => window.clearTimeout(timeoutId);
  }, [
    canvasLayoutKey,
    editorView,
    fitView,
    setCenter,
    visibleCanvasNodes.length,
  ]);

  useEffect(() => {
    if (editorView !== 'canvas') return;

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
  }, [canvasLayoutKey, editorView, nodesWithStartIndicator, reactFlowStore]);

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
          toast({
            title: 'Invalid Connection',
            description: validation.errorMessage,
            variant: 'destructive',
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

          setCreateStepSurface('dialog');
          setTimelineAddStepRequest(null);
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
      setCreateStepSurface('dialog');
      setTimelineAddStepRequest(null);
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

  const handleTimelineAddStep = useCallback(
    (request: TimelineAddStepRequest) => {
      const sourceNode = request.sourceNodeId
        ? nodes.find((node) => node.id === request.sourceNodeId)
        : undefined;
      const targetNode = request.targetNodeId
        ? nodes.find((node) => node.id === request.targetNodeId)
        : undefined;

      setCreateStepSurface('timeline');
      setPendingNodeSurface(null);
      setPendingNewNode(null);
      setTimelineAddStepRequest(request);
      closeNodeConfig();

      const getAbsolutePosition = (node: Node): { x: number; y: number } => {
        if (!node.parentId) return node.position;

        const parent = nodes.find(
          (candidate) => candidate.id === node.parentId
        );
        if (!parent) return node.position;

        const parentPosition = getAbsolutePosition(parent);
        return {
          x: parentPosition.x + node.position.x,
          y: parentPosition.y + node.position.y,
        };
      };

      const getNodeSize = (node: Node): { width: number; height: number } => {
        const nodeType = node.type || NODE_TYPES.BasicNode;
        const fallbackSize = NODE_TYPE_SIZES[nodeType] || {
          width: 180,
          height: 48,
        };

        return {
          width:
            typeof node.style?.width === 'number'
              ? node.style.width
              : typeof node.width === 'number'
                ? node.width
                : fallbackSize.width,
          height:
            typeof node.style?.height === 'number'
              ? node.style.height
              : typeof node.height === 'number'
                ? node.height
                : fallbackSize.height,
        };
      };

      let position = snapPositionToGrid({ x: 0, y: 0 });

      if (request.directStep) {
        const data = {
          ...form.initialValues,
          stepType: request.directStep.stepType,
          name: generateUniqueStepName(request.directStep.name, nodes),
          inputMapping: request.directStep.inputMapping || [],
        } as form.SchemaType;
        let finalPosition = position;
        let parentId = request.parentId;

        if (sourceNode) {
          const sourcePosition = getAbsolutePosition(sourceNode);
          const sourceSize = getNodeSize(sourceNode);
          const absolutePosition = snapPositionToGrid({
            x: sourcePosition.x + sourceSize.width + 180,
            y: sourcePosition.y,
          });

          parentId = sourceNode.parentId || request.parentId;
          finalPosition = absolutePosition;
          if (sourceNode.parentId) {
            const parentNode = nodes.find(
              (node) => node.id === sourceNode.parentId
            );
            if (parentNode) {
              const parentPosition = getAbsolutePosition(parentNode);
              finalPosition = {
                x: absolutePosition.x - parentPosition.x,
                y: absolutePosition.y - parentPosition.y,
              };
            }
          }
        }

        setPendingNewNode({
          id: uuidv4(),
          data: data as any,
          position: finalPosition,
          parentId,
          sourceNodeId: request.sourceNodeId,
          sourceHandle: request.sourceHandle,
        });
        setPendingNodeSurface('timeline');
        setCreateStepContext(null);
        return;
      }

      if (sourceNode && targetNode) {
        const sourcePosition = getAbsolutePosition(sourceNode);
        const targetPosition = getAbsolutePosition(targetNode);
        const sourceSize = getNodeSize(sourceNode);
        const targetSize = getNodeSize(targetNode);

        position = snapPositionToGrid({
          x: (sourcePosition.x + sourceSize.width + targetPosition.x) / 2,
          y:
            targetPosition.y +
            targetSize.height / 2 -
            (NODE_TYPE_SIZES[NODE_TYPES.BasicNode]?.height || 48) / 2,
        });

        setCreateStepContext({
          position,
          insertionEdge: {
            source: sourceNode.id,
            target: targetNode.id,
            sourceHandle: request.sourceHandle || 'source',
          },
        });
        return;
      }

      if (sourceNode) {
        const sourcePosition = getAbsolutePosition(sourceNode);
        const sourceSize = getNodeSize(sourceNode);

        position = snapPositionToGrid({
          x: sourcePosition.x + sourceSize.width + 180,
          y: sourcePosition.y,
        });

        setCreateStepContext({
          position,
          connection: {
            source: sourceNode.id,
            sourceHandle: request.sourceHandle || 'source',
          },
        });
        return;
      }

      if (targetNode) {
        const targetPosition = getAbsolutePosition(targetNode);
        position = snapPositionToGrid({
          x: Math.max(0, targetPosition.x - 288),
          y: targetPosition.y,
        });

        setCreateStepContext({
          position,
          insertionEdge: {
            source: '__start_indicator__',
            target: targetNode.id,
            sourceHandle: 'source',
          },
        });
        return;
      }

      setCreateStepContext({
        position,
        parentId: request.parentId,
      });
    },
    [nodes, setPendingNewNode, closeNodeConfig]
  );

  // Handle step picker selection - stores pending node for user confirmation
  const handleStepPickerSelect = useCallback(
    (result: StepPickerResult) => {
      if (!createStepContext) return;

      const newNodeId = uuidv4();
      const uniqueName = generateUniqueStepName(result.name, nodes);
      const data = {
        ...form.initialValues,
        stepType: result.stepType,
        name: uniqueName,
        agentId: result.agentId || '',
        capabilityId: result.capabilityId || '',
      } as form.SchemaType;
      const setPendingNodeForCurrentSurface = (
        node: Exclude<Parameters<typeof setPendingNewNode>[0], null>
      ) => {
        setPendingNewNode(node);
        setPendingNodeSurface(
          createStepSurface === 'timeline' ? 'timeline' : 'dialog'
        );
      };
      const getPendingPositionFromSource = (sourceNode: Node) => {
        const type = STEP_TYPES[data.stepType] || NODE_TYPES.BasicNode;
        const newNodeSize =
          NODE_TYPE_SIZES[type] || NODE_TYPE_SIZES[NODE_TYPES.BasicNode];

        let finalPosition = createStepContext.position;
        let parentId: string | undefined;

        const getAbsolutePosition = (node: Node): { x: number; y: number } => {
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
        const sourceHandleY = sourceAbsolute.y + (sourceNode.height || 48) / 2;
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

        return { finalPosition, parentId };
      };

      if (
        createStepSurface === 'timeline' &&
        timelineAddStepRequest?.aiAgentTool &&
        timelineAddStepRequest.sourceNodeId
      ) {
        const sourceNode = nodes.find(
          (node) => node.id === timelineAddStepRequest.sourceNodeId
        );
        if (!sourceNode) return;

        const toolName = generateUniqueAiToolName(result, nodes);
        const sourceInputMapping = [
          ...((sourceNode.data?.inputMapping ||
            []) as WorkflowInputMappingItem[]),
        ];
        updateNode(sourceNode.id, {
          inputMapping: appendToolToInputMapping(sourceInputMapping, toolName),
        } as any);

        const { finalPosition, parentId } =
          getPendingPositionFromSource(sourceNode);
        setPendingNodeForCurrentSurface({
          id: newNodeId,
          data: data as any,
          position: finalPosition,
          parentId,
          sourceNodeId: sourceNode.id,
          sourceHandle: toolName,
        });

        setCreateStepContext(null);
        setSelectedEdgeId(null);
        setShowStepPicker(false);
        return;
      }

      if (
        createStepSurface === 'timeline' &&
        timelineAddStepRequest?.aiAgentMemory &&
        timelineAddStepRequest.sourceNodeId
      ) {
        const sourceNode = nodes.find(
          (node) => node.id === timelineAddStepRequest.sourceNodeId
        );
        if (!sourceNode) return;

        const memoryNodeId = newNodeId;
        const memoryStepName = generateUniqueStepName(
          `${result.name || 'Memory provider'} (memory)`,
          nodes
        );
        const memoryNodeData = {
          ...form.initialValues,
          id: memoryNodeId,
          stepType: result.stepType,
          name: memoryStepName,
          agentId: result.agentId || '',
          capabilityId: result.capabilityId || '',
          inputMapping: createMemoryProviderInputMapping(),
        } as form.SchemaType;
        const { finalPosition, parentId } =
          getPendingPositionFromSource(sourceNode);
        const createdNodeId = addNode(
          memoryNodeData as any,
          finalPosition,
          parentId
        );

        if (createdNodeId) {
          addStoreEdge(sourceNode.id, createdNodeId, 'memory');

          const sourceInputMapping = [
            ...((sourceNode.data?.inputMapping ||
              []) as WorkflowInputMappingItem[]),
          ];
          setOrAddInputMappingValue(
            sourceInputMapping,
            'memoryEnabled',
            true,
            'boolean'
          );
          setOrAddInputMappingValue(
            sourceInputMapping,
            'memoryProviderStepId',
            createdNodeId,
            'string'
          );
          setOrAddInputMappingValue(
            sourceInputMapping,
            'memoryConversationId',
            '',
            'string',
            'reference'
          );
          setOrAddInputMappingValue(
            sourceInputMapping,
            'memoryMaxMessages',
            50,
            'integer'
          );
          setOrAddInputMappingValue(
            sourceInputMapping,
            'memoryStrategy',
            'summarize',
            'string'
          );
          updateNode(sourceNode.id, {
            inputMapping: sourceInputMapping,
          } as any);
          setPendingCenterNodeId(sourceNode.id);
          setSelectedNodeId(sourceNode.id);
        }

        setCreateStepContext(null);
        setSelectedEdgeId(null);
        setShowStepPicker(false);
        setTimelineAddStepRequest(null);
        setCreateStepSurface(null);
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
            setPendingNodeForCurrentSurface({
              id: newNodeId,
              data: data as any,
              position: newPosition,
              targetNodeId: createStepContext.insertionEdge.target,
            });

            setCreateStepContext(null);
            setSelectedEdgeId(null);
            setShowStepPicker(false);
            return;
          }
        }

        // Regular insertion between nodes
        setPendingNodeForCurrentSurface({
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
        setPendingNodeForCurrentSurface({
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
      setPendingNodeForCurrentSurface({
        id: newNodeId,
        data: data as any,
        position: finalPosition,
        parentId,
      });

      setCreateStepContext(null);
      setSelectedEdgeId(null);
      setShowStepPicker(false);
    },
    [
      createStepContext,
      createStepSurface,
      timelineAddStepRequest,
      addNode,
      addStoreEdge,
      getIntersectingNodes,
      nodes,
      setPendingNewNode,
      setPendingCenterNodeId,
      setSelectedNodeId,
      updateNode,
    ]
  );

  const handleCancelCreate = useCallback(() => {
    closeCreateStep();
    setPendingNewNode(null);
    setSelectedNodeId(null);
  }, [closeCreateStep, setPendingNewNode, setSelectedNodeId]);

  // Node config dialog handlers
  const handleNodeSave = useCallback(
    (nodeId: string, data: form.SchemaType) => {
      updateNode(nodeId, data as unknown as Partial<ExecutionGraphStepDto>);
      onResetNodeChanges?.(nodeId);
      closeNodeConfig();
    },
    [updateNode, onResetNodeChanges, closeNodeConfig]
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
    (nodeId: string, data: form.SchemaType) => {
      if (!pendingNewNode || pendingNewNode.id !== nodeId) return;

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
          if (pendingNewNode.targetNodeId) {
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
      setPendingNodeSurface(null);
      setTimelineAddStepRequest(null);
      setCreateStepSurface(null);
    },
    [
      pendingNewNode,
      addNode,
      addStoreEdge,
      addStoreEdges,
      insertNodeBetween,
      setPendingCenterNodeId,
      setSelectedNodeId,
      setPendingNewNode,
    ]
  );

  const handlePendingNodeCancel = useCallback(() => {
    setPendingNewNode(null);
    setPendingNodeSurface(null);
    setTimelineAddStepRequest(null);
    setCreateStepSurface(null);
  }, [setPendingNewNode]);

  const renderTimelineInlineEditor = useCallback(
    (nodeId: string) => {
      if (!workflow || !editingNodeData || editingNodeData.id !== nodeId) {
        return null;
      }

      return (
        <TimelineNodeConfigPanel
          nodeId={editingNodeData.id}
          parentNodeId={editingNodeData.parentId}
          nodeData={editingNodeData.data}
          originalNodeData={editingNodeData.originalData}
          outputSchemaFields={workflow.outputSchemaFields}
          inputSchemaFields={workflow.inputSchemaFields}
          variables={workflow.variables}
          onSave={handleNodeSave}
          onReset={onResetNodeChanges}
          onDelete={handleNodeDelete}
          onCancel={closeNodeConfig}
        />
      );
    },
    [
      workflow,
      editingNodeData,
      handleNodeSave,
      onResetNodeChanges,
      handleNodeDelete,
      closeNodeConfig,
    ]
  );

  const renderTimelineInlineAddStep = useCallback(() => {
    if (!workflow) return null;

    if (pendingNewNode && pendingNodeSurface === 'timeline') {
      return (
        <TimelineNodeConfigPanel
          nodeId={pendingNewNode.id}
          nodeData={pendingNewNode.data as unknown as form.SchemaType}
          originalNodeData={pendingNewNode.data as unknown as form.SchemaType}
          outputSchemaFields={workflow.outputSchemaFields}
          inputSchemaFields={workflow.inputSchemaFields}
          variables={workflow.variables}
          onSave={handlePendingNodeSave}
          onCancel={handlePendingNodeCancel}
          isCreate
          parentNodeId={
            pendingNewNode.sourceNodeId || pendingNewNode.insertionEdge?.source
          }
        />
      );
    }

    if (createStepSurface !== 'timeline' || !createStepContext) return null;

    return (
      <NodeFormProvider
        isAddingBefore={nodes.length === 0}
        parentNodeId={
          createStepContext.connection?.source ||
          createStepContext.insertionEdge?.source
        }
      >
        <StepPickerPanel
          active
          onSelect={handleStepPickerSelect}
          onCancel={handleCancelCreate}
          allowFinish={!createStepContext.insertionEdge}
          mode={timelineAddStepRequest?.pickerMode || 'all'}
        />
      </NodeFormProvider>
    );
  }, [
    workflow,
    pendingNewNode,
    pendingNodeSurface,
    handlePendingNodeSave,
    handlePendingNodeCancel,
    createStepSurface,
    createStepContext,
    timelineAddStepRequest?.pickerMode,
    nodes.length,
    handleStepPickerSelect,
    handleCancelCreate,
  ]);

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
      <div className="relative h-full w-full">
        <Tabs
          value={editorView}
          onValueChange={(value) => setEditorView(value as WorkflowEditorView)}
          className="h-full"
        >
          <div className="pointer-events-none absolute left-4 top-4 z-20">
            <TabsList className="pointer-events-auto shadow-sm">
              <TabsTrigger
                value="canvas"
                className="gap-2"
                data-testid="workflow-view-canvas"
              >
                <Network className="size-4" aria-hidden="true" />
                Canvas
              </TabsTrigger>
              <TabsTrigger
                value="timeline"
                className="gap-2"
                data-testid="workflow-view-timeline"
              >
                <ListTree className="size-4" aria-hidden="true" />
                Timeline
              </TabsTrigger>
            </TabsList>
          </div>

          <TabsContent
            value="canvas"
            className={cn('m-0 h-full', editorView !== 'canvas' && 'hidden')}
          >
            <NodeConfigProvider value={nodeConfigContextValue}>
              <EdgeContextProvider
                onInsertClick={handleEdgeInsertClick}
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
                  defaultEdgeOptions={{ zIndex: 1 }}
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
                      setEditingSurface('dialog');
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
                  <Controls />
                  <Background
                    gap={SNAP_GRID_SIZE}
                    size={1}
                    color={
                      document.documentElement.classList.contains('dark')
                        ? '#262626'
                        : undefined
                    }
                  />
                </ReactFlow>
              </EdgeContextProvider>
            </NodeConfigProvider>
          </TabsContent>

          <TabsContent
            forceMount
            value="timeline"
            className={cn('m-0 h-full', editorView !== 'timeline' && 'hidden')}
          >
            <NodeConfigProvider value={timelineNodeConfigContextValue}>
              <WorkflowTimelineView
                readOnly={readOnly}
                debugInspectMode={debugInspectMode}
                onEditNode={openTimelineNodeConfig}
                editingNodeId={
                  editingSurface === 'timeline' ? editingNodeId : null
                }
                renderInlineEditor={renderTimelineInlineEditor}
                activeAddStepRequest={timelineAddStepRequest}
                renderInlineAddStep={renderTimelineInlineAddStep}
                onAddStep={handleTimelineAddStep}
              />
            </NodeConfigProvider>
          </TabsContent>
        </Tabs>
      </div>

      {/* Step Picker Modal */}
      <NodeFormProvider
        isAddingBefore={nodes.length === 0}
        parentNodeId={
          createStepContext?.connection?.source ||
          createStepContext?.insertionEdge?.source
        }
      >
        <StepPickerModal
          open={showStepPicker}
          onOpenChange={(open) => {
            if (!open) {
              handleCancelCreate();
            }
          }}
          onSelect={handleStepPickerSelect}
          allowFinish={!createStepContext?.insertionEdge}
          closeOnSelect={false}
        />
      </NodeFormProvider>

      {/* Node Config Dialog - for editing existing nodes */}
      {editingNodeData && workflow && editingSurface === 'dialog' && (
        <NodeConfigDialog
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
          variables={workflow.variables}
          onSave={handleNodeSave}
          onStagedChange={onStagedNodeChange}
          onReset={onResetNodeChanges}
          onDelete={handleNodeDelete}
        />
      )}

      {/* Node Config Dialog - for creating new nodes */}
      {pendingNewNode && workflow && pendingNodeSurface !== 'timeline' && (
        <NodeConfigDialog
          open={!!pendingNewNode}
          onOpenChange={(open) => {
            if (!open) handlePendingNodeCancel();
          }}
          nodeId={pendingNewNode.id}
          nodeData={pendingNewNode.data as unknown as form.SchemaType}
          originalNodeData={pendingNewNode.data as unknown as form.SchemaType}
          outputSchemaFields={workflow.outputSchemaFields}
          inputSchemaFields={workflow.inputSchemaFields}
          variables={workflow.variables}
          onSave={handlePendingNodeSave}
          isCreate
          parentNodeId={
            pendingNewNode.sourceNodeId || pendingNewNode.insertionEdge?.source
          }
        />
      )}
    </>
  );
}
