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

const COLORS = ['#2563eb', '#16a34a', '#dc2626', '#9333ea', '#ea580c'];

type ChartData = {
  columns?: string[];
  rows?: unknown[][];
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
      <div className="h-80 rounded-lg border bg-background p-4">
        <ResponsiveContainer width="100%" height="100%">
          <PieChart>
            <Tooltip />
            <Legend />
            <Pie
              data={chartRows}
              dataKey={seriesField}
              nameKey={chart.x}
              innerRadius={chart.kind === 'donut' ? 60 : 0}
              outerRadius={110}
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
      <CartesianGrid strokeDasharray="3 3" />
      <XAxis dataKey={chart.x} />
      <YAxis />
      <Tooltip />
      <Legend />
    </>
  );

  return (
    <div className="h-80 rounded-lg border bg-background p-4">
      <ResponsiveContainer width="100%" height="100%">
        {chart.kind === 'bar' ? (
          <BarChart data={chartRows}>
            {common}
            {series.map((series, index) => (
              <Bar
                key={series.field}
                dataKey={series.field}
                name={series.label ?? series.field}
                fill={COLORS[index % COLORS.length]}
              />
            ))}
          </BarChart>
        ) : chart.kind === 'area' ? (
          <AreaChart data={chartRows}>
            {common}
            {series.map((series, index) => (
              <Area
                key={series.field}
                dataKey={series.field}
                name={series.label ?? series.field}
                stroke={COLORS[index % COLORS.length]}
                fill={COLORS[index % COLORS.length]}
                fillOpacity={0.18}
              />
            ))}
          </AreaChart>
        ) : (
          <LineChart data={chartRows}>
            {common}
            {series.map((series, index) => (
              <Line
                key={series.field}
                type="monotone"
                dataKey={series.field}
                name={series.label ?? series.field}
                stroke={COLORS[index % COLORS.length]}
                dot={false}
              />
            ))}
          </LineChart>
        )}
      </ResponsiveContainer>
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
