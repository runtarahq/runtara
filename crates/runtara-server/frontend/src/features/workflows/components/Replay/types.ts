/**
 * Graph Replay — shared types for the animated historical-execution renderer.
 *
 * The replay model is a pure projection of recorded step data onto the graph
 * that the instance actually ran (its `usedVersion`). Each recorded step carries
 * a `[startT, endT]` interval; **overlapping intervals are concurrency** — no
 * scheduler re-derivation. `deriveFrame(model, t)` maps a clock time `t` onto
 * per-node visual states + active edges, and is unit-tested with fixtures.
 */

/** Visual state of a graph node at a given replay time. */
export type ReplayNodeState =
  | 'idle' // not yet reached at time t (faint)
  | 'running' // t within its [start, end) interval (glow/pulse)
  | 'done' // completed, t past its end (green tick)
  | 'failed' // errored, t past its end (red)
  | 'suspended' // suspended / parked at this node (amber pulse)
  | 'skipped'; // never ran in this instance — e.g. false branch (dashed/grey)

/**
 * Structural minimum span (ms) applied to a step's end time. Deliberately tiny:
 * it only widens a 0ms step so it gets a distinct end boundary (hence its own
 * slice under `even` pacing, where instant steps become fully visible). It must
 * NOT be large — a big clamp would make fast *sequential* steps falsely overlap
 * and fabricate concurrency, breaking the "faithful projection" guarantee.
 * `real`-time visibility of instant steps is instead the job of even pacing.
 */
export const MIN_VISUAL_MS = 1;
/** A step whose recorded duration is below this (ms) is labelled near-instant. */
export const INSTANT_THRESHOLD_MS = 5;
/** How long an edge "flows" after its upstream node completes. */
export const EDGE_FLOW_MS = 260;

/** A single recorded execution of a graph step (one per Split/While iteration). */
export interface ReplayStepInstance {
  /** Unique per recorded execution: scopeId when present, else stepId. */
  key: string;
  /** Graph step id this execution belongs to. */
  stepId: string;
  stepName: string;
  stepType: string;
  scopeId: string | null;
  parentScopeId: string | null;
  /** Start time relative to `t0` (ms). */
  startT: number;
  /** End time relative to `t0` (ms), clamped up to `startT + MIN_VISUAL_MS`. */
  endT: number;
  /** Honest end time relative to `t0` (ms), before the min-visual clamp. */
  rawEndT: number;
  /** Normalized status: completed | failed | running | suspended. */
  status: ReplayNodeState;
  /** True when the recorded duration was below the min-visual threshold. */
  isInstant: boolean;
}

/** A graph step node in the replay DAG (top-level steps only in the MVP). */
export interface ReplayGraphNode {
  id: string;
  stepType: string;
  name: string;
}

/** A directed transition in the replay DAG. */
export interface ReplayEdge {
  id: string;
  source: string;
  target: string;
  sourceHandle?: string | null;
  /** Loop/back edge (target ranks at/above source) — drawn but excluded from layering. */
  isBackEdge?: boolean;
}

/** Iteration bookkeeping for composite nodes (Split/While/EmbedWorkflow). */
export interface ReplayIterationCounts {
  total: number;
  active: number;
  completed: number;
}

/** The normalized, pure replay model — the single source of truth for both views. */
export interface ReplayModel {
  /** Top-level graph step node ids, in a stable order. */
  nodeIds: string[];
  nodes: Map<string, ReplayGraphNode>;
  edges: ReplayEdge[];
  /** Every recorded step execution, sorted ascending by `startT`. */
  instances: ReplayStepInstance[];
  /** node id -> its own executions (stepId === node id). */
  instancesByStep: Map<string, ReplayStepInstance[]>;
  /** node id -> executions nested under this node's scope (for iteration counters). */
  childInstancesByStep: Map<string, ReplayStepInstance[]>;
  /** Absolute epoch ms of the earliest recorded start. */
  t0: number;
  /** Model duration (ms), clamped so trailing instant steps stay visible. */
  tEnd: number;
  /** Honest model duration (ms) before clamping. */
  rawTEnd: number;
  /** False when no step executions were recorded (track_events off / empty run). */
  hasEvents: boolean;
}

/** A single rendered frame: node states + active edges at model time `t`. */
export interface ReplayFrame {
  t: number;
  nodeStates: Map<string, ReplayNodeState>;
  nodeIterations: Map<string, ReplayIterationCounts>;
  activeEdges: Set<string>;
  /** Number of nodes currently in the `running` state (concurrency indicator). */
  runningCount: number;
}
