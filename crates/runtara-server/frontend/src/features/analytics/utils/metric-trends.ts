import type { MetricsBucket } from '@/generated/RuntaraRuntimeApi';

import { calculatePercentageChange } from './index';
import type { MetricsSummary } from './metrics-summary';

/**
 * One metrics bucket, exactly as `GET /api/runtime/metrics/tenant` returns it.
 *
 * An alias rather than a restatement: the endpoint serialises
 * `runtime_types::MetricsBucket` directly, and the generated client describes
 * that type correctly.
 */
export type MetricsDataPoint = MetricsBucket;

export interface MetricTrend {
  /**
   * Whether the move was an improvement, not which way the number went.
   *
   * `up` means better for this metric: more executions, but *fewer* failures.
   * The direction the number itself moved is the sign of `change`.
   */
  trend: 'up' | 'down' | 'stable';
  /**
   * The metric's actual signed change against the preceding period.
   *
   * `undefined` when there is nothing to compare against - no preceding period
   * fetched yet, or one that holds no runs. A percentage change out of zero is
   * undefined, and the card omits the indicator rather than inventing a swing.
   */
  change?: number;
}

export interface MetricTrends {
  executions: MetricTrend;
  successRate: MetricTrend;
  duration: MetricTrend;
  memory: MetricTrend;
  failures: MetricTrend;
  cancelled: MetricTrend;
}

const FLAT: MetricTrend = { trend: 'stable', change: undefined };

export const NO_TRENDS: MetricTrends = {
  executions: FLAT,
  successRate: FLAT,
  duration: FLAT,
  memory: FLAT,
  failures: FLAT,
  cancelled: FLAT,
};

/** Which way is an improvement for a given metric. */
type Direction = 'growth-is-good' | 'shrink-is-good';

/** Below this, a move is noise and the card shows no indicator at all. */
const STABLE_THRESHOLD_PERCENT = 5;

function compare(
  previous: number,
  current: number,
  direction: Direction
): MetricTrend {
  // Nothing to measure against. A tenant whose first runs fall inside this
  // window has no preceding period, and reporting "+100%" would dress up "no
  // prior data" as a measured doubling.
  if (previous === 0) return FLAT;

  // The magnitude is always the metric's own change. Whether that change is
  // welcome is a separate question, answered by `direction`.
  const change = calculatePercentageChange(current, previous);
  if (Math.abs(change) < STABLE_THRESHOLD_PERCENT) {
    return { trend: 'stable', change };
  }

  const grew = change > 0;
  const improved = direction === 'growth-is-good' ? grew : !grew;
  return { trend: improved ? 'up' : 'down', change };
}

/** A rate comparison is only meaningful when both periods actually ran work. */
function compareRates(
  previous: MetricsSummary,
  current: MetricsSummary
): MetricTrend {
  if (previous.totalExecutions === 0 || current.totalExecutions === 0) {
    return FLAT;
  }
  return compare(previous.successRate, current.successRate, 'growth-is-good');
}

/**
 * Compare this window against the equally long window immediately before it.
 *
 * This used to split the *displayed* window in half and compare its later half
 * to its earlier one. That produced a number for every metric without fetching
 * anything, and it was not the comparison the label claimed: on a 24-hour view
 * the "previous 12h" being compared against sat inside the same 24 hours the
 * headline was counting, so the figure and its trend described overlapping
 * data. The preceding period is now fetched, and `previous` is null until it
 * arrives - a comparison the data cannot support is not shown at all.
 */
export function comparePeriods(
  current: MetricsSummary,
  previous: MetricsSummary | null | undefined
): MetricTrends {
  if (!previous) return NO_TRENDS;

  return {
    executions: compare(
      previous.totalExecutions,
      current.totalExecutions,
      'growth-is-good'
    ),
    successRate: compareRates(previous, current),
    duration: compare(
      previous.avgDurationSeconds,
      current.avgDurationSeconds,
      'shrink-is-good'
    ),
    memory: compare(previous.avgMemory, current.avgMemory, 'shrink-is-good'),
    failures: compare(
      previous.failureCount,
      current.failureCount,
      'shrink-is-good'
    ),
    cancelled: compare(
      previous.cancelledCount,
      current.cancelledCount,
      'shrink-is-good'
    ),
  };
}
