import { memo, useCallback, useMemo, useState } from 'react';
import { Handle, Node, NodeProps, Position } from '@xyflow/react';
import { v4 as uuidv4 } from 'uuid';
import { Plus, Loader2, CheckCircle2, XCircle, Pause } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { BaseNode } from '../BaseNode.tsx';
import { StepTypeIcon } from '@/features/workflows/components/StepTypeIcon';
import * as form from '@/features/workflows/components/WorkflowEditor/NodeForm/NodeFormItem.tsx';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import {
  useValidationStore,
  getFirstValidationMessage,
} from '@/features/workflows/stores/validationStore';
import { snapPositionToGrid } from '@/features/workflows/config/workflow-editor';
import {
  NODE_TYPE_SIZES,
  NODE_TYPES,
} from '@/features/workflows/config/workflow.ts';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { cn } from '@/lib/utils.ts';
import { useNodeConfigContext } from '../NodeConfigContext';
import { NodeFormProvider } from '../NodeForm/NodeFormProvider';
import { StepPickerModal, StepPickerResult } from '../NodeForm/StepPickerModal';

/** Shorten model identifiers for badge display */
function formatModelName(model: string): string {
  const lower = model.toLowerCase();
  if (lower.startsWith('gpt-4.1-nano')) return 'GPT-4.1n';
  if (lower.startsWith('gpt-4.1-mini')) return 'GPT-4.1m';
  if (lower.startsWith('gpt-4.1')) return 'GPT-4.1';
  if (lower.startsWith('gpt-4o-mini')) return 'GPT-4o Mini';
  if (lower.startsWith('gpt-4o')) return 'GPT-4o';
  if (lower.startsWith('gpt-4')) return 'GPT-4';
  if (lower.startsWith('gpt-3.5')) return 'GPT-3.5';
  if (lower.startsWith('o4-mini') || lower.includes('o4-mini'))
    return 'o4 Mini';
  if (lower.startsWith('o3-mini') || lower.includes('o3-mini'))
    return 'o3 Mini';
  if (lower.startsWith('o3') || lower.includes('/o3')) return 'o3';
  if (lower.includes('claude-sonnet-4')) return 'Sonnet 4';
  if (
    lower.includes('claude-3-5-sonnet') ||
    lower.includes('claude-3.5-sonnet')
  )
    return 'Sonnet 3.5';
  if (lower.includes('claude-3-opus') || lower.includes('claude-3-5-opus'))
    return 'Opus 3';
  if (lower.includes('claude-3-5-haiku') || lower.includes('claude-3.5-haiku'))
    return 'Haiku 3.5';
  if (lower.includes('claude-3-haiku')) return 'Haiku 3';
  if (lower.includes('claude')) return 'Claude';
  if (lower.includes('nova-pro')) return 'Nova Pro';
  if (lower.includes('nova-lite')) return 'Nova Lite';
  if (lower.includes('nova-micro')) return 'Nova Micro';
  if (lower.includes('titan')) return 'Titan';
  if (lower.includes('mistral')) return 'Mistral';
  if (lower.includes('llama')) return 'Llama';
  if (model.length > 12) return model.slice(0, 12) + '\u2026';
  return model;
}

type AiAgentNodeProps = Node<form.SchemaType>;

