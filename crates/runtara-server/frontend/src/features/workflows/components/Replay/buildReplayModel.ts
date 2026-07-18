/**
 * Pure builder that folds recorded step summaries + the versioned graph into a
 * normalized {@link ReplayModel}. No React, no network — unit-tested with fixtures.
 *
 * Data source: step **summaries** (paired start/end rows), the same source the
 * timeline uses, so both views project one model. Each summary yields a
 * `[startT, endT]` interval; overlapping intervals reproduce the real parallel
 * structure exactly.
 */
import {
  INSTANT_THRESHOLD_MS,
  MIN_VISUAL_MS,
  type ReplayEdge,
  type ReplayGraphNode,
  type ReplayModel,
  type ReplayNodeState,
  type ReplayStepInstance,
} from './types';

/** Minimal shape of a recorded step summary (subset of StepSummaryResponse). */
export interface StepSummaryLike {
  stepId: string;
  stepName?: string | null;
  stepType: string;
  scopeId?: string | null;
  parentScopeId?: string | null;
  /** ISO 8601 start timestamp. */
  startedAt: string;
  /** ISO 8601 completion timestamp (null while running). */
  completedAt?: string | null;
  durationMs?: number | null;
  /**
   * Real launch/settle wall-clock (epoch ms) of a parallel branch's async work.
   * Present only for concurrent steps; when both are set they OVERLAP across
   * siblings, so we prefer `[launchedAtMs, settledAtMs]` as the interval over the
   * sequential assemble-order `startedAt`/`durationMs`. See `intervalOf`.
   */
  launchedAtMs?: number | null;
  settledAtMs?: number | null;
  status: string;
  error?: unknown;
  inputs?: unknown;
  outputs?: unknown;
}

/** Minimal shape of the versioned graph (top-level step nodes + transitions). */
export interface ReplayGraphInput {
  nodes: ReplayGraphNode[];
  edges: Array<{
    id: string;
    source: string;
    target: string;
    sourceHandle?: string | null;
  }>;
}

/** Normalize a recorded status string into one of the terminal replay states. */
export function normalizeStatus(status: string): ReplayNodeState {
  switch (status) {
    case 'completed':
      return 'done';
    case 'running':
    case 'queued':
      return 'running';
    case 'suspended':
      return 'suspended';
    case 'failed':
    case 'timeout':
    case 'cancelled':
    case 'error':
      return 'failed';
    default:
      return 'done';
  }
}

function parseMs(iso: string | null | undefined): number | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isNaN(t) ? null : t;
}

/**
 * Absolute `[startAbs, endAbs]` wall-clock (epoch ms) for a recorded step.
 *
 * Prefers the real parallel-branch launch/settle pair when BOTH are present:
 * those describe when the branch's async work actually ran, and OVERLAP across
 * sibling branches, so overlapping intervals become concurrent glow. Otherwise
 * falls back to the sequential assemble-order `startedAt` (+ `completedAt` /
 * `durationMs`) — which is when the step was recorded, not when it ran, and
 * cascades for parallel branches. Returns null when there is no usable start.
 */
function intervalOf(s: StepSummaryLike): { startAbs: number; endAbs: number } | null {
  if (
    s.launchedAtMs != null &&
    s.settledAtMs != null &&
    s.launchedAtMs > 0 &&
    s.settledAtMs > 0
  ) {
    return { startAbs: s.launchedAtMs, endAbs: Math.max(s.settledAtMs, s.launchedAtMs) };
  }
  const startAbs = parseMs(s.startedAt);
  if (startAbs == null) return null;
  const endAbs = parseMs(s.completedAt) ?? startAbs + Math.max(0, s.durationMs ?? 0);
  return { startAbs, endAbs: Math.max(endAbs, startAbs) };
}

/**
 * Detect loop/back edges: an edge whose target can reach its source via forward
 * edges (a cycle). Such edges are drawn but excluded from dagre layering so the
 * graph still lays out left→right. Uses a DFS reachability check per edge.
 */
function markBackEdges(
  nodeIds: string[],
  edges: Array<{ id: string; source: string; target: string }>
): Set<string> {
  const forward = new Map<string, string[]>();
  for (const id of nodeIds) forward.set(id, []);
  for (const e of edges) {
    if (!forward.has(e.source)) forward.set(e.source, []);
    forward.get(e.source)!.push(e.target);
  }
  const backEdgeIds = new Set<string>();
  // An edge u->v is a back edge if v can already reach u (ignoring this edge),
  // i.e. adding u->v closes a cycle. Process edges in order; treat an edge as a
  // back edge when its target reaches its source through edges seen as forward.
  const reaches = (from: string, to: string, skipEdgeId: string): boolean => {
    const seen = new Set<string>();
    const stack = [from];
    while (stack.length) {
      const cur = stack.pop()!;
      if (cur === to) return true;
      if (seen.has(cur)) continue;
      seen.add(cur);
      for (const e of edges) {
        if (e.id === skipEdgeId) continue;
        if (backEdgeIds.has(e.id)) continue;
        if (e.source === cur) stack.push(e.target);
      }
    }
    return false;
  };
  for (const e of edges) {
    if (e.source === e.target || reaches(e.target, e.source, e.id)) {
      backEdgeIds.add(e.id);
    }
  }
  return backEdgeIds;
}

/** Step types whose in-flight execution is what parks a suspended run. */
const DURABLE_WAIT_STEP_TYPES = new Set(['WaitForSignal', 'Delay', 'Wait']);

