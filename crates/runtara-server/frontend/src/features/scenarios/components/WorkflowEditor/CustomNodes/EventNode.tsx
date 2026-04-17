import { memo, useCallback, useState } from 'react';
import {
  Handle,
  Node,
  NodeProps,
  Position,
  useNodeConnections,
} from '@xyflow/react';
import { v4 as uuidv4 } from 'uuid';
import { Button } from '@/shared/components/ui/button.tsx';
import { ButtonHandle } from '../ButtonHandle.tsx';
import { Plus } from 'lucide-react';
import { BaseNode } from '../BaseNode.tsx';
import * as form from '@/features/scenarios/components/WorkflowEditor/NodeForm/NodeFormItem.tsx';
import { useExecutionStore } from '@/features/scenarios/stores/executionStore';
import { useWorkflowStore } from '@/features/scenarios/stores/workflowStore';
import {
  useValidationStore,
  getFirstValidationMessage,
} from '@/features/scenarios/stores/validationStore';
import { NodeFormProvider } from '../NodeForm/NodeFormProvider';
import { StepPickerModal, StepPickerResult } from '../NodeForm/StepPickerModal';

type EventNodeProps = Node<form.SchemaType>;

const SOURCE_ONSTART = 'onstart';
const SOURCE_SOURCE = 'source';

function EventNodeComponent({
  id,
  data,
  isConnectable,
  selected,
}: NodeProps<EventNodeProps>) {
  const [showStepPicker, setShowStepPicker] = useState<boolean>(false);
  const [activeSource, setActiveSource] = useState<string | null>(null);

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

  // Toggle breakpoint on this step
  const handleToggleBreakpoint = useCallback(() => {
    const { nodes, updateNode } = useWorkflowStore.getState();
    const node = nodes.find((n) => n.id === id);
    const current = !!(node?.data as any)?.breakpoint;
    updateNode(id, { breakpoint: current ? undefined : true } as any);
  }, [id]);

  // Check if this node has unsaved changes
  const hasUnsavedChanges = useWorkflowStore((state) =>
    state.stagedNodeIds.has(id)
  );

  // Check if this node has validation errors
  const hasValidationError = useWorkflowStore((state) =>
    state.stepsWithErrors.has(id)
  );
  const hasValidationErrorFromPanel = useValidationStore((state) =>
    state.stepsWithErrors.has(id)
  );
  const hasValidationWarning = useValidationStore((state) =>
    state.stepsWithWarnings.has(id)
  );
  const validationMessage = useValidationStore((state) =>
    getFirstValidationMessage(state, id)
  );
  const showValidationError = hasValidationError || hasValidationErrorFromPanel;

  const sourceConnections = useNodeConnections({
    handleType: 'source',
    handleId: SOURCE_SOURCE,
  });

  const onStartConnections = useNodeConnections({
    handleType: 'source',
    handleId: SOURCE_ONSTART,
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

      // Build node data from picker result
      const nodeData = {
        ...form.initialValues,
        stepType: result.stepType,
        name: result.name,
        agentId: result.agentId || '',
        capabilityId: result.capabilityId || '',
      } as form.SchemaType;

      const newId = uuidv4();

      // Set pending new node - will be created when user confirms in dialog
      setPendingNewNode({
        id: newId,
        data: nodeData as any,
        position: {
          x: currentPosAbsX + 225,
          y: currentPosAbsY,
        },
        sourceNodeId: id,
        sourceHandle: activeSource || undefined,
      });

      setShowStepPicker(false);
      setActiveSource(null);
    },
    [id, activeSource, setPendingNewNode]
  );

  return (
    <>
      <BaseNode
        id={id}
        name={data.name}
        stepType={data.stepType}
        agentId={data.agentId}
        selected={selected}
        executionStatus={executionStatus}
        hasUnsavedChanges={hasUnsavedChanges}
        hasValidationError={showValidationError}
        hasValidationWarning={hasValidationWarning}
        validationMessage={showValidationError ? validationMessage : null}
        isExecutionReadOnly={isExecuting}
        breakpoint={!!(data as any).breakpoint}
        onToggleBreakpoint={handleToggleBreakpoint}
      >
        <ButtonHandle
          className="!bg-primary/60"
          showButton={Boolean(!onStartConnections.length && !isExecuting)}
          id={SOURCE_ONSTART}
          type="source"
          position={Position.Top}
          isConnectable={isConnectable && !isExecuting}
        >
          <Button
            className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm"
            variant="outline"
            size="icon"
            onClick={(e) => {
              e.stopPropagation(); // Prevent node selection when clicking "+"
              setActiveSource(SOURCE_ONSTART);
              setShowStepPicker(true);
            }}
          >
            <Plus />
          </Button>
        </ButtonHandle>

        <ButtonHandle
          className="!bg-muted-foreground/40"
          showButton={Boolean(!sourceConnections.length && !isExecuting)}
          id={SOURCE_SOURCE}
          type="source"
          position={Position.Right}
          isConnectable={isConnectable && !isExecuting}
        >
          <Button
            className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm"
            variant="outline"
            size="icon"
            onClick={(e) => {
              e.stopPropagation(); // Prevent node selection when clicking "+"
              setActiveSource(SOURCE_SOURCE);
              setShowStepPicker(true);
            }}
          >
            <Plus />
          </Button>
        </ButtonHandle>

        <Handle
          type="target"
          id="target"
          position={Position.Left}
          className="!w-2 !h-2 !rounded-full !bg-muted-foreground/40 !border-0"
          isConnectable={isConnectable && !isExecuting}
        />
      </BaseNode>

      {/* Step Picker Modal - only render when open to avoid unnecessary subscriptions */}
      {showStepPicker && (
        <NodeFormProvider parentNodeId={id}>
          <StepPickerModal
            open={showStepPicker}
            onOpenChange={(open) => {
              setShowStepPicker(open);
              if (!open) {
                setActiveSource(null);
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
  prevProps: NodeProps<EventNodeProps>,
  nextProps: NodeProps<EventNodeProps>
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
  // positionAbsoluteX, positionAbsoluteY, zIndex, etc.

  return true;
}

export const EventNode = memo(EventNodeComponent, arePropsEqual);
