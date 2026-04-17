import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  getRateLimits,
  getConnectionRateLimitHistory,
  getConnectionRateLimitTimeline,
  type RateLimitHistoryParams,
  type RateLimitTimelineParams,
} from '../queries/rateLimits';

export function useRateLimits(interval?: string) {
  return useCustomQuery({
    queryKey: ['analytics', 'rateLimits', interval],
    queryFn: (token: string) => getRateLimits(token, interval),
    refetchInterval: 30 * 1000,
    refetchIntervalInBackground: false,
  });
}

export function useConnectionRateLimitTimeline(
  params: Omit<RateLimitTimelineParams, 'connectionId'> & {
    connectionId: string | null;
    dateRange: string;
  }
) {
  const { connectionId, dateRange, ...query } = params;
  return useCustomQuery({
    queryKey: queryKeys.analytics.rateLimitTimeline(
      connectionId ?? '',
      dateRange
    ),
    queryFn: (token: string) =>
      getConnectionRateLimitTimeline(token, {
        connectionId: connectionId!,
        ...query,
      }),
    enabled: !!connectionId,
    refetchInterval: 30 * 1000,
    refetchIntervalInBackground: false,
  });
}

export function useConnectionRateLimitHistory(
  params: Omit<RateLimitHistoryParams, 'connectionId'> & {
    connectionId: string | null;
  }
) {
  const { connectionId, from, to, limit } = params;
  return useCustomQuery({
    queryKey: ['analytics', 'rateLimitHistory', connectionId, from, to, limit],
    queryFn: (token: string) =>
      getConnectionRateLimitHistory(token, {
        connectionId: connectionId!,
        from,
        to,
        limit,
      }),
    enabled: !!connectionId,
    refetchInterval: 30 * 1000,
    refetchIntervalInBackground: false,
  });
}
