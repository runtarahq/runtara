/**
 * Pure projection: given a {@link ReplayModel} and a model time `t`, compute
 * each graph node's visual state, per-composite iteration counts, and the set of
 * "flowing" edges. Both the Graph and Timeline views render from this. No React.
 *
 * Concurrency falls out for free: two nodes whose recorded intervals overlap are
 * both `running` at the same `t`.
 */
import {
  EDGE_FLOW_MS,
  type ReplayFrame,
  type ReplayIterationCounts,
  type ReplayModel,
  type ReplayNodeState,
  type ReplayStepInstance,
} from './types';

/** Aggregate the state of one node from its recorded executions at time `t`. */
function nodeStateAt(insts: ReplayStepInstance[] | undefined, t: number): ReplayNodeState {
  if (!insts || insts.length === 0) return 'skipped';

  let anyStarted = false;
  let anyRunning = false;
  let allReached = true;
  let anyFailed = false;
  let anySuspended = false;
  let allDone = true;

  for (const inst of insts) {
    if (t < inst.startT) {
      allReached = false;
      continue;
    }
    anyStarted = true;
    if (t < inst.endT) {
      anyRunning = true;
      allDone = false;
      continue;
    }
    // reached its end
    switch (inst.status) {
      case 'failed':
        anyFailed = true;
        allDone = false;
        break;
      case 'suspended':
        anySuspended = true;
        allDone = false;
        break;
      case 'done':
        break;
      default:
        // recorded as still-running (crashed without a terminal end)
        allDone = false;
        break;
    }
  }

  if (!anyStarted) return 'idle';
  // A failed/suspended terminal wins even while a sibling execution still runs,
  // so a partially-failed composite reads as failed rather than perpetually busy.
  if (anyFailed) return 'failed';
  if (anySuspended && !anyRunning) return 'suspended';
  if (anyRunning || !allReached) return 'running';
  if (allDone) return 'done';
  return 'running';
}

/** Min start / max end across a node's own executions (for edge timing). */
function windowOf(insts: ReplayStepInstance[] | undefined): { start: number; end: number } | null {
  if (!insts || insts.length === 0) return null;
  let start = Infinity;
  let end = -Infinity;
  for (const i of insts) {
    if (i.startT < start) start = i.startT;
    if (i.endT > end) end = i.endT;
  }
  return { start, end };
}

/**
 * Partition a composite's nested executions into iteration buckets. Handles both
 * shapes: Split (one scope per iteration → one bucket per scope) and While (one
 * shared scope with steps repeated per pass → split by per-stepId occurrence).
 */
function iterationBuckets(children: ReplayStepInstance[]): ReplayStepInstance[][] {
  const byScope = new Map<string, ReplayStepInstance[]>();
  for (const c of children) {
    const scope = c.scopeId ?? c.key;
    const list = byScope.get(scope) ?? [];
    list.push(c);
    byScope.set(scope, list);
  }
  const buckets: ReplayStepInstance[][] = [];
  for (const group of byScope.values()) {
    const sorted = [...group].sort((a, b) => a.startT - b.startT);
    const occ = new Map<string, number>();
    const rounds: ReplayStepInstance[][] = [];
    for (const inst of sorted) {
      const round = occ.get(inst.stepId) ?? 0;
      occ.set(inst.stepId, round + 1);
      (rounds[round] ??= []).push(inst);
    }
    buckets.push(...rounds);
  }
  return buckets;
}

function iterationCountsAt(
  model: ReplayModel,
  nodeId: string,
  t: number
): ReplayIterationCounts | null {
  const children = model.childInstancesByStep.get(nodeId);
  if (!children || children.length === 0) return null;

  const buckets = iterationBuckets(children);
  let active = 0;
  let completed = 0;
  for (const members of buckets) {
    const started = members.some((m) => t >= m.startT);
    const running = members.some((m) => t >= m.startT && t < m.endT);
    const allEnded = members.every((m) => t >= m.endT);
    if (running) active += 1;
    else if (started && allEnded) completed += 1;
  }
  return { total: buckets.length, active, completed };
}

export function deriveFrame(model: ReplayModel, t: number): ReplayFrame {
  const nodeStates = new Map<string, ReplayNodeState>();
  const nodeIterations = new Map<string, ReplayIterationCounts>();
  let runningCount = 0;

  for (const nodeId of model.nodeIds) {
    const state = nodeStateAt(model.instancesByStep.get(nodeId), t);
    nodeStates.set(nodeId, state);
    if (state === 'running') runningCount += 1;

    const iters = iterationCountsAt(model, nodeId, t);
    if (iters) nodeIterations.set(nodeId, iters);
  }

  const activeEdges = new Set<string>();
  for (const edge of model.edges) {
    const vWin = windowOf(model.instancesByStep.get(edge.target));
    if (!vWin) continue; // target never ran → no flow
    const uWin = windowOf(model.instancesByStep.get(edge.source));
    if (!uWin) continue; // source never ran → no flow
    const lo = Math.min(uWin.end, vWin.start);
    const hi = vWin.start + EDGE_FLOW_MS;
    if (t >= lo && t <= hi) activeEdges.add(edge.id);
  }

  return { t, nodeStates, nodeIterations, activeEdges, runningCount };
}
