import { describe, expect, it } from 'vitest';
import {
  buildReplayModel,
  type ReplayGraphInput,
  type StepSummaryLike,
} from './buildReplayModel';
import { deriveFrame } from './deriveFrame';
import { buildTimeMap } from './timeMap';
import { MIN_VISUAL_MS } from './types';

const BASE = 1_700_000_000_000;
const iso = (relMs: number) => new Date(BASE + relMs).toISOString();

/** Build a step summary with times relative to a shared base epoch. */
function sum(
  stepId: string,
  startRel: number,
  durationMs: number,
  opts: Partial<StepSummaryLike> = {}
): StepSummaryLike {
  return {
    stepId,
    stepName: stepId,
    stepType: opts.stepType ?? 'Agent',
    status: opts.status ?? 'completed',
    startedAt: iso(startRel),
    completedAt:
      opts.completedAt !== undefined
        ? opts.completedAt
        : iso(startRel + durationMs),
    durationMs,
    launchedAtMs: opts.launchedAtMs,
    settledAtMs: opts.settledAtMs,
    scopeId: opts.scopeId ?? null,
    parentScopeId: opts.parentScopeId ?? null,
  };
}

function graph(
  nodes: Array<[string, string]>,
  edges: Array<[string, string]>
): ReplayGraphInput {
  return {
    nodes: nodes.map(([id, stepType]) => ({ id, stepType, name: id })),
    edges: edges.map(([source, target], i) => ({
      id: `${source}->${target}-${i}`,
      source,
      target,
    })),
  };
}

describe('buildReplayModel', () => {
  it('computes t0-relative intervals and duration bounds', () => {
    const model = buildReplayModel(
      [sum('a', 0, 300), sum('b', 300, 300)],
      graph([['a', 'Agent'], ['b', 'Agent']], [['a', 'b']])
    );
    expect(model.hasEvents).toBe(true);
    expect(model.t0).toBe(BASE);
    expect(model.tEnd).toBe(600);
    const a = model.instancesByStep.get('a')![0];
    expect(a.startT).toBe(0);
    expect(a.endT).toBe(300);
  });

  it('reports no events for an untracked/empty run', () => {
    const model = buildReplayModel([], graph([['a', 'Agent']], []));
    expect(model.hasEvents).toBe(false);
    expect(model.tEnd).toBe(0);
    expect(deriveFrame(model, 0).nodeStates.get('a')).toBe('skipped');
  });

  it('gives an instant step a distinct end boundary without faking duration', () => {
    const model = buildReplayModel(
      [sum('a', 0, 0, { stepType: 'Log' })],
      graph([['a', 'Log']], [])
    );
    const a = model.instancesByStep.get('a')![0];
    expect(a.isInstant).toBe(true);
    expect(a.rawEndT).toBe(0);
    // Only a 1ms structural widening — honest, not a fabricated 150ms window.
    expect(a.endT).toBe(MIN_VISUAL_MS);
    expect(deriveFrame(model, 0.5).nodeStates.get('a')).toBe('running');
    expect(deriveFrame(model, 100).nodeStates.get('a')).toBe('done');
    // Under even pacing the instant step still gets a full, visible slice.
    const map = buildTimeMap(model, { pacing: 'even' });
    expect(map.displayEnd).toBeGreaterThan(0);
  });

  it('does NOT fabricate concurrency for fast sequential steps', () => {
    // Three 2ms steps back-to-back — a linear chain, never concurrent.
    const model = buildReplayModel(
      [sum('a', 0, 2), sum('b', 4, 2), sum('c', 8, 2)],
      graph([['a', 'Agent'], ['b', 'Agent'], ['c', 'Agent']], [['a', 'b'], ['b', 'c']])
    );
    // At every instant at most one node is running (no false overlap).
    for (let t = 0; t <= 12; t += 0.5) {
      expect(deriveFrame(model, t).runningCount).toBeLessThanOrEqual(1);
    }
  });
});

describe('deriveFrame — sequential', () => {
  const model = buildReplayModel(
    [sum('a', 0, 300), sum('b', 300, 300)],
    graph([['a', 'Agent'], ['b', 'Agent']], [['a', 'b']])
  );

  it('a running, b idle mid-a', () => {
    const f = deriveFrame(model, 150);
    expect(f.nodeStates.get('a')).toBe('running');
    expect(f.nodeStates.get('b')).toBe('idle');
    expect(f.runningCount).toBe(1);
  });

  it('a done, b running mid-b', () => {
    const f = deriveFrame(model, 450);
    expect(f.nodeStates.get('a')).toBe('done');
    expect(f.nodeStates.get('b')).toBe('running');
  });

  it('both done at end', () => {
    const f = deriveFrame(model, 650);
    expect(f.nodeStates.get('a')).toBe('done');
    expect(f.nodeStates.get('b')).toBe('done');
    expect(f.runningCount).toBe(0);
  });

  it('edge flows in the handoff window', () => {
    const edgeId = model.edges[0].id;
    expect(deriveFrame(model, 350).activeEdges.has(edgeId)).toBe(true);
    expect(deriveFrame(model, 150).activeEdges.has(edgeId)).toBe(false);
  });
});

