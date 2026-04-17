import { useState, useMemo, useEffect } from 'react';
import { useSearchParams } from 'react-router';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { RefreshCw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Icons } from '@/shared/components/icons';
import {
  DateRangeOption,
  DateRangeSelector,
} from '@/shared/components/date-range-selector';
import { MetricCard } from '@/shared/components/metric-card';
import { ExecutionTrendChart } from '../../components/ExecutionTrendChart';
import { useTenantMetrics } from '../../hooks/useAnalytics';
import {
  calculatePercentageChange,
  determineTrend,
  formatDurationSeconds,
  formatMemory,
  formatNumber,
} from '../../utils';

// Unified metrics data point type supporting both old (camelCase) and new (snake_case) API formats
interface MetricsDataPoint {
  // New API format (snake_case)
  invocation_count?: number | null;
  success_count?: number | null;
  failure_count?: number | null;
  avg_duration_seconds?: number | null;
  avg_memory_bytes?: number | null;
  cancelled_count?: number | null;
  bucket_time?: string | null;
  success_rate_percent?: number | null;
  // Old API format (camelCase)
  invocationCount?: number | null;
  successCount?: number | null;
  failureCount?: number | null;
  avgDurationSeconds?: number | null;
  avgMemoryMb?: number | null;
  timeoutCount?: number | null;
  dayBucket?: string | null;
  successRatePercent?: number | null;
}

const VALID_DATE_RANGES: DateRangeOption[] = ['1h', '24h', '7d', '30d', '90d'];

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
    case '90d':
      return 90 * ONE_DAY;
    default:
      return 30 * ONE_DAY;
  }
}

