import type { Node } from '@xyflow/react';
import { canvasNodeStepId } from '@/features/workflows/components/WorkflowEditor/CustomNodes/utils.tsx';

type CanvasNode = Pick<Node, 'id' | 'parentId' | 'data'>;

/** Container step types whose iterations open a runtime scope. */
const SCOPED_CONTAINER_TYPES = new Set(['Split', 'While']);

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Matcher for the `scope_id` a step's runtime events carry. The runtime opens
 * a scope per Split/While iteration — `sc_<split>_<i>`, nested as
 * `<parent scope>_<inner>_<j>` — and a WaitForSignal `onWait` graph runs in
 * its parent's scope. A root step's events carry no scope.
 */
function scopeMatcher(
  node: CanvasNode,
  nodesById: Map<string, CanvasNode>
): (scopeId: string | null | undefined) => boolean {
  const chain: string[] = [];
  for (
    let container = node.parentId ? nodesById.get(node.parentId) : undefined;
    container;
    container = container.parentId
      ? nodesById.get(container.parentId)
      : undefined
  ) {
    const stepType = (container.data as { stepType?: string } | undefined)
      ?.stepType;
    if (stepType && SCOPED_CONTAINER_TYPES.has(stepType)) {
      chain.unshift(canvasNodeStepId(container.id));
    }
  }

  if (chain.length === 0) {
    return (scopeId) => !scopeId;
  }
  const pattern = new RegExp(
    `^sc_${chain.map((id) => `${escapeRegExp(id)}_\\d+`).join('_')}$`
  );
  return (scopeId) => typeof scopeId === 'string' && pattern.test(scopeId);
}

/**
 * Map a runtime step id to the canvas node it ran as.
 *
 * Step ids are unique per graph, so two subgraphs may share one, and the
 * canvas then gives the deeper step a qualified node id. Events carry the
 * step id plus the `scope_id` of the iteration that ran it; matching that
 * scope against each candidate's container chain picks the right node. A
 * step id used by only one node resolves without looking at the scope.
 */
export function createStepNodeResolver(nodes: CanvasNode[]) {
  const nodesById = new Map(nodes.map((node) => [node.id, node]));
  const candidatesByStepId = new Map<string, CanvasNode[]>();
  for (const node of nodes) {
    const stepId = canvasNodeStepId(node.id);
    const candidates = candidatesByStepId.get(stepId);
    if (candidates) {
      candidates.push(node);
    } else {
      candidatesByStepId.set(stepId, [node]);
    }
  }
  const matchers = new Map<
    string,
    (scopeId: string | null | undefined) => boolean
  >();

  return (stepId: string, scopeId?: string | null): string => {
    const candidates = candidatesByStepId.get(stepId);
    if (!candidates || candidates.length === 0) {
      return stepId;
    }
    if (candidates.length === 1) {
      return candidates[0].id;
    }
    for (const candidate of candidates) {
      let matches = matchers.get(candidate.id);
      if (!matches) {
        matches = scopeMatcher(candidate, nodesById);
        matchers.set(candidate.id, matches);
      }
      if (matches(scopeId)) {
        return candidate.id;
      }
    }
    // No scope to go on (older events): keep the plain-id node.
    return stepId;
  };
}

/** Real step ids of every container (Split, While, WaitForSignal) holding `node`, outermost first. */
function containerPath(
  node: CanvasNode,
  nodesById: Map<string, CanvasNode>
): string[] {
  const path: string[] = [];
  for (
    let container = node.parentId ? nodesById.get(node.parentId) : undefined;
    container;
    container = container.parentId
      ? nodesById.get(container.parentId)
      : undefined
  ) {
    path.unshift(canvasNodeStepId(container.id));
  }
  return path;
}

function samePath(a: string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((id, index) => id === b[index]);
}

/**
 * Map a validator finding to the canvas node it is about.
 *
 * The validator names a step by its DSL id or display name plus the graph
 * path of the body holding it (container step ids, outermost first; absent
 * for the root). Matching that path tells same-named steps in different
 * bodies apart. A step matched by only one node resolves without the path.
 */
export function createGraphNodeResolver(nodes: CanvasNode[]) {
  const nodesById = new Map(nodes.map((node) => [node.id, node]));
  const paths = new Map<string, string[]>();
  const pathOf = (node: CanvasNode) => {
    let path = paths.get(node.id);
    if (!path) {
      path = containerPath(node, nodesById);
      paths.set(node.id, path);
    }
    return path;
  };

  return (
    stepIdOrName: string,
    graphPath?: readonly string[] | null
  ): string | undefined => {
    const lowered = stepIdOrName.toLowerCase();
    const candidates = nodes.filter(
      (node) =>
        canvasNodeStepId(node.id) === stepIdOrName ||
        (typeof node.data?.name === 'string' &&
          node.data.name.toLowerCase() === lowered)
    );
    if (candidates.length <= 1) {
      return candidates[0]?.id;
    }
    const inGraph = candidates.find((node) =>
      samePath(pathOf(node), graphPath ?? [])
    );
    if (inGraph) {
      return inGraph.id;
    }
    // No path to go on: prefer the node keeping the plain step id.
    return (
      candidates.find((node) => node.id === stepIdOrName)?.id ??
      candidates[0].id
    );
  };
}
