import { describe, it, expect } from 'vitest';
import {
  CHOKE_THRESHOLD,
  findChokepoint,
  formatAge,
  formatCount,
  formatRate,
  isNotDraining,
  severityOf,
  sparklinePath,
  snapshotStuckAfterMs,
  stepsAreMeasured,
  stickyChokepoint,
  utilisation,
  type PipelineStage,
  type PipelineSnapshot,
} from './pipeline';

function stage(over: Partial<PipelineStage> & { key: string }): PipelineStage {
  return {
    label: over.key,
    knob: null,
    limit: null,
    used: null,
    oldestAgeMs: null,
    inflowKey: 'accepted',
    ...over,
  };
}

/// Four fixtures matching the states this view exists to tell apart.
const FIXTURES = {
  /// Everything below threshold and moving.
  healthy: [
    stage({ key: 'admission', limit: 2048, used: 1180 }),
    stage({ key: 'triggerQueue', used: 41, oldestAgeMs: 200 }),
    stage({ key: 'triggerWorkers', limit: 32, used: 9 }),
    stage({ key: 'runPermits', limit: 16, used: 11, oldestAgeMs: 2_700 }),
    stage({ key: 'executing', used: 11 }),
    stage({ key: 'parked', used: 1_009_739 }),
  ],
  /// At the ceiling, but turning work over — full is not a fault.
  saturated: [
    stage({ key: 'admission', limit: 2048, used: 2048 }),
    stage({ key: 'triggerQueue', used: 3_417, oldestAgeMs: 4_100 }),
    stage({ key: 'triggerWorkers', limit: 32, used: 31 }),
    stage({ key: 'runPermits', limit: 16, used: 16, oldestAgeMs: 2_900 }),
    stage({ key: 'executing', used: 16 }),
    stage({ key: 'parked', used: 1_009_739 }),
  ],
  /// Permits full and held for 48 minutes; the queue has merely piled up behind.
  stalled: [
    stage({ key: 'admission', limit: 64, used: 44 }),
    stage({ key: 'triggerQueue', used: 28, oldestAgeMs: 345_600_000 }),
    stage({ key: 'triggerWorkers', limit: 16, used: 0 }),
    stage({ key: 'runPermits', limit: 8, used: 8, oldestAgeMs: 2_880_000 }),
    stage({ key: 'executing', used: 8, oldestAgeMs: 2_880_000 }),
    stage({ key: 'parked', used: 1_766 }),
  ],
  /// Sources unreadable — Valkey down, runner not reporting.
  degraded: [
    stage({ key: 'admission', limit: 2048, used: null }),
    stage({ key: 'triggerQueue', used: null }),
    stage({ key: 'triggerWorkers', limit: null, used: null }),
    stage({ key: 'runPermits', limit: null, used: null }),
    stage({ key: 'executing', used: null }),
    stage({ key: 'parked', used: null }),
  ],
};

describe('utilisation', () => {
  it('is null for an unbounded stage rather than zero', () => {
    // An unbounded stage has no ceiling to be a fraction of. Reporting 0%
    // would have it render as comfortably empty when the truth is that the
    // question does not apply.
    expect(utilisation(stage({ key: 'parked', used: 1_009_739 }))).toBeNull();
  });

  it('is null when the occupancy could not be read', () => {
    expect(utilisation(stage({ key: 'q', limit: 16, used: null }))).toBeNull();
  });

  it('measures a bounded stage against its ceiling', () => {
    expect(utilisation(stage({ key: 'r', limit: 16, used: 12 }))).toBe(75);
  });
});

describe('severityOf', () => {
  it('separates unknown from healthy', () => {
    // The distinction the whole payload is built around: an unread source is
    // not an idle stage.
    expect(severityOf(stage({ key: 'a', limit: 16, used: null }))).toBe(
      'unknown'
    );
    expect(severityOf(stage({ key: 'a', limit: 16, used: 0 }))).toBe('ok');
  });

  it('escalates as a stage fills', () => {
    expect(severityOf(stage({ key: 'a', limit: 100, used: 50 }))).toBe('ok');
    expect(severityOf(stage({ key: 'a', limit: 100, used: 85 }))).toBe('warn');
    expect(severityOf(stage({ key: 'a', limit: 100, used: 100 }))).toBe('bad');
  });
});

