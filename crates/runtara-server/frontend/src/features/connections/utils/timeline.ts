import { format, parseISO } from 'date-fns';
import type { DateRangeOption } from '@/shared/components/date-range-selector';
import type { RateLimitTimelineBucket } from '@/generated/RuntaraRuntimeApi';

export type Granularity = 'minute' | 'hourly' | 'daily';

export function getGranularity(dateRange: DateRangeOption): Granularity {
  switch (dateRange) {
    case '1h':
      return 'minute';
    case '24h':
      return 'hourly';
    case '7d':
      return 'hourly';
    case '30d':
    case '90d':
      return 'daily';
    default:
      return 'hourly';
  }
}

const INTERVAL_MS: Record<Granularity, number> = {
  minute: 60_000,
  hourly: 3_600_000,
  daily: 86_400_000,
};

/**
 * Normalizes an ISO 8601 timestamp to millisecond epoch for consistent comparison.
 * The API may return "2026-02-12T14:00:00Z" while Date.toISOString() produces
 * "2026-02-12T14:00:00.000Z" — using epoch ms avoids string mismatch.
 */
function toEpochMs(iso: string): number {
  return new Date(iso).getTime();
}

export function fillBuckets(
  buckets: RateLimitTimelineBucket[],
  startTime: Date,
  endTime: Date,
  granularity: Granularity
): RateLimitTimelineBucket[] {
  const intervalMs = INTERVAL_MS[granularity];
  const bucketMap = new Map(buckets.map((b) => [toEpochMs(b.bucket), b]));
  const result: RateLimitTimelineBucket[] = [];

  const current = new Date(startTime);
  // Truncate to granularity boundary
  if (granularity === 'minute') current.setSeconds(0, 0);
  if (granularity === 'hourly') current.setMinutes(0, 0, 0);
  if (granularity === 'daily') current.setHours(0, 0, 0, 0);

  while (current < endTime) {
    const key = current.getTime();
    const iso = current.toISOString();
    result.push(
      bucketMap.get(key) ?? {
        bucket: iso,
        requestCount: 0,
        rateLimitedCount: 0,
        retryCount: 0,
      }
    );
    current.setTime(current.getTime() + intervalMs);
  }

  return result;
}

export function formatBucketLabel(
  bucket: string,
  granularity: Granularity
): string {
  const date = parseISO(bucket);
  switch (granularity) {
    case 'minute':
      return format(date, 'HH:mm');
    case 'hourly':
      return format(date, 'MMM dd HH:00');
    case 'daily':
      return format(date, 'MM/dd');
  }
}
