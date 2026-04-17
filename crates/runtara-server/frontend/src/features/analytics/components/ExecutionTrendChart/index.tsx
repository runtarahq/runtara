import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import {
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { format } from 'date-fns';

interface TrendDataPoint {
  timestamp: string;
  executions: number;
  successRate?: number;
  avgDuration?: number;
  avgMemory?: number;
}

interface ExecutionTrendChartProps {
  data: TrendDataPoint[];
  loading?: boolean;
}

/**
 * Determines the appropriate date format based on the time span of the data
 */
function getDateFormat(data: TrendDataPoint[]): string {
  if (data.length < 2) return 'MMM dd yyyy';

  const timestamps = data.map((d) => new Date(d.timestamp).getTime());
  const minTime = Math.min(...timestamps);
  const maxTime = Math.max(...timestamps);
  const spanMs = maxTime - minTime;

  const ONE_HOUR = 60 * 60 * 1000;
  const ONE_DAY = 24 * ONE_HOUR;
  const ONE_WEEK = 7 * ONE_DAY;

  if (spanMs <= ONE_HOUR) {
    // Very short span: show time with seconds
    return 'HH:mm:ss';
  } else if (spanMs <= ONE_DAY) {
    // Within a day: show hours and minutes
    return 'HH:mm';
  } else if (spanMs <= ONE_WEEK) {
    // Within a week: show day and time
    return 'MMM dd HH:mm';
  } else {
    // Longer spans: show date only
    return 'MMM dd';
  }
}

export function ExecutionTrendChart({
  data,
  loading = false,
}: ExecutionTrendChartProps) {
  if (loading) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>Execution Trends</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="h-[350px] bg-muted animate-pulse rounded" />
        </CardContent>
      </Card>
    );
  }

  if (!data || data.length === 0) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>Execution Trends</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="h-[350px] flex items-center justify-center text-muted-foreground">
            No data available for the selected period
          </div>
        </CardContent>
      </Card>
    );
  }

  const dateFormat = getDateFormat(data);
  const formattedData = data.map((item) => ({
    ...item,
    time: format(new Date(item.timestamp), dateFormat),
  }));

  const hasMemoryData = data.some((point) => (point.avgMemory ?? 0) > 0);

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle>Execution Trends</CardTitle>
        </CardHeader>
        <CardContent>
          <ResponsiveContainer width="100%" height={350}>
            <LineChart
              data={formattedData}
              margin={{ top: 5, right: 30, left: 20, bottom: 5 }}
            >
              <CartesianGrid strokeDasharray="3 3" className="stroke-muted" />
              <XAxis
                dataKey="time"
                className="text-xs"
                tick={{ fill: 'currentColor' }}
              />
              <YAxis
                yAxisId="left"
                className="text-xs"
                tick={{ fill: 'currentColor' }}
                label={{
                  value: 'Executions',
                  angle: -90,
                  position: 'insideLeft',
                }}
              />
              <YAxis
                yAxisId="right"
                orientation="right"
                className="text-xs"
                tick={{ fill: 'currentColor' }}
                domain={[0, 100]}
                label={{
                  value: 'Success Rate (%)',
                  angle: 90,
                  position: 'insideRight',
                }}
              />
              <Tooltip
                contentStyle={{
                  backgroundColor: 'hsl(var(--background))',
                  border: '1px solid hsl(var(--border))',
                  borderRadius: '6px',
                }}
                labelStyle={{ color: 'hsl(var(--foreground))' }}
              />
              <Legend />
              <Line
                yAxisId="left"
                type="monotone"
                dataKey="executions"
                stroke="hsl(var(--primary))"
                strokeWidth={2}
                name="Executions"
                dot={false}
              />
              {data[0].successRate !== undefined && (
                <Line
                  yAxisId="right"
                  type="monotone"
                  dataKey="successRate"
                  stroke="#22c55e"
                  strokeWidth={2}
                  name="Success Rate"
                  dot={false}
                />
              )}
            </LineChart>
          </ResponsiveContainer>
        </CardContent>
      </Card>

      {hasMemoryData && (
        <Card className="mt-4">
          <CardHeader>
            <CardTitle>Memory Usage</CardTitle>
          </CardHeader>
          <CardContent>
            <ResponsiveContainer width="100%" height={300}>
              <LineChart
                data={formattedData}
                margin={{ top: 5, right: 30, left: 20, bottom: 5 }}
              >
                <CartesianGrid strokeDasharray="3 3" className="stroke-muted" />
                <XAxis
                  dataKey="time"
                  className="text-xs"
                  tick={{ fill: 'currentColor' }}
                />
                <YAxis
                  className="text-xs"
                  tick={{ fill: 'currentColor' }}
                  label={{
                    value: 'Memory (MB)',
                    angle: -90,
                    position: 'insideLeft',
                  }}
                />
                <Tooltip
                  contentStyle={{
                    backgroundColor: 'hsl(var(--background))',
                    border: '1px solid hsl(var(--border))',
                    borderRadius: '6px',
                  }}
                  labelStyle={{ color: 'hsl(var(--foreground))' }}
                  formatter={(value: number) => [
                    `${value.toFixed(1)} MB`,
                    'Avg Memory',
                  ]}
                />
                <Legend />
                <Line
                  type="monotone"
                  dataKey="avgMemory"
                  stroke="#8b5cf6"
                  strokeWidth={2}
                  name="Avg Memory (MB)"
                  dot={false}
                />
              </LineChart>
            </ResponsiveContainer>
          </CardContent>
        </Card>
      )}
    </>
  );
}