describe('isNotDraining', () => {
  it('requires both full and old', () => {
    // Full while recycling is a system working as hard as it can; old with
    // headroom is nobody's constraint. Only the two together are a fault.
    expect(
      isNotDraining(stage({ key: 'a', limit: 8, used: 8, oldestAgeMs: 2_900 }))
    ).toBe(false);
    expect(
      isNotDraining(
        stage({ key: 'a', limit: 8, used: 2, oldestAgeMs: 9_999_999 })
      )
    ).toBe(false);
    expect(
      isNotDraining(
        stage({ key: 'a', limit: 8, used: 8, oldestAgeMs: 2_880_000 })
      )
    ).toBe(true);
  });

  it('is never true without an age to judge', () => {
    expect(isNotDraining(stage({ key: 'a', limit: 8, used: 8 }))).toBe(false);
  });

  it('calls out an old durable queue without blaming it as the capacity bound', () => {
    const queue = stage({
      key: 'launchQueued',
      limit: 64,
      used: 3,
      oldestAgeMs: 2_880_000,
    });
    expect(isNotDraining(queue)).toBe(true);
    expect(findChokepoint([queue])).toBeNull();
  });

  it('calls out an old preparation lease without mistaking it for its worker bound', () => {
    const preparing = stage({
      key: 'launchPreparing',
      used: 1,
      oldestAgeMs: 2_880_000,
    });
    expect(isNotDraining(preparing)).toBe(true);
    expect(findChokepoint([preparing])).toBeNull();
  });

  it('does not call retained terminal launch history not draining', () => {
    expect(
      isNotDraining(
        stage({
          key: 'launchExpired',
          used: 3,
          oldestAgeMs: 2_880_000,
        })
      )
    ).toBe(false);
    expect(
      isNotDraining(
        stage({
          key: 'launchCancelled',
          used: 3,
          oldestAgeMs: 2_880_000,
        })
      )
    ).toBe(false);
  });
});

describe('snapshotStuckAfterMs', () => {
  it('uses the server policy and only falls back for an older server', () => {
    const base: PipelineSnapshot = {
      capturedAt: '2026-09-03T00:00:00Z',
      windowMs: 1_000,
      rates: null,
      stages: [],
    };
    expect(snapshotStuckAfterMs({ ...base, stuckAfterMs: 2_000 })).toBe(2_000);
    expect(snapshotStuckAfterMs(base)).toBe(5 * 60 * 1000);
  });
});

describe('findChokepoint', () => {
  it('accuses nothing when every stage has headroom', () => {
    // Regression: this once named the run-slot stage at 69% on a healthy
    // pipeline.
    // A page that always points somewhere teaches its reader to ignore where
    // it points.
    expect(findChokepoint(FIXTURES.healthy)).toBeNull();
  });

  it('names the full bounded stage when the pipeline is saturated', () => {
    const choke = findChokepoint(FIXTURES.saturated);
    expect(choke?.key).toBe('admission');
  });

  it('names the run permits, not the queue, when work has stopped moving', () => {
    // Regression, and the reason this view exists. The trigger queue holds the
    // oldest item by four orders of magnitude (four days against forty-eight
    // minutes) and is the obvious thing to point at — but it is unbounded, so
    // it is where the evidence piles up rather than the constraint. The run
    // permits are full and not draining, and they are the fault.
    const choke = findChokepoint(FIXTURES.stalled);
    expect(choke?.key).toBe('runPermits');
    expect(choke?.key).not.toBe('triggerQueue');
  });

  it('prefers a stuck stage over a merely fuller one', () => {
    // Regression: ranking by utilisation alone let whichever full stage came
    // first win. Full stages are routine; stuck ones are the emergency.
    const stages = [
      stage({ key: 'busy', limit: 100, used: 100, oldestAgeMs: 500 }),
      stage({ key: 'stuck', limit: 8, used: 8, oldestAgeMs: 2_880_000 }),
    ];
    expect(findChokepoint(stages)?.key).toBe('stuck');
  });

  it('never accuses an unbounded stage however deep it gets', () => {
    const stages = [
      stage({ key: 'admission', limit: 2048, used: 10 }),
      stage({ key: 'triggerQueue', used: 5_000_000, oldestAgeMs: 999_999_999 }),
    ];
    expect(findChokepoint(stages)).toBeNull();
  });

  it('accuses nothing when nothing can be read', () => {
    // The false-red guard: missing data must never be evidence of a fault.
    expect(findChokepoint(FIXTURES.degraded)).toBeNull();
  });

  it('holds the threshold it documents', () => {
    const under = [stage({ key: 'a', limit: 100, used: CHOKE_THRESHOLD - 1 })];
    const over = [stage({ key: 'a', limit: 100, used: CHOKE_THRESHOLD })];
    expect(findChokepoint(under)).toBeNull();
    expect(findChokepoint(over)?.key).toBe('a');
  });
});

