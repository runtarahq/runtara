import { describe, expect, it } from 'vitest';
import { buildReplayModel, type StepSummaryLike } from './buildReplayModel';
import { buildTimeMap } from './timeMap';

const BASE = 1_700_000_000_000;
const iso = (relMs: number) => new Date(BASE + relMs).toISOString();

function sum(stepId: string, startRel: number, durationMs: number): StepSummaryLike {
  return {
    stepId,
    stepType: 'Agent',
    status: 'completed',
    startedAt: iso(startRel),
    completedAt: iso(startRel + durationMs),
    durationMs,
  };
}

const gid = (a: string) => ({ id: a, stepType: 'Agent', name: a });

describe('buildTimeMap', () => {
  it('real pacing without compression is identity', () => {
    const model = buildReplayModel(
      [sum('a', 0, 300), sum('b', 300, 300)],
      { nodes: [gid('a'), gid('b')], edges: [] }
    );
    const map = buildTimeMap(model, { pacing: 'real', compressIdle: false });
    expect(map.displayEnd).toBe(600);
    expect(map.toDisplay(0)).toBe(0);
    expect(map.toDisplay(450)).toBeCloseTo(450);
    expect(map.toModel(450)).toBeCloseTo(450);
    expect(map.gaps).toHaveLength(0);
  });

  it('real pacing compresses a long idle (parked) gap', () => {
    // a:[0,300], then a 5s park, then b:[5300,5600].
    const model = buildReplayModel(
      [sum('a', 0, 300), sum('b', 5300, 300)],
      { nodes: [gid('a'), gid('b')], edges: [] }
    );
    const map = buildTimeMap(model, {
      pacing: 'real',
      compressIdle: true,
      gapThresholdMs: 4000,
      gapDisplayMs: 700,
    });
    // 300 (active) + 700 (compressed gap) + 300 (active) = 1300.
    expect(map.displayEnd).toBe(1300);
    expect(map.gaps).toHaveLength(1);
    expect(map.gaps[0].modelDurationMs).toBe(5000);
    // Monotonic: b's start maps past the compressed gap.
    expect(map.toDisplay(5300)).toBeCloseTo(1000);
    expect(map.toModel(map.toDisplay(5350))).toBeCloseTo(5350, 0);
  });

  it('even pacing gives every inter-event interval equal screen time', () => {
    const model = buildReplayModel(
      [sum('a', 0, 300), sum('b', 300, 300)],
      { nodes: [gid('a'), gid('b')], edges: [] }
    );
    const map = buildTimeMap(model, { pacing: 'even', evenSliceMs: 900 });
    // boundaries {0,300,600} -> 2 slices.
    expect(map.displayEnd).toBe(1800);
    expect(map.toModel(900)).toBeCloseTo(300);
    expect(map.toDisplay(600)).toBeCloseTo(1800);
  });

  it('is monotonic non-decreasing across the whole display range', () => {
    const model = buildReplayModel(
      [sum('a', 0, 40), sum('b', 30, 200), sum('c', 900, 50)],
      { nodes: [gid('a'), gid('b'), gid('c')], edges: [] }
    );
    for (const pacing of ['real', 'even'] as const) {
      const map = buildTimeMap(model, { pacing, compressIdle: true });
      let prev = -Infinity;
      for (let d = 0; d <= map.displayEnd; d += map.displayEnd / 50) {
        const m = map.toModel(d);
        expect(m).toBeGreaterThanOrEqual(prev - 1e-6);
        prev = m;
      }
    }
  });

  it('gives a degenerate single-instant run a usable scrubber range', () => {
    const model = buildReplayModel([sum('a', 0, 0)], { nodes: [gid('a')], edges: [] });
    const real = buildTimeMap(model, { pacing: 'real', compressIdle: false });
    expect(real.displayEnd).toBeGreaterThan(0);
    const even = buildTimeMap(model, { pacing: 'even' });
    expect(even.displayEnd).toBeGreaterThan(0);
  });
});
