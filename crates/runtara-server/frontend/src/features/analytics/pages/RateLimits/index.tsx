import { useState, useMemo, useEffect } from 'react';
import { useSearchParams } from 'react-router';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { RefreshCw, Link, X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  Breadcrumb,
  ConsoleEmptyState,
  ConsoleErrorState,
  ConsoleTableShell,
  ConsoleToolbar,
} from '@/shared/components/console';
import {
  DateRangeOption,
  DateRangeSelector,
} from '@/shared/components/date-range-selector';
import {
  useRateLimits,
  useConnectionRateLimitHistory,
  useConnectionRateLimitTimeline,
} from '../../hooks/useRateLimits';
import {
  RateLimitCard,
  RateLimitCardSkeleton,
} from '../../components/RateLimitCard';
import {
  RateLimitHistory,
  RateLimitHistorySkeleton,
} from '../../components/RateLimitHistory';
import type { RateLimitStatusDto } from '@/generated/RuntaraRuntimeApi';
import { getGranularity, fillBuckets } from '../../utils/timeline';

const VALID_DATE_RANGES: DateRangeOption[] = ['1h', '24h', '7d', '30d'];

function getDateRangeMs(range: DateRangeOption): number {
  const ONE_HOUR = 60 * 60 * 1000;
  const ONE_DAY = 24 * ONE_HOUR;

  switch (range) {
    case '1h':
      return ONE_HOUR;
    case '24h':
      return ONE_DAY;
    case '7d':
      return 7 * ONE_DAY;
    case '30d':
      return 30 * ONE_DAY;
    default:
      return ONE_DAY;
  }
}

export function RateLimits() {
  usePageTitle('Rate Limits');

  const [searchParams, setSearchParams] = useSearchParams();
  const [selectedConnectionId, setSelectedConnectionId] = useState<
    string | null
  >(null);

  const urlPeriod = searchParams.get('period') as DateRangeOption;
  const initialDateRange = VALID_DATE_RANGES.includes(urlPeriod)
    ? urlPeriod
    : '24h';
  const [dateRange, setDateRange] = useState<DateRangeOption>(initialDateRange);

  useEffect(() => {
    const newParams = new URLSearchParams(searchParams);
    newParams.set('period', dateRange);
    setSearchParams(newParams, { replace: true });
  }, [dateRange, searchParams, setSearchParams]);

  const {
    data: rateLimitsResponse,
    isLoading,
    refetch,
    isError,
    error,
  } = useRateLimits(dateRange);

  const granularity = getGranularity(dateRange);

  const { startTime, endTime } = useMemo(() => {
    const now = new Date();
    return {
      startTime: new Date(now.getTime() - getDateRangeMs(dateRange)),
      endTime: now,
    };
  }, [dateRange]);

  const historyFrom = startTime.toISOString();

  const { data: historyResponse, isLoading: historyLoading } =
    useConnectionRateLimitHistory({
      connectionId: selectedConnectionId,
      from: historyFrom,
      limit: 1000,
    });

  const { data: timelineResponse, isLoading: timelineLoading } =
    useConnectionRateLimitTimeline({
      connectionId: selectedConnectionId,
      startTime: startTime.toISOString(),
      endTime: endTime.toISOString(),
      granularity,
      dateRange,
    });

  const timelineBuckets = useMemo(() => {
    const rawBuckets = timelineResponse?.data?.buckets ?? [];
    return fillBuckets(rawBuckets, startTime, endTime, granularity);
  }, [timelineResponse, startTime, endTime, granularity]);

  const handleRefresh = () => {
    refetch();
  };

  const handleDateRangeChange = (value: DateRangeOption) => {
    setDateRange(value);
  };

  const handleSelectConnection = (connection: RateLimitStatusDto) => {
    if (selectedConnectionId === connection.connectionId) {
      setSelectedConnectionId(null);
    } else {
      setSelectedConnectionId(connection.connectionId);
    }
  };

  const rateLimits = rateLimitsResponse?.data ?? [];
  const selectedConnection = rateLimits.find(
    (r) => r.connectionId === selectedConnectionId
  );

  return (
    <ConsoleTableShell
      bodyClassName="p-4 md:p-6"
      toolbar={
        <ConsoleToolbar
          left={
            <Breadcrumb
              items={[
                { label: 'Analytics', to: '/analytics/usage' },
                { label: 'Rate Limits' },
              ]}
            />
          }
          actions={
            <div className="flex items-center gap-2">
              <DateRangeSelector
                value={dateRange}
                onChange={handleDateRangeChange}
                options={VALID_DATE_RANGES}
              />
              <Button
                onClick={handleRefresh}
                variant="outline"
                size="sm"
                className="text-muted-foreground"
              >
                <RefreshCw className="mr-2 size-4" />
                Refresh
              </Button>
            </div>
          }
        />
      }
    >
      {isError && !isLoading ? (
        <ConsoleErrorState
          error={error}
          entityLabel="rate limits"
          className="h-auto rounded-lg border bg-muted/20"
        />
      ) : (
        <div className="space-y-4">
          {/* Connections Grid */}
          <section>
            <div className="mb-3 flex items-center justify-between">
              <h2 className="text-sm font-medium text-muted-foreground">
                Connections ({rateLimits.length})
              </h2>
              {selectedConnectionId && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedConnectionId(null)}
                  className="h-7 px-2 text-xs"
                >
                  <X className="mr-1 size-3" />
                  Clear selection
                </Button>
              )}
            </div>
            {isLoading ? (
              <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
                {Array.from({ length: 6 }).map((_, index) => (
                  <RateLimitCardSkeleton key={index} />
                ))}
              </div>
            ) : rateLimits.length === 0 ? (
              <ConsoleEmptyState
                icon={<Link className="mb-4 size-10 text-muted-foreground" />}
                title="No connections found"
                description="Create a connection to see rate limit status."
                className="h-auto rounded-lg border bg-muted/20"
              />
            ) : (
              <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
                {rateLimits.map((rateLimitStatus) => (
                  <RateLimitCard
                    key={rateLimitStatus.connectionId}
                    rateLimitStatus={rateLimitStatus}
                    onClick={() => handleSelectConnection(rateLimitStatus)}
                    selected={
                      selectedConnectionId === rateLimitStatus.connectionId
                    }
                  />
                ))}
              </div>
            )}
          </section>

          {/* History Section */}
          {selectedConnection && (
            <section className="mt-4">
              {historyLoading || timelineLoading ? (
                <RateLimitHistorySkeleton />
              ) : (
                <RateLimitHistory
                  events={historyResponse?.data ?? []}
                  status={selectedConnection}
                  loading={historyLoading || timelineLoading}
                  timelineBuckets={timelineBuckets}
                  granularity={granularity}
                />
              )}
            </section>
          )}

          {/* Hint when no connection selected */}
          {!selectedConnectionId && rateLimits.length > 0 && !isLoading && (
            <div className="py-4 text-center text-sm text-muted-foreground">
              Click on a connection card to view its rate limit history
            </div>
          )}
        </div>
      )}
    </ConsoleTableShell>
  );
}
