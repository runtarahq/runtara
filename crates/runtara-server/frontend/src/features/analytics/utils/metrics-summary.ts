import type { MetricsDataPoint } from './metric-trends';

export interface MetricsSummary {
  totalExecutions: number;
  successRate: number;
  avgDurationSeconds: number;
  failureCount: number;
  avgMemory: number;
  cancelledCount: number;
}

/// Summarise the tenant metric buckets behind the Usage cards.
///
/// Extracted from the page so the arithmetic is testable on its own. It was
/// not, and the averages were wrong by three orders of magnitude in a way the
/// chart beside them did not share.
export function summarizeMetrics(
  metrics: MetricsDataPoint[] | undefined | null
): MetricsSummary {
  if (!metrics || metrics.length === 0) {
    return {
      totalExecutions: 0,
      successRate: 0,
      avgDurationSeconds: 0,
      failureCount: 0,
      avgMemory: 0,
      cancelledCount: 0,
    };
  }

  const dataPoints = metrics;

  const totalExecutions = dataPoints.reduce(
    (sum, point) =>
      sum + (point.invocation_count ?? point.invocationCount ?? 0),
    0
  );

  const totalSuccesses = dataPoints.reduce(
    (sum, point) => sum + (point.success_count ?? point.successCount ?? 0),
    0
  );
  const successRate =
    totalExecutions > 0 ? (totalSuccesses / totalExecutions) * 100 : 0;

  // Weight each bucket by how many executions it represents, and skip the
  // ones that hold none. A bucket with no executions reports a null average,
  // and `??` turns that null into `undefined`, which `!== null` accepts - so
  // every empty bucket used to land in the divisor while adding nothing to
  // the sum. Over a 30-day window that is ~720 empty hours against one real
  // one, which understated the figure by about three orders of magnitude.
  // An unweighted mean of per-bucket means would be wrong too once more than
  // one bucket has data, since the buckets carry different counts.
  const durationWeighted = dataPoints.reduce(
    (acc, point) => {
      const value = point.avg_duration_seconds ?? point.avgDurationSeconds;
      const count = point.invocation_count ?? point.invocationCount ?? 0;
      if (value === null || value === undefined || count <= 0) return acc;
      return { total: acc.total + value * count, count: acc.count + count };
    },
    { total: 0, count: 0 }
  );
  const avgDurationSeconds =
    durationWeighted.count > 0
      ? durationWeighted.total / durationWeighted.count
      : 0;

  const failureCount = dataPoints.reduce(
    (sum, point) => sum + (point.failure_count ?? point.failureCount ?? 0),
    0
  );

  // Same weighting, and the same empty-bucket trap, as the duration above.
  // Supports both the old (avgMemoryMb) and current (avg_memory_bytes) shapes.
  const memoryWeighted = dataPoints.reduce(
    (acc, point) => {
      const mb =
        point.avg_memory_bytes !== undefined && point.avg_memory_bytes !== null
          ? point.avg_memory_bytes / (1024 * 1024)
          : point.avgMemoryMb;
      const count = point.invocation_count ?? point.invocationCount ?? 0;
      if (mb === null || mb === undefined || count <= 0) return acc;
      return { total: acc.total + mb * count, count: acc.count + count };
    },
    { total: 0, count: 0 }
  );
  const avgMemory =
    memoryWeighted.count > 0 ? memoryWeighted.total / memoryWeighted.count : 0;

  // Support both old (timeoutCount) and new (cancelled_count) API formats
  const cancelledCount = dataPoints.reduce(
    (sum, point) => sum + (point.cancelled_count ?? point.timeoutCount ?? 0),
    0
  );

  return {
    totalExecutions,
    successRate,
    avgDurationSeconds,
    failureCount,
    avgMemory,
    cancelledCount,
  };
}
