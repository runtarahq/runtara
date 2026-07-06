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
  Scatter,
  ScatterChart,
  Tooltip,
  XAxis,
  YAxis,
  ZAxis,
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

// Recharts scatter axes read the point's coordinates from fixed dataKeys.
// We project each series' Y (and optional bubble Z) onto these synthetic keys
// so a single XAxis/YAxis/ZAxis pair drives every cloud, regardless of whether
// the clouds come from `groupBy` partitions or from multiple Y series.
const SCATTER_Y_KEY = '__scatterY';
const SCATTER_Z_KEY = '__scatterZ';

type ChartData = {
  columns?: string[];
  rows?: unknown[][];
};

type TooltipPayload = {
  color?: string;
  dataKey?: string | number;
  name?: string | number;
  value?: unknown;
  /** The full data point behind this series entry (Recharts passes it). */
  payload?: Record<string, unknown>;
};

export function ChartBlock({
  block,
  result,
  onPointClick,
}: {
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  onPointClick?: (datum: Record<string, unknown>) => void;
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
  const handleChartClick = (event: unknown) => {
    const datum = chartClickDatum(event);
    if (datum) onPointClick?.(datum);
  };

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
      <div
        className={`h-80 overflow-hidden rounded-lg border bg-card p-4 shadow-sm ${
          onPointClick ? 'cursor-pointer' : ''
        }`}
      >
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
              onClick={handleChartClick}
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

  if (chart.kind === 'scatter') {
    const yField = series[0].field;
    const yLabel = series[0].label ?? yField;
    const sizeField = chart.sizeField ?? undefined;
    const sizeName = chart.sizeLabel ?? sizeField;
    const labelField = chart.labelField ?? undefined;
    const tooltipFields = chart.tooltipFields ?? [];
    const clouds = buildScatterClouds(chartRows, chart.x, series, {
      groupBy: chart.groupBy ?? undefined,
      sizeField,
    });
    return (
      <div
        className={`h-80 overflow-hidden rounded-lg border bg-card p-4 shadow-sm ${
          onPointClick ? 'cursor-pointer' : ''
        }`}
      >
        <ResponsiveContainer width="100%" height="100%">
          <ScatterChart margin={{ top: 8, right: 12, left: 0, bottom: 4 }}>
            <CartesianGrid
              stroke={GRID_COLOR}
              strokeDasharray="3 5"
              strokeOpacity={0.55}
            />
            <XAxis
              type="number"
              dataKey={chart.x}
              name={chart.x}
              domain={['auto', 'auto']}
              tickFormatter={compactNumber}
              axisLine={false}
              tickLine={false}
              tickMargin={10}
              tick={{ fill: AXIS_COLOR, fontSize: 12 }}
            />
            <YAxis
              type="number"
              dataKey={SCATTER_Y_KEY}
              name={yLabel}
              domain={['auto', 'auto']}
              width={52}
              tickFormatter={compactNumber}
              axisLine={false}
              tickLine={false}
              tickMargin={10}
              tick={{ fill: AXIS_COLOR, fontSize: 12 }}
            />
            {sizeField ? (
              <ZAxis
                type="number"
                dataKey={SCATTER_Z_KEY}
                name={sizeName}
                range={[60, 400]}
              />
            ) : null}
            <Tooltip
              cursor={{ strokeDasharray: '3 3' }}
              content={
                <ChartTooltip
                  labelField={labelField}
                  tooltipFields={tooltipFields}
                />
              }
            />
            <Legend
              iconType="circle"
              wrapperStyle={{ color: MUTED_TEXT_COLOR, fontSize: 12 }}
            />
            {clouds.map((cloud, index) => (
              <Scatter
                key={cloud.name}
                data={cloud.points}
                name={cloud.name}
                fill={COLORS[index % COLORS.length]}
                onClick={handleChartClick}
              />
            ))}
          </ScatterChart>
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
    <div
      className={`h-80 overflow-hidden rounded-lg border bg-card p-4 shadow-sm ${
        onPointClick ? 'cursor-pointer' : ''
      }`}
    >
      <ResponsiveContainer width="100%" height="100%">
        {chart.kind === 'bar' ? (
          <BarChart
            data={chartRows}
            margin={{ top: 8, right: 12, left: 0, bottom: 4 }}
            onClick={handleChartClick}
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
            onClick={handleChartClick}
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
            onClick={handleChartClick}
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

function chartClickDatum(event: unknown): Record<string, unknown> | null {
  if (!event || typeof event !== 'object') return null;
  const candidate = event as {
    activePayload?: Array<{ payload?: Record<string, unknown> }>;
    payload?: Record<string, unknown>;
  };
  return (
    candidate.activePayload?.[0]?.payload ??
    candidate.payload ??
    (event as Record<string, unknown>)
  );
}

function ChartTooltip({
  active,
  payload,
  label,
  labelField,
  tooltipFields,
}: {
  active?: boolean;
  payload?: TooltipPayload[];
  label?: unknown;
  /** Scatter-only: dimension whose value titles the point. Only the scatter
   *  chart passes this; the cartesian/pie tooltips leave it undefined and are
   *  byte-identical to before. */
  labelField?: string;
  /** Scatter-only: extra source columns to list under the title. */
  tooltipFields?: string[];
}) {
  if (!active || !payload?.length) {
    return null;
  }

  // The full data point behind the hovered marker (present for scatter).
  const point = payload[0]?.payload;
  // Gate strictly on the labelField prop being passed (scatter only) so other
  // chart kinds keep the numeric `label` header unchanged.
  const header =
    labelField && point && point[labelField] != null
      ? String(point[labelField])
      : String(label ?? '');
  const extraRows =
    point && tooltipFields?.length
      ? tooltipFields
          .filter((field) => field !== labelField)
          .map((field) => ({ field, value: point[field] }))
      : [];

  return (
    <div
      className="rounded-lg px-3 py-2 text-sm shadow-lg"
      style={{
        backgroundColor: SURFACE_COLOR,
        border: `1px solid ${BORDER_COLOR}`,
        color: TEXT_COLOR,
      }}
    >
      <div className="mb-1 font-medium">{header}</div>
      <div className="flex flex-col gap-1">
        {extraRows.map((row) => (
          <div
            key={`extra-${row.field}`}
            className="flex min-w-40 items-center justify-between gap-4"
          >
            <span className="truncate text-muted-foreground">{row.field}</span>
            <span className="font-medium">{formatTooltipValue(row.value)}</span>
          </div>
        ))}
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
): Array<{ field: string; label?: string | null }> {
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

type ScatterCloud = { name: string; points: Record<string, unknown>[] };

/**
 * Build one Recharts `<Scatter>` dataset per cloud. When `groupBy` is set the
 * points are partitioned by that field's distinct values (Y is always the first
 * series). Otherwise each configured Y series becomes its own cloud — so a
 * single-series scatter is one cloud and a multi-series scatter is N. Every
 * point's Y (and optional bubble Z) is projected onto the shared synthetic keys
 * the axes read, and coerced to a number so string-typed aggregates still plot.
 */
function buildScatterClouds(
  rows: Record<string, unknown>[],
  xField: string,
  series: Array<{ field: string; label?: string | null }>,
  opts: { groupBy?: string; sizeField?: string }
): ScatterCloud[] {
  const project = (
    row: Record<string, unknown>,
    yField: string
  ): Record<string, unknown> => ({
    ...row,
    [xField]: toChartNumber(row[xField]),
    [SCATTER_Y_KEY]: toChartNumber(row[yField]),
    ...(opts.sizeField
      ? { [SCATTER_Z_KEY]: toChartNumber(row[opts.sizeField]) }
      : {}),
  });

  if (opts.groupBy) {
    const groupField = opts.groupBy;
    const yField = series[0].field;
    const order: string[] = [];
    const byGroup = new Map<string, Record<string, unknown>[]>();
    for (const row of rows) {
      const key = String(row[groupField] ?? '—');
      let bucket = byGroup.get(key);
      if (!bucket) {
        bucket = [];
        byGroup.set(key, bucket);
        order.push(key);
      }
      bucket.push(project(row, yField));
    }
    return order.map((name) => ({
      name,
      points: byGroup.get(name) ?? [],
    }));
  }

  return series.map((s) => ({
    name: s.label ?? s.field,
    points: rows.map((row) => project(row, s.field)),
  }));
}

function toChartNumber(value: unknown): number | null {
  if (typeof value === 'number') return Number.isFinite(value) ? value : null;
  if (typeof value === 'string' && value.trim() !== '') {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

const COMPACT_NUMBER_FORMAT = new Intl.NumberFormat(undefined, {
  notation: 'compact',
  maximumFractionDigits: 1,
});

// Keeps numeric scatter axis ticks legible when values span orders of
// magnitude (e.g. 18,240,000 → "18.2M") so wide labels don't clip the plot.
function compactNumber(value: unknown): string {
  return typeof value === 'number' && Number.isFinite(value)
    ? COMPACT_NUMBER_FORMAT.format(value)
    : String(value ?? '');
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
