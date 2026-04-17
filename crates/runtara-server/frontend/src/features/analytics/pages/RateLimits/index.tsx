import { useState, useMemo, useEffect } from 'react';
import { useSearchParams } from 'react-router';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { RefreshCw, Link, X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Icons } from '@/shared/components/icons';
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

  if (isError && !isLoading) {
    const isNetworkError =
      error?.message?.includes('fetch') ||
      (error as { code?: string })?.code === 'ERR_NETWORK' ||
      !(error as { response?: unknown })?.response;

    return (
      <div className="w-full px-4 py-3">
        <div className="flex w-full flex-col gap-3">
          <section className="bg-transparent">
            <div className="flex flex-col gap-2 lg:flex-row lg:items-end lg:justify-between">
              <div className="space-y-0.5">
                <p className="text-xs font-semibold uppercase tracking-[0.2em] text-muted-foreground">
                  Analytics
                </p>
                <h1 className="text-xl font-semibold leading-tight text-slate-900/90 dark:text-slate-100">
                  Rate Limits
                </h1>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <DateRangeSelector
                  value={dateRange}
                  onChange={handleDateRangeChange}
                  options={VALID_DATE_RANGES}
                />
                <Button
                  onClick={handleRefresh}
                  variant="ghost"
                  size="sm"
                  className="h-9 px-3 text-xs font-medium text-muted-foreground hover:text-foreground"
                >
                  <RefreshCw className="h-4 w-4 mr-2" />
                  Refresh
                </Button>
              </div>
            </div>
          </section>

          <div>
            <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-10 text-center">
              <Icons.warning className="mb-4 h-10 w-10 text-destructive" />
              <p className="text-base font-semibold text-foreground">
                {isNetworkError
                  ? 'Unable to connect to backend'
                  : 'An error occurred'}
              </p>
              <p className="mt-1 text-sm text-muted-foreground">
                {isNetworkError
                  ? 'Please check your network connection and try again.'
                  : 'There was a problem loading rate limits. Please try again.'}
              </p>
              {import.meta.env.DEV && error && (
                <div className="mt-4 max-w-md rounded-lg bg-destructive/10 p-3 text-left">
                  <p className="text-xs font-mono text-destructive break-words">
                    {error.message || 'Unknown error'}
                  </p>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="w-full px-4 py-3">
      <div className="flex w-full flex-col gap-3">
        <section className="bg-transparent">
          <div className="flex flex-col gap-2 lg:flex-row lg:items-end lg:justify-between">
            <div className="space-y-0.5">
              <p className="text-xs font-semibold uppercase tracking-[0.2em] text-muted-foreground">
                Analytics
              </p>
              <h1 className="text-xl font-semibold leading-tight text-slate-900/90 dark:text-slate-100">
                Rate Limits
              </h1>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <DateRangeSelector
                value={dateRange}
                onChange={handleDateRangeChange}
                options={VALID_DATE_RANGES}
              />
              <Button
                onClick={handleRefresh}
                variant="ghost"
                size="sm"
                className="h-9 px-3 text-xs font-medium text-muted-foreground hover:text-foreground"
              >
                <RefreshCw className="h-4 w-4 mr-2" />
                Refresh
              </Button>
            </div>
          </div>
        </section>

        {/* Connections Grid */}
        <section>
          <div className="flex items-center justify-between mb-3">
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
                <X className="h-3 w-3 mr-1" />
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
            <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-10 text-center">
              <Link className="mb-4 h-10 w-10 text-muted-foreground" />
              <p className="text-base font-semibold text-foreground">
                No connections found
              </p>
              <p className="mt-1 text-sm text-muted-foreground">
                Create a connection to see rate limit status.
              </p>
            </div>
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
          <div className="text-center text-sm text-muted-foreground py-4">
            Click on a connection card to view its rate limit history
          </div>
        )}
      </div>
    </div>
  );
}
