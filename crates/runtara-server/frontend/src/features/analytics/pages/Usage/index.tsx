import { useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router';
import { RefreshCw } from 'lucide-react';

import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { Button } from '@/shared/components/ui/button';
import { Progress } from '@/shared/components/ui/progress';
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
import type { MetricsBucket } from '@/generated/RuntaraRuntimeApi';

import { ActivityMap } from '../../components/ActivityMap';
import { AnalyticsHeroCard } from '../../components/AnalyticsHeroCard';
import { HostResourceTiles } from '../../components/HostResources';
import {
  MetricTrendChart,
  type TrendPoint,
} from '../../components/MetricTrendChart';
import {
  usePreviousTenantMetrics,
  useSystemAnalytics,
  useTenantMetrics,
} from '../../hooks/useAnalytics';
import { summarizeMetrics } from '../../utils/metrics-summary';
import { comparePeriods } from '../../utils/metric-trends';
import {
  ACTIVITY_MAP_CONFIG,
  buildActivityMap,
  formatAxisLabel,
  formatCellRange,
} from '../../utils/activity-map';
import { formatDurationSeconds, formatMemory, formatNumber } from '../../utils';

const VALID_DATE_RANGES: DateRangeOption[] = ['1h', '24h', '7d', '30d', '90d'];

/** What the trend indicators are measured against, named for the reader. */
const COMPARISON_LABEL: Record<DateRangeOption, string> = {
  '1h': 'previous hour',
  '24h': 'previous 24 hours',
  '7d': 'previous 7 days',
  '30d': 'previous 30 days',
  '90d': 'previous 90 days',
};

/**
 * How many buckets each chart point folds together, and what to call it.
 *
 * Chosen so a point lands on a round interval rather than on whatever falls out
 * of dividing the bucket count by a target column count. The card used to say
 * "Grouped into 28 intervals", which described the implementation rather than
 * the chart: nobody reading a dashboard needs the chunk count, they need to
 * know how much time one point covers before deciding whether a spike matters.
 */
const SERIES_INTERVAL: Record<
  DateRangeOption,
  { buckets: number; label: string }
> = {
  '1h': { buckets: 1, label: 'minute' },
  '24h': { buckets: 5, label: '30 minutes' },
  '7d': { buckets: 15, label: '6 hours' },
  '30d': { buckets: 12, label: 'day' },
  '90d': { buckets: 12, label: '3 days' },
};

/**
 * Short forms for axis ticks.
 *
 * A tick has a fraction of the room a tooltip does. `formatMemory` renders
 * "62.1 MB", which wrapped inside the axis gutter and collided with the plot;
 * the decimal is noise on a gridline anyway, and the tooltip still carries it.
 */
function compactCount(value: number): string {
  // A decimal below 10k, because rounding to whole thousands rendered both
  // 1,050 and 1,400 as "1k" - two gridlines with the same label.
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 10_000) return `${Math.round(value / 1_000)}k`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return String(Math.round(value));
}

function compactMemory(mb: number): string {
  // Zero has no unit worth printing - the axis baseline read "0 KB".
  if (mb <= 0) return '0';
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  if (mb < 1) return `${Math.round(mb * 1024)} KB`;
  return `${Math.round(mb)} MB`;
}

/**
 * Fold the buckets down to a fixed number of chart columns.
 *
 * The map wants every bucket; the charts want a readable number of bars. At 421
 * buckets a bar is under two pixels wide, which is a texture rather than a
 * chart. Memory is averaged weighted by executions - an unweighted mean would
 * let a quiet interval count as much as a busy one.
 */
function toSeries(buckets: MetricsBucket[], period: DateRangeOption) {
  if (buckets.length === 0) return { executions: [], memory: [] };
  const config = ACTIVITY_MAP_CONFIG[period];
  const size = Math.max(1, SERIES_INTERVAL[period]?.buckets ?? 1);
  const executions: TrendPoint[] = [];
  const memory: TrendPoint[] = [];

  for (let i = 0; i < buckets.length; i += size) {
    const chunk = buckets.slice(i, i + size);
    const first = chunk[0];
    const label = first.bucket_time
      ? formatAxisLabel(new Date(first.bucket_time).getTime(), config)
      : '';
    const runs = chunk.reduce((sum, b) => sum + (b.invocation_count ?? 0), 0);
    const memBytes = chunk.reduce(
      (sum, b) => sum + (b.avg_memory_bytes ?? 0) * (b.invocation_count ?? 0),
      0
    );
    executions.push({ label, value: runs });
    memory.push({
      label,
      value: runs > 0 ? memBytes / runs / (1024 * 1024) : 0,
    });
  }
  return { executions, memory };
}

