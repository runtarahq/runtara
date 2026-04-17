import { memo, useCallback, useMemo, useState } from 'react';
import { Node, NodeProps, Position, useNodeConnections } from '@xyflow/react';
import { v4 as uuidv4 } from 'uuid';
import { Button } from '@/shared/components/ui/button.tsx';
import { BaseNode } from '../BaseNode.tsx';
import { ButtonHandle } from '../ButtonHandle.tsx';
import { Plus, AlertTriangle } from 'lucide-react';
import * as form from '@/features/scenarios/components/WorkflowEditor/NodeForm/NodeFormItem.tsx';
import { BASE_WIDTH } from './utils.tsx';
import { useWorkflowStore } from '@/features/scenarios/stores/workflowStore.ts';
import {
  useValidationStore,
  getFirstValidationMessage,
} from '@/features/scenarios/stores/validationStore';
import {
  STEP_TYPES,
  NODE_TYPES,
  NODE_TYPE_SIZES,
} from '@/features/scenarios/config/workflow.ts';

const BASIC_NODE_HEIGHT = NODE_TYPE_SIZES[NODE_TYPES.BasicNode].height;
import { useExecutionStore } from '@/features/scenarios/stores/executionStore';
import { NodeFormProvider } from '../NodeForm/NodeFormProvider';
import { StepPickerModal, StepPickerResult } from '../NodeForm/StepPickerModal';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getAgents, ExtendedAgent } from '@/features/scenarios/queries';
import { canStepHaveErrorHandler } from '@/features/scenarios/utils/step-error-support';

// Note: Node editing is now handled by the sidebar (EditorSidebar) via double-click on ReactFlow.
// The dialogs below are only for creating new nodes via the + button handles.

type BasicNodeProps = Node<form.SchemaType>;

