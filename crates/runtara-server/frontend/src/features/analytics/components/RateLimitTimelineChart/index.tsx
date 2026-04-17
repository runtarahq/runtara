import { useMemo } from 'react';
import {
  BarChart,
  Bar,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
  Legend,
} from 'recharts';
import { Activity, AlertCircle, RefreshCw, CheckCircle } from 'lucide-react';
import type { RateLimitTimelineBucket } from '@/generated/RuntaraRuntimeApi';
import { type Granularity, formatBucketLabel } from '../../utils/timeline';

interface RateLimitTimelineChartProps {
  buckets: RateLimitTimelineBucket[];
  granularity: Granularity;
}

interface ChartDataPoint {
  label: string;
  bucket: string;
  requestCount: number;
  rateLimitedCount: number;
  retryCount: number;
}

function TimelineSummary({ buckets }: { buckets: RateLimitTimelineBucket[] }) {
  const summary = useMemo(() => {
    let requests = 0;
    let rateLimited = 0;
    let retries = 0;

    for (const b of buckets) {
      requests += b.requestCount;
      rateLimited += b.rateLimitedCount;
      retries += b.retryCount;
    }

    const rateLimitRate =
      requests > 0 ? ((rateLimited / requests) * 100).toFixed(1) : '0';

    return { requests, rateLimited, retries, rateLimitRate };
  }, [buckets]);

  return (
    <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
      <div className="flex items-center gap-3 rounded-lg bg-muted/30 p-3">
        <div className="rounded-full bg-blue-500/10 p-2">
          <Activity className="h-4 w-4 text-blue-500" />
        </div>
        <div>
          <p className="text-xs text-muted-foreground">Requests</p>
          <p className="text-lg font-semibold">
            {summary.requests.toLocaleString()}
          </p>
        </div>
      </div>

      <div className="flex items-center gap-3 rounded-lg bg-muted/30 p-3">
        <div className="rounded-full bg-red-500/10 p-2">
          <AlertCircle className="h-4 w-4 text-red-500" />
        </div>
        <div>
          <p className="text-xs text-muted-foreground">Rate Limited</p>
          <p className="text-lg font-semibold">
            {summary.rateLimited.toLocaleString()}
          </p>
        </div>
      </div>

      <div className="flex items-center gap-3 rounded-lg bg-muted/30 p-3">
        <div className="rounded-full bg-yellow-500/10 p-2">
          <RefreshCw className="h-4 w-4 text-yellow-500" />
        </div>
        <div>
          <p className="text-xs text-muted-foreground">Retries</p>
          <p className="text-lg font-semibold">
            {summary.retries.toLocaleString()}
          </p>
        </div>
      </div>

      <div className="flex items-center gap-3 rounded-lg bg-muted/30 p-3">
        <div className="rounded-full bg-green-500/10 p-2">
          <CheckCircle className="h-4 w-4 text-green-500" />
        </div>
        <div>
          <p className="text-xs text-muted-foreground">Rate Limit %</p>
          <p className="text-lg font-semibold">{summary.rateLimitRate}%</p>
        </div>
      </div>
    </div>
  );
}

export function RateLimitTimelineChart({
  buckets,
  granularity,
}: RateLimitTimelineChartProps) {
  const chartData: ChartDataPoint[] = useMemo(
    () =>
      buckets.map((b) => ({
        label: formatBucketLabel(b.bucket, granularity),
        bucket: b.bucket,
        requestCount: b.requestCount,
        rateLimitedCount: b.rateLimitedCount,
        retryCount: b.retryCount,
      })),
    [buckets, granularity]
  );

  const hasData = buckets.some(
    (b) => b.requestCount > 0 || b.rateLimitedCount > 0 || b.retryCount > 0
  );

  return (
    <div className="space-y-4">
      <TimelineSummary buckets={buckets} />

      {hasData ? (
        <div className="pt-4">
          <h4 className="text-sm font-medium mb-3">
            Events Over Time (
            {granularity === 'minute'
              ? 'Per Minute'
              : granularity === 'hourly'
                ? 'Hourly'
                : 'Daily'}
            )
          </h4>
          <ResponsiveContainer width="100%" height={200}>
            <BarChart
              data={chartData}
              margin={{ top: 5, right: 20, left: 0, bottom: 5 }}
            >
              <CartesianGrid strokeDasharray="3 3" className="stroke-muted" />
              <XAxis
                dataKey="label"
                className="text-xs"
                tick={{ fill: 'currentColor', fontSize: 10 }}
                interval="preserveStartEnd"
              />
              <YAxis
                className="text-xs"
                tick={{ fill: 'currentColor', fontSize: 10 }}
                allowDecimals={false}
              />
              <Tooltip
                contentStyle={{
                  backgroundColor: 'hsl(var(--background))',
                  border: '1px solid hsl(var(--border))',
                  borderRadius: '6px',
                  fontSize: '12px',
                }}
                labelStyle={{ color: 'hsl(var(--foreground))' }}
              />
              <Legend wrapperStyle={{ fontSize: '12px' }} />
              <Bar
                dataKey="requestCount"
                name="Requests"
                fill="hsl(var(--primary))"
                stackId="a"
                radius={[4, 4, 0, 0]}
              />
              <Bar
                dataKey="rateLimitedCount"
                name="Rate Limited"
                fill="#ef4444"
                stackId="b"
                radius={[4, 4, 0, 0]}
              />
              <Bar
                dataKey="retryCount"
                name="Retries"
                fill="#eab308"
                stackId="b"
                radius={[4, 4, 0, 0]}
              />
            </BarChart>
          </ResponsiveContainer>
        </div>
      ) : (
        <div className="text-center text-sm text-muted-foreground py-8">
          No rate limit events in the selected period
        </div>
      )}
    </div>
  );
}