export function Usage() {
  usePageTitle('Usage');

  const [searchParams, setSearchParams] = useSearchParams();
  const urlPeriod = searchParams.get('period') as DateRangeOption;
  const [dateRange, setDateRange] = useState<DateRangeOption>(
    VALID_DATE_RANGES.includes(urlPeriod) ? urlPeriod : '30d'
  );

  useEffect(() => {
    const next = new URLSearchParams();
    next.set('period', dateRange);
    setSearchParams(next, { replace: true });
  }, [dateRange, setSearchParams]);

  const {
    data: tenantMetrics,
    isLoading,
    refetch,
    isError,
    error,
  } = useTenantMetrics(dateRange);

  const buckets = useMemo(
    () => (tenantMetrics?.data?.metrics ?? []) as MetricsBucket[],
    [tenantMetrics]
  );

  const {
    data: systemAnalytics,
    isLoading: systemLoading,
    refetch: refetchSystem,
  } = useSystemAnalytics();

  const { data: previousMetrics } = usePreviousTenantMetrics(dateRange);
  const previousBuckets = useMemo(
    () => (previousMetrics?.data?.metrics ?? []) as MetricsBucket[],
    [previousMetrics]
  );

  const metrics = useMemo(() => summarizeMetrics(buckets), [buckets]);
  const trends = useMemo(() => {
    // No indicator at all until the preceding period has actually arrived.
    if (previousBuckets.length === 0) return comparePeriods(metrics, null);
    return comparePeriods(metrics, summarizeMetrics(previousBuckets));
  }, [metrics, previousBuckets]);
  const series = useMemo(
    () => toSeries(buckets, dateRange),
    [buckets, dateRange]
  );

  // One grid, two readers. The busiest interval named on the executions card
  // and the one under the map are the same cell by construction.
  const map = useMemo(
    () => buildActivityMap(buckets, ACTIVITY_MAP_CONFIG[dateRange]),
    [buckets, dateRange]
  );

  const windowLabel = useMemo(() => {
    const start = tenantMetrics?.data?.startTime;
    const end = tenantMetrics?.data?.endTime;
    if (!start || !end) return '';
    const fmt = (iso: string) =>
      new Date(iso).toLocaleString(undefined, {
        month: 'short',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
      });
    return `${fmt(start)} – ${fmt(end)}`;
  }, [tenantMetrics]);

  const busiest = useMemo(() => {
    if (!map.peak) return null;
    return { count: map.peak.total, when: formatCellRange(map.peak) };
  }, [map]);

  // The maximum of the plotted series, stated as a number and nothing more.
  // This footnote used to call it the "busiest interval" - a word that means
  // "most executions" everywhere else on this page, pointing at a cell chosen
  // by memory instead. Ranking intervals by memory is not a claim this chart
  // is making, so it does not make one.
  const peakMemoryMb = useMemo(
    () => Math.max(0, ...series.memory.map((p) => p.value)),
    [series]
  );

  return (
    <ConsoleTableShell
      // The dashboard is meant to be read at a glance, so it sizes to the
      // viewport rather than stacking past the fold: `min-h-0` on every flex
      // descendant is what makes the charts shrink instead of pushing the map
      // off-screen. `overflow-auto` rather than `hidden` because below the
      // height this layout targets, scrolling is the honest failure - clipping
      // would silently hide a chart.
      bodyClassName="flex min-h-0 flex-col overflow-auto p-3 md:p-4"
      toolbar={
        <ConsoleToolbar
          left={<Breadcrumb items={[{ label: 'Usage' }]} />}
          actions={
            <div className="flex items-center gap-2">
              <DateRangeSelector value={dateRange} onChange={setDateRange} />
              <Button
                onClick={() => {
                  refetch();
                  refetchSystem();
                }}
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
      {isError && !isLoading ? (
        <ConsoleErrorState
          error={error}
          entityLabel="usage"
          className="h-auto rounded-lg border bg-muted/20"
        />
      ) : (
        <div className="flex min-h-0 flex-1 flex-col gap-2.5">
          {/* One row of six compact tiles: three for the selected window,
              three for the host as it stands now. The charts below are where
              the page spends its height. */}
          <section className="grid shrink-0 grid-cols-2 gap-2.5 md:grid-cols-3 xl:grid-cols-6">
            <AnalyticsHeroCard
              label="Total executions"
              value={formatNumber(metrics.totalExecutions)}
              change={trends.executions.change}
              trend={trends.executions.trend}
              comparisonLabel={COMPARISON_LABEL[dateRange]}
              loading={isLoading}
            >
              <span className="line-clamp-2 text-xs text-muted-foreground">
                {busiest
                  ? `Busiest ${formatNumber(busiest.count)} run${busiest.count === 1 ? '' : 's'} · ${busiest.when}`
                  : 'No executions in this window'}
              </span>
            </AnalyticsHeroCard>

            <AnalyticsHeroCard
              label="Success rate"
              value={`${metrics.successRate.toFixed(1)}%`}
              change={trends.successRate.change}
              trend={trends.successRate.trend}
              comparisonLabel={COMPARISON_LABEL[dateRange]}
              loading={isLoading}
            >
              <Progress
                value={Math.round(metrics.successRate)}
                className="h-1"
                aria-label="Success rate"
              />
              <span className="truncate text-xs text-muted-foreground">
                {formatNumber(metrics.failureCount)} failed ·{' '}
                {formatNumber(metrics.cancelledCount)} cancelled
              </span>
            </AnalyticsHeroCard>

            {/* Labelled "peak" deliberately: the server aggregates
                `AVG(memory_peak_bytes)`, an average of each run's high-water
                mark rather than of its consumption, and "Avg memory" invited
                the wrong reading. */}
            <AnalyticsHeroCard
              label="Avg duration"
              value={formatDurationSeconds(metrics.avgDurationSeconds)}
              loading={isLoading}
            >
              <span className="truncate text-xs text-muted-foreground">
                Avg peak memory{' '}
                <span className="font-medium tabular-nums text-foreground">
                  {formatMemory(metrics.avgMemory)}
                </span>
              </span>
            </AnalyticsHeroCard>

            <HostResourceTiles
              data={systemAnalytics?.data}
              loading={systemLoading}
            />
          </section>

          <div className="flex min-h-0 shrink-0 flex-col">
            <ActivityMap
              map={map}
              period={dateRange}
              windowLabel={windowLabel}
              loading={isLoading}
            />
          </div>

          {/* Equal columns: the two series carry the same weight, and the 7:5
              split gave the memory chart a narrower plot for no reason -
              it just wrapped its own description onto a second line. */}
          <section className="grid min-h-0 flex-1 gap-2.5 lg:grid-cols-2">
            <MetricTrendChart
              title="Execution volume"
              description={`One point per ${SERIES_INTERVAL[dateRange].label}`}
              seriesName="Executions"
              colorToken="--chart-1"
              data={series.executions}
              formatValue={(value) => formatNumber(Math.round(value))}
              formatTick={compactCount}
              loading={isLoading}
              footnote={
                metrics.failureCount > 0
                  ? `Success rate ${metrics.successRate.toFixed(1)}% over this window · ${formatNumber(metrics.failureCount)} failed executions.`
                  : 'No failed executions in this window.'
              }
            />
            <MetricTrendChart
              title="Peak memory per execution"
              description={`Average of per-run high-water marks, per ${SERIES_INTERVAL[dateRange].label}`}
              seriesName="Avg peak memory"
              colorToken="--chart-2"
              data={series.memory}
              formatValue={(value) => formatMemory(value)}
              formatTick={compactMemory}
              loading={isLoading}
              footnote={`Peak ${formatMemory(peakMemoryMb)} · average ${formatMemory(metrics.avgMemory)} per execution.`}
            />
          </section>
        </div>
      )}
    </ConsoleTableShell>
  );
}
