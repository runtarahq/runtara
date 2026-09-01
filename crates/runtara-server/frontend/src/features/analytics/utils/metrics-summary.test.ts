import { describe, expect, it } from 'vitest';
import { summarizeMetrics } from './metrics-summary';
import type { MetricsDataPoint } from './metric-trends';

/** One hour that ran work, padded with `empties` hours that ran none. */
function window(empties: number, active: Partial<MetricsDataPoint>[]) {
  const blank: MetricsDataPoint[] = Array.from({ length: empties }, () => ({
    invocation_count: 0,
    success_count: 0,
    failure_count: 0,
    cancelled_count: 0,
    avg_duration_seconds: null,
    avg_memory_bytes: null,
  })) as MetricsDataPoint[];
  return [...blank, ...active] as MetricsDataPoint[];
}

describe('summarizeMetrics', () => {
  it('ignores empty buckets when averaging', () => {
    // A 30-day window is ~720 hourly buckets. With one active hour, dividing
    // by every bucket rather than by the executions understated these by ~720x
    // (0.2235s read as 310us; 1245184 bytes read as 1.7KB).
    const s = summarizeMetrics(
      window(720, [
        {
          invocation_count: 89650,
          success_count: 89650,
          avg_duration_seconds: 0.2235,
          avg_memory_bytes: 1245184,
        },
      ])
    );
    expect(s.totalExecutions).toBe(89650);
    expect(s.successRate).toBe(100);
    expect(s.avgDurationSeconds).toBeCloseTo(0.2235, 6);
    expect(s.avgMemory).toBeCloseTo(1245184 / (1024 * 1024), 6);
  });

  it('weights buckets by their execution count', () => {
    // A plain mean of the two averages would give 3s and 3MB; the busy bucket
    // has 9x the executions, so the answer is much closer to its value.
    const s = summarizeMetrics(
      window(0, [
        {
          invocation_count: 900,
          success_count: 900,
          avg_duration_seconds: 1,
          avg_memory_bytes: 1024 * 1024,
        },
        {
          invocation_count: 100,
          success_count: 100,
          avg_duration_seconds: 5,
          avg_memory_bytes: 5 * 1024 * 1024,
        },
      ])
    );
    expect(s.avgDurationSeconds).toBeCloseTo(1.4, 6);
    expect(s.avgMemory).toBeCloseTo(1.4, 6);
  });

  it('reports zero rather than dividing by nothing', () => {
    expect(summarizeMetrics(window(24, [])).avgDurationSeconds).toBe(0);
    expect(summarizeMetrics([]).totalExecutions).toBe(0);
    expect(summarizeMetrics(undefined).avgMemory).toBe(0);
  });

  it('reads every count and average off the wire shape', () => {
    // This replaces a test that asserted support for a camelCase shape
    // (`invocationCount`, `timeoutCount`, `avgMemoryMb`). The OpenAPI DTO
    // declared that shape; the handler never emitted it, serialising
    // `MetricsBucket` in snake_case instead. The test only compiled because it
    // cast through `unknown`. Memory is bytes on the wire, not megabytes.
    const s = summarizeMetrics([
      {
        bucket_time: '2026-01-05T00:00:00Z',
        invocation_count: 10,
        success_count: 8,
        failure_count: 2,
        cancelled_count: 1,
        avg_memory_bytes: 4 * 1024 * 1024,
      },
    ] as MetricsDataPoint[]);
    expect(s.totalExecutions).toBe(10);
    expect(s.successRate).toBe(80);
    expect(s.failureCount).toBe(2);
    expect(s.cancelledCount).toBe(1);
    expect(s.avgMemory).toBeCloseTo(4, 6);
  });
});
