import {
  MouseEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  Background,
  Connection,
  Controls,
  Edge,
  EdgeTypes,
  Node,
  NodeTypes,
  ReactFlow,
  useReactFlow,
  OnConnectEnd,
} from '@xyflow/react';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { toast } from '@/shared/hooks/useToast';
import { Tabs, TabsContent } from '@/shared/components/ui/tabs';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
  STEP_TYPES,
} from '@/features/workflows/config/workflow.ts';
import {
  SNAP_GRID_SIZE,
  snapPositionToGrid,
} from '@/features/workflows/config/workflow-editor.ts';
import {
  validateConnection,
  getDownstreamNodes,
} from '@/features/workflows/utils/graph-validation';
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
  sortNodes,
} from './CustomNodes/utils.tsx';
import { NodeConfigDialog } from './NodeConfigDialog';
import { NodeFormProvider } from './NodeForm/NodeFormProvider';
import { StepPickerModal, StepPickerResult } from './NodeForm/StepPickerModal';
import { NodeConfigProvider } from './NodeConfigContext';
import {
  type TimelineAddStepRequest,
  WorkflowTimelineView,
} from './TimelineView';

// Re-export CreateStepContext type for external use
export interface CreateStepContext {
  position: { x: number; y: number };
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

export function WorkflowEditor({
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

  // Track selected edge ID for showing "+" button
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);

  // Dialog states
  const [editingNodeId, setEditingNodeId] = useState<string | null>(null);
  const [showStepPicker, setShowStepPicker] = useState(false);

  // Callback for child node components to open config dialogs (including hidden nodes)
  const openNodeConfig = useCallback(
    (nodeId: string) => {
      if (workflow && !readOnly) {
        setEditingNodeId(nodeId);
      }
    },
    [workflow, readOnly]
  );
  const nodeConfigContextValue = useMemo(
    () => ({ openNodeConfig }),
    [openNodeConfig]
  );

  // Pending new node state from store (for deferred creation until user confirms)
  const pendingNewNode = useWorkflowStore((state) => state.pendingNewNode);
  const setPendingNewNode = useWorkflowStore(
    (state) => state.setPendingNewNode
  );

  const ref = useRef(null);

  // Shift-drag context for moving connected nodes together
  const shiftDragRef = useRef<{
    isActive: boolean;
    draggedNodeId: string;
    connectedNodeIds: Set<string>;
    initialPositions: Map<string, { x: number; y: number }>;
  } | null>(null);

  // Track Alt key state globally for connected node movement
  const modifierKeyRef = useRef(false);
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.altKey) {
        modifierKeyRef.current = true;
      }
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      if (!e.altKey) {
        modifierKeyRef.current = false;
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
    };
  }, []);

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

  const entryPointNode = entryPointId
    ? nodes.find((n) => n.id === entryPointId)
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

  const nodesWithStartIndicator = useMemo(() => {
    // Filter out hidden tool/memory nodes
    const visibleNodes =
      hiddenNodeIds.size > 0
        ? nodes.filter((n) => !hiddenNodeIds.has(n.id))
        : nodes;
    if (!startIndicatorNode) return visibleNodes;
    return [startIndicatorNode, ...visibleNodes];
  }, [nodes, startIndicatorNode, hiddenNodeIds]);

  const edgesWithStartIndicator = useMemo(() => {
    // Create a set of node IDs that are inside a container (have parentId)
    const nodesInsideContainer = new Set(
      nodes.filter((n) => n.parentId).map((n) => n.id)
    );

    // Filter out hidden tool/memory edges and apply selection
    const edgesWithSelection = edges
      .filter((edge) => !hiddenEdgeIds.has(edge.id))
      .map((edge) => {
        // Check if this edge connects nodes inside a container
        const isInsideContainer =
          nodesInsideContainer.has(edge.source) ||
          nodesInsideContainer.has(edge.target);

        return {
          ...edge,
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
  }, [edges, nodes, startIndicatorEdge, selectedEdgeId, hiddenEdgeIds]);

  // Initialize store with provided nodes and edges
  const lastSyncedDataRef = useRef<string>('');
  const hasInitializedViewRef = useRef(false);

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
      // Reset view initialization flag when data changes (e.g., version switch)
      hasInitializedViewRef.current = false;

      if (initialNodes.length > 0) {
        setTimeout(() => {
          fitView({ duration: 200 });
          hasInitializedViewRef.current = true;
        }, 100);
      }
    }
  }, [initialNodes, initialEdges, syncFromReactFlow, fitView]);

  // Center viewport on virtual start step for new/empty workflows
  useEffect(() => {
    if (!hasInitializedViewRef.current && initialNodes.length === 0) {
      const startIndicatorSize = NODE_TYPE_SIZES[NODE_TYPES.StartIndicatorNode];
      const startCenterX = startIndicatorSize.width / 2;
      const startCenterY = startIndicatorSize.height / 2;

      setTimeout(() => {
        setCenter(startCenterX, startCenterY, { zoom: 1, duration: 200 });
        hasInitializedViewRef.current = true;
      }, 100);
    }
  }, [initialNodes.length, setCenter]);

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
          connection.sourceHandle || 'default'
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

  const handleTimelineAddStep = useCallback(
    (request: TimelineAddStepRequest) => {
      const sourceNode = request.sourceNodeId
        ? nodes.find((node) => node.id === request.sourceNodeId)
        : undefined;
      const targetNode = request.targetNodeId
        ? nodes.find((node) => node.id === request.targetNodeId)
        : undefined;

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
      });
    },
    [nodes]
  );

  // Handle step picker selection - stores pending node for user confirmation
  const handleStepPickerSelect = useCallback(
    (result: StepPickerResult) => {
      if (!createStepContext) return;

      const newNodeId = crypto.randomUUID();
      const uniqueName = generateUniqueStepName(result.name, nodes);
      const data = {
        ...form.initialValues,
        stepType: result.stepType,
        name: uniqueName,
        agentId: result.agentId || '',
        capabilityId: result.capabilityId || '',
      } as form.SchemaType;

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

      let parentId: string | undefined;
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

      if (groupNode) {
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
    setCreateStepContext(null);
    setSelectedEdgeId(null);
    setShowStepPicker(false);
    setSelectedNodeId(null);
  }, [setSelectedNodeId]);

  // Node config dialog handlers
  const handleNodeSave = useCallback(
    (nodeId: string, data: form.SchemaType) => {
      updateNode(nodeId, data as unknown as Partial<ExecutionGraphStepDto>);
      onResetNodeChanges?.(nodeId);
      setEditingNodeId(null);
    },
    [updateNode, onResetNodeChanges]
  );

  const handleNodeDelete = useCallback(
    (nodeId: string) => {
      removeNode(nodeId);
      setSelectedNodeId(null);
      setEditingNodeId(null);
    },
    [removeNode, setSelectedNodeId]
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
  }, [setPendingNewNode]);

  // Node drag handlers
  const onNodeDragStart = useCallback(
    (_event: MouseEvent, node: Node) => {
      const connectedNodeIds = getDownstreamNodes(edges, node.id);

      const initialPositions = new Map<string, { x: number; y: number }>();
      initialPositions.set(node.id, { ...node.position });

      nodes.forEach((n) => {
        if (connectedNodeIds.has(n.id)) {
          initialPositions.set(n.id, { ...n.position });
        }
      });

      shiftDragRef.current = {
        isActive: false,
        draggedNodeId: node.id,
        connectedNodeIds,
        initialPositions,
      };
    },
    [edges, nodes]
  );

  const handleNodesChange = useCallback(
    (changes: import('@xyflow/react').NodeChange[]) => {
      const context = shiftDragRef.current;

      if (
        context &&
        modifierKeyRef.current &&
        context.connectedNodeIds.size > 0
      ) {
        const draggedNodeChange = changes.find(
          (c) =>
            c.type === 'position' &&
            c.id === context.draggedNodeId &&
            (c as any).position
        ) as
          | {
              type: 'position';
              id: string;
              position?: { x: number; y: number };
            }
          | undefined;

        if (draggedNodeChange?.position) {
          const initialPos = context.initialPositions.get(
            context.draggedNodeId
          );
          if (initialPos) {
            const deltaX = draggedNodeChange.position.x - initialPos.x;
            const deltaY = draggedNodeChange.position.y - initialPos.y;

            const connectedChanges = Array.from(context.connectedNodeIds)
              .map((nodeId) => {
                const nodeInitialPos = context.initialPositions.get(nodeId);
                if (nodeInitialPos) {
                  return {
                    type: 'position' as const,
                    id: nodeId,
                    position: {
                      x: nodeInitialPos.x + deltaX,
                      y: nodeInitialPos.y + deltaY,
                    },
                  };
                }
                return null;
              })
              .filter(Boolean);

            onNodesChange([
              ...changes,
              ...connectedChanges,
            ] as import('@xyflow/react').NodeChange[]);
            return;
          }
        }
      }

      onNodesChange(changes);
    },
    [onNodesChange]
  );

  const onNodeDrag = useCallback(
    (event: MouseEvent, node: Node) => {
      modifierKeyRef.current = event.altKey;

      if (node.type === NODE_TYPES.ContainerNode) return;
      if (!node.parentId && node.type !== NODE_TYPES.ContainerNode) return;

      const intersections = getIntersectingNodes(node).filter(
        (n) => n.type === NODE_TYPES.ContainerNode
      );

      const groupNode = intersections[intersections.length - 1];
      const shouldHighlight =
        intersections.length && node.parentId !== groupNode?.id;

      const changes = nodes
        .filter((n) => n.type === NODE_TYPES.ContainerNode)
        .map((n) => ({
          id: n.id,
          type: 'className' as const,
          className: shouldHighlight ? 'active' : '',
        }));

      if (changes.length > 0) {
        onNodesChange(changes as any);
      }
    },
    [getIntersectingNodes, nodes, onNodesChange]
  );

  const onNodeDragStop = useCallback(
    (_: MouseEvent, node: Node) => {
      shiftDragRef.current = null;

      if (node.type === NODE_TYPES.ContainerNode && !node.parentId) return;

      const intersections = getIntersectingNodes(node).filter((n) => {
        if (n.type === NODE_TYPES.ContainerNode && n.id === node.parentId) {
          return n;
        } else if (node.type !== NODE_TYPES.ContainerNode) {
          return n;
        }
      });

      const groupNode = intersections[intersections.length - 1];

      if (intersections.length && node.parentId !== groupNode.id) {
        const nextNodes: Node[] = nodes
          .map((n) => {
            if (n.id === node.id) {
              const position = getNodePositionInsideParent(n, groupNode) ?? {
                x: 0,
                y: 0,
              };

              return {
                ...n,
                position,
                parentId: groupNode.id,
                expandParent: true,
                extent: 'parent',
              } as Node;
            }

            return n;
          })
          .sort(sortNodes);

        syncFromReactFlow(nextNodes, edges);
      }
    },
    [getIntersectingNodes, nodes, edges, syncFromReactFlow]
  );

  return (
    <>
      <div className="relative h-full w-full">
        <Tabs value="timeline" className="h-full">
          <TabsContent value="canvas" className="m-0 h-full">
            <NodeConfigProvider value={nodeConfigContextValue}>
              <EdgeContextProvider
                onInsertClick={handleEdgeInsertClick}
                allEdges={edges}
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
                  onNodeDragStart={readOnly ? undefined : onNodeDragStart}
                  onNodeDrag={readOnly ? undefined : onNodeDrag}
                  onNodeDragStop={readOnly ? undefined : onNodeDragStop}
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
                  nodesDraggable={!readOnly}
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

          <TabsContent forceMount value="timeline" className="m-0 h-full">
            <NodeConfigProvider value={nodeConfigContextValue}>
              <WorkflowTimelineView
                readOnly={readOnly}
                debugInspectMode={debugInspectMode}
                onEditNode={openNodeConfig}
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
        />
      </NodeFormProvider>

      {/* Node Config Dialog - for editing existing nodes */}
      {editingNodeData && workflow && (
        <NodeConfigDialog
          open={!!editingNodeId}
          onOpenChange={(open) => {
            if (!open) setEditingNodeId(null);
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
      {pendingNewNode && workflow && (
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
