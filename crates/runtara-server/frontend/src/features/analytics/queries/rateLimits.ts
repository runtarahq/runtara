import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

export async function getRateLimits(token: string, interval?: string) {
  const result = await RuntimeREST.api.listRateLimitsHandler(
    interval ? { interval } : undefined,
    createAuthHeaders(token)
  );
  return result.data;
}

export interface RateLimitHistoryParams {
  connectionId: string;
  limit?: number;
  offset?: number;
  eventType?: string;
  from?: string;
  to?: string;
}

export async function getConnectionRateLimitHistory(
  token: string,
  params: RateLimitHistoryParams
) {
  const { connectionId, ...query } = params;
  const result = await RuntimeREST.api.getConnectionRateLimitHistoryHandler(
    connectionId,
    {
      limit: query.limit,
      offset: query.offset,
      event_type: query.eventType,
      from: query.from,
      to: query.to,
    },
    createAuthHeaders(token)
  );
  return result.data;
}

export interface RateLimitTimelineParams {
  connectionId: string;
  startTime?: string;
  endTime?: string;
  granularity?: 'minute' | 'hourly' | 'daily';
  tag?: string;
}

export async function getConnectionRateLimitTimeline(
  token: string,
  params: RateLimitTimelineParams
) {
  const { connectionId, ...query } = params;
  const result = await RuntimeREST.api.getConnectionRateLimitTimelineHandler(
    connectionId,
    {
      startTime: query.startTime,
      endTime: query.endTime,
      granularity: query.granularity,
      tag: query.tag,
    },
    createAuthHeaders(token)
  );
  return result.data;
}
