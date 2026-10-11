import { describe, expect, it } from 'vitest';
import { computeWindow, scrollTopForIndex } from './useVirtualWindow';

describe('computeWindow', () => {
  it('renders the visible rows plus overscan', () => {
    // 28px rows, 280px viewport → 10 visible rows.
    expect(computeWindow(1_000, 28, 280, 0, 5)).toEqual({
      start: 0,
      end: 15,
      totalHeight: 28_000,
      offsetTop: 0,
    });
    expect(computeWindow(1_000, 28, 280, 2_800, 5)).toEqual({
      start: 95,
      end: 115,
      totalHeight: 28_000,
      offsetTop: 95 * 28,
    });
  });

  it('never renders past the end of the list', () => {
    const w = computeWindow(1_000, 28, 280, 27_720, 5);
    expect(w.end).toBe(1_000);
    expect(w.start).toBe(985);
  });

  it('clamps a stale scroll position after the list shrank', () => {
    // Was scrolled deep into 1,000 rows; a fold left 20.
    expect(computeWindow(20, 28, 280, 20_000, 5)).toMatchObject({
      start: 5,
      end: 20,
    });
  });

  it('renders everything for a list shorter than the viewport', () => {
    expect(computeWindow(3, 28, 280, 0, 5)).toMatchObject({
      start: 0,
      end: 3,
      totalHeight: 84,
    });
    expect(computeWindow(0, 28, 280, 0, 5)).toMatchObject({
      start: 0,
      end: 0,
      totalHeight: 0,
    });
  });

  it('keeps the window small for a million rows', () => {
    const w = computeWindow(1_000_000, 28, 420, 14_000_000, 8);
    expect(w.end - w.start).toBeLessThanOrEqual(15 + 16 + 1);
  });
});

describe('scrollTopForIndex', () => {
  it('leaves the scroll alone when the row is visible', () => {
    expect(scrollTopForIndex(3, 28, 280, 0)).toBe(0);
  });

  it('scrolls up to a row above the viewport', () => {
    expect(scrollTopForIndex(2, 28, 280, 560)).toBe(56);
  });

  it('scrolls down just enough to show a row below the viewport', () => {
    expect(scrollTopForIndex(20, 28, 280, 0)).toBe(21 * 28 - 280);
  });
});
