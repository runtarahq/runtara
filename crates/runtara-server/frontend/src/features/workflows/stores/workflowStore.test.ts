import { beforeEach, describe, expect, it } from 'vitest';

import { useWorkflowStore } from './workflowStore';

describe('workflowStore edge metadata', () => {
  beforeEach(() => {
    useWorkflowStore.getState().resetState();
  });

  it('updates and clears execution-plan edge metadata', () => {
    const store = useWorkflowStore.getState();

    store.addNode(
      {
        id: 'start',
        stepType: 'Agent',
        name: 'Start',
        agentId: 'utils',
        capabilityId: 'noop',
      },
      { x: 0, y: 0 }
    );
    useWorkflowStore.getState().addNode(
      {
        id: 'next',
        stepType: 'Agent',
        name: 'Next',
        agentId: 'utils',
        capabilityId: 'noop',
      },
      { x: 240, y: 0 }
    );
    useWorkflowStore.getState().addEdge('start', 'next');

    const edgeId = useWorkflowStore.getState().edges[0].id;
    useWorkflowStore.getState().updateEdgeData(edgeId, {
      condition: {
        type: 'operation',
        op: 'EQ',
        arguments: ['data.status', 'ready'],
      },
      priority: 10,
    });

    expect(useWorkflowStore.getState().edges[0].data).toEqual({
      condition: {
        type: 'operation',
        op: 'EQ',
        arguments: ['data.status', 'ready'],
      },
      priority: 10,
    });
    expect(useWorkflowStore.getState().isDirty).toBe(true);
    expect(useWorkflowStore.getState().isStructurallyDirty).toBe(true);

    useWorkflowStore.getState().updateEdgeData(edgeId, {
      condition: undefined,
      priority: undefined,
    });

    expect(useWorkflowStore.getState().edges[0].data).toBeUndefined();
  });
});