describe('deriveFrame — edges do not stay animated after the run ends', () => {
  // Diamond that re-converges on a SHORT terminal node `f`, so the flow window
  // (vStart + EDGE_FLOW_MS) extends past f's end. No edge may animate at tEnd.
  const model = buildReplayModel(
    [
      sum('s', 0, 100),
      sum('c', 120, 200), // [120,320]
      sum('b', 120, 220), // [120,340]
      sum('f', 360, 20), // short terminal [360,380]
    ],
    graph(
      [['s', 'Agent'], ['c', 'Agent'], ['b', 'Agent'], ['f', 'Finish']],
      [['s', 'c'], ['s', 'b'], ['c', 'f'], ['b', 'f']]
    )
  );

  it('no edges are active once the target (f) is done / at tEnd', () => {
    // f ends at 380 == tEnd. Before the fix, c->f and b->f stayed active here.
    expect(deriveFrame(model, model.tEnd).activeEdges.size).toBe(0);
    expect(deriveFrame(model, model.tEnd + 5).activeEdges.size).toBe(0);
    // c->f still flows while f runs.
    const cf = model.edges.find((e) => e.source === 'c' && e.target === 'f')!;
    expect(deriveFrame(model, 365).activeEdges.has(cf.id)).toBe(true);
  });
});

describe('deriveFrame — parallel overlap IS concurrency', () => {
  // Start fans out to A and B whose intervals overlap.
  const model = buildReplayModel(
    [
      sum('start', 0, 150, { stepType: 'Start' }),
      sum('a', 150, 300), // [150, 450]
      sum('b', 200, 300), // [200, 500]
    ],
    graph(
      [['start', 'Start'], ['a', 'Agent'], ['b', 'Agent']],
      [['start', 'a'], ['start', 'b']]
    )
  );

  it('A and B are both running while their intervals overlap', () => {
    const f = deriveFrame(model, 300);
    expect(f.nodeStates.get('a')).toBe('running');
    expect(f.nodeStates.get('b')).toBe('running');
    expect(f.runningCount).toBe(2);
  });

  it('serializes correctly outside the overlap', () => {
    // a: [150,450], b: [200,500]. At t=470 a is done, b still running.
    const f = deriveFrame(model, 470);
    expect(f.nodeStates.get('a')).toBe('done');
    expect(f.nodeStates.get('b')).toBe('running');
    expect(f.runningCount).toBe(1);
  });
});

describe('deriveFrame — real launch/settle overrides the assemble cascade', () => {
  // The recorded assemble ORDER is sequential — by startedAt/durationMs the two
  // branches read a:[0,300], b:[300,600], never overlapping (exactly the cascade
  // the parallel-visibility bug produced). But the branches truly ran CONCURRENTLY,
  // stamped as launch/settle: a launched at +100 settled at +600, b launched at
  // +110 settled at +590. The model must prefer the launch/settle interval.
  const overlapModel = buildReplayModel(
    [
      sum('start', 0, 10, { stepType: 'Start' }),
      sum('a', 0, 300, { launchedAtMs: BASE + 100, settledAtMs: BASE + 600 }),
      sum('b', 300, 300, { launchedAtMs: BASE + 110, settledAtMs: BASE + 590 }),
    ],
    graph(
      [['start', 'Start'], ['a', 'Agent'], ['b', 'Agent']],
      [['start', 'a'], ['start', 'b']]
    )
  );

  it('uses [launchedAtMs, settledAtMs] as the interval, not startedAt/durationMs', () => {
    // t0 = start's startedAt (BASE). Relative: a [100,600], b [110,590].
    const a = overlapModel.instancesByStep.get('a')![0];
    const b = overlapModel.instancesByStep.get('b')![0];
    expect(a.startT).toBe(100);
    expect(a.endT).toBe(600);
    expect(b.startT).toBe(110);
    expect(b.endT).toBe(590);

    const f = deriveFrame(overlapModel, 200);
    expect(f.nodeStates.get('a')).toBe('running');
    expect(f.nodeStates.get('b')).toBe('running');
    expect(f.runningCount).toBe(2);
  });

  it('the SAME rows without launch/settle read as a sequential cascade', () => {
    const cascadeModel = buildReplayModel(
      [
        sum('start', 0, 10, { stepType: 'Start' }),
        sum('a', 0, 300), // [0,300]
        sum('b', 300, 300), // [300,600]
      ],
      graph(
        [['start', 'Start'], ['a', 'Agent'], ['b', 'Agent']],
        [['start', 'a'], ['start', 'b']]
      )
    );
    // a ends exactly as b begins — they never overlap (only one Agent at a time).
    expect(deriveFrame(cascadeModel, 200).nodeStates.get('a')).toBe('running');
    expect(deriveFrame(cascadeModel, 200).nodeStates.get('b')).toBe('idle');
    expect(deriveFrame(cascadeModel, 450).nodeStates.get('a')).toBe('done');
    expect(deriveFrame(cascadeModel, 450).nodeStates.get('b')).toBe('running');
  });
});

