import { memo, useCallback, useState, useMemo } from 'react';
import { Handle, Node, NodeProps, Position } from '@xyflow/react';
import { v4 as uuidv4 } from 'uuid';
import { Button } from '@/shared/components/ui/button.tsx';
import { Plus } from 'lucide-react';
import { BaseNode } from '../BaseNode.tsx';
import { BASE_WIDTH } from './utils.tsx';
import * as form from '@/features/workflows/components/WorkflowEditor/NodeForm/NodeFormItem.tsx';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import {
  useValidationStore,
  getFirstValidationMessage,
} from '@/features/workflows/stores/validationStore';
import {
  snapPositionToGrid,
  snapToGrid,
} from '@/features/workflows/config/workflow-editor';
import { NodeFormProvider } from '../NodeForm/NodeFormProvider';
import { StepPickerModal, StepPickerResult } from '../NodeForm/StepPickerModal';
import { SWITCH_FIRST_HANDLE_TOP, SWITCH_HANDLE_SPACING } from './layout';

type SwitchNodeProps = Node<form.SchemaType>;

// Helper function to format large numbers (e.g., 10000000 -> "10M")
function formatNumber(num: number): string {
  if (num >= 1000000) {
    return `${num / 1000000}M`;
  } else if (num >= 1000) {
    return `${num / 1000}K`;
  }
  return num.toString();
}

// Helper function to format range match values into readable labels
function formatRangeLabel(match: any): string {
  if (!match || typeof match !== 'object') {
    return String(match || '');
  }

  // Handle {min, max} format
  if ('min' in match || 'max' in match) {
    const min = match.min;
    const max = match.max;

    if (min !== undefined && max !== undefined) {
      // Both min and max: "5M-10M"
      return `${formatNumber(min)}-${formatNumber(max)}`;
    } else if (min !== undefined) {
      // Only min: "≥ 10M"
      return `≥ ${formatNumber(min)}`;
    } else if (max !== undefined) {
      // Only max: "< 10M"
      return `< ${formatNumber(max)}`;
    }
  }

  // Handle {gte, gt, lt, lte} format
  const parts: string[] = [];
  if ('gte' in match) {
    parts.push(`≥ ${formatNumber(match.gte)}`);
  } else if ('gt' in match) {
    parts.push(`> ${formatNumber(match.gt)}`);
  }

  if ('lt' in match) {
    parts.push(`< ${formatNumber(match.lt)}`);
  } else if ('lte' in match) {
    parts.push(`≤ ${formatNumber(match.lte)}`);
  }

  if (parts.length > 0) {
    return parts.join(', ');
  }

  // Fallback: stringify the object
  return JSON.stringify(match);
}

// Helper function to get a label for a case
function getCaseLabel(
  caseItem: any,
  index: number,
  preferRoute = false
): string {
  // In routing mode, prefer route name if available
  if (preferRoute && caseItem.route) {
    return caseItem.route;
  }

  const matchType = caseItem.matchType || 'exact';
  const match = caseItem.match;

  switch (matchType) {
    case 'range':
      return formatRangeLabel(match);
    case 'exact':
      return String(match || `Case ${index + 1}`);
    case 'ne':
      return `≠ ${match || ''}`;
    case 'in':
      if (Array.isArray(match)) {
        return match.join(', ');
      }
      return String(match || `Case ${index + 1}`);
    case 'not_in':
      if (Array.isArray(match)) {
        return `not: ${match.join(', ')}`;
      }
      return String(match || `Case ${index + 1}`);
    case 'gt':
      return `> ${match}`;
    case 'gte':
      return `≥ ${match}`;
    case 'lt':
      return `< ${match}`;
    case 'lte':
      return `≤ ${match}`;
    case 'between':
      if (Array.isArray(match) && match.length >= 2) {
        return `${match[0]}-${match[1]}`;
      }
      return String(match || `Case ${index + 1}`);
    case 'starts_with':
      return `^${match || ''}`;
    case 'ends_with':
      return `${match || ''}$`;
    case 'contains':
      return `*${match || ''}*`;
    case 'is_defined':
      return 'defined?';
    case 'is_empty':
      return 'empty?';
    case 'is_not_empty':
      return 'not empty?';
    default:
      return `Case ${index + 1}`;
  }
}

