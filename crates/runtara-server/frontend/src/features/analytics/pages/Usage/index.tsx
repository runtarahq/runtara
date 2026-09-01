import { summarizeMetrics } from '../../utils/metrics-summary';
import { useState, useMemo, useEffect } from 'react';
import { useSearchParams } from 'react-router';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { RefreshCw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  Breadcrumb,
  ConsoleErrorState,
  ConsoleTableShell,
  ConsoleToolbar,
} from '@/shared/components/console';
import {
  DateRangeOption,
  DateRangeSelector,
} from '@/shared/components/date-range-selector';
import { MetricCard } from '@/shared/components/metric-card';
import { ExecutionTrendChart } from '../../components/ExecutionTrendChart';
import { useTenantMetrics } from '../../hooks/useAnalytics';
import { formatDurationSeconds, formatMemory, formatNumber } from '../../utils';
import {
  computeMetricTrends,
  type MetricsDataPoint,
} from '../../utils/metric-trends';

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

  const metrics = useMemo(
    () => summarizeMetrics(tenantMetrics?.data?.metrics as MetricsDataPoint[]),
    [tenantMetrics]
  );

  const trends = useMemo(
    () =>
      computeMetricTrends(
        tenantMetrics?.data?.metrics as MetricsDataPoint[] | undefined
      ),
    [tenantMetrics]
  );

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

  return (
    <ConsoleTableShell
      bodyClassName="p-4 md:p-6"
      toolbar={
        <ConsoleToolbar
          left={
            <Breadcrumb
              items={[
                { label: 'Analytics', to: '/analytics/usage' },
                { label: 'Usage' },
              ]}
            />
          }
          actions={
            <div className="flex items-center gap-2">
              <DateRangeSelector
                value={dateRange}
                onChange={handleDateRangeChange}
              />
              <Button
                onClick={handleRefresh}
                variant="secondary"
                bordered
                size="sm"
              >
                <RefreshCw className="mr-2 size-4" />
                Refresh
              </Button>
            </div>
          }
        />
      }
    >
      {metricsError && !metricsLoading ? (
        <ConsoleErrorState
          error={error}
          entityLabel="analytics"
          className="h-auto rounded-lg border bg-muted/20"
        />
      ) : (
        <div className="space-y-4">
          <section className="grid gap-4 md:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-6">
            <MetricCard
              title="Total Executions"
              value={formatNumber(metrics.totalExecutions)}
              change={trends.executions.change}
              trend={trends.executions.trend}
              loading={metricsLoading}
            />
            <MetricCard
              title="Success Rate"
              value={`${metrics.successRate.toFixed(1)}%`}
              change={trends.successRate.change}
              trend={trends.successRate.trend}
              loading={metricsLoading}
            />
            <MetricCard
              title="Avg Duration"
              value={formatDurationSeconds(metrics.avgDurationSeconds)}
              change={trends.duration.change}
              trend={trends.duration.trend}
              loading={metricsLoading}
            />
            <MetricCard
              title="Avg Memory"
              value={formatMemory(metrics.avgMemory)}
              change={trends.memory.change}
              trend={trends.memory.trend}
              loading={metricsLoading}
            />
            <MetricCard
              title="Failed Executions"
              value={formatNumber(metrics.failureCount)}
              change={trends.failures.change}
              trend={trends.failures.trend}
              loading={metricsLoading}
            />
            <MetricCard
              title="Cancelled"
              value={formatNumber(metrics.cancelledCount)}
              change={trends.cancelled.change}
              trend={trends.cancelled.trend}
              loading={metricsLoading}
            />
          </section>

          <section>
            <ExecutionTrendChart data={chartData} loading={metricsLoading} />
          </section>
        </div>
      )}
    </ConsoleTableShell>
  );
}
