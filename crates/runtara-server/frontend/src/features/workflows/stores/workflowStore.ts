import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';
import {
  applyEdgeChanges,
  applyNodeChanges,
  Edge,
  EdgeChange,
  Node,
  NodeChange,
} from '@xyflow/react';
import { v4 as uuidv4 } from 'uuid';
import {
  ExecutionGraphDto,
  ExecutionGraphStepDto,
} from '@/features/workflows/types/execution-graph';
import { ValidationError } from '@/shared/hooks/api';
import {
  composeExecutionGraph,
  executionGraphToReactFlow,
  getLayoutedElements,
} from '@/features/workflows/components/WorkflowEditor/CustomNodes/utils.tsx';
import {
  snapToGrid,
  snapPositionToGrid,
} from '@/features/workflows/config/workflow-editor';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
  STEP_TYPES,
} from '@/features/workflows/config/workflow.ts';
import { validateConnection } from '@/features/workflows/utils/graph-validation';

interface HistoryEntry {
  executionGraph: ExecutionGraphDto | null;
  nodes: Node[];
  edges: Edge[];
}

interface WorkflowState {
  // Core workflow data (backend format)
  executionGraph: ExecutionGraphDto | null;

  // React Flow visual representation
  nodes: Node[];
  edges: Edge[];

  // Metadata
  isDirty: boolean;
  // True only for structural changes (not position/layout-only); used to block execution
  isStructurallyDirty: boolean;
  selectedNodes: string[];
  selectedEdges: string[];

  // Staged node IDs (nodes with unsaved changes in sidebar)
  stagedNodeIds: Set<string>;

  // Selected node ID for sidebar editing (shared across components)
  selectedNodeId: string | null;

  // Node ID that should be centered in viewport (for newly created nodes)
  pendingCenterNodeId: string | null;

  // Validation errors from last save attempt (for highlighting steps with issues)
  validationErrors: ValidationError[];

  // Step IDs that have validation errors (derived from validationErrors for quick lookup)
  stepsWithErrors: Set<string>;

  // Pending new node (for deferred creation until user confirms in dialog)
  pendingNewNode: {
    id: string;
    data: Partial<ExecutionGraphStepDto>;
    position: { x: number; y: number };
    parentId?: string;
    sourceNodeId?: string; // Node to connect FROM (new node is target)
    targetNodeId?: string; // Node to connect TO (new node is source)
    sourceHandle?: string;
    insertionEdge?: {
      source: string;
      target: string;
      sourceHandle: string;
    };
  } | null;

  // Resize tracking - stores original position/size when resize starts
  resizeTracking: Record<
    string,
    {
      originalPosition: { x: number; y: number };
      originalWidth: number;
      originalHeight: number;
      downstreamNodeOriginalPositions: Record<string, { x: number; y: number }>;
      resizeHandle?: {
        isLeft: boolean;
        isRight: boolean;
        isTop: boolean;
        isBottom: boolean;
      };
    }
  >;

  // History management
  history: HistoryEntry[];
  historyIndex: number;
  maxHistorySize: number;

  // Actions - State Management
  setExecutionGraph: (graph: ExecutionGraphDto) => void;
  resetState: () => void;
  clearDirtyFlag: () => void;
  setStagedNodeIds: (ids: Set<string>) => void;
  setSelectedNodeId: (nodeId: string | null) => void;
  setPendingCenterNodeId: (nodeId: string | null) => void;
  setPendingNewNode: (node: WorkflowState['pendingNewNode']) => void;

  // Actions - Node Config Dialog
  requestEditNodeId: string | null;
  setRequestEditNodeId: (nodeId: string | null) => void;

  // Actions - Validation Errors
  setValidationErrors: (errors: ValidationError[]) => void;
  clearValidationErrors: () => void;
  getFirstErrorStepId: () => string | null;

  // Actions - Node Operations
  addNode: (
    step: Partial<ExecutionGraphStepDto>,
    position: { x: number; y: number },
    parentId?: string
  ) => string;
  updateNode: (nodeId: string, updates: Partial<ExecutionGraphStepDto>) => void;
  removeNode: (nodeId: string) => void;

  // Actions - Note Operations
  addNote: (position: { x: number; y: number }, content?: string) => string;

  // Actions - Edge Operations
  addEdge: (from: string, to: string, label?: string) => void;
  addEdges: (
    edges: Array<{ from: string; to: string; label?: string }>
  ) => void;
  removeEdge: (from: string, to: string, label?: string) => void;
  insertNodeBetween: (
    sourceNodeId: string,
    targetNodeId: string,
    sourceHandle: string,
    newStepData: Partial<ExecutionGraphStepDto>,
    position: { x: number; y: number }
  ) => void;

  // Actions - React Flow Integration
  onNodesChange: (changes: NodeChange[]) => void;
  onEdgesChange: (changes: EdgeChange[]) => void;

  // Actions - Selection
  setSelectedNodes: (nodeIds: string[]) => void;
  setSelectedEdges: (edgeIds: string[]) => void;

