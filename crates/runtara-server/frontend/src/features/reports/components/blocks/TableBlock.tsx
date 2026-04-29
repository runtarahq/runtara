import {
  ArrowDown,
  ArrowUp,
  ChevronLeft,
  ChevronRight,
  ChevronsUpDown,
  Search,
} from 'lucide-react';
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
} from 'recharts';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportOrderBy,
  ReportTableColumn,
} from '../../types';
import { formatCellValue, humanizeFieldName } from '../../utils';

type TableColumn = {
  key: string;
  label?: string;
  format?: string | null;
  type?: 'value' | 'chart';
  chart?: ReportTableColumn['chart'];
};

type TableData = {
  columns?: Array<string | TableColumn>;
  rows?: Array<Record<string, unknown> | unknown[]>;
  page?: {
    offset: number;
    size: number;
    totalCount?: number;
    hasNextPage?: boolean;
  };
};

type TableBlockProps = {
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  search: string;
  sort: ReportOrderBy[];
  onPageChange: (offset: number, size: number) => void;
  onSearchChange: (search: string) => void;
  onSortChange: (sort: ReportOrderBy[]) => void;
  onRowClick?: (row: Record<string, unknown>) => void;
  onCellClick?: (cell: Record<string, unknown>) => boolean;
};

export function TableBlock({
  block,
  result,
  search,
  sort,
  onPageChange,
  onSearchChange,
  onSortChange,
  onRowClick,
  onCellClick,
}: TableBlockProps) {
  const data = (result.data ?? {}) as TableData;
  const rows = data.rows ?? [];
  const configuredColumns = block.table?.columns ?? [];
  const columns = normalizeColumns(data.columns, configuredColumns);
  const page = data.page ?? { offset: 0, size: 50, hasNextPage: false };
  const pageSizeOptions = getPageSizeOptions(block, page.size);

  if (columns.length === 0) {
    return (
      <div className="rounded-lg border bg-background p-6 text-sm text-muted-foreground">
        This table has no configured columns.
      </div>
    );
  }

  return (
    <div className="rounded-lg border bg-background">
      <div className="report-print-hidden flex flex-col gap-3 border-b px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="relative w-full sm:max-w-sm">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="search"
            value={search}
            onChange={(event) => onSearchChange(event.target.value)}
            placeholder="Search table"
            className="pl-9"
          />
        </div>
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            {columns.map((column) => {
              const sortDirection = getColumnSortDirection(column.key, sort);
              const isSortable = column.type !== 'chart';
              return (
                <TableHead
                  key={column.key}
                  aria-sort={getAriaSort(sortDirection)}
                  className="whitespace-nowrap"
                >
                  {isSortable ? (
                    <button
                      type="button"
                      className="flex w-full items-center gap-2 text-left text-xs font-semibold uppercase tracking-wide text-muted-foreground transition-colors hover:text-foreground"
                      onClick={() =>
                        onSortChange(nextSortForColumn(column.key, sort))
                      }
                    >
                      <span>
                        {column.label ?? humanizeFieldName(column.key)}
                      </span>
                      <SortIcon direction={sortDirection} />
                    </button>
                  ) : (
                    <span className="block text-left text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      {column.label ?? humanizeFieldName(column.key)}
                    </span>
                  )}
                </TableHead>
              );
            })}
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.length === 0 ? (
            <TableRow>
              <TableCell
                colSpan={columns.length}
                className="py-8 text-center text-muted-foreground"
              >
                No rows match the current filters.
              </TableCell>
            </TableRow>
          ) : (
            rows.map((row, rowIndex) => {
              const rowObject = getRowObject(row, columns);
              return (
                <TableRow
                  key={getRowKey(row, rowIndex)}
                  className={
                    onRowClick
                      ? 'cursor-pointer transition-colors hover:bg-muted/40'
                      : undefined
                  }
                  onClick={() => onRowClick?.(rowObject)}
                >
                  {columns.map((column, columnIndex) => {
                    const value = getCellValue(row, column, columnIndex);
                    return (
                      <TableCell
                        key={column.key}
                        onClick={(event) => {
                          if (!onCellClick) return;
                          const handled = onCellClick({
                            ...rowObject,
                            field: column.key,
                            value,
                          });
                          if (handled) {
                            event.stopPropagation();
                          }
                        }}
                      >
                        <TableCellValue column={column} value={value} />
                      </TableCell>
                    );
                  })}
                </TableRow>
              );
            })
          )}
        </TableBody>
      </Table>
      <div className="report-print-hidden flex flex-col gap-3 border-t px-4 py-3 text-sm text-muted-foreground sm:flex-row sm:items-center sm:justify-between">
        <span>{formatPageRange(page)}</span>
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
          {pageSizeOptions.length > 1 && (
            <div className="flex items-center gap-2">
              <span>Rows</span>
              <Select
                value={String(page.size)}
                onValueChange={(value) => onPageChange(0, Number(value))}
              >
                <SelectTrigger className="h-8 w-24">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {pageSizeOptions.map((size) => (
                    <SelectItem key={size} value={String(size)}>
                      {size}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={page.offset <= 0}
              onClick={() =>
                onPageChange(Math.max(0, page.offset - page.size), page.size)
              }
            >
              <ChevronLeft className="mr-1 h-4 w-4" />
              Previous
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={!page.hasNextPage}
              onClick={() => onPageChange(page.offset + page.size, page.size)}
            >
              Next
              <ChevronRight className="ml-1 h-4 w-4" />
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

function normalizeColumns(
  dataColumns: TableData['columns'],
  configuredColumns: ReportTableColumn[]
): TableColumn[] {
  const configuredByField = new Map(
    configuredColumns.map((column) => [column.field, column])
  );
  const sourceColumns =
    dataColumns && dataColumns.length > 0
      ? dataColumns
      : configuredColumns.map((column) => column.field);

  return sourceColumns.map((column) => {
    const key = typeof column === 'string' ? column : column.key;
    const configured = configuredByField.get(key);
    if (typeof column === 'string') {
      return {
        key,
        label: configured?.label ?? humanizeFieldName(key),
        format: configured?.format,
        type: configured?.type,
        chart: configured?.chart,
      };
    }

    return {
      ...column,
      label: column.label ?? configured?.label ?? humanizeFieldName(key),
      format: column.format ?? configured?.format,
      type: column.type ?? configured?.type,
      chart: column.chart ?? configured?.chart,
    };
  });
}

function getCellValue(
  row: Record<string, unknown> | unknown[],
  column: TableColumn,
  columnIndex: number
) {
  if (Array.isArray(row)) {
    return row[columnIndex];
  }
  return row[column.key];
}

function getRowObject(
  row: Record<string, unknown> | unknown[],
  columns: TableColumn[]
): Record<string, unknown> {
  if (!Array.isArray(row)) return row;
  return columns.reduce<Record<string, unknown>>((acc, column, index) => {
    acc[column.key] = row[index];
    return acc;
  }, {});
}

function getRowKey(row: Record<string, unknown> | unknown[], rowIndex: number) {
  if (Array.isArray(row)) {
    return String(row[0] ?? rowIndex);
  }
  return String(row.id ?? rowIndex);
}

function TableCellValue({
  column,
  value,
}: {
  column: TableColumn;
  value: unknown;
}) {
  if (column.type === 'chart') {
    return <InlineTableChart column={column} value={value} />;
  }

  return <>{formatCellValue(value, column.format ?? undefined)}</>;
}

type InlineChartData = {
  columns?: string[];
  rows?: unknown[][];
};

const INLINE_CHART_COLOR = 'hsl(var(--primary))';

function InlineTableChart({
  column,
  value,
}: {
  column: TableColumn;
  value: unknown;
}) {
  const data = (value ?? {}) as InlineChartData;
  const columns = data.columns ?? [];
  const rows = data.rows ?? [];
  const chart = column.chart;
  const series = getInlineChartSeries(column, columns);

  if (!chart || rows.length === 0 || series.length === 0) {
    return <span className="text-muted-foreground">-</span>;
  }

  const chartRows = rows.map((row) =>
    columns.reduce<Record<string, unknown>>((acc, columnName, index) => {
      acc[columnName] = row[index];
      return acc;
    }, {})
  );
  const seriesField = series[0].field;

  return (
    <div className="h-11 min-w-32">
      <ResponsiveContainer width="100%" height="100%">
        {chart.kind === 'bar' ? (
          <BarChart data={chartRows}>
            <Tooltip
              cursor={false}
              contentStyle={{ fontSize: 12 }}
              labelStyle={{ fontSize: 12 }}
            />
            <Bar
              dataKey={seriesField}
              fill={INLINE_CHART_COLOR}
              radius={[3, 3, 0, 0]}
            />
          </BarChart>
        ) : chart.kind === 'area' ? (
          <AreaChart data={chartRows}>
            <Tooltip
              cursor={false}
              contentStyle={{ fontSize: 12 }}
              labelStyle={{ fontSize: 12 }}
            />
            <Area
              type="monotone"
              dataKey={seriesField}
              stroke={INLINE_CHART_COLOR}
              fill={INLINE_CHART_COLOR}
              fillOpacity={0.16}
              strokeWidth={2}
              dot={false}
            />
          </AreaChart>
        ) : (
          <LineChart data={chartRows}>
            <Tooltip
              cursor={false}
              contentStyle={{ fontSize: 12 }}
              labelStyle={{ fontSize: 12 }}
            />
            <Line
              type="monotone"
              dataKey={seriesField}
              stroke={INLINE_CHART_COLOR}
              strokeWidth={2}
              dot={false}
            />
          </LineChart>
        )}
      </ResponsiveContainer>
    </div>
  );
}

function getInlineChartSeries(column: TableColumn, columns: string[]) {
  const configuredSeries = column.chart?.series ?? [];
  if (configuredSeries.length > 0) {
    return configuredSeries;
  }

  const fallbackField = columns.find((candidate) => candidate !== column.chart?.x);
  return fallbackField ? [{ field: fallbackField, label: fallbackField }] : [];
}

function getColumnSortDirection(field: string, sort: ReportOrderBy[]) {
  const entry = sort.find((item) => item.field === field);
  if (!entry) return undefined;
  return entry.direction?.toLowerCase() === 'desc' ? 'desc' : 'asc';
}

function nextSortForColumn(
  field: string,
  sort: ReportOrderBy[]
): ReportOrderBy[] {
  const currentDirection = getColumnSortDirection(field, sort);
  return [
    {
      field,
      direction: currentDirection === 'asc' ? 'desc' : 'asc',
    },
  ];
}

function SortIcon({ direction }: { direction?: 'asc' | 'desc' }) {
  if (direction === 'asc') {
    return <ArrowUp className="h-3.5 w-3.5 text-foreground" />;
  }
  if (direction === 'desc') {
    return <ArrowDown className="h-3.5 w-3.5 text-foreground" />;
  }
  return <ChevronsUpDown className="h-3.5 w-3.5 opacity-45" />;
}

function getAriaSort(direction?: 'asc' | 'desc') {
  if (direction === 'asc') return 'ascending';
  if (direction === 'desc') return 'descending';
  return 'none';
}

function getPageSizeOptions(block: ReportBlockDefinition, currentSize: number) {
  const configured = block.table?.pagination?.allowedPageSizes ?? [];
  const sizes = configured.length > 0 ? configured : [25, 50, 100];
  return Array.from(new Set([...sizes, currentSize]))
    .filter((size) => Number.isFinite(size) && size > 0)
    .sort((left, right) => left - right);
}

function formatPageRange(page: NonNullable<TableData['page']>) {
  if (page.totalCount === undefined) {
    return `Offset ${page.offset}`;
  }
  if (page.totalCount === 0) {
    return '0 of 0';
  }
  return `${page.offset + 1}-${Math.min(
    page.offset + page.size,
    page.totalCount
  )} of ${page.totalCount}`;
}
