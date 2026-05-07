import type { Edge } from '@xyflow/react';

export function isSwitchCaseHandle(handleId?: string | null): boolean {
  return Boolean(handleId?.startsWith('case-'));
}

export function getCaseEdgeLabel(handleId?: string | null): string {
  if (!isSwitchCaseHandle(handleId)) return '';
  const caseIndex = Number.parseInt(handleId!.split('-')[1], 10);
  return Number.isFinite(caseIndex) ? `Case ${caseIndex + 1}` : '';
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
