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
 * Fetch tenant-level metrics aggregated across all scenarios
 * @param dateRange - Date range option
 */
export function useTenantMetrics(dateRange: DateRangeOption) {
  const params = useMemo(() => getDateRangeParams(dateRange), [dateRange]);

  return useCustomQuery({
    queryKey: queryKeys.analytics.tenant(dateRange),
    queryFn: (token: string) =>
      getTenantMetrics(token, params.startTime, params.endTime),
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
