import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';
import { enableMapSet } from 'immer';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';

// Enable Map and Set support in Immer
enableMapSet();

export interface NodeExecutionStatus {
  status: ExecutionStatus;
  startedAt?: string;
  completedAt?: string;
  executionTime?: number;
  error?: string;
}

export interface BreakpointHitInfo {
  stepId: string;
  stepName: string;
  stepType: string;
  /** Resolved inputs the step is about to process (null for Delay/WaitForSignal/AiAgent) */
  inputs?: any;
  stepsContext: Record<
    string,
    { stepId: string; stepName?: string; stepType?: string; outputs: any }
  >;
}

export interface StepDebugData {
  inputs?: any;
  outputs?: any;
  durationMs?: number;
  error?: string;
}

interface ExecutionState {
  // Core execution state
  executingInstanceId: string | null;
  workflowId: string | null;
  debugMode: boolean;
  instanceStatus: ExecutionStatus | null;
  panelOpen: boolean;

  // Debug execution mode (breakpoints active)
  isDebugExecution: boolean;
  isSuspended: boolean;
  breakpointHit: BreakpointHitInfo | null;

  // Selected invocation in the panel (for viewing history)
  selectedInvocationId: string | null;

  // Node-level execution status (nodeId -> status)
  nodeExecutionStatus: Map<string, NodeExecutionStatus>;

  // Per-step debug data (inputs/outputs) collected from step events
  stepDebugData: Map<string, StepDebugData>;

  // Version counter to force re-renders
  statusVersion: number;

  // Actions
  startExecution: (
    instanceId: string,
    workflowId: string,
    debugMode: boolean,
    isDebugExecution?: boolean
  ) => void;
  updateNodeStatus: (nodeId: string, status: NodeExecutionStatus) => void;
  updateInstanceStatus: (status: ExecutionStatus) => void;
  setSuspended: (
    suspended: boolean,
    breakpointHit?: BreakpointHitInfo | null
  ) => void;
  setStepDebugData: (stepId: string, data: StepDebugData) => void;
  resetExecution: () => void;
  setPanelOpen: (open: boolean) => void;
  setSelectedInvocationId: (id: string | null) => void;
}

export const useExecutionStore = create<ExecutionState>()(
  devtools(
    immer((set) => ({
      // Initial state
      executingInstanceId: null,
      workflowId: null,
      debugMode: false,
      instanceStatus: null,
      panelOpen: false,
      isDebugExecution: false,
      isSuspended: false,
      breakpointHit: null,
      selectedInvocationId: null,
      nodeExecutionStatus: new Map(),
      stepDebugData: new Map(),
      statusVersion: 0,

      // Actions
      startExecution: (instanceId, workflowId, debugMode, isDebugExecution) => {
        set((state) => {
          state.executingInstanceId = instanceId;
          state.workflowId = workflowId;
          state.debugMode = debugMode;
          state.isDebugExecution = isDebugExecution || false;
          state.isSuspended = false;
          state.breakpointHit = null;
          state.panelOpen = false; // Don't open automatically - user must click "View Details"
          state.nodeExecutionStatus = new Map();
          state.stepDebugData = new Map();
          state.instanceStatus = ExecutionStatus.Queued;
          state.statusVersion += 1; // Increment to clear previous execution
        });
      },

      updateNodeStatus: (nodeId, status) => {
        set((state) => {
          state.nodeExecutionStatus.set(nodeId, status);
          state.statusVersion += 1;
        });
      },

      updateInstanceStatus: (status) => {
        set((state) => {
          state.instanceStatus = status;
        });
      },

      setSuspended: (suspended, breakpointHit) => {
        set((state) => {
          state.isSuspended = suspended;
          state.breakpointHit = breakpointHit ?? null;
          state.statusVersion += 1;
        });
      },

      setStepDebugData: (stepId, data) => {
        set((state) => {
          const existing = state.stepDebugData.get(stepId);
          state.stepDebugData.set(stepId, { ...existing, ...data });
        });
      },

      resetExecution: () => {
        set((state) => {
          state.executingInstanceId = null;
          state.workflowId = null;
          state.debugMode = false;
          state.isDebugExecution = false;
          state.isSuspended = false;
          state.breakpointHit = null;
          state.instanceStatus = null;
          state.panelOpen = false;
          state.selectedInvocationId = null;
          state.nodeExecutionStatus = new Map();
          state.stepDebugData = new Map();
          state.statusVersion += 1; // Increment to force re-render
        });
      },

      setPanelOpen: (open) => {
        set((state) => {
          state.panelOpen = open;
        });
      },

      setSelectedInvocationId: (id) => {
        set((state) => {
          state.selectedInvocationId = id;
        });
      },
    })),
    { name: 'execution-store' }
  )
);
