import { describe, expect, it, beforeEach } from 'vitest';
import { useExecutionStore, NodeExecutionStatus } from './executionStore';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';

// Note: ExecutionStatus enum values are: Queued, Compiling, Running, Completed, Failed, Timeout, Cancelled

describe('executionStore', () => {
  beforeEach(() => {
    // Reset store state before each test
    useExecutionStore.setState({
      executingInstanceId: null,
      workflowId: null,
      debugMode: false,
      instanceStatus: null,
      panelOpen: false,
      nodeExecutionStatus: new Map(),
      statusVersion: 0,
    });
  });

  describe('initial state', () => {
    it('has correct initial values', () => {
      const state = useExecutionStore.getState();

      expect(state.executingInstanceId).toBeNull();
      expect(state.workflowId).toBeNull();
      expect(state.debugMode).toBe(false);
      expect(state.instanceStatus).toBeNull();
      expect(state.panelOpen).toBe(false);
      expect(state.nodeExecutionStatus.size).toBe(0);
      expect(state.statusVersion).toBe(0);
    });
  });

  describe('startExecution', () => {
    it('sets execution state correctly', () => {
      useExecutionStore.getState().startExecution('inst-123', 'scen-456', true);

      const state = useExecutionStore.getState();
      expect(state.executingInstanceId).toBe('inst-123');
      expect(state.workflowId).toBe('scen-456');
      expect(state.debugMode).toBe(true);
      expect(state.instanceStatus).toBe(ExecutionStatus.Queued);
      expect(state.panelOpen).toBe(false);
    });

    it('clears previous node execution status', () => {
      // Set up some existing node status
      useExecutionStore.getState().updateNodeStatus('node-1', {
        status: ExecutionStatus.Completed,
      });

      useExecutionStore
        .getState()
        .startExecution('inst-new', 'scen-new', false);

      expect(useExecutionStore.getState().nodeExecutionStatus.size).toBe(0);
    });

    it('increments status version', () => {
      const initialVersion = useExecutionStore.getState().statusVersion;

      useExecutionStore.getState().startExecution('inst-1', 'scen-1', false);

      expect(useExecutionStore.getState().statusVersion).toBe(
        initialVersion + 1
      );
    });

    it('sets debugMode to false when not debugging', () => {
      useExecutionStore.getState().startExecution('inst-1', 'scen-1', false);

      expect(useExecutionStore.getState().debugMode).toBe(false);
    });
  });

  describe('updateNodeStatus', () => {
    it('updates status for a specific node', () => {
      const status: NodeExecutionStatus = {
        status: ExecutionStatus.Running,
        startedAt: '2024-01-01T00:00:00Z',
      };

      useExecutionStore.getState().updateNodeStatus('node-1', status);

      const nodeStatus = useExecutionStore
        .getState()
        .nodeExecutionStatus.get('node-1');
      expect(nodeStatus?.status).toBe(ExecutionStatus.Running);
      expect(nodeStatus?.startedAt).toBe('2024-01-01T00:00:00Z');
    });

    it('can track multiple nodes', () => {
      useExecutionStore.getState().updateNodeStatus('node-1', {
        status: ExecutionStatus.Completed,
      });
      useExecutionStore.getState().updateNodeStatus('node-2', {
        status: ExecutionStatus.Running,
      });
      useExecutionStore.getState().updateNodeStatus('node-3', {
        status: ExecutionStatus.Failed,
        error: 'Something went wrong',
      });

      const state = useExecutionStore.getState();
      expect(state.nodeExecutionStatus.size).toBe(3);
      expect(state.nodeExecutionStatus.get('node-1')?.status).toBe(
        ExecutionStatus.Completed
      );
      expect(state.nodeExecutionStatus.get('node-2')?.status).toBe(
        ExecutionStatus.Running
      );
      expect(state.nodeExecutionStatus.get('node-3')?.error).toBe(
        'Something went wrong'
      );
    });

    it('increments status version on each update', () => {
      const initialVersion = useExecutionStore.getState().statusVersion;

      useExecutionStore.getState().updateNodeStatus('node-1', {
        status: ExecutionStatus.Running,
      });
      useExecutionStore.getState().updateNodeStatus('node-1', {
        status: ExecutionStatus.Completed,
      });

      expect(useExecutionStore.getState().statusVersion).toBe(
        initialVersion + 2
      );
    });

    it('updates existing node status', () => {
      useExecutionStore.getState().updateNodeStatus('node-1', {
        status: ExecutionStatus.Running,
        startedAt: '2024-01-01T00:00:00Z',
      });

      useExecutionStore.getState().updateNodeStatus('node-1', {
        status: ExecutionStatus.Completed,
        startedAt: '2024-01-01T00:00:00Z',
        completedAt: '2024-01-01T00:01:00Z',
        executionTime: 60000,
      });

      const nodeStatus = useExecutionStore
        .getState()
        .nodeExecutionStatus.get('node-1');
      expect(nodeStatus?.status).toBe(ExecutionStatus.Completed);
      expect(nodeStatus?.completedAt).toBe('2024-01-01T00:01:00Z');
      expect(nodeStatus?.executionTime).toBe(60000);
    });
  });

  describe('updateInstanceStatus', () => {
    it('updates the instance status', () => {
      useExecutionStore.getState().startExecution('inst-1', 'scen-1', false);
      useExecutionStore
        .getState()
        .updateInstanceStatus(ExecutionStatus.Running);

      expect(useExecutionStore.getState().instanceStatus).toBe(
        ExecutionStatus.Running
      );
    });

    it('can set status to failed', () => {
      useExecutionStore.getState().updateInstanceStatus(ExecutionStatus.Failed);

      expect(useExecutionStore.getState().instanceStatus).toBe(
        ExecutionStatus.Failed
      );
    });

    it('can set status to succeeded', () => {
      useExecutionStore
        .getState()
        .updateInstanceStatus(ExecutionStatus.Completed);

      expect(useExecutionStore.getState().instanceStatus).toBe(
        ExecutionStatus.Completed
      );
    });
  });

  describe('resetExecution', () => {
    it('resets all execution state', () => {
      // Set up some state
      useExecutionStore.getState().startExecution('inst-1', 'scen-1', true);
      useExecutionStore.getState().updateNodeStatus('node-1', {
        status: ExecutionStatus.Completed,
      });
      useExecutionStore.getState().setPanelOpen(true);

      // Reset
      useExecutionStore.getState().resetExecution();

      const state = useExecutionStore.getState();
      expect(state.executingInstanceId).toBeNull();
      expect(state.workflowId).toBeNull();
      expect(state.debugMode).toBe(false);
      expect(state.instanceStatus).toBeNull();
      expect(state.panelOpen).toBe(false);
      expect(state.nodeExecutionStatus.size).toBe(0);
    });

    it('increments status version', () => {
      useExecutionStore.getState().startExecution('inst-1', 'scen-1', false);
      const versionBeforeReset = useExecutionStore.getState().statusVersion;

      useExecutionStore.getState().resetExecution();

      expect(useExecutionStore.getState().statusVersion).toBe(
        versionBeforeReset + 1
      );
    });
  });

  describe('setPanelOpen', () => {
    it('opens the panel', () => {
      useExecutionStore.getState().setPanelOpen(true);

      expect(useExecutionStore.getState().panelOpen).toBe(true);
    });

    it('closes the panel', () => {
      useExecutionStore.getState().setPanelOpen(true);
      useExecutionStore.getState().setPanelOpen(false);

      expect(useExecutionStore.getState().panelOpen).toBe(false);
    });
  });

  describe('execution flow', () => {
    it('handles a complete execution lifecycle', () => {
      const store = useExecutionStore.getState();

      // Start execution
      store.startExecution('inst-1', 'scen-1', false);
      expect(useExecutionStore.getState().instanceStatus).toBe(
        ExecutionStatus.Queued
      );

      // Running
      useExecutionStore
        .getState()
        .updateInstanceStatus(ExecutionStatus.Running);
      useExecutionStore.getState().updateNodeStatus('step-1', {
        status: ExecutionStatus.Running,
        startedAt: '2024-01-01T00:00:00Z',
      });

      // Step 1 completes
      useExecutionStore.getState().updateNodeStatus('step-1', {
        status: ExecutionStatus.Completed,
        startedAt: '2024-01-01T00:00:00Z',
        completedAt: '2024-01-01T00:00:30Z',
        executionTime: 30000,
      });

      // Step 2 runs
      useExecutionStore.getState().updateNodeStatus('step-2', {
        status: ExecutionStatus.Running,
        startedAt: '2024-01-01T00:00:30Z',
      });

      // Step 2 completes
      useExecutionStore.getState().updateNodeStatus('step-2', {
        status: ExecutionStatus.Completed,
        startedAt: '2024-01-01T00:00:30Z',
        completedAt: '2024-01-01T00:01:00Z',
        executionTime: 30000,
      });

      // Execution completes
      useExecutionStore
        .getState()
        .updateInstanceStatus(ExecutionStatus.Completed);

      const finalState = useExecutionStore.getState();
      expect(finalState.instanceStatus).toBe(ExecutionStatus.Completed);
      expect(finalState.nodeExecutionStatus.size).toBe(2);
      expect(finalState.nodeExecutionStatus.get('step-1')?.status).toBe(
        ExecutionStatus.Completed
      );
      expect(finalState.nodeExecutionStatus.get('step-2')?.status).toBe(
        ExecutionStatus.Completed
      );
    });
  });
});
