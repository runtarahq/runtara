import { describe, expect, it } from 'vitest';
import type { MetricsBucket } from '@/generated/RuntaraRuntimeApi';

import {
  ACTIVITY_MAP_CONFIG,
  buildActivityMap,
  formatCellRange,
  type ActivityMapConfig,
} from './activity-map';

const MINUTE = 60_000;

function bucket(index: number, overrides: Partial<MetricsBucket> = {}) {
  return {
    bucket_time: new Date(index * MINUTE).toISOString(),
    invocation_count: 0,
    success_count: 0,
    failure_count: 0,
    cancelled_count: 0,
    ...overrides,
  } as MetricsBucket;
}

/** A spine of `count` one-minute buckets, oldest first. */
function spine(
  count: number,
  fill: (i: number) => Partial<MetricsBucket> = () => ({})
) {
  return Array.from({ length: count }, (_, i) => bucket(i, fill(i)));
}

const ONE_HOUR: ActivityMapConfig = ACTIVITY_MAP_CONFIG['1h'];

describe('buildActivityMap', () => {
  it('is empty for no data rather than throwing', () => {
    for (const input of [undefined, null, []]) {
      const map = buildActivityMap(input as MetricsBucket[], ONE_HOUR);
      expect(map.rows).toEqual([]);
      expect(map.total).toBe(0);
      expect(map.peak).toBeNull();
    }
  });

  it('lays an hour of minutes into a single row', () => {
    const map = buildActivityMap(spine(60), ONE_HOUR);
    expect(map.rows).toHaveLength(1);
    expect(map.rows[0].cells).toHaveLength(60);
    expect(map.rows[0].label).toBe('+0');
  });

  it('gives every returned bucket a cell', () => {
    // The API spine is inclusive of both edges, so it is one longer than
    // cols * rows. The grid grows a column rather than dropping the overflow:
    // discarding a bucket let the map's "busiest interval" disagree with the
    // same figure computed over the full series on the card above it.
    const map = buildActivityMap(
      spine(61, (i) => ({ invocation_count: i })),
      ONE_HOUR
    );
    const cells = map.rows[0].cells;
    expect(cells).toHaveLength(61);
    expect(map.total).toBe(spine(61).length * 0 + (60 * 61) / 2);
    expect(map.peak?.total).toBe(60);
  });

  it('orders cells column-major so time reads left to right', () => {
    // 24h: four six-minute rows per column. Column 0 holds buckets 0..3.
    const config = ACTIVITY_MAP_CONFIG['24h'];
    const map = buildActivityMap(
      Array.from({ length: config.cols * config.rows }, (_, i) =>
        bucket(i, { invocation_count: i })
      ),
      config
    );
    expect(map.rows).toHaveLength(4);
    expect(map.rows[0].cells[0].total).toBe(0);
    expect(map.rows[1].cells[0].total).toBe(1);
    expect(map.rows[3].cells[0].total).toBe(3);
    expect(map.rows[0].cells[1].total).toBe(4);
  });

  it('labels rows by their offset inside a column', () => {
    expect(
      buildActivityMap(spine(240), ACTIVITY_MAP_CONFIG['24h']).rows.map(
        (r) => r.label
      )
    ).toEqual(['+0', '+6m', '+12m', '+18m']);
    expect(
      buildActivityMap(spine(360), ACTIVITY_MAP_CONFIG['30d']).rows.map(
        (r) => r.label
      )
    ).toEqual(['+0', '+2h', '+4h', '+6h', '+8h', '+10h']);
  });

  it('keeps one unit across a column of row labels', () => {
    // 24-minute rows: offsets 0, 24, 48, 72, 96, 120, 144. Only 120 is a whole
    // hour, and labelling per-row rendered "+96m, +2h, +144m" in one stack.
    const labels = buildActivityMap(
      spine(420),
      ACTIVITY_MAP_CONFIG['7d']
    ).rows.map((r) => r.label);
    expect(labels).toEqual([
      '+0',
      '+24m',
      '+48m',
      '+72m',
      '+96m',
      '+120m',
      '+144m',
    ]);

    for (const config of Object.values(ACTIVITY_MAP_CONFIG)) {
      const suffixes = new Set(
        buildActivityMap(spine(config.cols * config.rows), config)
          .rows.map((r) => r.label)
          .filter((label) => label !== '+0')
          .map((label) => label.slice(-1))
      );
      expect(suffixes.size).toBeLessThanOrEqual(1);
    }
  });

  it('totals every count across the grid', () => {
    const map = buildActivityMap(
      spine(60, () => ({
        invocation_count: 3,
        success_count: 2,
        failure_count: 1,
        cancelled_count: 0,
      })),
      ONE_HOUR
    );
    expect(map.total).toBe(180);
    expect(map.success).toBe(120);
    expect(map.failed).toBe(60);
  });

  it('keeps a single run visible against a peak of hundreds', () => {
    const map = buildActivityMap(
      spine(60, (i) => ({
        invocation_count: i === 0 ? 1 : i === 59 ? 400 : 0,
      })),
      ONE_HOUR
    );
    const cells = map.rows[0].cells;
    expect(cells[0].intensity).toBeGreaterThan(0);
    expect(cells[59].intensity).toBe(1);
    expect(cells[30].intensity).toBe(0);
  });

  it('spreads a steady workload across all four levels', () => {
    // Every cell between 50 and 80 runs. Scaling against the maximum puts them
    // all in one band and the map reads as a flat wash; ranking keeps the
    // variation visible.
    const map = buildActivityMap(
      spine(60, (i) => ({ invocation_count: 50 + (i % 30) })),
      ONE_HOUR
    );
    const levels = new Set(map.rows[0].cells.map((c) => c.intensity));
    expect(levels.size).toBe(4);
  });

  it('orders levels the same way as the counts', () => {
    const map = buildActivityMap(
      spine(60, (i) => ({ invocation_count: i + 1 })),
      ONE_HOUR
    );
    const cells = map.rows[0].cells;
    for (let i = 1; i < cells.length; i++) {
      expect(cells[i].intensity).toBeGreaterThanOrEqual(cells[i - 1].intensity);
    }
  });

  it('does not invent variation where a window has none', () => {
    const map = buildActivityMap(
      spine(60, () => ({ invocation_count: 7 })),
      ONE_HOUR
    );
    expect(new Set(map.rows[0].cells.map((c) => c.intensity)).size).toBe(1);
  });

  it('flags an interval that failed worse than the window did', () => {
    const map = buildActivityMap(
      spine(60, (i) => ({
        invocation_count: 10,
        // One bad interval: half its runs failed. Everywhere else, none.
        failure_count: i === 59 ? 5 : 0,
      })),
      ONE_HOUR
    );
    const cells = map.rows[0].cells;
    expect(cells[59].elevated).toBe(true);
    expect(cells[59].failRate).toBeCloseTo(0.5);
    expect(cells[0].elevated).toBe(false);
  });

  it('does not flag every cell just because the window has failures', () => {
    // This is the failure mode that made the map unreadable: a window with a
    // low, evenly spread failure rate lit up almost every cell, so the marker
    // said nothing and buried the density underneath it.
    const map = buildActivityMap(
      spine(60, () => ({ invocation_count: 100, failure_count: 2 })),
      ONE_HOUR
    );
    expect(map.rows[0].cells.every((c) => !c.elevated)).toBe(true);
  });

  it('ignores a lone failure in an otherwise quiet interval', () => {
    // One failure out of one run is a 100% rate, but flagging it would make
    // every sparse interval look like an incident.
    const map = buildActivityMap(
      spine(60, (i) => ({
        invocation_count: i === 3 ? 1 : 50,
        failure_count: i === 3 ? 1 : 0,
      })),
      ONE_HOUR
    );
    expect(map.rows[0].cells[3].elevated).toBe(false);
  });

  it('flags repeated failures against a clean window', () => {
    // Baseline near zero, so the floor decides: two failures out of ten is
    // worth a look even though the window average is almost nothing.
    const map = buildActivityMap(
      spine(60, (i) => ({
        invocation_count: i === 10 ? 10 : 500,
        failure_count: i === 10 ? 2 : 0,
      })),
      ONE_HOUR
    );
    expect(map.rows[0].cells[10].elevated).toBe(true);
  });

  it('reports the busiest cell, and nothing when the window is empty', () => {
    const busy = buildActivityMap(
      spine(60, (i) => ({ invocation_count: i === 12 ? 99 : 1 })),
      ONE_HOUR
    );
    expect(busy.peak?.total).toBe(99);

    expect(buildActivityMap(spine(60), ONE_HOUR).peak).toBeNull();
  });

  it('counts cells with failures separately from elevated ones', () => {
    // The "all failures" toggle says how many extra intervals it will reveal,
    // so the two counts have to be distinguishable: most windows have failures
    // scattered widely and elevated ones rarely.
    const map = buildActivityMap(
      spine(60, (i) => ({
        invocation_count: 100,
        // Two cells fail badly, ten fail a little, the rest are clean.
        failure_count: i < 2 ? 60 : i < 12 ? 1 : 0,
      })),
      ONE_HOUR
    );

    expect(map.cellsWithFailures).toBe(12);
    expect(map.elevatedCells).toBe(2);
    // A single failure in 100 runs is not an incident.
    expect(map.rows[0].cells[5].elevated).toBe(false);
    expect(map.rows[0].cells[5].failed).toBe(1);
  });

  it('reports no failure cells for a clean window', () => {
    const map = buildActivityMap(
      spine(60, () => ({ invocation_count: 10 })),
      ONE_HOUR
    );
    expect(map.cellsWithFailures).toBe(0);
    expect(map.elevatedCells).toBe(0);
  });

  it('produces axis labels that do not overlap', () => {
    const map = buildActivityMap(spine(60), ONE_HOUR);
    expect(map.xLabels.length).toBeGreaterThan(1);
    for (let i = 1; i < map.xLabels.length; i++) {
      const previous = map.xLabels[i - 1];
      expect(map.xLabels[i].column).toBeGreaterThanOrEqual(
        previous.column + previous.span
      );
    }
    expect(map.xLabels[map.xLabels.length - 1].alignEnd).toBe(true);
  });

  it('never places a label outside the grid', () => {
    for (const config of Object.values(ACTIVITY_MAP_CONFIG)) {
      const map = buildActivityMap(spine(config.cols * config.rows), config);
      for (const label of map.xLabels) {
        expect(label.column).toBeGreaterThanOrEqual(1);
        expect(label.column + label.span - 1).toBeLessThanOrEqual(config.cols);
      }
    }
  });

  it('describes the interval a cell covers', () => {
    const map = buildActivityMap(spine(60), ONE_HOUR);
    // One minute wide, so the two clock times differ by exactly a minute.
    expect(formatCellRange(map.rows[0].cells[0])).toMatch(
      /^\w{3} \d{2} · \d{2}:\d{2}–\d{2}:\d{2}$/
    );
  });

  it('matches the bucket width the hook requests for each period', () => {
    // The map labels cells with `grain`; if these drift from BUCKET_WIDTH the
    // label lies about the data.
    const expected: Record<string, number> = {
      '1h': 1,
      '24h': 6,
      '7d': 24,
      '30d': 120,
      '90d': 360,
    };
    for (const [period, config] of Object.entries(ACTIVITY_MAP_CONFIG)) {
      expect(config.unitMinutes).toBe(expected[period]);
      expect(config.cols * config.rows * config.unitMinutes).toBe(
        { '1h': 60, '24h': 1440, '7d': 10080, '30d': 43200, '90d': 129600 }[
          period
        ]
      );
    }
  });
});