function BasicNodeComponent({
  id,
  data,
  isConnectable,
  selected,
}: NodeProps<BasicNodeProps>) {
  const [showStepPicker, setShowStepPicker] = useState<boolean>(false);
  const [isAddingBefore, setIsAddingBefore] = useState<boolean>(false);

  // Get actions using individual selectors to prevent re-renders on unrelated state changes
  const setPendingNewNode = useWorkflowStore(
    (state) => state.setPendingNewNode
  );

  // Get execution status for this node
  const executionStatus = useExecutionStore((state) =>
    state.nodeExecutionStatus.get(id)
  );

  // Check if scenario is executing (read-only mode)
  const isExecuting = useExecutionStore((state) => !!state.executingInstanceId);

  // Check if this node has unsaved changes
  const hasUnsavedChanges = useWorkflowStore((state) =>
    state.stagedNodeIds.has(id)
  );

  // Check if this node has validation errors (from workflowStore or validationStore)
  const hasValidationError = useWorkflowStore((state) =>
    state.stepsWithErrors.has(id)
  );
  const hasValidationErrorFromPanel = useValidationStore((state) =>
    state.stepsWithErrors.has(id)
  );

  // Check if this node has validation warnings
  const hasValidationWarning = useValidationStore((state) =>
    state.stepsWithWarnings.has(id)
  );

  // Get the first validation error message for this node
  const validationMessage = useValidationStore((state) =>
    getFirstValidationMessage(state, id)
  );

  // Combine error states from both stores
  const showValidationError = hasValidationError || hasValidationErrorFromPanel;

  // Fetch agents for agent name lookup
  const agentsQuery = useCustomQuery({
    queryKey: queryKeys.agents.all,
    queryFn: getAgents,
    placeholderData: { agents: [] },
  });

  // Lookup agent name for Agent steps
  const agentName = useMemo(() => {
    if (data.stepType !== 'Agent' || !data.agentId) return undefined;
    const agents =
      (agentsQuery.data as { agents: ExtendedAgent[] })?.agents || [];
    const agentIdLower = data.agentId.toLowerCase();
    const agent = agents.find((a) => a.id.toLowerCase() === agentIdLower);
    return agent?.name;
  }, [data.stepType, data.agentId, agentsQuery.data]);

  // Toggle breakpoint on this step — read current value from store at click time to avoid stale closures
  const handleToggleBreakpoint = useCallback(() => {
    const { nodes, updateNode } = useWorkflowStore.getState();
    const node = nodes.find((n) => n.id === id);
    const current = !!(node?.data as any)?.breakpoint;
    updateNode(id, { breakpoint: current ? undefined : true } as any);
  }, [id]);

  const sourceConnections = useNodeConnections({
    handleType: 'source',
    handleId: 'source',
  });
  const targetConnections = useNodeConnections({ handleType: 'target' });
  const errorConnections = useNodeConnections({
    handleType: 'source',
    handleId: 'onError',
  });

  const handleCreate = useCallback(
    (result: StepPickerResult) => {
      // Get current position from store to avoid dependency on position props
      const { nodes } = useWorkflowStore.getState();
      const currentNode = nodes.find((n) => n.id === id) as Node & {
        positionAbsolute?: { x: number; y: number };
      };
      if (!currentNode) return;

      const currentPosAbsX =
        currentNode.positionAbsolute?.x ?? currentNode.position.x;
      const currentPosAbsY =
        currentNode.positionAbsolute?.y ?? currentNode.position.y;
      const currentWidth =
        (currentNode.style?.width as number) ||
        (currentNode.width as number) ||
        BASE_WIDTH;
      const currentParentId = currentNode.parentId;

      // Build node data from picker result
      const nodeData = {
        ...form.initialValues,
        stepType: result.stepType,
        name: result.name,
        agentId: result.agentId || '',
        capabilityId: result.capabilityId || '',
      } as form.SchemaType;

      // Determine the type and size of the new node
      const type = STEP_TYPES[result.stepType] || NODE_TYPES.BasicNode;
      const newNodeSize = NODE_TYPE_SIZES[type];

      // Get current node's actual height from config (don't trust height prop)
      const currentNodeHeight = NODE_TYPE_SIZES[NODE_TYPES.BasicNode].height; // 48px

      // Calculate handle positions to align them (handles are at node center)
      const currentHandleY = currentPosAbsY + currentNodeHeight / 2;
      const newNodeHandleOffset = newNodeSize.height / 2;

      // Calculate new node's absolute position
      const absolutePosition = {
        x: currentPosAbsX + currentWidth + 108,
        y: currentHandleY - newNodeHandleOffset,
      };

      // If we have a parent, we need to convert to relative position
      let finalPosition = absolutePosition;

      if (currentParentId) {
        const parentNode = nodes.find((n) => n.id === currentParentId);

        if (parentNode) {
          const extendedParentNode = parentNode as Node & {
            positionAbsolute?: { x: number; y: number };
          };
          const parentAbsoluteX =
            extendedParentNode.positionAbsolute?.x ?? parentNode.position.x;
          const parentAbsoluteY =
            extendedParentNode.positionAbsolute?.y ?? parentNode.position.y;

          finalPosition = {
            x: absolutePosition.x - parentAbsoluteX,
            y: absolutePosition.y - parentAbsoluteY,
          };
        }
      }

      // Set pending new node - will be created when user confirms in dialog
      const newNodeId = uuidv4();
      setPendingNewNode({
        id: newNodeId,
        data: nodeData as any,
        position: finalPosition,
        parentId: currentParentId,
        sourceNodeId: id, // Connect FROM current node TO new node
      });

      setShowStepPicker(false);
      setIsAddingBefore(false);
    },
    [setPendingNewNode, id]
  );

  const handleOpenCreate = (e: React.MouseEvent) => {
    e.stopPropagation(); // Prevent node selection when clicking "+"
    setIsAddingBefore(false);
    setShowStepPicker(true);
  };

  const handleOpenCreateError = (e: React.MouseEvent) => {
    e.stopPropagation(); // Prevent node selection when clicking "+"
    // Create Error step directly so users land on error configuration dialog
    const { nodes } = useWorkflowStore.getState();
    const currentNode = nodes.find((n) => n.id === id) as Node & {
      positionAbsolute?: { x: number; y: number };
    };
    if (!currentNode) return;

    const currentPosAbsX =
      currentNode.positionAbsolute?.x ?? currentNode.position.x;
    const currentPosAbsY =
      currentNode.positionAbsolute?.y ?? currentNode.position.y;
    const currentWidth =
      (currentNode.style?.width as number) ||
      (currentNode.width as number) ||
      BASE_WIDTH;
    const currentParentId = currentNode.parentId;

    const nodeData = {
      ...form.initialValues,
      stepType: 'Error',
      name: 'Error handler',
    } as form.SchemaType;

    const newNodeSize = NODE_TYPE_SIZES[NODE_TYPES.BasicNode];
    const currentNodeHeight = NODE_TYPE_SIZES[NODE_TYPES.BasicNode].height;

    const absolutePosition = {
      x: currentPosAbsX + currentWidth / 2 - newNodeSize.width / 2 + 50,
      y: currentPosAbsY + currentNodeHeight + 80,
    };

    let finalPosition = absolutePosition;
    if (currentParentId) {
      const parentNode = nodes.find((n) => n.id === currentParentId);
      if (parentNode) {
        const extendedParentNode = parentNode as Node & {
          positionAbsolute?: { x: number; y: number };
        };
        const parentAbsoluteX =
          extendedParentNode.positionAbsolute?.x ?? parentNode.position.x;
        const parentAbsoluteY =
          extendedParentNode.positionAbsolute?.y ?? parentNode.position.y;

        finalPosition = {
          x: absolutePosition.x - parentAbsoluteX,
          y: absolutePosition.y - parentAbsoluteY,
        };
      }
    }

    setPendingNewNode({
      id: uuidv4(),
      data: nodeData as any,
      position: finalPosition,
      parentId: currentParentId,
      sourceNodeId: id,
      sourceHandle: 'onError',
    });
  };

  const handleCreateBefore = useCallback(
    (result: StepPickerResult) => {
      // Get current position from store to avoid dependency on position props
      const { nodes, edges } = useWorkflowStore.getState();
      const currentNode = nodes.find((n) => n.id === id) as Node & {
        positionAbsolute?: { x: number; y: number };
      };
      if (!currentNode) return;

      const currentPosAbsX =
        currentNode.positionAbsolute?.x ?? currentNode.position.x;
      const currentPosAbsY =
        currentNode.positionAbsolute?.y ?? currentNode.position.y;
      const currentWidth =
        (currentNode.style?.width as number) ||
        (currentNode.width as number) ||
        BASE_WIDTH;

      // Build node data from picker result
      const nodeData = {
        ...form.initialValues,
        stepType: result.stepType,
        name: result.name,
        agentId: result.agentId || '',
        capabilityId: result.capabilityId || '',
      } as form.SchemaType;

      // Determine the type and size of the new node
      const type = STEP_TYPES[result.stepType] || NODE_TYPES.BasicNode;
      const newNodeSize = NODE_TYPE_SIZES[type];

      // Get current node's actual height from config
      const currentNodeHeight = NODE_TYPE_SIZES[NODE_TYPES.BasicNode].height; // 48px

      // Calculate handle positions to align them (handles are at node center)
      const currentHandleY = currentPosAbsY + currentNodeHeight / 2;
      const newNodeHandleOffset = newNodeSize.height / 2;

      const position = {
        x: currentPosAbsX - currentWidth - 108, // Position to the left with spacing
        y: currentHandleY - newNodeHandleOffset, // Keep precise for handle alignment
      };

      // Find the incoming edge to determine insertion context
      const incomingEdge = edges.find((e) => e.target === id);

      const newNodeId = uuidv4();

      if (incomingEdge) {
        // Insert between parent and current node
        setPendingNewNode({
          id: newNodeId,
          data: nodeData as any,
          position,
          insertionEdge: {
            source: incomingEdge.source,
            target: id,
            sourceHandle: incomingEdge.sourceHandle || 'source',
          },
        });
      } else {
        // No incoming edge - just connect new node to current node
        setPendingNewNode({
          id: newNodeId,
          data: nodeData as any,
          position,
          targetNodeId: id, // Connect FROM new node TO current node
        });
      }

      setShowStepPicker(false);
      setIsAddingBefore(false);
    },
    [setPendingNewNode, id]
  );

  const handleOpenCreateBefore = (e: React.MouseEvent) => {
    e.stopPropagation(); // Prevent node selection when clicking "+"
    setIsAddingBefore(true);
    setShowStepPicker(true);
  };

  const isFinishStep = data.stepType === 'Finish';
  const isStartStep = data.stepType === 'Start';

  // Determine if this step can have error handlers based on:
  // 1. Step type (evaluation steps like Conditional/Switch cannot fail)
  // 2. For Agent steps: whether the capability has knownErrors defined
  const canFail = useMemo(() => {
    const agents =
      (agentsQuery.data as { agents: ExtendedAgent[] })?.agents || [];
    return canStepHaveErrorHandler(
      data.stepType,
      data.agentId,
      data.capabilityId,
      agents
    );
  }, [data.stepType, data.agentId, data.capabilityId, agentsQuery.data]);

  return (
    <>
      <BaseNode
        id={id}
        name={data.name}
        stepType={data.stepType}
        agentId={data.agentId}
        agentName={agentName}
        inputMapping={data.inputMapping}
        selected={selected}
        executionStatus={executionStatus}
        hasUnsavedChanges={hasUnsavedChanges}
        hasValidationError={showValidationError}
        hasValidationWarning={hasValidationWarning}
        validationMessage={showValidationError ? validationMessage : null}
        isExecutionReadOnly={isExecuting}
        breakpoint={!!(data as any).breakpoint}
        onToggleBreakpoint={isStartStep ? undefined : handleToggleBreakpoint}
        style={{ height: `${BASIC_NODE_HEIGHT}px` }}
      >
        {/* Only show source handle for non-Finish steps */}
        {!isFinishStep && (
          <ButtonHandle
            showButton={Boolean(!sourceConnections.length && !isExecuting)}
            id="source"
            type="source"
            position={Position.Right}
            isConnectable={isConnectable && !isExecuting}
            className="!bg-muted-foreground/40"
          >
            <Button
              className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm"
              variant="outline"
              size="icon"
              onClick={handleOpenCreate}
            >
              <Plus />
            </Button>
          </ButtonHandle>
        )}

        {/* Only show target handle for non-Start steps */}
        {!isStartStep && (
          <ButtonHandle
            showButton={Boolean(!targetConnections.length && !isExecuting)}
            id="target"
            type="target"
            position={Position.Left}
            isConnectable={isConnectable && !isExecuting}
            className="!bg-muted-foreground/40"
          >
            <Button
              className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm"
              variant="outline"
              size="icon"
              onClick={handleOpenCreateBefore}
            >
              <Plus />
            </Button>
          </ButtonHandle>
        )}

        {/* Error handle for error transitions - only for steps that can actually fail */}
        {/* Handle is invisible by default, shown only when node is selected to reduce UI clutter */}
        {(canFail || errorConnections.length > 0) && (
          <ButtonHandle
            showButton={Boolean(
              selected && !errorConnections.length && !isExecuting
            )}
            id="onError"
            type="source"
            position={Position.Bottom}
            isConnectable={isConnectable && !isExecuting}
            className={selected ? '!bg-destructive/40' : '!opacity-0'}
          >
            <Button
              className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm bg-destructive/10 hover:bg-destructive/20"
              variant="outline"
              size="icon"
              onClick={handleOpenCreateError}
              title="Add error handler"
            >
              <AlertTriangle className="text-destructive" />
            </Button>
          </ButtonHandle>
        )}
      </BaseNode>

      {/* Step Picker Modal - only render when open to avoid unnecessary subscriptions */}
      {showStepPicker && (
        <NodeFormProvider
          isAddingBefore={isAddingBefore}
          parentNodeId={isAddingBefore ? undefined : id}
        >
          <StepPickerModal
            open={showStepPicker}
            onOpenChange={(open) => {
              setShowStepPicker(open);
              if (!open) {
                setIsAddingBefore(false);
              }
            }}
            onSelect={isAddingBefore ? handleCreateBefore : handleCreate}
          />
        </NodeFormProvider>
      )}
    </>
  );
}

