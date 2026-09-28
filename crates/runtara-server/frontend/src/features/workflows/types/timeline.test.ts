import { describe, expect, it } from 'vitest';
import type { StepSummaryResponse } from '@/generated/RuntaraRuntimeApi';
import {
  barLabelPlacement,
  calculateMaxTimestamp,
  estimateBarLabelPx,
  isOpenEndedStep,
  timelineBarPosition,
  toHierarchicalStep,
} from './timeline';

const T0 = Date.parse('2026-09-27T20:00:00Z');
const step = (
  stepId: string,
  startOffsetMs: number,
  extra: Partial<StepSummaryResponse> = {}
): StepSummaryResponse =>
  ({
    stepId,
    stepType: 'Agent',
    status: 'completed',
    startedAt: new Date(T0 + startOffsetMs).toISOString(),
    durationMs: 10,
    ...extra,
  }) as StepSummaryResponse;

describe('open-ended timeline steps', () => {
  const finance = step('finance', 0, { durationMs: 30 });
  const wait = step('wait', 40, { status: 'suspended', durationMs: null });
  const now = T0 + 10_000;

  it('treats an unfinished running or suspended step as open-ended', () => {
    expect(isOpenEndedStep(wait)).toBe(true);
    expect(
      isOpenEndedStep(step('r', 0, { status: 'running', durationMs: null }))
    ).toBe(true);
    expect(isOpenEndedStep(finance)).toBe(false);
    // The run's terminal status for a step that never finished is not live.
    expect(
      isOpenEndedStep(step('c', 0, { status: 'cancelled', durationMs: null }))
    ).toBe(false);
  });

  it('runs a parked step from its start to now', () => {
    expect(calculateMaxTimestamp([finance, wait], now)).toBe(now);
    const parked = toHierarchicalStep(wait, 0, T0, now);
    expect(parked).toMatchObject({
      isOpenEnded: true,
      startMs: 40,
      spanMs: 10_000 - 40,
      // The recorded duration stays unknown (stats and details use it).
      durationMs: null,
    });
    expect(toHierarchicalStep(finance, 0, T0, now)).toMatchObject({
      isOpenEnded: false,
      spanMs: 30,
    });
  });

  it('keeps a parked step visible instead of placing it past the track', () => {
    // Before the fix the parked step started at the last timestamp, so its
    // bar sat at left: 100% and was clipped.
    const atEnd = timelineBarPosition({ startMs: 55, spanMs: 0 }, 55);
    expect(atEnd.left).toBe('min(100%, calc(100% - 60px))');
    const parked = timelineBarPosition(
      toHierarchicalStep(wait, 0, T0, now),
      now - T0
    );
    expect(parked.left).toBe('min(0.4%, calc(100% - 60px))');
    expect(parseFloat(parked.width)).toBeCloseTo(99.6);
  });

  it('never lets a bar run past the end of the track', () => {
    const pos = timelineBarPosition({ startMs: 90, spanMs: 50 }, 100);
    expect(parseFloat(pos.width)).toBe(10);
    expect(timelineBarPosition({ startMs: 0, spanMs: 0 }, 0)).toMatchObject({
      left: '0%',
      width: '100%',
    });
  });
});

describe('bar label placement', () => {
  const label = estimateBarLabelPx('28ms', 'Completed');

  it('keeps the label inside a bar wide enough for it', () => {
    expect(
      barLabelPlacement({ leftPct: 0, widthPct: 50 }, 1000, label)
    ).toEqual({ side: 'inside' });
    // Unmeasured track: nothing to compare against, keep it inside.
    expect(barLabelPlacement({ leftPct: 0, widthPct: 0.1 }, 0, label)).toEqual({
      side: 'inside',
    });
  });

  it('moves the label to the right of a narrow bar', () => {
    // 0.1% of 972px is under the 60px minimum, so the bar is 60px wide.
    expect(
      barLabelPlacement({ leftPct: 0, widthPct: 0.1 }, 972, label)
    ).toEqual({ side: 'right', leftPx: 66 });
  });

  it('moves the label to the left of a narrow bar at the track end', () => {
    // The bar is pulled back to 972 - 60 = 912px; no room on its right.
    expect(
      barLabelPlacement({ leftPct: 99.9, widthPct: 0.1 }, 972, label)
    ).toEqual({ side: 'left', rightPx: 66 });
  });

  it('sizes the label from its text', () => {
    expect(estimateBarLabelPx('28ms', 'Completed')).toBeGreaterThan(60);
    expect(
      estimateBarLabelPx('28ms', 'waiting for input', true)
    ).toBeGreaterThan(estimateBarLabelPx('28ms', 'Completed'));
  });
});
