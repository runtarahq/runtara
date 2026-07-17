/**
 * Pure time-warp between **model time** (true recorded ms) and **display time**
 * (what the scrubber/clock advance through). Handles the two pacings and
 * idle-gap compression for long parked (suspended) runs. Unit-tested.
 *
 * - `real`  : display == model (optionally compressing idle gaps > threshold).
 * - `even`  : every inter-event interval gets equal screen time — legible, and
 *             collapses huge parked gaps implicitly. This is the default.
 */
import type { ReplayModel } from './types';

export type ReplayPacing = 'real' | 'even';

export interface TimeMapOptions {
  pacing: ReplayPacing;
  /** Compress idle gaps (only meaningful for `real` pacing). */
  compressIdle?: boolean;
  gapThresholdMs?: number;
  gapDisplayMs?: number;
  evenSliceMs?: number;
}

export interface TimeMap {
  displayEnd: number;
  /** model ms -> display ms */
  toDisplay: (modelT: number) => number;
  /** display ms -> model ms */
  toModel: (displayT: number) => number;
  /** Parked/compressed gaps, in display space, for rendering markers. */
  gaps: Array<{ displayStart: number; displayEnd: number; modelDurationMs: number }>;
}

const DEFAULTS = {
  gapThresholdMs: 4000,
  gapDisplayMs: 700,
  evenSliceMs: 900,
  /** Idle (nothing running) intervals get less screen time under even pacing. */
  evenIdleSliceMs: 220,
};

function mergeCoverage(model: ReplayModel): Array<[number, number]> {
  const spans = model.instances
    .map((i) => [i.startT, i.endT] as [number, number])
    .sort((a, b) => a[0] - b[0]);
  const merged: Array<[number, number]> = [];
  for (const [s, e] of spans) {
    const last = merged[merged.length - 1];
    if (last && s <= last[1]) {
      last[1] = Math.max(last[1], e);
    } else {
      merged.push([s, e]);
    }
  }
  return merged;
}

interface Segment {
  modelStart: number;
  modelEnd: number;
  displayStart: number;
  displayEnd: number;
  isGap: boolean;
  modelDurationMs: number;
}

function interpolate(
  segments: Segment[],
  value: number,
  from: 'model' | 'display'
): number {
  if (segments.length === 0) return 0;
  const startKey = from === 'model' ? 'modelStart' : 'displayStart';
  const endKey = from === 'model' ? 'modelEnd' : 'displayEnd';
  const outStartKey = from === 'model' ? 'displayStart' : 'modelStart';
  const outEndKey = from === 'model' ? 'displayEnd' : 'modelEnd';

  const first = segments[0];
  if (value <= first[startKey]) return first[outStartKey];
  const last = segments[segments.length - 1];
  if (value >= last[endKey]) return last[outEndKey];

  for (const seg of segments) {
    if (value >= seg[startKey] && value <= seg[endKey]) {
      const span = seg[endKey] - seg[startKey];
      const outSpan = seg[outEndKey] - seg[outStartKey];
      if (span <= 0) return seg[outStartKey];
      return seg[outStartKey] + ((value - seg[startKey]) / span) * outSpan;
    }
  }
  return last[outEndKey];
}

export function buildTimeMap(model: ReplayModel, options: TimeMapOptions): TimeMap {
  const opts = { ...DEFAULTS, ...options };
  const tEnd = model.tEnd;

  // Build model-time boundary segments.
  const boundaries: Array<{ start: number; end: number; isGap: boolean }> = [];

  if (options.pacing === 'even') {
    const set = new Set<number>([0, tEnd]);
    for (const inst of model.instances) {
      if (inst.startT >= 0 && inst.startT <= tEnd) set.add(inst.startT);
      if (inst.endT >= 0 && inst.endT <= tEnd) set.add(inst.endT);
    }
    const points = [...set].sort((a, b) => a - b);
    for (let i = 0; i < points.length - 1; i++) {
      const start = points[i];
      const end = points[i + 1];
      // Mark intervals with no running step as idle so they get less time.
      const active = model.instances.some(
        (inst) => inst.startT < end && inst.endT > start
      );
      boundaries.push({ start, end, isGap: !active });
    }
  } else {
    const coverage = mergeCoverage(model);
    let cursor = 0;
    for (const [s, e] of coverage) {
      if (s > cursor) boundaries.push({ start: cursor, end: s, isGap: true });
      boundaries.push({ start: s, end: e, isGap: false });
      cursor = e;
    }
    if (cursor < tEnd) boundaries.push({ start: cursor, end: tEnd, isGap: true });
  }

  // Assign display lengths.
  const segments: Segment[] = [];
  let displayCursor = 0;
  const gaps: TimeMap['gaps'] = [];
  for (const b of boundaries) {
    const modelLen = b.end - b.start;
    let displayLen: number;
    if (options.pacing === 'even') {
      displayLen = b.isGap ? opts.evenIdleSliceMs : opts.evenSliceMs;
    } else if (b.isGap && opts.compressIdle && modelLen > opts.gapThresholdMs) {
      displayLen = opts.gapDisplayMs;
    } else {
      displayLen = modelLen;
    }
    const seg: Segment = {
      modelStart: b.start,
      modelEnd: b.end,
      displayStart: displayCursor,
      displayEnd: displayCursor + displayLen,
      isGap: b.isGap,
      modelDurationMs: modelLen,
    };
    if (
      b.isGap &&
      ((opts.compressIdle && modelLen > opts.gapThresholdMs) ||
        (options.pacing === 'even' && modelLen > opts.gapThresholdMs))
    ) {
      gaps.push({
        displayStart: seg.displayStart,
        displayEnd: seg.displayEnd,
        modelDurationMs: modelLen,
      });
    }
    segments.push(seg);
    displayCursor = seg.displayEnd;
  }

  // Degenerate run (single instant step / zero span): give the scrubber range.
  let displayEnd = displayCursor;
  if (segments.length === 0 || displayEnd <= 0) {
    displayEnd = Math.max(displayEnd, options.pacing === 'even' ? opts.evenSliceMs : 1);
    segments.push({
      modelStart: 0,
      modelEnd: Math.max(tEnd, 1),
      displayStart: 0,
      displayEnd,
      isGap: false,
      modelDurationMs: Math.max(tEnd, 1),
    });
  }

  return {
    displayEnd,
    toDisplay: (m) => interpolate(segments, m, 'model'),
    toModel: (d) => interpolate(segments, d, 'display'),
    gaps,
  };
}
