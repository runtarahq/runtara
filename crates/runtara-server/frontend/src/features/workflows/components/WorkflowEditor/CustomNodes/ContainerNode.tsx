import { memo, useCallback, useState } from 'react';
import {
  Handle,
  Node,
  NodeProps,
  Position,
  useNodeConnections,
} from '@xyflow/react';
import { v4 as uuidv4 } from 'uuid';
import { Plus, AlertTriangle } from 'lucide-react';
import { ButtonHandle } from '../ButtonHandle.tsx';
import { Button } from '@/shared/components/ui/button.tsx';
import * as form from '@/features/workflows/components/WorkflowEditor/NodeForm/NodeFormItem.tsx';
import { BASE_GROUP_WIDTH } from './utils.tsx';
import { BaseResizableNode } from './BaseResizableNode.tsx';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import {
  STEP_TYPES,
  NODE_TYPES,
  NODE_TYPE_SIZES,
} from '@/features/workflows/config/workflow.ts';
import { NodeFormProvider } from '../NodeForm/NodeFormProvider';
import { StepPickerModal, StepPickerResult } from '../NodeForm/StepPickerModal';

type ContainerNodeProps = Node<form.SchemaType>;

function Container({
  id,
  data,
  isConnectable,
  selected,
}: NodeProps<ContainerNodeProps>) {
  const [showStepPicker, setShowStepPicker] = useState<boolean>(false);
  const [isCreatingInside, setIsCreatingInside] = useState<boolean>(false);

  // Get actions using individual selectors to prevent re-renders on unrelated state changes
  const setPendingNewNode = useWorkflowStore(
    (state) => state.setPendingNewNode
  );
  // Check if workflow is executing (read-only mode)
  const isExecuting = useExecutionStore((state) => !!state.executingInstanceId);

  // Only check connections when needed for rendering
  const connections = useNodeConnections({
    handleType: 'source',
    handleId: 'source',
  });
  const errorConnections = useNodeConnections({
    handleType: 'source',
    handleId: 'onError',
  });

  // Check if container has any child nodes using a stable selector
  // Returns a boolean, so it only re-renders when hasChildren actually changes
  const hasChildren = useWorkflowStore((state) =>
    state.nodes.some((node) => node.parentId === id)
  );

  // Check if this node has validation errors
  const hasValidationError = useWorkflowStore((state) =>
    state.stepsWithErrors.has(id)
  );

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
        BASE_GROUP_WIDTH;
      const currentHeight =
        (currentNode.style?.height as number) ||
        (currentNode.height as number) ||
        168;

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

      let position;
      let parentNodeId: string | undefined;

      if (isCreatingInside) {
        // Creating a child node inside the container
        // Position it in the center of the container (relative coordinates)
        position = {
          x: (currentWidth - newNodeSize.width) / 2,
          y: (currentHeight - newNodeSize.height) / 2,
        };
        parentNodeId = id;
      } else {
        // Creating a sibling node outside the container (original behavior)
        // Get current node's actual height from config (don't trust height prop - it can change with resizing)
        const currentNodeHeight =
          NODE_TYPE_SIZES[NODE_TYPES.ContainerNode].height; // 168px

        // Calculate handle positions to align them (handles are at node center)
        const currentHandleY = currentPosAbsY + currentNodeHeight / 2;
        const newNodeHandleOffset = newNodeSize.height / 2;

        position = {
          x: currentPosAbsX + currentWidth + 108, // Will be snapped to grid by store
          y: currentHandleY - newNodeHandleOffset, // Keep precise for handle alignment
        };
      }

      // Set pending new node - will be created when user confirms in dialog
      const newNodeId = uuidv4();
      setPendingNewNode({
        id: newNodeId,
        data: nodeData as any,
        position,
        parentId: parentNodeId,
        // Connect it to the current node only if creating outside
        sourceNodeId: !isCreatingInside ? id : undefined,
      });

      setShowStepPicker(false);
      setIsCreatingInside(false);
    },
    [id, isCreatingInside, setPendingNewNode]
  );

  const handleOpenCreate = (e: React.MouseEvent) => {
    e.stopPropagation(); // Prevent node selection when clicking "+"
    setIsCreatingInside(false);
    setShowStepPicker(true);
  };

  const handleOpenCreateInside = (e: React.MouseEvent) => {
    e.stopPropagation(); // Prevent node selection when clicking "+"
    setIsCreatingInside(true);
    setShowStepPicker(true);
  };

  const handleOpenCreateError = (e: React.MouseEvent) => {
    e.stopPropagation(); // Prevent node selection when clicking "+"
    setIsCreatingInside(false);

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
      BASE_GROUP_WIDTH;
    const currentHeight =
      (currentNode.style?.height as number) ||
      (currentNode.height as number) ||
      168;

    const nodeData = {
      ...form.initialValues,
      stepType: 'Error',
      name: 'Error handler',
    } as form.SchemaType;

    const newNodeSize = NODE_TYPE_SIZES[NODE_TYPES.BasicNode];

    const position = {
      x: currentPosAbsX + currentWidth / 2 - newNodeSize.width / 2,
      y: currentPosAbsY + currentHeight + 80,
    };

    setPendingNewNode({
      id: uuidv4(),
      data: nodeData as any,
      position,
      sourceNodeId: id,
      sourceHandle: 'onError',
    });
  };

  return (
    <>
      {/* Step name label - positioned above the container */}
      {data.name && (
        <div className="absolute -top-6 left-0 text-sm font-medium text-foreground pointer-events-none">
          {data.name}
        </div>
      )}

      <BaseResizableNode
        id={id}
        name="" // Don't show name inside the node
        selected={selected}
        hasValidationError={hasValidationError}
      >
        {/* Placeholder button when container is empty - hidden during execution */}
        {!hasChildren && !isExecuting && (
          <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
            <Button
              className="pointer-events-auto nodrag nopan"
              variant="outline"
              size="sm"
              onClick={handleOpenCreateInside}
            >
              <Plus className="mr-2 h-4 w-4" />
              Add first step
            </Button>
          </div>
        )}
      </BaseResizableNode>
      <Handle
        type="target"
        id="target"
        position={Position.Left}
        className="!w-2 !h-2 !rounded-full !bg-muted-foreground/40 !border-0"
        isConnectable={isConnectable && !isExecuting}
      />
      <ButtonHandle
        id="source"
        type="source"
        position={Position.Right}
        showButton={Boolean(!connections.length && !isExecuting)}
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

      {/* Error handle for error transitions - only for Split steps that can fail */}
      {/* Handle is invisible by default, shown only when node is selected to reduce UI clutter */}
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

      {/* Step Picker Modal - only render when open to avoid unnecessary subscriptions */}
      {showStepPicker && (
        <NodeFormProvider parentNodeId={isCreatingInside ? id : undefined}>
          <StepPickerModal
            open={showStepPicker}
            onOpenChange={(open) => {
              setShowStepPicker(open);
              if (!open) {
                setIsCreatingInside(false);
              }
            }}
            onSelect={handleCreate}
          />
        </NodeFormProvider>
      )}
    </>
  );
}

// Custom comparison to prevent re-renders during drag operations
// Position props change during drag but we don't need to re-render for that
function arePropsEqual(
  prevProps: NodeProps<ContainerNodeProps>,
  nextProps: NodeProps<ContainerNodeProps>
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
      prevData.capabilityId !== nextData.capabilityId
    ) {
      return false;
    }
  }

  // Ignore position changes - they happen during drag and we don't render based on them
  // positionAbsoluteX, positionAbsoluteY, width, height, zIndex, etc.

  return true;
}

export const ContainerNode = memo(Container, arePropsEqual);
