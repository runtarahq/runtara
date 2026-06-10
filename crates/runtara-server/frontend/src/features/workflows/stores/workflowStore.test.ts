import { beforeEach, describe, expect, it } from 'vitest';

import {
  rewriteStepReferencesInString,
  useWorkflowStore,
} from './workflowStore';

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

describe('rewriteStepReferencesInString', () => {
  it('rewrites dot, single-quote bracket, and double-quote bracket forms', () => {
    expect(
      rewriteStepReferencesInString('steps.fetch.outputs.items', 'fetch', 'f2')
    ).toBe('steps.f2.outputs.items');
    expect(
      rewriteStepReferencesInString(
        "steps['fetch'].outputs.items",
        'fetch',
        'f2'
      )
    ).toBe("steps['f2'].outputs.items");
    expect(
      rewriteStepReferencesInString(
        'steps["fetch"].outputs.items',
        'fetch',
        'f2'
      )
    ).toBe('steps["f2"].outputs.items');
  });

  it('rewrites references embedded in template strings', () => {
    expect(
      rewriteStepReferencesInString(
        "Total {{ steps.fetch.outputs.count }} / {{ steps['fetch'].outputs.total }}",
        'fetch',
        'fetch-orders'
      )
    ).toBe(
      "Total {{ steps.fetch-orders.outputs.count }} / {{ steps['fetch-orders'].outputs.total }}"
    );
    // Bare reference at end of template expression (no trailing dot)
    expect(
      rewriteStepReferencesInString('{{ steps.fetch }}', 'fetch', 'f2')
    ).toBe('{{ steps.f2 }}');
  });

  it('is boundary-safe for ids that prefix other ids', () => {
    expect(rewriteStepReferencesInString('steps.ab.outputs.x', 'a', 'z')).toBe(
      'steps.ab.outputs.x'
    );
    expect(
      rewriteStepReferencesInString("steps['ab'].outputs.x", 'a', 'z')
    ).toBe("steps['ab'].outputs.x");
    expect(rewriteStepReferencesInString('steps.a.outputs.x', 'a', 'z')).toBe(
      'steps.z.outputs.x'
    );
  });

  it('does not rewrite inside other identifiers', () => {
    expect(
      rewriteStepReferencesInString('mysteps.a.outputs.x', 'a', 'z')
    ).toBe('mysteps.a.outputs.x');
    expect(
      rewriteStepReferencesInString('data.steps.a.outputs.x', 'a', 'z')
    ).toBe('data.steps.a.outputs.x');
  });
});

