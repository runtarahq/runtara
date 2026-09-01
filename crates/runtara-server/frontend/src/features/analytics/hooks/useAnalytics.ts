import { useCustomQuery } from '@/shared/hooks/api';
import { useMemo } from 'react';
import { getTenantMetrics, getSystemAnalytics } from '../queries';
import { subDays, subHours } from 'date-fns';
import { DateRangeOption } from '@/shared/components/date-range-selector';
import { queryKeys } from '@/shared/queries/query-keys';

/**
 * Get date range parameters in ISO 8601 format for Runtime API
 * @param range - Date range option
 * @returns Object with startTime and endTime in ISO 8601 format
 */
function getDateRangeParams(range: DateRangeOption) {
  const now = new Date();
  let from: Date;

  switch (range) {
    case '1h':
      from = subHours(now, 1);
      break;
    case '24h':
      from = subHours(now, 24);
      break;
    case '7d':
      from = subDays(now, 7);
      break;
    case '30d':
      from = subDays(now, 30);
      break;
    case '90d':
      from = subDays(now, 90);
      break;
    default:
      from = subDays(now, 30);
  }

  return {
    startTime: from.toISOString(),
    endTime: now.toISOString(),
  };
}

/**
 * Bucket width to request for each period.
 *
 * Chosen so the activity map gets a full grid of squares without asking for
 * more than it can legibly draw, and so a long window is not paid for at
 * hourly resolution: 90 days used to return 2161 buckets on every poll and now
 * returns 361.
 */
export const BUCKET_WIDTH: Record<DateRangeOption, string> = {
  '1h': '1m', // 61 buckets
  '24h': '6m', // 241
  '7d': '24m', // 421
  '30d': '2h', // 361
  '90d': '6h', // 361
};

/**
 * Bucket width for the preceding period, which is only ever summed.
 *
 * One bucket spanning the whole window: the comparison needs totals and
 * averages, not a shape, so asking for the same resolution as the displayed
 * window would fetch hundreds of rows to add them all up again.
 */
const PREVIOUS_PERIOD_WIDTH: Record<DateRangeOption, string> = {
  '1h': '1h',
  '24h': '1d',
  '7d': '7d',
  '30d': '30d',
  '90d': '90d',
};

/** Milliseconds each period covers, used to step back one whole window. */
const PERIOD_MS: Record<DateRangeOption, number> = {
  '1h': 60 * 60 * 1000,
  '24h': 24 * 60 * 60 * 1000,
  '7d': 7 * 24 * 60 * 60 * 1000,
  '30d': 30 * 24 * 60 * 60 * 1000,
  '90d': 90 * 24 * 60 * 60 * 1000,
};

/**
 * Fetch tenant-level metrics aggregated across all workflows
 * @param dateRange - Date range option
 */
export function useTenantMetrics(dateRange: DateRangeOption) {
  const params = useMemo(() => getDateRangeParams(dateRange), [dateRange]);
  const granularity = BUCKET_WIDTH[dateRange] ?? 'hourly';

  return useCustomQuery({
    // Granularity is part of the key: two periods could otherwise share a
    // cached response bucketed at a width neither asked for.
    queryKey: queryKeys.analytics.tenant(dateRange, granularity),
    queryFn: (token: string) =>
      getTenantMetrics(token, params.startTime, params.endTime, granularity),
    refetchInterval: 60 * 1000, // Refresh every 60 seconds
    refetchIntervalInBackground: false,
  });
}

/**
 * Fetch system analytics including memory, disk space, and CPU information
 */
export function useSystemAnalytics() {
  return useCustomQuery({
    queryKey: queryKeys.analytics.system(),
    queryFn: (token: string) => getSystemAnalytics(token),
    refetchInterval: 30 * 1000, // Refresh every 30 seconds for real-time system info
    refetchIntervalInBackground: false,
  });
}

/**
 * Fetch the equally long period immediately before the displayed one.
 *
 * Exists so the dashboard's trend indicators describe a real comparison. They
 * previously split the displayed window in half and compared its halves, which
 * needed no request but was not what the label claimed: on a 24-hour view the
 * "previous 12h" sat inside the 24 hours the headline counted.
 *
 * Deliberately cheap - a single bucket, so the response is one row - and
 * deliberately allowed to fail: the cards show no indicator rather than a
 * fabricated one when this has not arrived.
 */
export function usePreviousTenantMetrics(dateRange: DateRangeOption) {
  const params = useMemo(() => {
    const current = getDateRangeParams(dateRange);
    const span = PERIOD_MS[dateRange] ?? PERIOD_MS['30d'];
    const end = new Date(current.startTime);
    return {
      startTime: new Date(end.getTime() - span).toISOString(),
      endTime: end.toISOString(),
    };
  }, [dateRange]);

  const granularity = PREVIOUS_PERIOD_WIDTH[dateRange] ?? 'daily';

  return useCustomQuery({
    queryKey: queryKeys.analytics.tenantPrevious(dateRange, granularity),
    queryFn: (token: string) =>
      getTenantMetrics(token, params.startTime, params.endTime, granularity),
    refetchInterval: 60 * 1000,
    refetchIntervalInBackground: false,
  });
}