function SwitchNodeComponent({
  id,
  data,
  isConnectable,
  selected,
}: NodeProps<SwitchNodeProps>) {
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

  // Check if workflow is executing (read-only mode)
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

  // Extract cases and routing mode from inputMapping
  const inputMapping = data.inputMapping || [];
  const casesField = inputMapping.find((item: any) => item.type === 'cases');
  const cases = Array.isArray(casesField?.value) ? casesField.value : [];
  const routingModeField = inputMapping.find(
    (item: any) => item.type === 'routingMode'
  );
  const isRoutingMode =
    routingModeField?.value === true ||
    cases.some((c: any) => c.route && c.route !== '');

  // Get edges from store (stable - doesn't change during node drag)
  const edges = useWorkflowStore((state) => state.edges);
  const hasConnection = useMemo(() => {
    return (handleId: string) => {
      return edges.some(
        (edge) => edge.source === id && edge.sourceHandle === handleId
      );
    };
  }, [edges, id]);

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

      let position;

      // Calculate vertical offset based on which case handle was clicked
      let verticalOffset = 0;
      if (activeSource && activeSource.startsWith('case-')) {
        const caseIndex = parseInt(activeSource.split('-')[1], 10);
        // Offset based on the rendered case handle position.
        const handleTop =
          SWITCH_FIRST_HANDLE_TOP + caseIndex * SWITCH_HANDLE_SPACING;
        const nodeCenter = 36;
        verticalOffset = snapToGrid(handleTop - nodeCenter);
      } else if (activeSource === 'default') {
        // Default handle is at the bottom
        const handleTop =
          SWITCH_FIRST_HANDLE_TOP + cases.length * SWITCH_HANDLE_SPACING;
        const nodeCenter = 36;
        verticalOffset = snapToGrid(handleTop - nodeCenter);
      }

      if (currentParentId) {
        // When inside a parent (split/container), use relative positioning
        // Position relative to current node's position within the parent
        position = snapPositionToGrid({
          x: currentNode.position.x + currentWidth + 108,
          y: currentNode.position.y + verticalOffset,
        });
      } else {
        // When not inside a parent, use absolute positioning
        position = snapPositionToGrid({
          x: currentPosAbsX + currentWidth + 108,
          y: currentPosAbsY + verticalOffset,
        });
      }

      // Set pending new node - will be created when user confirms in dialog
      const newNodeId = uuidv4();
      setPendingNewNode({
        id: newNodeId,
        data: nodeData as any,
        position,
        parentId: currentParentId,
        sourceNodeId: id, // Connect FROM switch node TO new node
        sourceHandle: activeSource || 'default',
      });

      setShowStepPicker(false);
      setActiveSource(null);
    },
    [id, activeSource, cases.length, setPendingNewNode]
  );

  // Calculate total node height based on number of cases
  const handleSpacing = SWITCH_HANDLE_SPACING;
  const firstHandleTop = SWITCH_FIRST_HANDLE_TOP;

  // In routing mode: dynamic height for case handles + default
  // In value mode: standard height (single output)
  const totalHandles = isRoutingMode ? cases.length + 1 : 0; // cases + default
  const minHeight = isRoutingMode ? 72 : 36;
  const lastHandleTop = firstHandleTop + (totalHandles - 1) * handleSpacing;
  const requiredHeight = isRoutingMode
    ? Math.max(minHeight, lastHandleTop + 24) // 24px padding at bottom
    : minHeight;

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
        style={{ height: `${requiredHeight}px` }}
        rightReservedWidth={isRoutingMode ? 75 : undefined}
      >
        {isRoutingMode ? (
          <>
            {/* Routing mode: Render a handle for each case */}
            {cases.map((caseItem: any, index: number) => {
              const handleId = `case-${index}`;
              const label = getCaseLabel(caseItem, index, true);
              const topPosition = firstHandleTop + index * handleSpacing;

              return (
                <div key={handleId}>
                  <Handle
                    id={handleId}
                    type="source"
                    position={Position.Right}
                    className="!w-2 !h-2 !rounded-full !bg-blue-500/60 dark:!bg-blue-500/40 !border-0"
                    style={{ top: `${topPosition}px` }}
                    isConnectable={isConnectable && !isExecuting}
                  />
                  {!hasConnection(handleId) && !isExecuting && (
                    <div
                      className="absolute flex items-center pointer-events-none"
                      style={{
                        right: '-40px',
                        top: `${topPosition}px`,
                        transform: 'translateY(-50%)',
                        zIndex: -1,
                      }}
                    >
                      <div className="bg-border h-[1px] w-6 ml-1" />
                      <Button
                        className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm pointer-events-auto nodrag nopan"
                        variant="outline"
                        size="icon"
                        onClick={(e) => {
                          e.stopPropagation();
                          setActiveSource(handleId);
                          setShowStepPicker(true);
                        }}
                      >
                        <Plus />
                      </Button>
                    </div>
                  )}
                  {/* Case label */}
                  <div
                    className="absolute text-[0.65rem] text-muted-foreground pointer-events-none whitespace-nowrap"
                    style={{
                      right: '12px',
                      top: `${topPosition}px`,
                      transform: 'translateY(-50%)',
                    }}
                  >
                    {label}
                  </div>
                </div>
              );
            })}

            {/* Default handle */}
            <Handle
              id="default"
              type="source"
              position={Position.Right}
              className="!w-2 !h-2 !rounded-full !bg-muted-foreground/40 !border-0"
              style={{
                top: `${firstHandleTop + cases.length * handleSpacing}px`,
              }}
              isConnectable={isConnectable && !isExecuting}
            />
            {!hasConnection('default') && !isExecuting && (
              <div
                className="absolute flex items-center pointer-events-none"
                style={{
                  right: '-40px',
                  top: `${firstHandleTop + cases.length * handleSpacing}px`,
                  transform: 'translateY(-50%)',
                }}
              >
                <div className="bg-border h-[1px] w-6 ml-1" />
                <Button
                  className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm pointer-events-auto nodrag nopan"
                  variant="outline"
                  size="icon"
                  onClick={(e) => {
                    e.stopPropagation();
                    setActiveSource('default');
                    setShowStepPicker(true);
                  }}
                >
                  <Plus />
                </Button>
              </div>
            )}
            {/* Default label */}
            <div
              className="absolute text-[0.65rem] text-muted-foreground pointer-events-none whitespace-nowrap"
              style={{
                right: '12px',
                top: `${firstHandleTop + cases.length * handleSpacing}px`,
                transform: 'translateY(-50%)',
              }}
            >
              default
            </div>
          </>
        ) : (
          <>
            {/* Value mode: Single source handle (like BasicNode) */}
            <Handle
              id="source"
              type="source"
              position={Position.Right}
              className="!w-2 !h-2 !rounded-full !bg-muted-foreground/40 !border-0"
              isConnectable={isConnectable && !isExecuting}
            />
          </>
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
  prevProps: NodeProps<SwitchNodeProps>,
  nextProps: NodeProps<SwitchNodeProps>
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
    // For inputMapping (contains cases), compare reference
    if (prevData.inputMapping !== nextData.inputMapping) return false;
  }

  // Ignore position changes - they happen during drag and we don't render based on them
  // positionAbsoluteX, positionAbsoluteY, zIndex, etc.

  return true;
}

export const SwitchNode = memo(SwitchNodeComponent, arePropsEqual);