describe('workflowStore renameStep', () => {
  beforeEach(() => {
    useWorkflowStore.getState().resetState();
  });

  function addAgentNode(
    id: string,
    extra: Record<string, unknown> = {},
    position = { x: 0, y: 0 },
    parentId?: string
  ) {
    useWorkflowStore.getState().addNode(
      {
        id,
        stepType: 'Agent',
        name: `Step ${id}`,
        agentId: 'utils',
        capabilityId: 'noop',
        ...extra,
      } as never,
      position,
      parentId
    );
  }

  it('re-points edges and container parentId, and renames node id + data.id', () => {
    addAgentNode('start');
    useWorkflowStore.getState().addNode(
      { id: 'split', stepType: 'Split', name: 'Split items' } as never,
      { x: 240, y: 0 }
    );
    addAgentNode('child', {}, { x: 24, y: 24 }, 'split');
    addAgentNode('after', {}, { x: 480, y: 0 });
    useWorkflowStore.getState().addEdge('start', 'split');
    useWorkflowStore.getState().addEdge('split', 'after');

    const error = useWorkflowStore
      .getState()
      .renameStep('split', 'iterate-items');
    expect(error).toBeNull();

    const state = useWorkflowStore.getState();
    expect(state.nodes.some((n) => n.id === 'split')).toBe(false);

    const renamed = state.nodes.find((n) => n.id === 'iterate-items');
    expect(renamed).toBeDefined();
    expect(renamed!.data.id).toBe('iterate-items');

    const child = state.nodes.find((n) => n.id === 'child');
    expect(child!.parentId).toBe('iterate-items');

    const incoming = state.edges.find((e) => e.target === 'iterate-items');
    const outgoing = state.edges.find((e) => e.source === 'iterate-items');
    expect(incoming).toBeDefined();
    expect(incoming!.source).toBe('start');
    expect(outgoing).toBeDefined();
    expect(outgoing!.target).toBe('after');
    // No edge still references the old id
    expect(
      state.edges.some((e) => e.source === 'split' || e.target === 'split')
    ).toBe(false);
    expect(state.isDirty).toBe(true);
    expect(state.isStructurallyDirty).toBe(true);
    expect(state.lastStepRename).toEqual({
      oldId: 'split',
      newId: 'iterate-items',
    });
  });

  it('rewrites dot and bracket references in mapping values, condition arguments, and templates', () => {
    addAgentNode('fetch');
    addAgentNode(
      'consume',
      {
        inputMapping: [
          {
            type: 'items',
            value: "steps['fetch'].outputs.items",
            valueType: 'reference',
          },
          {
            type: 'fallback',
            value: 'steps["fetch"].outputs.items',
            valueType: 'reference',
          },
          {
            type: 'summary',
            value: 'Fetched {{ steps.fetch.outputs.count }} items',
            valueType: 'template',
          },
          {
            type: 'nested',
            valueType: 'composite',
            value: {
              inner: {
                valueType: 'reference',
                value: 'steps.fetch.outputs.first',
              },
            },
          },
        ],
        condition: {
          type: 'operation',
          op: 'EQ',
          arguments: ['steps.fetch.outputs.status', 'ready'],
        },
      },
      { x: 240, y: 0 }
    );
    useWorkflowStore.getState().addEdge('fetch', 'consume');
    const edgeId = useWorkflowStore.getState().edges[0].id;
    useWorkflowStore.getState().updateEdgeData(edgeId, {
      condition: {
        type: 'operation',
        op: 'GT',
        arguments: ["steps['fetch'].outputs.count", 0],
      },
    });

    const error = useWorkflowStore
      .getState()
      .renameStep('fetch', 'fetch-orders');
    expect(error).toBeNull();

    const state = useWorkflowStore.getState();
    const consume = state.nodes.find((n) => n.id === 'consume')!;
    const mapping = consume.data.inputMapping as Array<{
      type: string;
      value: unknown;
    }>;
    expect(mapping.find((m) => m.type === 'items')!.value).toBe(
      "steps['fetch-orders'].outputs.items"
    );
    expect(mapping.find((m) => m.type === 'fallback')!.value).toBe(
      'steps["fetch-orders"].outputs.items'
    );
    expect(mapping.find((m) => m.type === 'summary')!.value).toBe(
      'Fetched {{ steps.fetch-orders.outputs.count }} items'
    );
    expect(
      (
        mapping.find((m) => m.type === 'nested')!.value as {
          inner: { value: string };
        }
      ).inner.value
    ).toBe('steps.fetch-orders.outputs.first');
    expect(
      (consume.data.condition as { arguments: unknown[] }).arguments[0]
    ).toBe('steps.fetch-orders.outputs.status');

    const edge = state.edges.find(
      (e) => e.source === 'fetch-orders' && e.target === 'consume'
    )!;
    expect(
      (edge.data!.condition as { arguments: unknown[] }).arguments[0]
    ).toBe("steps['fetch-orders'].outputs.count");
  });

  it('does not corrupt references to a step whose id has the renamed id as prefix', () => {
    addAgentNode('a');
    addAgentNode('ab', {}, { x: 240, y: 0 });
    addAgentNode(
      'consumer',
      {
        inputMapping: [
          {
            type: 'fromA',
            value: 'steps.a.outputs.x',
            valueType: 'reference',
          },
          {
            type: 'fromAb',
            value: 'steps.ab.outputs.x',
            valueType: 'reference',
          },
          {
            type: 'bracketA',
            value: "steps['a'].outputs.x",
            valueType: 'reference',
          },
          {
            type: 'bracketAb',
            value: "steps['ab'].outputs.x",
            valueType: 'reference',
          },
          {
            type: 'tmpl',
            value: '{{ steps.a.outputs.x }} and {{ steps.ab.outputs.x }}',
            valueType: 'template',
          },
        ],
      },
      { x: 480, y: 0 }
    );

    const error = useWorkflowStore.getState().renameStep('a', 'alpha');
    expect(error).toBeNull();

    const consumer = useWorkflowStore
      .getState()
      .nodes.find((n) => n.id === 'consumer')!;
    const mapping = consumer.data.inputMapping as Array<{
      type: string;
      value: unknown;
    }>;
    expect(mapping.find((m) => m.type === 'fromA')!.value).toBe(
      'steps.alpha.outputs.x'
    );
    expect(mapping.find((m) => m.type === 'fromAb')!.value).toBe(
      'steps.ab.outputs.x'
    );
    expect(mapping.find((m) => m.type === 'bracketA')!.value).toBe(
      "steps['alpha'].outputs.x"
    );
    expect(mapping.find((m) => m.type === 'bracketAb')!.value).toBe(
      "steps['ab'].outputs.x"
    );
    expect(mapping.find((m) => m.type === 'tmpl')!.value).toBe(
      '{{ steps.alpha.outputs.x }} and {{ steps.ab.outputs.x }}'
    );
  });

  it('rejects duplicate, invalid, same, reserved, and unknown ids without mutating state', () => {
    addAgentNode('start');
    addAgentNode('next', {}, { x: 240, y: 0 });
    useWorkflowStore.getState().addEdge('start', 'next');
    useWorkflowStore.getState().clearDirtyFlag();

    const rename = (oldId: string, newId: string) =>
      useWorkflowStore.getState().renameStep(oldId, newId);

    expect(rename('start', 'next')).toMatch(/already exists/);
    expect(rename('start', 'start')).toMatch(/same as the current id/);
    expect(rename('start', '')).toMatch(/cannot be empty/);
    expect(rename('start', 'has space')).toMatch(/may only contain/);
    expect(rename('start', 'steps.foo')).toMatch(/may only contain/);
    expect(rename('start', '__error')).toMatch(/reserved/);
    expect(rename('missing', 'whatever')).toMatch(/was not found/);

    const state = useWorkflowStore.getState();
    expect(state.nodes.map((n) => n.id).sort()).toEqual(['next', 'start']);
    expect(state.edges[0].source).toBe('start');
    expect(state.edges[0].target).toBe('next');
    expect(state.isDirty).toBe(false);
    expect(state.isStructurallyDirty).toBe(false);
    expect(state.lastStepRename).toBeNull();
  });
});