function AiAgentNodeComponent({
  id,
  data,
  isConnectable,
  selected,
}: NodeProps<AiAgentNodeProps>) {
  // 'tool' | 'memory' | null — which picker is open
  const [stepPickerMode, setStepPickerMode] = useState<
    'tool' | 'memory' | null
  >(null);

  const { openNodeConfig } = useNodeConfigContext();

  const executionStatus = useExecutionStore((state) =>
    state.nodeExecutionStatus.get(id)
  );
  const isExecuting = useExecutionStore((state) => !!state.executingInstanceId);

  // Toggle breakpoint on this step
  const handleToggleBreakpoint = useCallback(() => {
    const { nodes, updateNode } = useWorkflowStore.getState();
    const node = nodes.find((n) => n.id === id);
    const current = !!(node?.data as any)?.breakpoint;
    updateNode(id, { breakpoint: current ? undefined : true } as any);
  }, [id]);

  const hasUnsavedChanges = useWorkflowStore((state) =>
    state.stagedNodeIds.has(id)
  );
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

  // Extract tool names from inputMapping
  const toolNames = useMemo(() => {
    const inputMapping = data.inputMapping || [];
    const toolsField = inputMapping.find((item: any) => item.type === 'tools');
    return Array.isArray(toolsField?.value)
      ? (toolsField.value as string[])
      : [];
  }, [data.inputMapping]);

  // Check if memory is enabled
  const hasMemory = useMemo(() => {
    const inputMapping = data.inputMapping || [];
    const memoryField = inputMapping.find(
      (item: any) => item.type === 'memoryEnabled'
    );
    return memoryField?.value === true;
  }, [data.inputMapping]);

  // Extract model name from inputMapping
  const modelName = useMemo(() => {
    const inputMapping = data.inputMapping || [];
    const modelField = inputMapping.find((item: any) => item.type === 'model');
    const value = modelField?.value;
    if (!value || typeof value !== 'string') return null;
    return formatModelName(value);
  }, [data.inputMapping]);

  // Resolve tool type labels and tool node IDs from connected target nodes
  const edges = useWorkflowStore((state) => state.edges);
  const nodes = useWorkflowStore((state) => state.nodes);

  const { toolTypeLabels, toolNodeIds } = useMemo(() => {
    const labels: Record<string, string> = {};
    const nodeIds: Record<string, string> = {};
    for (const toolName of toolNames) {
      const edge = edges.find(
        (e) => e.source === id && e.sourceHandle === toolName
      );
      if (edge) {
        nodeIds[toolName] = edge.target;
        const targetNode = nodes.find((n) => n.id === edge.target);
        const agentId = (targetNode?.data as any)?.agentId;
        if (agentId) {
          labels[toolName] = agentId.charAt(0).toUpperCase() + agentId.slice(1);
        }
      }
    }
    return { toolTypeLabels: labels, toolNodeIds: nodeIds };
  }, [toolNames, edges, id, nodes]);

  // Resolve memory type label and memory node ID
  const memoryTypeLabel = useMemo(() => {
    const inputMapping = data.inputMapping || [];
    const strategyField = inputMapping.find(
      (item: any) => item.type === 'memoryStrategy'
    );
    if (strategyField?.value === 'slidingWindow') return 'Window';
    if (strategyField?.value === 'summarize') return 'Summary';
    return 'Buffer';
  }, [data.inputMapping]);

  const setPendingNewNode = useWorkflowStore(
    (state) => state.setPendingNewNode
  );
  const addNode = useWorkflowStore((state) => state.addNode);
  const addStoreEdge = useWorkflowStore((state) => state.addEdge);

  // "Add tool" → open step picker to choose agent/capability, then create hidden tool node
  const handleAddToolSelect = useCallback(
    (result: StepPickerResult) => {
      const { nodes: storeNodes } = useWorkflowStore.getState();
      const currentNode = storeNodes.find((n) => n.id === id) as Node & {
        positionAbsolute?: { x: number; y: number };
      };
      if (!currentNode) return;

      // Collect ALL tool names across ALL AI Agent nodes for global uniqueness
      const allToolNames = new Set<string>();
      const allNodeNames = new Set<string>();
      for (const n of storeNodes) {
        const nd = n.data as any;
        if (nd.name) allNodeNames.add(nd.name);
        if (n.type !== NODE_TYPES.AiAgentNode) continue;
        const tf = (nd.inputMapping || []).find((m: any) => m.type === 'tools');
        if (Array.isArray(tf?.value)) {
          for (const t of tf.value) allToolNames.add(t as string);
        }
      }

      // Generate globally unique tool name from the capability/step name
      const baseName = (result.name || 'tool')
        .toLowerCase()
        .replace(/[^a-z0-9_]/g, '_')
        .replace(/_+/g, '_')
        .replace(/^_|_$/g, '');
      let toolName = baseName;
      let i = 1;
      while (allToolNames.has(toolName)) {
        toolName = `${baseName}_${i++}`;
      }

      // Generate globally unique step name
      let stepName = result.name || 'New step';
      let j = 2;
      while (allNodeNames.has(stepName)) {
        stepName = `${result.name || 'New step'} ${j++}`;
      }

      // Add tool name to AI Agent's inputMapping
      const currentMapping = [
        ...((currentNode.data as any).inputMapping || []),
      ];
      const toolsIndex = currentMapping.findIndex(
        (item: any) => item.type === 'tools'
      );
      if (toolsIndex >= 0) {
        currentMapping[toolsIndex] = {
          ...currentMapping[toolsIndex],
          value: [...(currentMapping[toolsIndex].value as string[]), toolName],
        };
      } else {
        currentMapping.push({
          type: 'tools',
          value: [toolName],
          typeHint: 'json',
          valueType: 'immediate',
        });
      }

      // Update AI Agent node data
      const updatedNodes = storeNodes.map((n) => {
        if (n.id !== id) return n;
        return {
          ...n,
          data: { ...n.data, inputMapping: currentMapping },
        };
      });
      useWorkflowStore.setState({
        nodes: updatedNodes,
        isDirty: true,
        isStructurallyDirty: true,
      });

      // Create hidden tool step node positioned off to the right
      const currentPosAbsX =
        currentNode.positionAbsolute?.x ?? currentNode.position.x;
      const currentPosAbsY =
        currentNode.positionAbsolute?.y ?? currentNode.position.y;
      const currentWidth =
        (currentNode.style?.width as number) ||
        NODE_TYPE_SIZES[NODE_TYPES.AiAgentNode]?.width ||
        252;

      const nodeData = {
        ...form.initialValues,
        stepType: result.stepType,
        name: stepName,
        agentId: result.agentId || '',
        capabilityId: result.capabilityId || '',
      } as form.SchemaType;

      const position = snapPositionToGrid({
        x: currentPosAbsX + currentWidth + 108,
        y: currentPosAbsY,
      });

      const newNodeId = uuidv4();
      setPendingNewNode({
        id: newNodeId,
        data: nodeData as any,
        position,
        parentId: currentNode.parentId,
        sourceNodeId: id,
        sourceHandle: toolName,
      });

      setStepPickerMode(null);
    },
    [id, setPendingNewNode]
  );

  // "Add memory" → create hidden memory provider node directly (no config dialog), enable memory
  const handleAddMemorySelect = useCallback(
    (result: StepPickerResult) => {
      const { nodes: storeNodes } = useWorkflowStore.getState();
      const currentNode = storeNodes.find((n) => n.id === id) as Node & {
        positionAbsolute?: { x: number; y: number };
      };
      if (!currentNode) return;

      // Generate globally unique step name
      const allNodeNames = new Set<string>();
      for (const n of storeNodes) {
        if ((n.data as any).name) allNodeNames.add((n.data as any).name);
      }
      let stepName = `${result.name || 'Memory provider'} (memory)`;
      let j = 2;
      while (allNodeNames.has(stepName)) {
        stepName = `${result.name || 'Memory provider'} (memory) ${j++}`;
      }

      // Create memory provider node directly (skip config dialog)
      const currentPosAbsX =
        currentNode.positionAbsolute?.x ?? currentNode.position.x;
      const currentPosAbsY =
        currentNode.positionAbsolute?.y ?? currentNode.position.y;
      const currentWidth =
        (currentNode.style?.width as number) ||
        NODE_TYPE_SIZES[NODE_TYPES.AiAgentNode]?.width ||
        252;

      const position = snapPositionToGrid({
        x: currentPosAbsX + currentWidth + 108,
        y: currentPosAbsY + 60,
      });

      const newNodeId = uuidv4();

      // Pre-populate memory provider step with required inputs
      // (conversation_id and messages are normally filled at runtime by the AI Agent,
      //  but the static validation requires them in inputMapping)
      const memoryStepInputMapping = [
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

      const nodeData = {
        ...form.initialValues,
        id: newNodeId,
        stepType: result.stepType,
        name: stepName,
        agentId: result.agentId || '',
        capabilityId: result.capabilityId || '',
        inputMapping: memoryStepInputMapping,
      } as form.SchemaType;

      // Add the memory provider node directly to the store
      addNode(nodeData as any, position, currentNode.parentId);

      // Add memory edge from AI Agent to memory provider
      addStoreEdge(id, newNodeId, 'memory');

      // Enable memory + set all defaults on the AI Agent's inputMapping
      const currentMapping = [
        ...((currentNode.data as any).inputMapping || []),
      ];

      const setOrAdd = (
        key: string,
        value: any,
        typeHint: string,
        valueType = 'immediate'
      ) => {
        const idx = currentMapping.findIndex((item: any) => item.type === key);
        if (idx >= 0) {
          currentMapping[idx] = { ...currentMapping[idx], value, valueType };
        } else {
          currentMapping.push({ type: key, value, typeHint, valueType });
        }
      };

      setOrAdd('memoryEnabled', true, 'boolean');
      setOrAdd('memoryProviderStepId', newNodeId, 'string');
      setOrAdd('memoryConversationId', '', 'string', 'reference');
      setOrAdd('memoryMaxMessages', 50, 'integer');
      setOrAdd('memoryStrategy', 'summarize', 'string');

      // Update AI Agent node data
      const latestNodes = useWorkflowStore.getState().nodes;
      const updatedNodes = latestNodes.map((n) => {
        if (n.id !== id) return n;
        return {
          ...n,
          data: { ...n.data, inputMapping: currentMapping },
        };
      });
      useWorkflowStore.setState({
        nodes: updatedNodes,
        isDirty: true,
        isStructurallyDirty: true,
      });

      setStepPickerMode(null);
    },
    [id, addNode, addStoreEdge]
  );

  // Execution status icon for header
  const renderStatusIcon = () => {
    if (!executionStatus) return null;
    const s = executionStatus.status;
    switch (s) {
      case ExecutionStatus.Running:
      case ExecutionStatus.Compiling:
        return <Loader2 className="h-3 w-3 animate-spin text-blue-500" />;
      case ExecutionStatus.Completed:
        return <CheckCircle2 className="h-3 w-3 text-green-500" />;
      case ExecutionStatus.Failed:
      case ExecutionStatus.Timeout:
        return <XCircle className="h-3 w-3 text-red-500" />;
      case ExecutionStatus.Queued:
        return <Pause className="h-3 w-3 text-yellow-500" />;
      case ExecutionStatus.Cancelled:
        return <XCircle className="h-3 w-3 text-gray-400" />;
      default:
        return null;
    }
  };

  return (
    <>
      <BaseNode
        id={id}
        selected={selected}
        executionStatus={executionStatus}
        hasUnsavedChanges={hasUnsavedChanges}
        hasValidationError={showValidationError}
        hasValidationWarning={hasValidationWarning}
        validationMessage={showValidationError ? validationMessage : null}
        isExecutionReadOnly={isExecuting}
        breakpoint={!!(data as any).breakpoint}
        onToggleBreakpoint={handleToggleBreakpoint}
        style={{
          width: `${NODE_TYPE_SIZES[NODE_TYPES.AiAgentNode]?.width || 252}px`,
        }}
      >
        <div className="flex flex-col w-full h-full">
          {/* Header */}
          <div className="flex items-center gap-1.5 px-2 py-1.5 border-b border-border/50">
            {data.stepType && (
              <div className="flex-shrink-0 w-4 h-4 flex items-center justify-center rounded-sm bg-muted/30 [&_svg]:w-2.5 [&_svg]:h-2.5">
                <StepTypeIcon type={data.stepType} />
              </div>
            )}
            <span
              className="text-[11px] font-medium text-foreground truncate flex-1"
              title={data.name || undefined}
            >
              {data.name || (
                <span className="italic text-muted-foreground">
                  Unnamed step
                </span>
              )}
            </span>
            {modelName && (
              <span className="flex-shrink-0 text-[9px] px-1.5 py-0.5 rounded bg-violet-100 text-violet-700 dark:bg-violet-900/40 dark:text-violet-300 font-medium whitespace-nowrap">
                {modelName}
              </span>
            )}
            <div className="flex-shrink-0 w-3 h-3 flex items-center justify-center">
              {renderStatusIcon()}
            </div>
          </div>

          {/* Tools section */}
          <div className="px-2 pt-1.5 pb-1">
            <span className="text-[8px] font-semibold uppercase text-muted-foreground tracking-wider leading-none">
              Tools
            </span>
            {toolNames.map((toolName: string) => {
              const typeLabel = toolTypeLabels[toolName];
              const toolStepNodeId = toolNodeIds[toolName];
              return (
                <div
                  key={toolName}
                  className="flex items-center gap-1.5 h-[22px] cursor-pointer nodrag nopan hover:bg-muted/30 rounded-sm -mx-0.5 px-0.5"
                  onClick={(e) => {
                    e.stopPropagation();
                    // Open the tool step's config dialog (not the AI Agent's)
                    if (toolStepNodeId) {
                      openNodeConfig(toolStepNodeId);
                    } else {
                      // No connected step yet — open AI Agent config
                      openNodeConfig(id);
                    }
                  }}
                >
                  <div
                    className={cn(
                      'w-1.5 h-1.5 rounded-full flex-shrink-0',
                      'bg-violet-500 dark:bg-violet-400'
                    )}
                  />
                  <span className="text-[10px] text-foreground truncate flex-1">
                    {toolName}
                  </span>
                  {typeLabel && (
                    <span className="text-[9px] text-muted-foreground flex-shrink-0">
                      {typeLabel}
                    </span>
                  )}
                </div>
              );
            })}
            {/* Add tool row */}
            {!isExecuting && (
              <div
                className="flex items-center gap-1.5 h-[22px] cursor-pointer nodrag nopan hover:bg-muted/30 rounded-sm -mx-0.5 px-0.5"
                onClick={(e) => {
                  e.stopPropagation();
                  setStepPickerMode('tool');
                }}
              >
                <div className="w-1.5 h-1.5 flex-shrink-0" />
                <span className="text-[10px] text-muted-foreground/60 italic flex-1">
                  Add tool…
                </span>
                <Button
                  className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm pointer-events-none flex-shrink-0"
                  variant="outline"
                  size="icon"
                  tabIndex={-1}
                >
                  <Plus />
                </Button>
              </div>
            )}
          </div>

          {/* Memory section */}
          <div className="px-2 pt-1 pb-1.5 border-t border-border/30">
            <span className="text-[8px] font-semibold uppercase text-muted-foreground tracking-wider leading-none">
              Memory
            </span>
            {hasMemory && (
              <div
                className="flex items-center gap-1.5 h-[22px] cursor-pointer nodrag nopan hover:bg-muted/30 rounded-sm -mx-0.5 px-0.5"
                onClick={(e) => {
                  e.stopPropagation();
                  // Always open AI Agent config — memory settings are managed there
                  openNodeConfig(id);
                }}
              >
                <div
                  className={cn(
                    'w-1.5 h-1.5 rounded-full flex-shrink-0',
                    'bg-blue-500 dark:bg-blue-400'
                  )}
                />
                <span className="text-[10px] text-foreground truncate flex-1">
                  Conversation
                </span>
                <span className="text-[9px] text-muted-foreground flex-shrink-0">
                  {memoryTypeLabel}
                </span>
              </div>
            )}
            {/* Add memory row */}
            {!isExecuting && !hasMemory && (
              <div
                className="flex items-center gap-1.5 h-[22px] cursor-pointer nodrag nopan hover:bg-muted/30 rounded-sm -mx-0.5 px-0.5"
                onClick={(e) => {
                  e.stopPropagation();
                  setStepPickerMode('memory');
                }}
              >
                <div className="w-1.5 h-1.5 flex-shrink-0" />
                <span className="text-[10px] text-muted-foreground/60 italic flex-1">
                  Add memory…
                </span>
                <Button
                  className="w-4 h-4 rounded-full [&_svg]:size-2 shadow-sm pointer-events-none flex-shrink-0"
                  variant="outline"
                  size="icon"
                  tabIndex={-1}
                >
                  <Plus />
                </Button>
              </div>
            )}
          </div>
        </div>

        {/* Only source (next) and target handles */}
        <Handle
          id="source"
          type="source"
          position={Position.Right}
          className="!w-2 !h-2 !rounded-full !bg-muted-foreground/40 !border-0"
          isConnectable={isConnectable && !isExecuting}
        />
        <Handle
          type="target"
          id="target"
          position={Position.Left}
          className="!w-2 !h-2 !rounded-full !bg-muted-foreground/40 !border-0"
          isConnectable={isConnectable && !isExecuting}
        />
      </BaseNode>

      {stepPickerMode && (
        <NodeFormProvider parentNodeId={id}>
          <StepPickerModal
            open={!!stepPickerMode}
            onOpenChange={(open) => {
              if (!open) setStepPickerMode(null);
            }}
            onSelect={
              stepPickerMode === 'tool'
                ? handleAddToolSelect
                : handleAddMemorySelect
            }
            mode={stepPickerMode}
          />
        </NodeFormProvider>
      )}
    </>
  );
}

function arePropsEqual(
  prevProps: NodeProps<AiAgentNodeProps>,
  nextProps: NodeProps<AiAgentNodeProps>
): boolean {
  if (prevProps.id !== nextProps.id) return false;
  if (prevProps.selected !== nextProps.selected) return false;
  if (prevProps.isConnectable !== nextProps.isConnectable) return false;
  if (prevProps.dragging !== nextProps.dragging) return false;
  if (prevProps.parentId !== nextProps.parentId) return false;

  if (prevProps.data !== nextProps.data) {
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
    if (prevData.inputMapping !== nextData.inputMapping) return false;
  }

  return true;
}

export const AiAgentNode = memo(AiAgentNodeComponent, arePropsEqual);