export function Usage() {
  usePageTitle('Usage Analytics');

  const [searchParams, setSearchParams] = useSearchParams();

  const urlPeriod = searchParams.get('period') as DateRangeOption;

  const initialDateRange = VALID_DATE_RANGES.includes(urlPeriod)
    ? urlPeriod
    : '30d';

  const [dateRange, setDateRange] = useState<DateRangeOption>(initialDateRange);

  useEffect(() => {
    const newParams = new URLSearchParams();
    newParams.set('period', dateRange);
    setSearchParams(newParams, { replace: true });
  }, [dateRange, setSearchParams]);

  const {
    data: tenantMetrics,
    isLoading: metricsLoading,
    refetch: refetchMetrics,
    isError: metricsError,
    error,
  } = useTenantMetrics(dateRange);

  const handleRefresh = () => {
    refetchMetrics();
  };

  const handleDateRangeChange = (value: DateRangeOption) => {
    setDateRange(value);
  };

  const metrics = useMemo(() => {
    if (
      !tenantMetrics?.data?.metrics ||
      tenantMetrics.data.metrics.length === 0
    ) {
      return {
        totalExecutions: 0,
        successRate: 0,
        avgDurationSeconds: 0,
        failureCount: 0,
        avgMemory: 0,
        cancelledCount: 0,
      };
    }

    const dataPoints = tenantMetrics.data.metrics as MetricsDataPoint[];

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

    const pointsWithDuration = dataPoints.filter(
      (point) =>
        (point.avg_duration_seconds ?? point.avgDurationSeconds) !== null
    );
    const avgDurationSeconds =
      pointsWithDuration.length > 0
        ? pointsWithDuration.reduce(
            (sum, point) =>
              sum +
              (point.avg_duration_seconds ?? point.avgDurationSeconds ?? 0),
            0
          ) / pointsWithDuration.length
        : 0;

    const failureCount = dataPoints.reduce(
      (sum, point) => sum + (point.failure_count ?? point.failureCount ?? 0),
      0
    );

    // Support both old (avgMemoryMb) and new (avg_memory_bytes) API formats
    const pointsWithMemory = dataPoints.filter(
      (point) => (point.avg_memory_bytes ?? point.avgMemoryMb) !== null
    );
    const avgMemory =
      pointsWithMemory.length > 0
        ? pointsWithMemory.reduce((sum, point) => {
            if (
              point.avg_memory_bytes !== undefined &&
              point.avg_memory_bytes !== null
            ) {
              return sum + point.avg_memory_bytes / (1024 * 1024); // Convert bytes to MB
            }
            return sum + (point.avgMemoryMb ?? 0);
          }, 0) / pointsWithMemory.length
        : 0;

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
  }, [tenantMetrics]);

  const trends = useMemo(() => {
    if (
      !tenantMetrics?.data?.metrics ||
      tenantMetrics.data.metrics.length < 2
    ) {
      return {
        executionsTrend: 'stable' as const,
        executionsChange: 0,
        successTrend: 'stable' as const,
        durationTrend: 'stable' as const,
        memoryTrend: 'stable' as const,
        cancelledTrend: 'stable' as const,
      };
    }

    const dataPoints = tenantMetrics.data.metrics as MetricsDataPoint[];
    const midPoint = Math.floor(dataPoints.length / 2);
    const firstHalf = dataPoints.slice(0, midPoint);
    const secondHalf = dataPoints.slice(midPoint);

    const firstHalfExecutions = firstHalf.reduce(
      (sum, p) => sum + (p.invocation_count ?? p.invocationCount ?? 0),
      0
    );
    const secondHalfExecutions = secondHalf.reduce(
      (sum, p) => sum + (p.invocation_count ?? p.invocationCount ?? 0),
      0
    );

    const firstHalfSuccesses = firstHalf.reduce(
      (sum, p) => sum + (p.success_count ?? p.successCount ?? 0),
      0
    );
    const secondHalfSuccesses = secondHalf.reduce(
      (sum, p) => sum + (p.success_count ?? p.successCount ?? 0),
      0
    );

    const firstHalfSuccessRate =
      firstHalfExecutions > 0
        ? (firstHalfSuccesses / firstHalfExecutions) * 100
        : 0;
    const secondHalfSuccessRate =
      secondHalfExecutions > 0
        ? (secondHalfSuccesses / secondHalfExecutions) * 100
        : 0;

    const firstHalfDuration =
      firstHalf.reduce(
        (sum, p) => sum + (p.avg_duration_seconds ?? p.avgDurationSeconds ?? 0),
        0
      ) / firstHalf.length;
    const secondHalfDuration =
      secondHalf.reduce(
        (sum, p) => sum + (p.avg_duration_seconds ?? p.avgDurationSeconds ?? 0),
        0
      ) / secondHalf.length;

    const getMemoryMb = (p: MetricsDataPoint): number => {
      if (p.avg_memory_bytes !== undefined && p.avg_memory_bytes !== null) {
        return p.avg_memory_bytes / (1024 * 1024);
      }
      return p.avgMemoryMb ?? 0;
    };

    const firstHalfMemory =
      firstHalf.reduce((sum, p) => sum + getMemoryMb(p), 0) / firstHalf.length;
    const secondHalfMemory =
      secondHalf.reduce((sum, p) => sum + getMemoryMb(p), 0) /
      secondHalf.length;

    const firstHalfCancelled = firstHalf.reduce(
      (sum, p) => sum + (p.cancelled_count ?? p.timeoutCount ?? 0),
      0
    );
    const secondHalfCancelled = secondHalf.reduce(
      (sum, p) => sum + (p.cancelled_count ?? p.timeoutCount ?? 0),
      0
    );

    return {
      executionsTrend: determineTrend(
        secondHalfExecutions,
        firstHalfExecutions
      ),
      executionsChange: calculatePercentageChange(
        secondHalfExecutions,
        firstHalfExecutions
      ),
      successTrend: determineTrend(secondHalfSuccessRate, firstHalfSuccessRate),
      durationTrend: determineTrend(firstHalfDuration, secondHalfDuration),
      memoryTrend: determineTrend(firstHalfMemory, secondHalfMemory),
      cancelledTrend: determineTrend(firstHalfCancelled, secondHalfCancelled),
    };
  }, [tenantMetrics]);

  const chartData = useMemo(() => {
    if (!tenantMetrics?.data?.metrics) return [];

    const metricsData = tenantMetrics.data.metrics as MetricsDataPoint[];
    const now = new Date();

    return metricsData.map((point, index) => {
      // Support both old (dayBucket) and new (bucket_time) API formats
      let timestamp: string;
      if (point.bucket_time) {
        timestamp = point.bucket_time;
      } else if (point.dayBucket) {
        timestamp = point.dayBucket;
      } else {
        // Generate timestamps spread across the date range when bucket is missing
        const offsetMs =
          ((metricsData.length - 1 - index) /
            Math.max(metricsData.length - 1, 1)) *
          getDateRangeMs(dateRange);
        timestamp = new Date(now.getTime() - offsetMs).toISOString();
      }

      // Support both old (avgMemoryMb) and new (avg_memory_bytes) API formats
      let avgMemoryMb = 0;
      if (
        point.avg_memory_bytes !== undefined &&
        point.avg_memory_bytes !== null
      ) {
        avgMemoryMb = point.avg_memory_bytes / (1024 * 1024); // Convert bytes to MB
      } else if (
        point.avgMemoryMb !== undefined &&
        point.avgMemoryMb !== null
      ) {
        avgMemoryMb = point.avgMemoryMb;
      }

      return {
        timestamp,
        executions: point.invocation_count ?? point.invocationCount ?? 0,
        successRate:
          point.success_rate_percent ?? point.successRatePercent ?? 0,
        avgDuration:
          ((point.avg_duration_seconds ?? point.avgDurationSeconds) || 0) *
          1000,
        avgMemory: avgMemoryMb,
      };
    });
  }, [tenantMetrics, dateRange]);

  if (metricsError && !metricsLoading) {
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
                  Usage
                </h1>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <DateRangeSelector
                  value={dateRange}
                  onChange={handleDateRangeChange}
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
                  : 'There was a problem loading analytics. Please try again.'}
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
                Usage
              </h1>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <DateRangeSelector
                value={dateRange}
                onChange={handleDateRangeChange}
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

        <section className="grid gap-4 md:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-6">
          <MetricCard
            title="Total Executions"
            value={formatNumber(metrics.totalExecutions)}
            change={trends.executionsChange}
            trend={trends.executionsTrend}
            loading={metricsLoading}
          />
          <MetricCard
            title="Success Rate"
            value={`${metrics.successRate.toFixed(1)}%`}
            change={Math.abs(trends.executionsChange * 0.1)}
            trend={trends.successTrend}
            loading={metricsLoading}
          />
          <MetricCard
            title="Avg Duration"
            value={formatDurationSeconds(metrics.avgDurationSeconds)}
            change={Math.abs(trends.executionsChange * 0.05)}
            trend={trends.durationTrend}
            loading={metricsLoading}
          />
          <MetricCard
            title="Avg Memory"
            value={formatMemory(metrics.avgMemory)}
            change={Math.abs(trends.executionsChange * 0.08)}
            trend={trends.memoryTrend}
            loading={metricsLoading}
          />
          <MetricCard
            title="Failed Executions"
            value={formatNumber(metrics.failureCount)}
            change={Math.abs(trends.executionsChange * 0.15)}
            trend={metrics.failureCount > 0 ? 'down' : 'stable'}
            loading={metricsLoading}
          />
          <MetricCard
            title="Cancelled"
            value={formatNumber(metrics.cancelledCount)}
            change={Math.abs(trends.executionsChange * 0.12)}
            trend={trends.cancelledTrend}
            loading={metricsLoading}
          />
        </section>

        <section>
          <ExecutionTrendChart data={chartData} loading={metricsLoading} />
        </section>
      </div>
    </div>
  );
}
