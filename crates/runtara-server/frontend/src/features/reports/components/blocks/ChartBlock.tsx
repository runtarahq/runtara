import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Legend,
  Line,
  LineChart,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { ReportBlockDefinition, ReportBlockResult } from '../../types';

const COLORS = [
  'hsl(var(--primary))',
  '#14b8a6',
  '#8b5cf6',
  '#f59e0b',
  '#ef4444',
  '#06b6d4',
  '#22c55e',
  '#ec4899',
  '#64748b',
  '#a855f7',
];
const GRID_COLOR = 'hsl(var(--border))';
const AXIS_COLOR = 'hsl(var(--muted-foreground))';
const SURFACE_COLOR = 'hsl(var(--card))';
const BORDER_COLOR = 'hsl(var(--border))';
const TEXT_COLOR = 'hsl(var(--foreground))';
const MUTED_TEXT_COLOR = 'hsl(var(--muted-foreground))';

type ChartData = {
  columns?: string[];
  rows?: unknown[][];
};

type TooltipPayload = {
  color?: string;
  dataKey?: string | number;
  name?: string | number;
  value?: unknown;
};

export function ChartBlock({
  block,
  result,
}: {
  block: ReportBlockDefinition;
  result: ReportBlockResult;
}) {
  const data = (result.data ?? {}) as ChartData;
  const rows = data.rows ?? [];
  const columns = data.columns ?? [];
  const chartRows = rows.map((row) =>
    columns.reduce<Record<string, unknown>>((acc, column, index) => {
      acc[column] = row[index];
      return acc;
    }, {})
  );
  const chart = block.chart;
  const series = getChartSeries(block, columns);

  if (!chart || chartRows.length === 0 || series.length === 0) {
    return (
      <div className="flex min-h-72 items-center justify-center rounded-lg border bg-background p-6 text-sm text-muted-foreground">
        No chart data for the current filters.
      </div>
    );
  }

  if (chart.kind === 'pie' || chart.kind === 'donut') {
    const seriesField = series[0].field;
    return (
      <div className="h-80 overflow-hidden rounded-lg border bg-card p-4 shadow-sm">
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Tooltip content={<ChartTooltip />} />
            <Legend
              iconType="circle"
              wrapperStyle={{ color: MUTED_TEXT_COLOR, fontSize: 12 }}
            />
            <Pie
              data={chartRows}
              dataKey={seriesField}
              nameKey={chart.x}
              innerRadius={chart.kind === 'donut' ? 60 : 0}
              outerRadius={110}
              paddingAngle={2}
              cornerRadius={6}
              stroke={SURFACE_COLOR}
              strokeWidth={2}
            >
              {chartRows.map((_, index) => (
                <Cell key={index} fill={COLORS[index % COLORS.length]} />
              ))}
            </Pie>
          </PieChart>
        </ResponsiveContainer>
      </div>
    );
  }

  const common = (
    <>
      <CartesianGrid
        stroke={GRID_COLOR}
        strokeDasharray="3 5"
        strokeOpacity={0.55}
        vertical={false}
      />
      <XAxis
        dataKey={chart.x}
        axisLine={false}
        tickLine={false}
        tickMargin={10}
        minTickGap={20}
        tick={{ fill: AXIS_COLOR, fontSize: 12 }}
      />
      <YAxis
        axisLine={false}
        tickLine={false}
        tickMargin={10}
        tick={{ fill: AXIS_COLOR, fontSize: 12 }}
      />
      <Tooltip content={<ChartTooltip />} />
      <Legend
        iconType="circle"
        wrapperStyle={{ color: MUTED_TEXT_COLOR, fontSize: 12 }}
      />
    </>
  );

  return (
    <div className="h-80 overflow-hidden rounded-lg border bg-card p-4 shadow-sm">
      <ResponsiveContainer width="100%" height="100%">
        {chart.kind === 'bar' ? (
          <BarChart
            data={chartRows}
            margin={{ top: 8, right: 12, left: 0, bottom: 4 }}
          >
            <defs>
              {series.map((series, index) => (
                <linearGradient
                  key={series.field}
                  id={gradientId(block.id, series.field)}
                  x1="0"
                  y1="0"
                  x2="0"
                  y2="1"
                >
                  <stop
                    offset="0%"
                    stopColor={COLORS[index % COLORS.length]}
                    stopOpacity={0.95}
                  />
                  <stop
                    offset="100%"
                    stopColor={COLORS[index % COLORS.length]}
                    stopOpacity={0.58}
                  />
                </linearGradient>
              ))}
            </defs>
            {common}
            {series.map((series) => (
              <Bar
                key={series.field}
                dataKey={series.field}
                name={series.label ?? series.field}
                fill={`url(#${gradientId(block.id, series.field)})`}
                maxBarSize={42}
                radius={[6, 6, 0, 0]}
              />
            ))}
          </BarChart>
        ) : chart.kind === 'area' ? (
          <AreaChart
            data={chartRows}
            margin={{ top: 8, right: 12, left: 0, bottom: 4 }}
          >
            <defs>
              {series.map((series, index) => (
                <linearGradient
                  key={series.field}
                  id={gradientId(block.id, series.field)}
                  x1="0"
                  y1="0"
                  x2="0"
                  y2="1"
                >
                  <stop
                    offset="0%"
                    stopColor={COLORS[index % COLORS.length]}
                    stopOpacity={0.26}
                  />
                  <stop
                    offset="100%"
                    stopColor={COLORS[index % COLORS.length]}
                    stopOpacity={0.03}
                  />
                </linearGradient>
              ))}
            </defs>
            {common}
            {series.map((series, index) => (
              <Area
                key={series.field}
                dataKey={series.field}
                name={series.label ?? series.field}
                stroke={COLORS[index % COLORS.length]}
                strokeWidth={2.5}
                fill={`url(#${gradientId(block.id, series.field)})`}
                activeDot={{
                  r: 5,
                  stroke: SURFACE_COLOR,
                  strokeWidth: 2,
                }}
              />
            ))}
          </AreaChart>
        ) : (
          <LineChart
            data={chartRows}
            margin={{ top: 8, right: 12, left: 0, bottom: 4 }}
          >
            {common}
            {series.map((series, index) => (
              <Line
                key={series.field}
                type="monotone"
                dataKey={series.field}
                name={series.label ?? series.field}
                stroke={COLORS[index % COLORS.length]}
                strokeWidth={2.5}
                dot={false}
                activeDot={{
                  r: 5,
                  stroke: SURFACE_COLOR,
                  strokeWidth: 2,
                }}
              />
            ))}
          </LineChart>
        )}
      </ResponsiveContainer>
    </div>
  );
}