describe('deriveFrame — failed run', () => {
  const model = buildReplayModel(
    [
      sum('a', 0, 300),
      sum('b', 300, 300, { status: 'failed', completedAt: null }), // [300,600] failed
    ],
    graph(
      [['a', 'Agent'], ['b', 'Agent'], ['c', 'Agent']],
      [['a', 'b'], ['b', 'c']]
    )
  );

  it('failed node shows failed past its end', () => {
    const f = deriveFrame(model, 700);
    expect(f.nodeStates.get('a')).toBe('done');
    expect(f.nodeStates.get('b')).toBe('failed');
  });

  it('a node with no recorded execution is skipped', () => {
    // c never ran — it is downstream of a failure and stays skipped throughout.
    expect(deriveFrame(model, 700).nodeStates.get('c')).toBe('skipped');
    expect(deriveFrame(model, 0).nodeStates.get('c')).toBe('skipped');
  });
});

describe('deriveFrame — suspended / parked run', () => {
  const model = buildReplayModel(
    [
      sum('a', 0, 300),
      sum('wait', 300, 200, {
        stepType: 'Delay',
        status: 'suspended',
        completedAt: null,
      }), // [300,500]
    ],
    graph([['a', 'Agent'], ['wait', 'Delay']], [['a', 'wait']])
  );

  it('parked node reads as suspended past its interval', () => {
    expect(deriveFrame(model, 400).nodeStates.get('wait')).toBe('running');
    expect(deriveFrame(model, 800).nodeStates.get('wait')).toBe('suspended');
  });

  it('marks a still-running durable wait as suspended for a parked instance', () => {
    // The runtime records a parked WaitForSignal as status "running" (not
    // "suspended") — the amber state comes from the instance being suspended.
    const parked = buildReplayModel(
      [
        sum('a', 0, 300),
        sum('w', 300, 200, {
          stepType: 'WaitForSignal',
          status: 'running',
          completedAt: null,
        }),
      ],
      graph([['a', 'Agent'], ['w', 'WaitForSignal']], [['a', 'w']]),
      { instanceStatus: 'suspended' }
    );
    expect(deriveFrame(parked, 900).nodeStates.get('w')).toBe('suspended');
  });
});

describe('deriveFrame — Split with iterations', () => {
  // split: [0,900] with two iteration scopes c1 [30,420], c2 [30,870].
  const model = buildReplayModel(
    [
      sum('split', 0, 900, { stepType: 'Split', scopeId: 'S' }),
      sum('child', 30, 390, { scopeId: 'c1', parentScopeId: 'S' }), // [30,420]
      sum('child', 30, 840, { scopeId: 'c2', parentScopeId: 'S' }), // [30,870]
    ],
    graph([['split', 'Split']], [])
  );

  it('counts total iterations and active/completed over time', () => {
    const early = deriveFrame(model, 200).nodeIterations.get('split');
    expect(early).toEqual({ total: 2, active: 2, completed: 0 });

    const mid = deriveFrame(model, 500).nodeIterations.get('split');
    expect(mid).toEqual({ total: 2, active: 1, completed: 1 });

    const end = deriveFrame(model, 880).nodeIterations.get('split');
    expect(end).toEqual({ total: 2, active: 0, completed: 2 });
  });

  it('the split node itself is running while iterations run, done after', () => {
    expect(deriveFrame(model, 450).nodeStates.get('split')).toBe('running');
    expect(deriveFrame(model, 950).nodeStates.get('split')).toBe('done');
  });
});

describe('deriveFrame — While loop (shared scope, repeated steps)', () => {
  // While node `wl` runs 2 passes in one shared scope `sc_wl` (matches the real
  // runtime convention where parentScopeId is null and steps repeat per pass).
  const model = buildReplayModel(
    [
      sum('wl', 0, 300, { stepType: 'While' }),
      sum('d', 10, 40, { scopeId: 'sc_wl' }), // pass 1: [10,50]
      sum('df', 55, 15, { scopeId: 'sc_wl' }),
      sum('d', 80, 40, { scopeId: 'sc_wl' }), // pass 2: [80,120]
      sum('df', 125, 15, { scopeId: 'sc_wl' }),
    ],
    graph([['wl', 'While']], [])
  );

  it('counts loop passes from a single shared scope', () => {
    expect(deriveFrame(model, 30).nodeIterations.get('wl')).toEqual({
      total: 2,
      active: 1,
      completed: 0,
    });
    expect(deriveFrame(model, 100).nodeIterations.get('wl')).toEqual({
      total: 2,
      active: 1,
      completed: 1,
    });
    expect(deriveFrame(model, 200).nodeIterations.get('wl')).toEqual({
      total: 2,
      active: 0,
      completed: 2,
    });
  });
});
