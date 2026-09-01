import { describe, expect, it } from 'vitest';

import { comparePeriods, NO_TRENDS } from './metric-trends';
import type { MetricsSummary } from './metrics-summary';

function summary(overrides: Partial<MetricsSummary> = {}): MetricsSummary {
  return {
    totalExecutions: 0,
    successRate: 0,
    avgDurationSeconds: 0,
    failureCount: 0,
    avgMemory: 0,
    cancelledCount: 0,
    ...overrides,
  };
}

describe('comparePeriods', () => {
  it('shows nothing at all without a preceding period', () => {
    // The trends used to be derived by splitting the displayed window in half,
    // so a number always existed. It described overlapping data. With no prior
    // period fetched there is no comparison to make, and the cards say so by
    // showing no indicator.
    for (const previous of [null, undefined]) {
      expect(
        comparePeriods(summary({ totalExecutions: 100 }), previous)
      ).toEqual(NO_TRENDS);
    }
  });

  it('measures each metric against the same metric in the prior period', () => {
    const trends = comparePeriods(
      summary({ totalExecutions: 200, successRate: 99, failureCount: 2 }),
      summary({ totalExecutions: 100, successRate: 90, failureCount: 10 })
    );

    expect(trends.executions.change).toBeCloseTo(100);
    expect(trends.executions.trend).toBe('up');
    expect(trends.successRate.change).toBeCloseTo(10);
    expect(trends.successRate.trend).toBe('up');
    expect(trends.failures.change).toBeCloseTo(-80);
    expect(trends.failures.trend).toBe('up'); // fewer failures is better
  });

  it("reports the metric's own change, not the inverse ratio", () => {
    // Which way a metric *should* move never changes the magnitude reported.
    // Executions halving and duration halving are both -50%; only the
    // sentiment differs. Flipped operands used to render a duration halving as
    // "+100%", and failures rising 1 -> 9 as "-88.9%".
    const executions = comparePeriods(
      summary({ totalExecutions: 50 }),
      summary({ totalExecutions: 100 })
    );
    const duration = comparePeriods(
      summary({ avgDurationSeconds: 50 }),
      summary({ avgDurationSeconds: 100 })
    );

    expect(executions.executions.change).toBeCloseTo(-50);
    expect(duration.duration.change).toBeCloseTo(-50);
    expect(executions.executions.trend).toBe('down'); // fewer runs is worse
    expect(duration.duration.trend).toBe('up'); // faster runs are better
  });

  it('treats a rise in failures as a regression, at its true magnitude', () => {
    const trends = comparePeriods(
      summary({ failureCount: 9 }),
      summary({ failureCount: 1 })
    );

    expect(trends.failures.trend).toBe('down');
    expect(trends.failures.change).toBeCloseTo(800);
  });

  it('reads a memory drop as an improvement', () => {
    const trends = comparePeriods(
      summary({ avgMemory: 4 }),
      summary({ avgMemory: 8 })
    );

    expect(trends.memory.trend).toBe('up');
    expect(trends.memory.change).toBeCloseTo(-50);
  });

  it('calls a move inside the noise threshold stable', () => {
    // A 2% wobble is not news, and the card draws no indicator for it.
    const trends = comparePeriods(
      summary({ totalExecutions: 102 }),
      summary({ totalExecutions: 100 })
    );
    expect(trends.executions.trend).toBe('stable');
  });

  it('reports no percentage when the prior period holds nothing', () => {
    // A tenant whose first runs land inside this window has no baseline.
    // "+100%" there would dress up "no prior data" as a measured doubling.
    const trends = comparePeriods(
      summary({ totalExecutions: 500, failureCount: 3 }),
      summary()
    );
    expect(trends.executions).toEqual({ trend: 'stable', change: undefined });
    expect(trends.failures).toEqual({ trend: 'stable', change: undefined });
  });

  it('does not read an idle period as a collapse in success rate', () => {
    // An idle period has no runs to succeed or fail. Scoring it as 0% rendered
    // a red "100% down" underneath a 100% success rate.
    const idlePrevious = comparePeriods(
      summary({ totalExecutions: 7, successRate: 100 }),
      summary({ totalExecutions: 0, successRate: 0 })
    );
    const idleCurrent = comparePeriods(
      summary({ totalExecutions: 0, successRate: 0 }),
      summary({ totalExecutions: 7, successRate: 100 })
    );

    expect(idlePrevious.successRate).toEqual({
      trend: 'stable',
      change: undefined,
    });
    expect(idleCurrent.successRate).toEqual({
      trend: 'stable',
      change: undefined,
    });
    // The execution count genuinely did move, and that is still reported.
    expect(idleCurrent.executions.trend).toBe('down');
  });

  it('never produces NaN from empty summaries', () => {
    const trends = comparePeriods(summary(), summary());
    for (const metric of Object.values(trends)) {
      expect(Number.isNaN(metric.change)).toBe(false);
      expect(metric.change).toBeUndefined();
    }
  });
});