function ChartTooltip({
  active,
  payload,
  label,
}: {
  active?: boolean;
  payload?: TooltipPayload[];
  label?: unknown;
}) {
  if (!active || !payload?.length) {
    return null;
  }

  return (
    <div
      className="rounded-lg px-3 py-2 text-sm shadow-lg"
      style={{
        backgroundColor: SURFACE_COLOR,
        border: `1px solid ${BORDER_COLOR}`,
        color: TEXT_COLOR,
      }}
    >
      <div className="mb-1 font-medium">{String(label ?? '')}</div>
      <div className="flex flex-col gap-1">
        {payload.map((item) => (
          <div
            key={String(item.dataKey ?? item.name)}
            className="flex min-w-40 items-center justify-between gap-4"
          >
            <span className="flex min-w-0 items-center gap-2 text-muted-foreground">
              <span
                className="size-2 rounded-full"
                style={{ backgroundColor: item.color ?? COLORS[0] }}
              />
              <span className="truncate">
                {String(item.name ?? item.dataKey)}
              </span>
            </span>
            <span className="font-medium">
              {formatTooltipValue(item.value)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function getChartSeries(
  block: ReportBlockDefinition,
  columns: string[]
): Array<{ field: string; label?: string }> {
  const configuredSeries = block.chart?.series ?? [];
  if (configuredSeries.length > 0) {
    return configuredSeries;
  }

  const aggregateAliases = block.source.aggregates
    ?.map((aggregate) => aggregate.alias)
    .filter(Boolean);
  const inferredField = aggregateAliases?.[aggregateAliases.length - 1];
  if (inferredField) {
    return [{ field: inferredField, label: inferredField }];
  }

  const fallbackField = columns.find((column) => column !== block.chart?.x);
  return fallbackField ? [{ field: fallbackField, label: fallbackField }] : [];
}

function gradientId(blockId: string, field: string): string {
  return `report-chart-${blockId}-${field}`.replace(/[^a-zA-Z0-9_-]/g, '-');
}

function formatTooltipValue(value: unknown): string {
  if (typeof value === 'number') {
    return new Intl.NumberFormat(undefined, {
      maximumFractionDigits: value % 1 === 0 ? 0 : 2,
    }).format(value);
  }

  if (value === null || value === undefined) {
    return '';
  }

  return String(value);
}
