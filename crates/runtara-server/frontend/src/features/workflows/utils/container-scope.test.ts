import { describe, expect, it } from 'vitest';
import {
  resolveContainerScope,
  splitItemSchemaFieldsFromScope,
  type ScopeNode,
} from './container-scope';

const SPLIT_SCHEMA = {
  sku: { type: 'string', required: true, description: 'Item SKU' },
  qty: { type: 'integer', required: false, description: '' },
};

/**
 * Store-node tree: workflow > split(has iteration schema) > while > agent,
 * plus a top-level sibling after the split.
 */
const NODES: ScopeNode[] = [
  { id: 'split', data: { stepType: 'Split', inputSchema: SPLIT_SCHEMA } },
  { id: 'while', parentId: 'split', data: { stepType: 'While' } },
  { id: 'agent', parentId: 'while', data: { stepType: 'Agent' } },
  { id: 'sibling', data: { stepType: 'Agent' } },
  { id: 'wait', data: { stepType: 'WaitForSignal' } },
  { id: 'waitChild', parentId: 'wait', data: { stepType: 'Agent' } },
];

describe('resolveContainerScope', () => {
  it('is top-level (workflow scope) when there is no container', () => {
    const scope = resolveContainerScope(NODES, undefined);
    expect(scope.dataScope.kind).toBe('workflow');
    expect(scope.isInsideSplit).toBe(false);
    expect(scope.isInsideWhileLoop).toBe(false);
    expect(scope.isInsideWaitScope).toBe(false);
  });

  it('detects a direct Split body', () => {
    const scope = resolveContainerScope(NODES, 'split');
    expect(scope.dataScope.kind).toBe('split');
    expect(scope.isInsideSplit).toBe(true);
  });

  it('carries Split scope transitively through a While body', () => {
    // The agent is inside While inside Split; the runtime passes the Split
    // data scope through the While body (DataScope::for_while_body).
    const scope = resolveContainerScope(NODES, 'while');
    expect(scope.dataScope.kind).toBe('split');
    expect(scope.isInsideSplit).toBe(true);
    // The immediate container is the While, so loop.* is in scope.
    expect(scope.isInsideWhileLoop).toBe(true);
    if (scope.dataScope.kind === 'split') {
      expect(scope.dataScope.splitNode.id).toBe('split');
    }
  });

  it('is workflow scope for a top-level sibling of a Split', () => {
    // The bug this replaces: adding a step AFTER a Split classified it as
    // inside the Split. A sibling has no container.
    const scope = resolveContainerScope(NODES, undefined);
    expect(scope.isInsideSplit).toBe(false);
  });

  it('marks onWait as an unknown (undeclared) data scope', () => {
    const scope = resolveContainerScope(NODES, 'wait');
    expect(scope.dataScope.kind).toBe('unknown');
    expect(scope.isInsideWaitScope).toBe(true);
    expect(scope.isInsideSplit).toBe(false);
  });

  it('does not loop on a cyclic parentId chain', () => {
    const cyclic: ScopeNode[] = [
      { id: 'a', parentId: 'b', data: { stepType: 'While' } },
      { id: 'b', parentId: 'a', data: { stepType: 'While' } },
    ];
    // Terminates (no hang); an all-While chain passes the data scope through
    // to the workflow root.
    const scope = resolveContainerScope(cyclic, 'a');
    expect(scope.dataScope.kind).toBe('workflow');
    expect(scope.isInsideWhileLoop).toBe(true);
  });
});

describe('splitItemSchemaFieldsFromScope', () => {
  it('returns the parsed iteration schema for a Split scope', () => {
    const scope = resolveContainerScope(NODES, 'while');
    const fields = splitItemSchemaFieldsFromScope(scope);
    expect(fields?.map((f) => f.name)).toEqual(['sku', 'qty']);
    expect(fields?.find((f) => f.name === 'sku')?.type).toBe('string');
    expect(fields?.find((f) => f.name === 'qty')?.type).toBe('integer');
  });

  it('returns undefined outside a Split scope', () => {
    expect(
      splitItemSchemaFieldsFromScope(resolveContainerScope(NODES, undefined))
    ).toBeUndefined();
  });

  it('returns undefined when the Split declares no schema', () => {
    const nodes: ScopeNode[] = [
      { id: 'split', data: { stepType: 'Split' } },
      { id: 'child', parentId: 'split', data: { stepType: 'Agent' } },
    ];
    expect(
      splitItemSchemaFieldsFromScope(resolveContainerScope(nodes, 'split'))
    ).toBeUndefined();
  });
});