describe('stickyChokepoint', () => {
  it('does not flicker when a stage hovers at the threshold', () => {
    // Regression: recomputed every tick, a stage wobbling either side of 80%
    // had its highlight appear and vanish once a second. Unreadable, and it
    // makes a real constraint look like noise.
    const above = [stage({ key: 'runPermits', limit: 100, used: 81 })];
    const below = [stage({ key: 'runPermits', limit: 100, used: 78 })];

    const first = stickyChokepoint(above, null);
    expect(first).toBe('runPermits');
    expect(stickyChokepoint(below, first)).toBe('runPermits');
  });

  it('releases a stage that genuinely recovers', () => {
    const recovered = [stage({ key: 'runPermits', limit: 100, used: 40 })];
    expect(stickyChokepoint(recovered, 'runPermits')).toBeNull();
  });

  it('does not resurrect a stage that has vanished from the snapshot', () => {
    expect(stickyChokepoint([], 'runPermits')).toBeNull();
  });
});

describe('stepsAreMeasured', () => {
  it('treats a null steps rate as unmeasured, not as zero', () => {
    // trackEvents is compile-time: a workflow built without it runs perfectly
    // and reports nothing. Rendering that as 0/s would let a reader conclude a
    // healthy deployment had stopped dead.
    const rates = {
      offered: 400,
      accepted: 400,
      denied: 0,
      started: 398,
      finished: 396,
      steps: null,
    };
    expect(stepsAreMeasured(rates)).toBe(false);
    expect(stepsAreMeasured({ ...rates, steps: 0 })).toBe(true);
  });

  it('is false before the first window closes', () => {
    const snapshot: PipelineSnapshot = {
      capturedAt: '2026-09-02T00:00:00Z',
      windowMs: 0,
      rates: null,
      stages: [],
    };
    expect(stepsAreMeasured(snapshot.rates)).toBe(false);
  });
});

describe('formatting', () => {
  it('coarsens an age as it grows', () => {
    expect(formatAge(420)).toBe('420ms');
    expect(formatAge(2_700)).toBe('2.7s');
    expect(formatAge(2_880_000)).toBe('48m');
    expect(formatAge(345_600_000)).toBe('4d');
    expect(formatAge(null)).toBeNull();
  });

  it('renders a million parked as a million', () => {
    expect(formatCount(1_009_739)).toBe('1.01M');
    expect(formatCount(1_766)).toBe('1,766');
    expect(formatCount(null)).toBe('—');
  });

  it('shows an unread rate as a dash, not a zero', () => {
    expect(formatRate(null)).toBe('—');
    expect(formatRate(0)).toBe('0.0');
    expect(formatRate(398)).toBe('398');
  });
});

describe('sparklinePath', () => {
  it('scales from zero so a flat stock stays flat', () => {
    // Regression: scaling to the series' own min–max turned a 0.06% drift
    // across a million parked instances into a dramatic climb. Zero-based, a
    // stock that is not moving looks like one that is not moving.
    const parked = Array.from({ length: 60 }, (_, i) => 1_009_100 + i * 11);
    const path = sparklinePath(parked, null);
    expect(path).not.toBeNull();

    const ys = [...path!.line.matchAll(/[ML][\d.]+ ([\d.]+)/g)].map((m) =>
      Number(m[1])
    );
    expect(Math.max(...ys) - Math.min(...ys)).toBeLessThan(1);
  });

  it('draws a bounded stage against its ceiling', () => {
    const path = sparklinePath([0, 8, 16], 16);
    const ys = [...path!.line.matchAll(/[ML][\d.]+ ([\d.]+)/g)].map((m) =>
      Number(m[1])
    );
    expect(ys[0]).toBeCloseTo(100, 1);
    expect(ys[2]).toBeCloseTo(0, 1);
  });

  it('ignores gaps where a reading was missing', () => {
    const path = sparklinePath([4, null, 8, null, 12], 16);
    expect(path).not.toBeNull();
    expect(path!.line.split('L')).toHaveLength(3);
  });

  it('draws nothing from a single point', () => {
    expect(sparklinePath([5], 16)).toBeNull();
    expect(sparklinePath([], 16)).toBeNull();
  });
});
