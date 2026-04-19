import { Edge } from '@xyflow/react';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore';
import {
  useExecutionStore,
  NodeExecutionStatus,
} from '@/features/workflows/stores/executionStore';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';

/**
 * Reset all stores to clean default state.
 * Call this at the start of each story to ensure isolation.
 */
export function resetStores() {
  useWorkflowStore.setState({
    nodes: [],
    edges: [],
    stagedNodeIds: new Set<string>(),
    stepsWithErrors: new Set<string>(),
    validationErrors: [],
  });

  useExecutionStore.setState({
    executingInstanceId: null,
    nodeExecutionStatus: new Map<string, NodeExecutionStatus>(),
    instanceStatus: null,
    workflowId: null,
    debugMode: false,
  });
}

/**
 * Set edges in the workflow store.
 * Needed for nodes that check connection status (e.g., to hide "+" buttons).
 */
export function setStoreEdges(edges: Edge[]) {
  useWorkflowStore.setState({ edges });
}

/**
 * Mark a node as having unsaved changes.
 */
export function setNodeUnsaved(nodeId: string) {
  const current = useWorkflowStore.getState();
  useWorkflowStore.setState({
    stagedNodeIds: new Set([...current.stagedNodeIds, nodeId]),
  });
}

/**
 * Mark a node as having a validation error.
 */
export function setNodeValidationError(nodeId: string) {
  const current = useWorkflowStore.getState();
  useWorkflowStore.setState({
    stepsWithErrors: new Set([...current.stepsWithErrors, nodeId]),
  });
}

/**
 * Set execution status for a specific node.
 */
export function setNodeExecutionStatus(
  nodeId: string,
  status: ExecutionStatus,
  opts?: { executionTime?: number; error?: string }
) {
  const current = useExecutionStore.getState();
  const newMap = new Map(current.nodeExecutionStatus);
  newMap.set(nodeId, {
    status,
    executionTime: opts?.executionTime,
    error: opts?.error,
  });
  useExecutionStore.setState({
    nodeExecutionStatus: newMap,
  });
}

/**
 * Set the executing instance ID (puts workflow in read-only mode).
 */
export function setExecuting(instanceId: string = 'story-instance-123') {
  useExecutionStore.setState({
    executingInstanceId: instanceId,
  });
}
