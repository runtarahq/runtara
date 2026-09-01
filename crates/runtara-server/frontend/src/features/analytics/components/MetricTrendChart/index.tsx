import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';

export interface TrendPoint {
  label: string;
  value: number;
}

interface MetricTrendChartProps {
  title: string;
  description: string;
  /** Legend text beside the colour swatch. */
  seriesName: string;
  /** A `--chart-N` token name, e.g. `--chart-1`. */
  colorToken: string;
  data: TrendPoint[];
  /** Full-precision formatting, used in the tooltip. */
  formatValue: (value: number) => string;
  /**
   * Axis-tick formatting. Defaults to `formatValue`, but a tick has a fraction
   * of the room a tooltip does - "62.1 MB" wrapped onto two lines and collided
   * with the plot - so a shorter form belongs here.
   */
  formatTick?: (value: number) => string;
  /** One-line reading of the series, under the plot. */
  footnote?: string;
  loading?: boolean;
}

// Entry animations are off. The page refetches every 60 seconds, so animating
// each arrival makes the dashboard twitch rather than update; it also means the
// chart is fully drawn on first paint instead of depending on animation frames
// that a backgrounded tab never delivers.

/** Floor below which a chart stops being readable. */
const MIN_CHART_HEIGHT = 84;

/** Room at the top for the highest tick label, which sits on the plot edge. */
const CHART_MARGIN = { top: 8, right: 8, left: 0, bottom: 0 } as const;

/**
 * One series, one axis, drawn as a line over a soft area.
 *
 * This replaces a dual-axis line chart that plotted execution counts and a
 * success-rate percentage on the same canvas: two quantities with no shared
 * scale, where the shape of one said nothing true about the other.
 *
 * Both series here are levels sampled over time rather than independent
 * tallies - "how busy was it around then", "how much memory were runs taking" -
 * and a continuous line carries that better than columns, which imply each
 * interval is its own discrete measurement. The area fill gives the eye the
 * magnitude without a second encoding.
 */
export function MetricTrendChart({
  title,
  description,
  seriesName,
  colorToken,
  data,
  formatValue,
  formatTick,
  footnote,
  loading = false,
}: MetricTrendChartProps) {
  const color = `hsl(var(${colorToken}))`;
  const tick = formatTick ?? formatValue;
  const gradientId = `fill-${colorToken.replace(/[^a-z0-9]/gi, '')}`;

  return (
    <Card className="flex min-h-0 flex-col border-border/40 shadow-none">
      <CardHeader className="flex shrink-0 flex-row items-start justify-between space-y-0 p-4 pb-1">
        <div className="flex flex-col gap-1">
          <CardTitle className="text-base">{title}</CardTitle>
          <CardDescription>{description}</CardDescription>
        </div>
        <div className="flex items-center gap-1.5 whitespace-nowrap text-xs text-muted-foreground">
          <span
            className="inline-block size-2 rounded-full"
            style={{ background: color }}
          />
          {seriesName}
        </div>
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col px-4 pb-3 pt-2">
        {loading ? (
          <div
            className="min-h-0 flex-1 animate-pulse rounded bg-muted"
            style={{ minHeight: MIN_CHART_HEIGHT }}
          />
        ) : data.length === 0 ? (
          <div
            className="flex min-h-0 flex-1 items-center justify-center text-sm text-muted-foreground"
            style={{ minHeight: MIN_CHART_HEIGHT }}
          >
            No data for the selected period
          </div>
        ) : (
          <div
            className="min-h-0 flex-1"
            style={{ minHeight: MIN_CHART_HEIGHT }}
          >
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart data={data} margin={CHART_MARGIN}>
                <defs>
                  <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stopColor={color} stopOpacity={0.28} />
                    <stop offset="100%" stopColor={color} stopOpacity={0.02} />
                  </linearGradient>
                </defs>
                <CartesianGrid
                  strokeDasharray="3 3"
                  vertical={false}
                  className="stroke-border"
                />
                <XAxis
                  dataKey="label"
                  tickLine={false}
                  axisLine={false}
                  interval="preserveStartEnd"
                  minTickGap={48}
                  tickMargin={8}
                  tick={{ fill: 'currentColor' }}
                  className="text-xs text-muted-foreground"
                />
                <YAxis
                  width={62}
                  tickLine={false}
                  axisLine={false}
                  tickMargin={6}
                  tick={{ fill: 'currentColor' }}
                  tickFormatter={tick}
                  className="text-xs text-muted-foreground"
                />
                <Tooltip
                  cursor={{ stroke: 'hsl(var(--border))', strokeWidth: 1 }}
                  contentStyle={{
                    backgroundColor: 'hsl(var(--popover))',
                    border: '1px solid hsl(var(--border))',
                    borderRadius: '6px',
                    fontSize: '12px',
                  }}
                  labelStyle={{ color: 'hsl(var(--popover-foreground))' }}
                  formatter={(value: number) => [
                    formatValue(value),
                    seriesName,
                  ]}
                />
                <Area
                  type="monotone"
                  dataKey="value"
                  stroke={color}
                  strokeWidth={2}
                  fill={`url(#${gradientId})`}
                  dot={false}
                  activeDot={{ r: 3, strokeWidth: 0 }}
                  isAnimationActive={false}
                />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        )}
        {footnote ? (
          <p className="mt-2 shrink-0 text-xs text-muted-foreground">
            {footnote}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