  // Actions - Synchronization
  syncFromReactFlow: (nodes: Node[], edges: Edge[]) => void;
  getReactFlowElements: () => { nodes: Node[]; edges: Edge[] };

  // Actions - History
  undo: () => void;
  redo: () => void;
  saveToHistory: () => void;

  // Actions - Layout
  applyAutoLayout: () => void;

  // Computed
  canUndo: () => boolean;
  canRedo: () => boolean;
}

const initialState = {
  executionGraph: null,
  nodes: [],
  edges: [],
  isDirty: false,
  isStructurallyDirty: false,
  selectedNodes: [],
  selectedEdges: [],
  stagedNodeIds: new Set<string>(),
  selectedNodeId: null,
  pendingCenterNodeId: null,
  validationErrors: [] as ValidationError[],
  stepsWithErrors: new Set<string>(),
  pendingNewNode: null,
  requestEditNodeId: null,
  resizeTracking: {},
  history: [],
  historyIndex: -1,
  maxHistorySize: 50,
};

export const useWorkflowStore = create<WorkflowState>()(
  devtools(
    immer((set, get) => ({
      ...initialState,

      // State Management
      setExecutionGraph: (graph) =>
        set((state) => {
          const { nodes, edges } = executionGraphToReactFlow(graph);
          state.executionGraph = graph;
          state.nodes = nodes;
          state.edges = edges;
          state.isDirty = false;
          state.isStructurallyDirty = false;
          state.saveToHistory();
        }),

      resetState: () => set(() => initialState),

      clearDirtyFlag: () =>
        set((state) => {
          state.isDirty = false;
          state.isStructurallyDirty = false;
        }),

      setStagedNodeIds: (ids: Set<string>) =>
        set((state) => {
          state.stagedNodeIds = ids;
        }),

      setSelectedNodeId: (nodeId: string | null) =>
        set((state) => {
          state.selectedNodeId = nodeId;
        }),

      setPendingCenterNodeId: (nodeId: string | null) =>
        set((state) => {
          state.pendingCenterNodeId = nodeId;
        }),

      setPendingNewNode: (node) =>
        set((state) => {
          state.pendingNewNode = node;
        }),

      setRequestEditNodeId: (nodeId) =>
        set((state) => {
          state.requestEditNodeId = nodeId;
        }),

      // Validation Errors
      setValidationErrors: (errors: ValidationError[]) =>
        set((state) => {
          state.validationErrors = errors;
          // Build a Set of step IDs that have errors for quick lookup
          const stepIds = new Set<string>();
          errors.forEach((error) => {
            if (error.stepId) {
              stepIds.add(error.stepId);
            }
            // Also include related step IDs
            if (error.relatedStepIds) {
              error.relatedStepIds.forEach((id) => stepIds.add(id));
            }
          });
          state.stepsWithErrors = stepIds;
        }),

      clearValidationErrors: () =>
        set((state) => {
          state.validationErrors = [];
          state.stepsWithErrors = new Set<string>();
        }),

      getFirstErrorStepId: () => {
        const state = get();
        // Find the first validation error that has a stepId
        const firstError = state.validationErrors.find((e) => e.stepId);
        return firstError?.stepId || null;
      },

      // Node Operations
      addNode: (step, position, parentId) => {
        const nodeId = step.id || uuidv4();
        set((state) => {
          const nodeType = step.stepType
            ? STEP_TYPES[step.stepType as keyof typeof STEP_TYPES] ||
              NODE_TYPES.BasicNode
            : NODE_TYPES.BasicNode;
          const size = NODE_TYPE_SIZES[nodeType] || { width: 180, height: 48 }; // Pill fallback (15*12 x 4*12)

          // Position should already be in the correct coordinate space:
          // - Relative if parentId is provided
          // - Absolute if no parentId
          // Callers are responsible for conversion
          const snappedPosition = snapPositionToGrid(position);

          const newNode: Node = {
            id: nodeId,
            type: nodeType,
            position: snappedPosition,
            data: {
              ...step,
              id: nodeId,
              // Keep inputMapping as array for React Flow nodes
              inputMapping: step.inputMapping || [],
            },
            width: snapToGrid(size.width), // Explicit width to prevent auto-sizing
            height: snapToGrid(size.height), // Explicit height to prevent auto-sizing
            style: {
              width: snapToGrid(size.width), // Snap width to grid
              height: snapToGrid(size.height), // Snap height to grid
            },
            ...(parentId && { parentId, extent: 'parent', expandParent: true }),
          };

          state.nodes.push(newNode);

          // If node has a parent, expand parent to fit all children
          if (parentId) {
            const parentNode = state.nodes.find((n) => n.id === parentId);
            if (parentNode) {
              const childNodes = state.nodes.filter(
                (n) => n.parentId === parentId
              );

              // Calculate required parent width and height to fit all children with padding
              const padding = 48; // Padding inside container
              let maxX = 0;
              let maxY = 0;

              childNodes.forEach((child) => {
                const childRight = child.position.x + (child.width || 180);
                const childBottom = child.position.y + (child.height || 48);
                maxX = Math.max(maxX, childRight);
                maxY = Math.max(maxY, childBottom);
              });

              const requiredWidth = snapToGrid(maxX + padding);
              const requiredHeight = snapToGrid(maxY + padding);

              // Only expand, never shrink
              const currentWidth =
                typeof parentNode.style?.width === 'number'
                  ? parentNode.style.width
                  : parentNode.width || 204;
              const currentHeight =
                typeof parentNode.style?.height === 'number'
                  ? parentNode.style.height
                  : parentNode.height || 168;

              if (
                requiredWidth > currentWidth ||
                requiredHeight > currentHeight
              ) {
                parentNode.width = Math.max(requiredWidth, currentWidth);
                parentNode.height = Math.max(requiredHeight, currentHeight);
                parentNode.style = {
                  ...parentNode.style,
                  width: Math.max(requiredWidth, currentWidth),
                  height: Math.max(requiredHeight, currentHeight),
                };
              }
            }
          }

          state.isDirty = true;
          state.isStructurallyDirty = true;
          state.saveToHistory();
        });
        return nodeId;
      },

      updateNode: (nodeId, updates) =>
        set((state) => {
          const nodeIndex = state.nodes.findIndex((n) => n.id === nodeId);
          if (nodeIndex !== -1) {
            // Update node type if stepType changed
            if (
              updates.stepType &&
              updates.stepType !== state.nodes[nodeIndex].data.stepType
            ) {
              const nodeType =
                STEP_TYPES[updates.stepType as keyof typeof STEP_TYPES] ||
                NODE_TYPES.BasicNode;
              state.nodes[nodeIndex].type = nodeType;
            }

            state.nodes[nodeIndex].data = {
              ...state.nodes[nodeIndex].data,
              ...updates,
              // Keep inputMapping as array for React Flow nodes
            };
            state.isDirty = true;
            state.isStructurallyDirty = true;
            state.saveToHistory();
          }
        }),

      removeNode: (nodeId) =>
        set((state) => {
          // Collect hidden tool/memory target nodes that will become orphaned.
          // These are targets of edges from this node with non-standard sourceHandles
          // (tool names or 'memory') that have no other incoming edges.
          const outgoingHiddenTargets = new Set<string>();
          for (const edge of state.edges) {
            if (
              edge.source === nodeId &&
              edge.sourceHandle &&
              edge.sourceHandle !== 'source'
            ) {
              const targetId = edge.target;
              const hasOtherIncoming = state.edges.some(
                (e) => e.target === targetId && e.source !== nodeId
              );
              if (!hasOtherIncoming) {
                outgoingHiddenTargets.add(targetId);
              }
            }
          }

          // Remove the node, its children, and orphaned tool/memory targets
          state.nodes = state.nodes.filter(
            (n) =>
              n.id !== nodeId &&
              n.parentId !== nodeId &&
              !outgoingHiddenTargets.has(n.id)
          );

          // Remove edges connected to any of the removed nodes
          const removedNodes = new Set([nodeId, ...outgoingHiddenTargets]);
          state.edges = state.edges.filter(
            (e) => !removedNodes.has(e.source) && !removedNodes.has(e.target)
          );

          state.isDirty = true;
          state.isStructurallyDirty = true;
          state.saveToHistory();
        }),

      // Note Operations
      addNote: (position, content = 'New note...') => {
        const noteId = uuidv4();
        set((state) => {
          const size = NODE_TYPE_SIZES[NODE_TYPES.NoteNode] || {
            width: 240,
            height: 120,
          };
          const snappedPosition = snapPositionToGrid(position);

          const newNode: Node = {
            id: noteId,
            type: NODE_TYPES.NoteNode,
            position: snappedPosition,
            data: {
              id: noteId,
              content,
            },
            width: snapToGrid(size.width),
            height: snapToGrid(size.height),
            style: {
              width: snapToGrid(size.width),
              height: snapToGrid(size.height),
            },
          };

          state.nodes.push(newNode);
          state.isDirty = true;
          state.isStructurallyDirty = true;
          state.saveToHistory();
        });
        return noteId;
      },

      // Edge Operations
      addEdge: (from, to, label = 'default') =>
        set((state) => {
          // Validate the connection before adding
          const validation = validateConnection(
            state.edges,
            state.nodes,
            from,
            to
          );
          if (!validation.isValid) {
            console.error(
              `Connection validation failed: ${validation.errorMessage}`
            );
            return; // Don't add the edge if validation fails
          }

          const edgeId = `${from}-${to}-${label}`;
          const sourceHandle = label === 'default' ? 'source' : label;

          const newEdge: Edge = {
            id: edgeId,
            source: from,
            target: to,
            sourceHandle,
            targetHandle: 'target',
          };

          state.edges.push(newEdge);
          state.isDirty = true;
          state.isStructurallyDirty = true;
          state.saveToHistory();
        }),

      addEdges: (edgesToAdd) =>
        set((state) => {
          // Add multiple edges at once (useful for conditional nodes)
          // Validates only the first edge to the target, then adds all edges
          if (edgesToAdd.length === 0) return;

          const firstEdge = edgesToAdd[0];
          const validation = validateConnection(
            state.edges,
            state.nodes,
            firstEdge.from,
            firstEdge.to
          );
          if (!validation.isValid) {
            console.error(
              `Connection validation failed: ${validation.errorMessage}`
            );
            return;
          }

          for (const edge of edgesToAdd) {
            const label = edge.label || 'default';
            const edgeId = `${edge.from}-${edge.to}-${label}`;
            const sourceHandle = label === 'default' ? 'source' : label;

            const newEdge: Edge = {
              id: edgeId,
              source: edge.from,
              target: edge.to,
              sourceHandle,
              targetHandle: 'target',
            };

            state.edges.push(newEdge);
          }

          state.isDirty = true;
          state.isStructurallyDirty = true;
          state.saveToHistory();
        }),

      removeEdge: (from, to, label = 'default') =>
        set((state) => {
          const sourceHandle = label === 'default' ? 'source' : label;
          state.edges = state.edges.filter(
            (e) =>
              !(
                e.source === from &&
                e.target === to &&
                e.sourceHandle === sourceHandle
              )
          );
          state.isDirty = true;
          state.isStructurallyDirty = true;
          state.saveToHistory();
        }),

      insertNodeBetween: (
        sourceNodeId,
        targetNodeId,
        sourceHandle,
        newStepData,
        // eslint-disable-next-line @typescript-eslint/no-unused-vars
        _position
      ) =>
        set((state) => {
          // Create new node ID
          const newNodeId = newStepData.id || uuidv4();
          const nodeType = newStepData.stepType
            ? STEP_TYPES[newStepData.stepType as keyof typeof STEP_TYPES] ||
              NODE_TYPES.BasicNode
            : NODE_TYPES.BasicNode;
          const size = NODE_TYPE_SIZES[nodeType] || { width: 180, height: 48 }; // Pill fallback (15*12 x 4*12)

          // Find source and target nodes to calculate positioning
          const sourceNode = state.nodes.find((n) => n.id === sourceNodeId);
          const targetNode = state.nodes.find((n) => n.id === targetNodeId);

          if (!sourceNode || !targetNode) return;

          // Calculate source node dimensions
          const sourceNodeWidth =
            typeof sourceNode.style?.width === 'number'
              ? sourceNode.style.width
              : typeof sourceNode.width === 'number'
                ? sourceNode.width
                : 96;

          // Calculate the space needed:
          // - From source right edge to new node left edge: 108px minimum
          // - New node width: size.width
          // - From new node right edge to target left edge: 108px minimum
          const minSpaceNeeded = snapToGrid(108 + size.width + 108);

          // Calculate current distance between source and target
          const sourceRightEdge = sourceNode.position.x + sourceNodeWidth;
          const currentDistance = targetNode.position.x - sourceRightEdge;

          // Determine how much we need to shift the target and downstream nodes
          const horizontalShift = Math.max(
            0,
            snapToGrid(minSpaceNeeded - currentDistance)
          );

          // Shift target node and all downstream nodes if needed
          if (horizontalShift > 0) {
            // Find all nodes that need to be shifted
            const nodesToShift = new Set<string>();

            // Helper function to find all downstream nodes recursively
            const findDownstreamNodes = (nodeId: string) => {
              state.edges.forEach((edge) => {
                if (edge.source === nodeId) {
                  const downstreamNode = state.nodes.find(
                    (n) => n.id === edge.target
                  );
                  if (downstreamNode && !nodesToShift.has(edge.target)) {
                    nodesToShift.add(edge.target);
                    findDownstreamNodes(edge.target);
                  }
                }
              });
            };

            // Start from the target node
            nodesToShift.add(targetNodeId);
            findDownstreamNodes(targetNodeId);

            // Shift all downstream nodes to the right
            state.nodes = state.nodes.map((node) => {
              if (nodesToShift.has(node.id)) {
                return {
                  ...node,
                  position: snapPositionToGrid({
                    x: node.position.x + horizontalShift,
                    y: node.position.y,
                  }),
                };
              }
              return node;
            });
          }

          // Calculate the position for the new node AFTER shifting
          // Get the updated target position
          const targetNodeAfterShift = state.nodes.find(
            (n) => n.id === targetNodeId
          );
          const finalTargetX = targetNodeAfterShift
            ? targetNodeAfterShift.position.x
            : targetNode.position.x;

          // Position new node horizontally in the middle between source and shifted target
          const midpointX = (sourceRightEdge + finalTargetX) / 2;

          // Calculate Y position based on handle alignment (not simple averaging)
          // Get source node's type to determine its height from config
          const sourceNodeType = sourceNode.type || NODE_TYPES.BasicNode;
          const sourceNodeSize = NODE_TYPE_SIZES[sourceNodeType] || {
            width: 180,
            height: 48,
          };

          // Calculate source handle Y position (at center of source node)
          // NOTE: sourceNode.position is RELATIVE to parent if it has one
          const sourceHandleY =
            sourceNode.position.y + sourceNodeSize.height / 2;

          // Calculate new node Y so its handle aligns with source handle
          const newNodeHandleOffset = size.height / 2;
          const calculatedY = sourceHandleY - newNodeHandleOffset;

          // Calculate position for new node
          // Note: midpointX and calculatedY are already in the same coordinate space as sourceNode.position
          // If sourceNode has a parent, these are relative coordinates; otherwise they're absolute
          const newNodePosition = snapPositionToGrid({
            x: midpointX - size.width / 2,
            y: calculatedY,
          });

          // Create the new node
          // Inherit parentId from source node to maintain subgraph structure
          const newNode: Node = {
            id: newNodeId,
            type: nodeType,
            position: newNodePosition,
            data: {
              ...newStepData,
              id: newNodeId,
              inputMapping: newStepData.inputMapping || [],
            },
            style: {
              width: snapToGrid(size.width),
              height: snapToGrid(size.height),
            },
            ...(sourceNode.parentId && {
              parentId: sourceNode.parentId,
              extent: 'parent',
              expandParent: true,
            }),
          };

          // Add the new node
          state.nodes.push(newNode);

          // Remove the old edge between source and target
          // Filter by source, target, and sourceHandle since edge IDs may have additional suffixes
          state.edges = state.edges.filter(
            (e) =>
              !(
                e.source === sourceNodeId &&
                e.target === targetNodeId &&
                e.sourceHandle === sourceHandle
              )
          );

          // Create new edges based on node type
          const isConditional = nodeType === NODE_TYPES.ConditionalNode;

          // First edge: source → new node (preserve original source handle)
          // Validate before adding
          const firstValidation = validateConnection(
            state.edges,
            state.nodes,
            sourceNodeId,
            newNodeId
          );
          if (!firstValidation.isValid) {
            console.error(
              `Cannot create edge from source to new node: ${firstValidation.errorMessage}`
            );
            return;
          }

          const firstEdge: Edge = {
            id: `${sourceNodeId}-${newNodeId}-${sourceHandle}`,
            source: sourceNodeId,
            target: newNodeId,
            sourceHandle: sourceHandle === 'default' ? 'source' : sourceHandle,
            targetHandle: 'target',
          };
          state.edges.push(firstEdge);

          // Second edge(s): new node → target
          if (isConditional) {
            // Validate before adding conditional edges
            const secondValidation = validateConnection(
              [...state.edges],
              state.nodes,
              newNodeId,
              targetNodeId
            );
            if (!secondValidation.isValid) {
              console.error(
                `Cannot create edge from new node to target: ${secondValidation.errorMessage}`
              );
              // Remove the first edge we just added
              state.edges = state.edges.filter((e) => e.id !== firstEdge.id);
              return;
            }

            // Connect both true and false paths to target
            const trueEdge: Edge = {
              id: `${newNodeId}-${targetNodeId}-true`,
              source: newNodeId,
              target: targetNodeId,
              sourceHandle: 'true',
              targetHandle: 'target',
            };
            const falseEdge: Edge = {
              id: `${newNodeId}-${targetNodeId}-false`,
              source: newNodeId,
              target: targetNodeId,
              sourceHandle: 'false',
              targetHandle: 'target',
            };
            state.edges.push(trueEdge, falseEdge);
          } else {
            // Validate before adding standard edge
            const secondValidation = validateConnection(
              [...state.edges],
              state.nodes,
              newNodeId,
              targetNodeId
            );
            if (!secondValidation.isValid) {
              console.error(
                `Cannot create edge from new node to target: ${secondValidation.errorMessage}`
              );
              // Remove the first edge we just added
              state.edges = state.edges.filter((e) => e.id !== firstEdge.id);
              return;
            }

            // Standard single connection
            const secondEdge: Edge = {
              id: `${newNodeId}-${targetNodeId}-source`,
              source: newNodeId,
              target: targetNodeId,
              sourceHandle: 'source',
              targetHandle: 'target',
            };
            state.edges.push(secondEdge);
          }

          // If new node has a parent, expand parent to fit all children
          if (sourceNode.parentId) {
            const parentNode = state.nodes.find(
              (n) => n.id === sourceNode.parentId
            );
            if (parentNode) {
              const childNodes = state.nodes.filter(
                (n) => n.parentId === sourceNode.parentId
              );

              // Calculate required parent width and height to fit all children with padding
              const padding = 48;
              let maxX = 0;
              let maxY = 0;

              childNodes.forEach((child) => {
                const childRight = child.position.x + (child.width || 180);
                const childBottom = child.position.y + (child.height || 48);
                maxX = Math.max(maxX, childRight);
                maxY = Math.max(maxY, childBottom);
              });

              const requiredWidth = snapToGrid(maxX + padding);
              const requiredHeight = snapToGrid(maxY + padding);

              // Only expand, never shrink
              const currentWidth =
                typeof parentNode.style?.width === 'number'
                  ? parentNode.style.width
                  : parentNode.width || 204;
              const currentHeight =
                typeof parentNode.style?.height === 'number'
                  ? parentNode.style.height
                  : parentNode.height || 168;

              if (
                requiredWidth > currentWidth ||
                requiredHeight > currentHeight
              ) {
                parentNode.width = Math.max(requiredWidth, currentWidth);
                parentNode.height = Math.max(requiredHeight, currentHeight);
                parentNode.style = {
                  ...parentNode.style,
                  width: Math.max(requiredWidth, currentWidth),
                  height: Math.max(requiredHeight, currentHeight),
                };
              }
            }
          }

          state.isDirty = true;
          state.isStructurallyDirty = true;
          state.saveToHistory();
        }),

      // React Flow Integration
      onNodesChange: (changes) => {
        // Apply selection changes directly for visual selection (needed for NodeResizer on notes)
        const selectionChanges = changes.filter((c) => c.type === 'select');
        const otherChanges = changes.filter((c) => c.type !== 'select');

        // Handle selection changes separately - apply them but don't mark dirty
        if (selectionChanges.length > 0) {
          const currentState = get();
          const nodesWithSelection = applyNodeChanges(
            selectionChanges,
            currentState.nodes
          );
          if (nodesWithSelection !== currentState.nodes) {
            set({ nodes: nodesWithSelection });
          }
        }

        // If only selection changes, we're done
        if (otherChanges.length === 0) {
          return;
        }

        const filteredChanges = otherChanges;

        // Check if this is a position-only change (drag operation)
        // Position-only changes are the most frequent and need to be fast
        const isPositionOnly = filteredChanges.every(
          (c) => c.type === 'position'
        );

        if (isPositionOnly) {
          // For position-only changes, bypass immer by using get/set directly
          // This avoids creating new references for unchanged parts of state
          const currentState = get();
          const newNodes = applyNodeChanges(
            filteredChanges,
            currentState.nodes
          );

          // Only update if nodes actually changed
          if (newNodes !== currentState.nodes) {
            set({ nodes: newNodes, isDirty: true });
          }
          return;
        }

        // For other changes (dimensions, add, remove), use the full immer-based approach
        set((state) => {
          // Track dimension changes for ContainerNodes to handle centering and downstream shifts
          const dimensionChanges = new Map<
            string,
            {
              oldWidth: number;
              oldHeight: number;
              newWidth: number;
              newHeight: number;
              oldPosition: { x: number; y: number };
            }
          >();

          // Check if this batch has any dimension changes
          const hasDimensionChanges = filteredChanges.some(
            (c) => c.type === 'dimensions'
          );

          // Check if this batch has position changes - if so, this is a drag operation
          // and we should skip dimension change processing to prevent infinite loops
          const hasPositionChanges = filteredChanges.some(
            (c) => c.type === 'position' && c.position
          );

          // Skip dimension change processing during drag operations
          // Dimension changes during drag are React Flow's expandParent behavior,
          // not user-initiated resizing
          const shouldProcessDimensions =
            hasDimensionChanges && !hasPositionChanges;

          // If no dimension changes, clear resize tracking (resize operation finished)
          if (
            !hasDimensionChanges &&
            Object.keys(state.resizeTracking).length > 0
          ) {
            state.resizeTracking = {};
          }

          // First pass: identify nodes being resized and initialize tracking
          // Skip this during drag operations to prevent infinite loops
          filteredChanges.forEach((change) => {
            if (change.type === 'dimensions' && shouldProcessDimensions) {
              const node = state.nodes.find((n) => n.id === change.id);
              if (node && node.type === NODE_TYPES.ContainerNode) {
                // Initialize resize tracking if this is the first resize event for this node
                if (!state.resizeTracking[change.id]) {
                  const originalWidth =
                    typeof node.style?.width === 'number'
                      ? node.style.width
                      : node.width || 204;
                  const originalHeight =
                    typeof node.style?.height === 'number'
                      ? node.style.height
                      : node.height || 168;

                  // Find all downstream nodes and store their original positions
                  const downstreamNodeOriginalPositions: Record<
                    string,
                    { x: number; y: number }
                  > = {};
                  const nodesToTrack = new Set<string>();
                  const findDownstreamNodes = (currentNodeId: string) => {
                    state.edges.forEach((edge) => {
                      if (
                        edge.source === currentNodeId &&
                        !nodesToTrack.has(edge.target)
                      ) {
                        nodesToTrack.add(edge.target);
                        findDownstreamNodes(edge.target);
                      }
                    });
                  };
                  findDownstreamNodes(change.id);

                  // Store original positions of all downstream nodes
                  nodesToTrack.forEach((nodeId) => {
                    const downstreamNode = state.nodes.find(
                      (n) => n.id === nodeId
                    );
                    if (downstreamNode) {
                      downstreamNodeOriginalPositions[nodeId] = {
                        ...downstreamNode.position,
                      };
                    }
                  });

                  // Capture resize handle direction from node data
                  const resizeHandle = node.data?.__resizeHandle as
                    | {
                        isLeft: boolean;
                        isRight: boolean;
                        isTop: boolean;
                        isBottom: boolean;
                      }
                    | undefined;

                  state.resizeTracking[change.id] = {
                    originalPosition: { ...node.position },
                    originalWidth,
                    originalHeight,
                    downstreamNodeOriginalPositions,
                    resizeHandle,
                  };
                }
              }
            }
          });

          // Filter and transform changes
          const snappedChanges = filteredChanges
            .filter((change) => {
              // Block dimension changes for non-container nodes to enforce fixed sizing
              if (change.type === 'dimensions') {
                const node = state.nodes.find((n) => n.id === change.id);
                if (node && node.type !== NODE_TYPES.ContainerNode) {
                  return false; // Reject dimension changes for non-container nodes
                }

                // Skip dimension changes during drag operations to prevent infinite loops
                // During drag, React Flow fires dimension changes due to expandParent,
                // but we don't want to process them as resize operations
                if (!shouldProcessDimensions) {
                  return false;
                }

                // Only allow dimension changes during active user resize
                // __resizeHandle is set by BaseResizableNode on resize start and cleared on end
                // Without this, React Flow's re-measurement on selection can trigger
                // unwanted container resizing and downstream node shifts
                if (node && !node.data?.__resizeHandle) {
                  return false;
                }

                // Track ContainerNode dimension changes for downstream node shifting
                if (
                  node &&
                  node.type === NODE_TYPES.ContainerNode &&
                  change.dimensions
                ) {
                  // Use tracked original values, or current values if tracking not initialized
                  const tracking = state.resizeTracking[change.id];
                  const oldWidth =
                    tracking?.originalWidth ||
                    (typeof node.style?.width === 'number'
                      ? node.style.width
                      : node.width || 204);
                  const oldHeight =
                    tracking?.originalHeight ||
                    (typeof node.style?.height === 'number'
                      ? node.style.height
                      : node.height || 168);
                  const oldPosition = tracking?.originalPosition || {
                    ...node.position,
                  };

                  dimensionChanges.set(change.id, {
                    oldWidth,
                    oldHeight,
                    newWidth: change.dimensions.width,
                    newHeight: change.dimensions.height,
                    oldPosition,
                  });
                }
              }
              return true;
            })
            .map((change) => {
              // Snap container dimension changes to grid
              if (change.type === 'dimensions' && change.dimensions) {
                return {
                  ...change,
                  dimensions: {
                    width: snapToGrid(change.dimensions.width),
                    height: snapToGrid(change.dimensions.height),
                  },
                };
              }
              // Snap position changes to grid
              if (change.type === 'position' && change.position) {
                return {
                  ...change,
                  position: snapPositionToGrid(change.position),
                };
              }
              return change;
            });

          // Apply the changes first
          state.nodes = applyNodeChanges(snappedChanges, state.nodes);

          // Handle ContainerNode resizing: edge-based resizing based on handle direction
          dimensionChanges.forEach((changeInfo, nodeId) => {
            const { oldWidth, newWidth } = changeInfo;
            const widthDelta = newWidth - oldWidth;

            if (widthDelta !== 0) {
              const tracking = state.resizeTracking[nodeId];
              const handle = tracking?.resizeHandle;

              // React Flow's NodeResizer already handles position correctly:
              // - Right edge drag: keeps left edge fixed (position.x unchanged)
              // - Left edge drag: keeps right edge fixed (adjusts position.x)
              // We only need to shift downstream nodes based on how the right edge moved

              // Calculate how much the right edge moved in absolute coordinates
              let rightEdgeShift = 0;

              if (handle) {
                if (handle.isRight && !handle.isLeft) {
                  // Right edge drag: right edge moved by full widthDelta
                  rightEdgeShift = widthDelta;
                } else if (handle.isLeft && !handle.isRight) {
                  // Left edge drag: right edge stayed in place (growth went left)
                  rightEdgeShift = 0;
                } else {
                  // Corner drag or both edges: shouldn't happen with standard handles
                  rightEdgeShift = widthDelta / 2;
                }
              }

              // Shift downstream nodes based on right edge movement
              if (
                rightEdgeShift !== 0 &&
                tracking?.downstreamNodeOriginalPositions
              ) {
                state.nodes.forEach((node) => {
                  const originalPos =
                    tracking.downstreamNodeOriginalPositions[node.id];
                  if (originalPos) {
                    node.position = {
                      x: originalPos.x + rightEdgeShift,
                      y: node.position.y, // Keep Y unchanged
                    };
                  }
                });
              }
            }
          });

          // Only mark as dirty for dimension, add, remove changes
          // Position-only changes are handled in the fast path above
          // Use snappedChanges (not filteredChanges) so that dimension events
          // already filtered out for non-container nodes don't falsely set dirty
          const hasUserChanges = snappedChanges.some(
            (change) =>
              change.type === 'dimensions' ||
              change.type === 'remove' ||
              change.type === 'add'
          );
          if (hasUserChanges) {
            state.isDirty = true;
            state.isStructurallyDirty = true;
          }
        });
      },

      onEdgesChange: (changes) =>
        set((state) => {
          // Filter out selection changes to prevent unnecessary re-renders
          const filteredChanges = changes.filter(
            (change) => change.type !== 'select'
          );

          // If only selection changes, skip the update entirely
          if (filteredChanges.length === 0) {
            return;
          }

          state.edges = applyEdgeChanges(filteredChanges, state.edges);
          // Only mark as dirty for actual edge changes
          const hasUserChanges = filteredChanges.some(
            (change) => change.type === 'remove' || change.type === 'add'
          );
          if (hasUserChanges) {
            state.isDirty = true;
            state.isStructurallyDirty = true;
          }
        }),

      // Selection
      setSelectedNodes: (nodeIds) =>
        set((state) => {
          state.selectedNodes = nodeIds;
        }),

      setSelectedEdges: (edgeIds) =>
        set((state) => {
          state.selectedEdges = edgeIds;
        }),

      // Synchronization
      syncFromReactFlow: (nodes, edges) =>
        set((state) => {
          state.nodes = nodes;
          state.edges = edges;
          state.executionGraph = composeExecutionGraph(nodes, edges);
          // Don't mark as dirty when syncing from external data (e.g., initial load)
          state.isDirty = false;
          state.isStructurallyDirty = false;
        }),

      getReactFlowElements: () => {
        const state = get();
        return { nodes: state.nodes, edges: state.edges };
      },

      // History Management
      saveToHistory: () =>
        set((state) => {
          // Remove any future history if we're not at the latest
          if (state.historyIndex < state.history.length - 1) {
            state.history = state.history.slice(0, state.historyIndex + 1);
          }

          // Add current state to history
          const historyEntry: HistoryEntry = {
            executionGraph: state.executionGraph,
            nodes: [...state.nodes],
            edges: [...state.edges],
          };

          state.history.push(historyEntry);

          // Limit history size
          if (state.history.length > state.maxHistorySize) {
            state.history = state.history.slice(-state.maxHistorySize);
          }

          state.historyIndex = state.history.length - 1;
        }),

      undo: () =>
        set((state) => {
          if (state.historyIndex > 0) {
            state.historyIndex--;
            const historyEntry = state.history[state.historyIndex];
            state.executionGraph = historyEntry.executionGraph;
            state.nodes = [...historyEntry.nodes];
            state.edges = [...historyEntry.edges];
          }
        }),

      redo: () =>
        set((state) => {
          if (state.historyIndex < state.history.length - 1) {
            state.historyIndex++;
            const historyEntry = state.history[state.historyIndex];
            state.executionGraph = historyEntry.executionGraph;
            state.nodes = [...historyEntry.nodes];
            state.edges = [...historyEntry.edges];
          }
        }),

      // Layout
      applyAutoLayout: () =>
        set((state) => {
          // Identify hidden AI Agent tool/memory nodes and edges
          // These are rendered inline in the AI Agent card, not on the canvas
          const hiddenNodeIds = new Set<string>();
          const hiddenEdgeIds = new Set<string>();
          const aiAgentNodes = state.nodes.filter(
            (n) => n.type === NODE_TYPES.AiAgentNode
          );
          for (const agentNode of aiAgentNodes) {
            for (const edge of state.edges) {
              if (
                edge.source === agentNode.id &&
                edge.sourceHandle &&
                edge.sourceHandle !== 'source'
              ) {
                hiddenEdgeIds.add(edge.id);
                hiddenNodeIds.add(edge.target);
              }
            }
          }

          // Layout only visible nodes and edges
          const visibleNodes = state.nodes.filter(
            (n) => !hiddenNodeIds.has(n.id)
          );
          const visibleEdges = state.edges.filter(
            (e) => !hiddenEdgeIds.has(e.id)
          );

          const { nodes: layoutedNodes } = getLayoutedElements(
            visibleNodes,
            visibleEdges
          );

          // Merge layouted visible nodes back with hidden nodes (unchanged)
          const layoutedMap = new Map(layoutedNodes.map((n) => [n.id, n]));
          state.nodes = state.nodes.map((n) => layoutedMap.get(n.id) ?? n);
          state.isDirty = true;
          state.saveToHistory();
        }),

      canUndo: () => {
        const state = get();
        return state.historyIndex > 0;
      },

      canRedo: () => {
        const state = get();
        return state.historyIndex < state.history.length - 1;
      },
    })),
    {
      name: 'workflow-store',
    }
  )
);