// Custom comparison to prevent re-renders during drag operations
// Position props change during drag but we don't need to re-render for that
function arePropsEqual(
  prevProps: NodeProps<BasicNodeProps>,
  nextProps: NodeProps<BasicNodeProps>
): boolean {
  // Always re-render if these change
  if (prevProps.id !== nextProps.id) return false;
  if (prevProps.selected !== nextProps.selected) return false;
  if (prevProps.isConnectable !== nextProps.isConnectable) return false;
  if (prevProps.dragging !== nextProps.dragging) return false;
  if (prevProps.parentId !== nextProps.parentId) return false;

  // Deep compare data object
  if (prevProps.data !== nextProps.data) {
    // Quick check for common data changes
    const prevData = prevProps.data;
    const nextData = nextProps.data;
    if (
      prevData.name !== nextData.name ||
      prevData.stepType !== nextData.stepType ||
      prevData.agentId !== nextData.agentId ||
      prevData.capabilityId !== nextData.capabilityId ||
      (prevData as any).breakpoint !== (nextData as any).breakpoint
    ) {
      return false;
    }
    // For inputMapping, just compare reference (it's usually stable)
    if (prevData.inputMapping !== nextData.inputMapping) return false;
  }

  // Ignore position changes - they happen during drag and we don't render based on them
  // positionAbsoluteX, positionAbsoluteY, zIndex, etc.

  return true;
}

export const BasicNode = memo(BasicNodeComponent, arePropsEqual);
