/**
 * Where a new parallel branch rejoins.
 *
 * A parallel step N next to an existing route S → T is wired S → N → J, where
 * J is the step the branches re-converge on (the validator requires the
 * fan-out to re-converge, E073). For a step with a single route J is that
 * route's target. Once S already fans out — S → B, S → B1, both reaching C —
 * J is C, not the branch target, and adding a third branch must go S → N → C.
 * Before this, the parallel action was only offered on a single route, so a
 * third branch could not be added from the canvas at all.
 */

export interface RouteEdge {
  source: string;
  target: string;
  sourceHandle?: string | null;
}

/** Targets of a step's unconditional `source` routes, in order, unique. */
export function unconditionalTargets(
  edges: RouteEdge[],
  source: string,
  isUnconditional: (edge: RouteEdge) => boolean
): string[] {
  const targets: string[] = [];
  for (const edge of edges) {
    if (
      edge.source === source &&
      (edge.sourceHandle || 'source') === 'source' &&
      isUnconditional(edge) &&
      !targets.includes(edge.target)
    ) {
      targets.push(edge.target);
    }
  }
  return targets;
}

function distancesFrom(start: string, edges: RouteEdge[]): Map<string, number> {
  const distances = new Map<string, number>([[start, 0]]);
  const queue = [start];
  while (queue.length > 0) {
    const current = queue.shift()!;
    const next = distances.get(current)! + 1;
    for (const edge of edges) {
      if (edge.source === current && !distances.has(edge.target)) {
        distances.set(edge.target, next);
        queue.push(edge.target);
      }
    }
  }
  return distances;
}

/**
 * The step a new branch from `source` should rejoin, or null when the
 * existing branches never re-converge (no valid place to put one).
 */
export function findBranchJoin(
  edges: RouteEdge[],
  source: string,
  isUnconditional: (edge: RouteEdge) => boolean = () => true
): string | null {
  const targets = unconditionalTargets(edges, source, isUnconditional);
  if (targets.length === 0) return null;
  if (targets.length === 1) return targets[0];

  const reach = targets.map((target) => distancesFrom(target, edges));
  let best: { id: string; furthest: number; total: number } | null = null;
  for (const [id] of reach[0]) {
    if (id === source || !reach.every((distances) => distances.has(id))) {
      continue;
    }
    const steps = reach.map((distances) => distances.get(id)!);
    const furthest = Math.max(...steps);
    const total = steps.reduce((sum, value) => sum + value, 0);
    if (
      !best ||
      furthest < best.furthest ||
      (furthest === best.furthest && total < best.total)
    ) {
      best = { id, furthest, total };
    }
  }
  return best?.id ?? null;
}
