import type { Edge } from '@xyflow/react';

export function isSwitchCaseHandle(handleId?: string | null): boolean {
  return Boolean(handleId?.startsWith('case-'));
}

export function hasVisiblePortLabel(edge: Pick<Edge, 'sourceHandle'>): boolean {
  return (
    isSwitchCaseHandle(edge.sourceHandle) || edge.sourceHandle === 'default'
  );
}

export function shouldHideDuplicateEdgeLabel(
  edge: Pick<Edge, 'sourceHandle'>
): boolean {
  return hasVisiblePortLabel(edge);
}
