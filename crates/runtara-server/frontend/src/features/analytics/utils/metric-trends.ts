import { calculatePercentageChange, determineTrend } from './index';

/**
 * One metrics bucket. Both API shapes are accepted because the server has
 * emitted snake_case and camelCase at different times.
 */
export interface MetricsDataPoint {
  // New API format (snake_case)
  invocation_count?: number | null;
  success_count?: number | null;
  failure_count?: number | null;
  avg_duration_seconds?: number | null;
  avg_memory_bytes?: number | null;
  cancelled_count?: number | null;
  bucket_time?: string | null;
  success_rate_percent?: number | null;
  // Old API format (camelCase)
  invocationCount?: number | null;
  successCount?: number | null;
  failureCount?: number | null;
  avgDurationSeconds?: number | null;
  avgMemoryMb?: number | null;
  timeoutCount?: number | null;
  dayBucket?: string | null;
  successRatePercent?: number | null;
}

export interface MetricTrend {
  trend: 'up' | 'down' | 'stable';
  /**
   * Percentage change behind `trend`, measured on this metric's own values.
   * Signed consistently with `trend` — the card renders its sign from `trend`
   * and its magnitude from here, so the two must be derived from the same
   * comparison.
   *
   * `undefined` when the earlier period holds no data for this metric: a
   * percentage change out of zero is undefined, and the card omits the
   * indicator rather than claiming a measured swing.
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

/**
 * Which way is an improvement for a given metric.
 *
 * Stated explicitly rather than encoded in argument order, so that "is the
 * baseline empty?" stays a question about the earlier period and not about
 * whichever operand happens to sit first.
 */
type Direction = 'growth-is-good' | 'shrink-is-good';

/** Compare a metric's earlier half against its later half. */
function compare(
  earlier: number,
  later: number,
  direction: Direction
): MetricTrend {
  // Nothing to measure against. The dashboard's default window is 30 days of
  // hourly buckets, so a tenant that only started running recently has an
  // entirely empty earlier half — reporting "+100%" there would dress up
  // "no prior data" as a measured doubling.
  if (earlier === 0) return FLAT;

  // Flipping the operands is what makes "up" read as an improvement for the
  // metrics we want to see fall.
  const [current, previous] =
    direction === 'growth-is-good' ? [later, earlier] : [earlier, later];

  return {
    trend: determineTrend(current, previous),
    change: calculatePercentageChange(current, previous),
  };
}

function sum(
  points: MetricsDataPoint[],
  pick: (p: MetricsDataPoint) => number
) {
  return points.reduce((total, point) => total + pick(point), 0);
}

function mean(
  points: MetricsDataPoint[],
  pick: (p: MetricsDataPoint) => number
) {
  return points.length === 0 ? 0 : sum(points, pick) / points.length;
}

const executionsOf = (p: MetricsDataPoint) =>
  p.invocation_count ?? p.invocationCount ?? 0;
const successesOf = (p: MetricsDataPoint) =>
  p.success_count ?? p.successCount ?? 0;
const failuresOf = (p: MetricsDataPoint) =>
  p.failure_count ?? p.failureCount ?? 0;
const cancelledOf = (p: MetricsDataPoint) =>
  p.cancelled_count ?? p.timeoutCount ?? 0;
const durationOf = (p: MetricsDataPoint) =>
  p.avg_duration_seconds ?? p.avgDurationSeconds ?? 0;
const memoryMbOf = (p: MetricsDataPoint) =>
  p.avg_memory_bytes !== undefined && p.avg_memory_bytes !== null
    ? p.avg_memory_bytes / (1024 * 1024)
    : (p.avgMemoryMb ?? 0);

function successRateOf(points: MetricsDataPoint[]): number {
  const executions = sum(points, executionsOf);
  return executions > 0 ? (sum(points, successesOf) / executions) * 100 : 0;
}

/**
 * Half-over-half trend for every KPI on the usage dashboard.
 *
 * Each metric is compared against its own earlier half. Previously only the
 * execution count was measured and the other cards derived their percentage
 * from it by a fixed multiplier, which produced numbers with no relationship
 * to the metric they sat under — including a positive change on a metric whose
 * value was zero.
 */
export function computeMetricTrends(
  dataPoints: MetricsDataPoint[] | undefined | null
): MetricTrends {
  // A single bucket has nothing to compare against.
  if (!dataPoints || dataPoints.length < 2) return NO_TRENDS;

  const midPoint = Math.floor(dataPoints.length / 2);
  const earlier = dataPoints.slice(0, midPoint);
  const later = dataPoints.slice(midPoint);

  return {
    executions: compare(
      sum(earlier, executionsOf),
      sum(later, executionsOf),
      'growth-is-good'
    ),
    successRate: compare(
      successRateOf(earlier),
      successRateOf(later),
      'growth-is-good'
    ),
    duration: compare(
      mean(earlier, durationOf),
      mean(later, durationOf),
      'shrink-is-good'
    ),
    memory: compare(
      mean(earlier, memoryMbOf),
      mean(later, memoryMbOf),
      'shrink-is-good'
    ),
    failures: compare(
      sum(earlier, failuresOf),
      sum(later, failuresOf),
      'shrink-is-good'
    ),
    cancelled: compare(
      sum(earlier, cancelledOf),
      sum(later, cancelledOf),
      'shrink-is-good'
    ),
  };
}
