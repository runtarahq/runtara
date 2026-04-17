import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { Badge } from '@/shared/components/ui/badge';
import { format, parseISO } from 'date-fns';
import type {
  RateLimitEventDto,
  RateLimitStatusDto,
  RateLimitTimelineBucket,
} from '@/generated/RuntaraRuntimeApi';
import { RateLimitTimelineChart } from '../RateLimitTimelineChart';
import type { Granularity } from '../../utils/timeline';

interface RateLimitHistoryProps {
  events: RateLimitEventDto[];
  status: RateLimitStatusDto;
  loading?: boolean;
  timelineBuckets: RateLimitTimelineBucket[];
  granularity: Granularity;
}

function RecentEventsTable({ events }: { events: RateLimitEventDto[] }) {
  const recentEvents = events.slice(0, 20);

  if (recentEvents.length === 0) {
    return (
      <div className="text-center text-sm text-muted-foreground py-4">
        No recent events
      </div>
    );
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b">
            <th className="text-left py-2 px-2 font-medium text-muted-foreground">
              Time
            </th>
            <th className="text-left py-2 px-2 font-medium text-muted-foreground">
              Event
            </th>
            <th className="text-left py-2 px-2 font-medium text-muted-foreground">
              Details
            </th>
          </tr>
        </thead>
        <tbody>
          {recentEvents.map((event) => (
            <tr key={event.id} className="border-b last:border-0">
              <td className="py-2 px-2 text-xs text-muted-foreground whitespace-nowrap">
                {format(parseISO(event.createdAt), 'MMM dd HH:mm:ss')}
              </td>
              <td className="py-2 px-2">
                <Badge
                  variant={
                    event.eventType === 'request'
                      ? 'default'
                      : event.eventType === 'rate_limited'
                        ? 'destructive'
                        : 'outline'
                  }
                  className="text-xs"
                >
                  {event.eventType === 'rate_limited'
                    ? 'Rate Limited'
                    : event.eventType}
                </Badge>
              </td>
              <td className="py-2 px-2 text-xs text-muted-foreground max-w-[200px] truncate">
                {event.metadata ? JSON.stringify(event.metadata) : '-'}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function RateLimitHistory({
  events,
  status,
  loading,
  timelineBuckets,
  granularity,
}: RateLimitHistoryProps) {
  if (loading) {
    return (
      <div className="space-y-4">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">
              Rate Limit History: {status.connectionTitle}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="h-[200px] bg-muted animate-pulse rounded" />
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader className="pb-2">
          <div className="flex items-center justify-between">
            <CardTitle className="text-base">
              Rate Limit History: {status.connectionTitle}
            </CardTitle>
            <Badge
              variant={status.metrics.isRateLimited ? 'destructive' : 'success'}
            >
              {status.metrics.isRateLimited ? 'Rate Limited' : 'OK'}
            </Badge>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          <RateLimitTimelineChart
            buckets={timelineBuckets}
            granularity={granularity}
          />
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-base">Recent Events</CardTitle>
        </CardHeader>
        <CardContent>
          <RecentEventsTable events={events} />
        </CardContent>
      </Card>
    </div>
  );
}

export function RateLimitHistorySkeleton() {
  return (
    <div className="space-y-4">
      <Card>
        <CardHeader>
          <div className="h-5 w-48 bg-muted animate-pulse rounded" />
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
            {Array.from({ length: 4 }).map((_, i) => (
              <div key={i} className="h-16 bg-muted animate-pulse rounded-lg" />
            ))}
          </div>
          <div className="h-[200px] bg-muted animate-pulse rounded" />
        </CardContent>
      </Card>
    </div>
  );
}
