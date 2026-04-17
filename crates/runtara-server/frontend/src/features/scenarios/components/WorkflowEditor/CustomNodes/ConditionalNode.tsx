import { memo, useCallback, useState, useMemo } from 'react';
import {
  Handle,
  Node,
  NodeProps,
  Position,
  useNodeConnections,
} from '@xyflow/react';
import { v4 as uuidv4 } from 'uuid';
import { Button } from '@/shared/components/ui/button.tsx';
import { Plus } from 'lucide-react';
import { BaseNode } from '../BaseNode.tsx';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
  STEP_TYPES,
} from '@/features/scenarios/config/workflow.ts';
import { BASE_WIDTH } from './utils.tsx';
import * as form from '@/features/scenarios/components/WorkflowEditor/NodeForm/NodeFormItem.tsx';
import { useWorkflowStore } from '@/features/scenarios/stores/workflowStore.ts';
import {
  snapPositionToGrid,
  snapToGrid,
} from '@/features/scenarios/config/workflow-editor';
import { useExecutionStore } from '@/features/scenarios/stores/executionStore';
import {
  useValidationStore,
  getFirstValidationMessage,
} from '@/features/scenarios/stores/validationStore';
import { NodeFormProvider } from '../NodeForm/NodeFormProvider';
import { StepPickerModal, StepPickerResult } from '../NodeForm/StepPickerModal';
import {
  renderConditionReadable,
  Condition,
} from '@/shared/components/ui/condition-editor';
import { getConditionFromInputMapping } from '@/shared/utils/condition-utils';

type ConditionalNodeProps = Node<form.SchemaType>;

const SOURCE_TRUE = 'true';
const SOURCE_FALSE = 'false';

function ConditionalNodeComponent({
  id,
  data,
  isConnectable,
  selected,
}: NodeProps<ConditionalNodeProps>) {
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

  // Extract condition summary for display
  const conditionSummary = useMemo(() => {
    // Try capabilityId first (primary storage for condition JSON)
    if (data.capabilityId && typeof data.capabilityId === 'string') {
      try {
        const condition = JSON.parse(data.capabilityId) as Condition;
        if (condition && condition.op) {
          return renderConditionReadable(condition);
        }
      } catch {
        // Fall through to inputMapping
      }
    }

    // Fallback: reconstruct from inputMapping
    if (data.inputMapping && Array.isArray(data.inputMapping)) {
      // Filter to only condition-related entries
      const conditionEntries = data.inputMapping.filter(
        (entry: any) =>
          entry.type?.startsWith('condition.expression.') ||
          entry.type?.startsWith('expression.')
      );
      if (conditionEntries.length > 0) {
        const condition = getConditionFromInputMapping(conditionEntries);
        if (condition) {
          return renderConditionReadable(condition as Condition);
        }
      }
    }

    return null;
  }, [data.capabilityId, data.inputMapping]);

  const trueConnections = useNodeConnections({
    handleType: 'source',
    handleId: SOURCE_TRUE,
  });

  const falseConnections = useNodeConnections({
    handleType: 'source',
    handleId: SOURCE_FALSE,
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

      const type = STEP_TYPES[result.stepType] || NODE_TYPES.BasicNode;
      const style = NODE_TYPE_SIZES[type];

      let position;

      if (currentParentId) {
        // When inside a parent (split/container), use relative positioning
        // Calculate Y offset based on handle (true = above, false = below)
        const verticalOffset = activeSource === SOURCE_TRUE ? -120 : 120;

        // Position relative to current node's position within the parent
        position = snapPositionToGrid({
          x: currentNode.position.x + currentWidth + 108,
          y: currentNode.position.y + verticalOffset,
        });
      } else {
        // When not inside a parent, use absolute positioning
        const positionShiftY = snapToGrid(
          activeSource === SOURCE_TRUE ? -style.width : style.width
        );
        position = snapPositionToGrid({
          x: currentPosAbsX + currentWidth + 108,
          y: currentPosAbsY + positionShiftY,
        });
      }

      // Set pending new node - will be created when user confirms in dialog
      const newNodeId = uuidv4();
      setPendingNewNode({
        id: newNodeId,
        data: nodeData as any,
        position,
        parentId: currentParentId,
        sourceNodeId: id, // Connect FROM conditional node TO new node
        sourceHandle: activeSource || 'default',
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
        subtitle={conditionSummary}
        style={{
          height: `${NODE_TYPE_SIZES[NODE_TYPES.ConditionalNode].height}px`,
        }}
      >
        {/* True handle - pill centered on the right border, positioned at 30% from top */}
        <Handle
          id={SOURCE_TRUE}
          type="source"
          position={Position.Right}
          className="!w-2 !h-2 !rounded-full !bg-green-500 dark:!bg-green-400 !border-0"
          style={{ top: '30%', right: '-4px', transform: 'translateY(-50%)' }}
          isConnectable={isConnectable && !isExecuting}
        />
        {!trueConnections.length && !isExecuting && (
          <div
            className="absolute flex items-center pointer-events-none"
            style={{
              right: '-32px',
              top: '30%',
              transform: 'translateY(-50%)',
              zIndex: -1,
            }}
          >
            <div className="bg-border h-[1px] w-4 ml-0.5" />
            <Button
              className="w-3.5 h-3.5 rounded-full [&_svg]:size-1.5 shadow-sm pointer-events-auto nodrag nopan"
              variant="outline"
              size="icon"
              onClick={(e) => {
                e.stopPropagation(); // Prevent node selection when clicking "+"
                setActiveSource(SOURCE_TRUE);
                setShowStepPicker(true);
              }}
            >
              <Plus />
            </Button>
          </div>
        )}

        {/* False handle - pill centered on the right border, positioned at 70% from top */}
        <Handle
          id={SOURCE_FALSE}
          type="source"
          position={Position.Right}
          className="!w-2 !h-2 !rounded-full !bg-red-500 dark:!bg-red-400 !border-0"
          style={{ top: '70%', right: '-4px', transform: 'translateY(-50%)' }}
          isConnectable={isConnectable && !isExecuting}
        />
        {!falseConnections.length && !isExecuting && (
          <div
            className="absolute flex items-center pointer-events-none"
            style={{
              right: '-32px',
              top: '70%',
              transform: 'translateY(-50%)',
              zIndex: -1,
            }}
          >
            <div className="bg-border h-[1px] w-4 ml-0.5" />
            <Button
              className="w-3.5 h-3.5 rounded-full [&_svg]:size-1.5 shadow-sm pointer-events-auto nodrag nopan"
              variant="outline"
              size="icon"
              onClick={(e) => {
                e.stopPropagation(); // Prevent node selection when clicking "+"
                setActiveSource(SOURCE_FALSE);
                setShowStepPicker(true);
              }}
            >
              <Plus />
            </Button>
          </div>
        )}

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
  prevProps: NodeProps<ConditionalNodeProps>,
  nextProps: NodeProps<ConditionalNodeProps>
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

export const ConditionalNode = memo(ConditionalNodeComponent, arePropsEqual);