export function buildReplayModel(
  summaries: StepSummaryLike[],
  graph: ReplayGraphInput,
  options: { instanceStatus?: string } = {}
): ReplayModel {
  // A parked run records its durable-wait step as `running` (never `suspended`),
  // and suspension is an instance-level state. So when the instance is suspended,
  // reclassify a still-running WaitForSignal/Delay as the parked (amber) node.
  const instanceSuspended = options.instanceStatus === 'suspended';
  const nodes = new Map<string, ReplayGraphNode>();
  const nodeIds: string[] = [];
  for (const n of graph.nodes) {
    if (nodes.has(n.id)) continue;
    nodes.set(n.id, n);
    nodeIds.push(n.id);
  }

  const backEdgeIds = markBackEdges(
    nodeIds,
    graph.edges.map((e) => ({ id: e.id, source: e.source, target: e.target }))
  );
  const edges: ReplayEdge[] = graph.edges.map((e) => ({
    id: e.id,
    source: e.source,
    target: e.target,
    sourceHandle: e.sourceHandle,
    isBackEdge: backEdgeIds.has(e.id),
  }));

  // Absolute-time pass: gather starts/ends (real launch/settle when present,
  // else assemble timing), find t0.
  const raw = summaries
    .map((s) => {
      const iv = intervalOf(s);
      if (iv == null) return null;
      return { s, startAbs: iv.startAbs, endAbs: iv.endAbs };
    })
    .filter((x): x is NonNullable<typeof x> => x != null);

  const hasEvents = raw.length > 0;
  const t0 = hasEvents ? Math.min(...raw.map((r) => r.startAbs)) : 0;

  const instances: ReplayStepInstance[] = raw.map(({ s, startAbs, endAbs }, idx) => {
    const startT = startAbs - t0;
    const rawEndT = endAbs - t0;
    const endT = Math.max(rawEndT, startT + MIN_VISUAL_MS);
    let status = normalizeStatus(s.status);
    if (
      instanceSuspended &&
      status === 'running' &&
      DURABLE_WAIT_STEP_TYPES.has(s.stepType)
    ) {
      status = 'suspended';
    }
    return {
      // Unique per recorded execution — several steps can share one scope
      // (Split's it/itf, a While's repeated steps), so the scope alone is not
      // unique. The source index guarantees distinctness.
      key: `${s.scopeId ?? 'root'}::${s.stepId}::${idx}`,
      stepId: s.stepId,
      stepName: s.stepName ?? s.stepId,
      stepType: s.stepType,
      scopeId: s.scopeId ?? null,
      parentScopeId: s.parentScopeId ?? null,
      startT,
      endT,
      rawEndT,
      status,
      isInstant: rawEndT - startT < INSTANT_THRESHOLD_MS,
    };
  });
  instances.sort((a, b) => a.startT - b.startT || a.endT - b.endT);

  const rawTEnd = hasEvents ? Math.max(...instances.map((i) => i.rawEndT)) : 0;
  const tEnd = hasEvents ? Math.max(...instances.map((i) => i.endT)) : 0;

  // Index instances by the graph node they belong to.
  const nodeIdSet = new Set(nodeIds);
  const instancesByStep = new Map<string, ReplayStepInstance[]>();
  for (const inst of instances) {
    if (!nodeIdSet.has(inst.stepId)) continue; // nested/subgraph steps handled below
    const list = instancesByStep.get(inst.stepId) ?? [];
    list.push(inst);
    instancesByStep.set(inst.stepId, list);
  }

  // Scope tree for iteration counters: scopeId -> parentScopeId.
  const scopeParent = new Map<string, string | null>();
  for (const inst of instances) {
    if (inst.scopeId && !scopeParent.has(inst.scopeId)) {
      scopeParent.set(inst.scopeId, inst.parentScopeId);
    }
  }
  const isDescendantScope = (scope: string, ancestor: string): boolean => {
    let cur: string | null | undefined = scope;
    const guard = new Set<string>();
    while (cur && !guard.has(cur)) {
      if (cur === ancestor) return true;
      guard.add(cur);
      cur = scopeParent.get(cur) ?? null;
      if (cur === ancestor) return true;
    }
    return false;
  };

  // Associate each composite node with its nested executions. Iteration scopes
  // are named by convention — `sc_<stepId>` (While: one shared scope, repeated
  // steps) or `sc_<stepId>_<n>` (Split: one scope per iteration) — and their
  // `parentScopeId` is null, so a scope-tree walk can't find them. Match by that
  // convention, and also union in any true descendants (nested composites where
  // parentScopeId IS threaded) so both shapes are covered.
  const childInstancesByStep = new Map<string, ReplayStepInstance[]>();
  for (const nodeId of nodeIds) {
    const own = instancesByStep.get(nodeId);
    if (!own || own.length === 0) continue;
    const ownScope = own.find((i) => i.scopeId)?.scopeId ?? nodeId;
    const namePrefix = `sc_${nodeId}`;
    const seen = new Set<string>();
    const children: ReplayStepInstance[] = [];
    for (const i of instances) {
      if (!i.scopeId || i.scopeId === ownScope) continue;
      const byName =
        i.scopeId === namePrefix || i.scopeId.startsWith(`${namePrefix}_`);
      const byTree = ownScope !== nodeId && isDescendantScope(i.scopeId, ownScope);
      if ((byName || byTree) && !seen.has(i.key)) {
        seen.add(i.key);
        children.push(i);
      }
    }
    if (children.length > 0) childInstancesByStep.set(nodeId, children);
  }

  return {
    nodeIds,
    nodes,
    edges,
    instances,
    instancesByStep,
    childInstancesByStep,
    t0,
    tEnd,
    rawTEnd,
    hasEvents,
  };
}
