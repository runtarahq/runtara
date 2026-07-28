import { describe, expect, it } from 'vitest';

import { computeMetricTrends, type MetricsDataPoint } from './metric-trends';

/** Two buckets: an earlier half and a later half. */
function series(
  earlier: Partial<MetricsDataPoint>,
  later: Partial<MetricsDataPoint>
): MetricsDataPoint[] {
  return [earlier, later];
}

describe('computeMetricTrends', () => {
  it('measures each metric against its own earlier half', () => {
    const trends = computeMetricTrends(
      series(
        { invocation_count: 10, success_count: 5 },
        { invocation_count: 20, success_count: 20 }
      )
    );

    // Executions doubled.
    expect(trends.executions.change).toBeCloseTo(100);
    expect(trends.executions.trend).toBe('up');
    // Success rate went 50% -> 100%, which is its own change, not the
    // execution change scaled by a constant.
    expect(trends.successRate.change).toBeCloseTo(100);
    expect(trends.successRate.trend).toBe('up');
  });

  it('reports no percentage when the earlier period is empty', () => {
    // The default window is 30 days of hourly buckets, so a tenant that only
    // started running recently has a wholly empty earlier half. "+100%" there
    // would present "no prior data" as a measured doubling.
    const trends = computeMetricTrends(
      series(
        { invocation_count: 0, success_count: 0, avg_duration_seconds: null },
        { invocation_count: 104, success_count: 94, avg_duration_seconds: 43 }
      )
    );

    expect(trends.executions.change).toBeUndefined();
    expect(trends.executions.trend).toBe('stable');
    expect(trends.successRate.change).toBeUndefined();
    expect(trends.duration.change).toBeUndefined();
  });

  it('does not invent a change for a metric that never moved', () => {
    // The bug: every card derived its percentage from the execution change, so
    // a flat metric still reported movement while executions grew.
    const trends = computeMetricTrends(
      series(
        { invocation_count: 10, cancelled_count: 0, failure_count: 0 },
        { invocation_count: 20, cancelled_count: 0, failure_count: 0 }
      )
    );

    expect(trends.executions.change).toBeCloseTo(100);
    expect(trends.cancelled.change).toBeUndefined();
    expect(trends.cancelled.trend).toBe('stable');
    expect(trends.failures.change).toBeUndefined();
    expect(trends.failures.trend).toBe('stable');
  });

  it('treats a drop in duration as an improvement', () => {
    const trends = computeMetricTrends(
      series({ avg_duration_seconds: 10 }, { avg_duration_seconds: 5 })
    );

    expect(trends.duration.trend).toBe('up');
    expect(trends.duration.change).toBeGreaterThan(0);
  });

  it('treats a rise in failures as a regression', () => {
    const trends = computeMetricTrends(
      series({ failure_count: 1 }, { failure_count: 9 })
    );

    expect(trends.failures.trend).toBe('down');
    expect(trends.failures.change).toBeLessThan(0);
  });

  it('reads memory from either API shape', () => {
    const snake = computeMetricTrends(
      series(
        { avg_memory_bytes: 4 * 1024 * 1024 },
        { avg_memory_bytes: 2 * 1024 * 1024 }
      )
    );
    const camel = computeMetricTrends(
      series({ avgMemoryMb: 4 }, { avgMemoryMb: 2 })
    );

    expect(snake.memory.trend).toBe('up'); // halved, so an improvement
    expect(camel.memory).toEqual(snake.memory);
  });

  it('reads counts from the legacy camelCase shape', () => {
    const trends = computeMetricTrends(
      series(
        { invocationCount: 10, successCount: 10, timeoutCount: 4 },
        { invocationCount: 20, successCount: 20, timeoutCount: 1 }
      )
    );

    expect(trends.executions.change).toBeCloseTo(100);
    expect(trends.cancelled.trend).toBe('up'); // fewer cancellations
  });

  it('reports everything flat when there is nothing to compare', () => {
    for (const input of [undefined, null, [], [{ invocation_count: 5 }]]) {
      const trends = computeMetricTrends(input as MetricsDataPoint[]);
      for (const metric of Object.values(trends)) {
        expect(metric).toEqual({ trend: 'stable', change: undefined });
      }
    }
  });

  it('treats missing fields as zero rather than NaN', () => {
    const trends = computeMetricTrends(series({}, {}));

    for (const metric of Object.values(trends)) {
      expect(Number.isNaN(metric.change)).toBe(false);
      expect(metric.change).toBeUndefined();
    }
  });
});
