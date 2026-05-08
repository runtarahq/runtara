import {
  ArrowDown,
  ArrowUp,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
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
import { Badge } from '@/shared/components/ui/badge';
import { cn } from '@/lib/utils';
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
  secondaryField?: string;
  linkField?: string;
  tooltipField?: string;
  pillVariants?: ReportTableColumn['pillVariants'];
  levels?: string[];
  align?: 'left' | 'right' | 'center';
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
  diagnostics?: Array<{
    severity?: 'warning' | string;
    code?: string;
    message: string;
  }>;
};

type TableBlockProps = {
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  sort: ReportOrderBy[];
  onPageChange: (offset: number, size: number) => void;
  onSortChange: (sort: ReportOrderBy[]) => void;
  onRowClick?: (row: Record<string, unknown>) => void;
  onCellClick?: (cell: Record<string, unknown>) => boolean;
};

export function TableBlock({
  block,
  result,
  sort,
  onPageChange,
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
  const diagnostics = data.diagnostics ?? [];

  const showPagination =
    page.hasNextPage ||
    page.offset > 0 ||
    (typeof page.totalCount === 'number' && page.totalCount > page.size);

  if (columns.length === 0) {
    return (
      <div className="rounded-lg border bg-background p-6 text-sm text-muted-foreground">
        This table has no configured columns.
      </div>
    );
  }

  return (
    <div className="overflow-hidden rounded-lg border bg-background">
      <Table>
        <TableHeader>
          <TableRow className="group/header bg-muted/30 hover:bg-muted/30">
            {columns.map((column) => {
              const sortDirection = getColumnSortDirection(column.key, sort);
              const isSortable = column.type !== 'chart';
              return (
                <TableHead
                  key={column.key}
                  aria-sort={getAriaSort(sortDirection)}
                  className={cn(
                    'h-10 whitespace-nowrap',
                    column.align === 'right' && 'text-right'
                  )}
                >
                  {isSortable ? (
                    <button
                      type="button"
                      className={cn(
                        'group/h flex w-full items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground transition-colors hover:text-foreground',
                        column.align === 'right' && 'justify-end'
                      )}
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
                    <span
                      className={cn(
                        'block text-xs font-semibold uppercase tracking-wide text-muted-foreground',
                        column.align === 'right' ? 'text-right' : 'text-left'
                      )}
                    >
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
                className="py-12 text-center text-sm text-muted-foreground"
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
                        className={cn(
                          'py-3 align-top',
                          column.align === 'right' &&
                            'text-right tabular-nums'
                        )}
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
                        <TableCellValue
                          column={column}
                          value={value}
                          row={rowObject}
                        />
                      </TableCell>
                    );
                  })}
                </TableRow>
              );
            })
          )}
        </TableBody>
      </Table>
      {showPagination && (
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
      )}
      {diagnostics.length > 0 && (
        <div className="border-t bg-muted/20 px-4 py-3 text-xs text-muted-foreground">
          {diagnostics.map((diagnostic, index) => (
            <p key={`${diagnostic.code ?? 'diagnostic'}-${index}`}>
              {diagnostic.message}
            </p>
          ))}
        </div>
      )}
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
    const merged: TableColumn =
      typeof column === 'string'
        ? {
            key,
            label: configured?.label ?? humanizeFieldName(key),
            format: configured?.format,
            type: configured?.type,
            chart: configured?.chart,
          }
        : {
            ...column,
            label: column.label ?? configured?.label ?? humanizeFieldName(key),
            format: column.format ?? configured?.format,
            type: column.type ?? configured?.type,
            chart: column.chart ?? configured?.chart,
          };

    return {
      ...merged,
      secondaryField: configured?.secondaryField ?? merged.secondaryField,
      linkField: configured?.linkField ?? merged.linkField,
      tooltipField: configured?.tooltipField ?? merged.tooltipField,
      pillVariants: configured?.pillVariants ?? merged.pillVariants,
      levels: configured?.levels ?? merged.levels,
      align: configured?.align ?? merged.align ?? defaultAlign(merged.format),
    };
  });
}

function defaultAlign(format?: string | null): TableColumn['align'] {
  if (!format) return undefined;
  if (
    format === 'currency' ||
    format === 'currency_compact' ||
    format === 'number' ||
    format === 'decimal' ||
    format === 'percent'
  ) {
    return 'right';
  }
  return undefined;
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
  row,
}: {
  column: TableColumn;
  value: unknown;
  row: Record<string, unknown>;
}) {
  if (column.type === 'chart') {
    return <InlineTableChart column={column} value={value} />;
  }

  if (column.format === 'pill') {
    return <PillCell column={column} value={value} />;
  }

  if (column.format === 'avatar_label') {
    return <AvatarLabelCell column={column} value={value} row={row} />;
  }

  if (column.format === 'bar_indicator') {
    return <BarIndicatorCell column={column} value={value} />;
  }

  if (column.secondaryField || column.linkField) {
    return <PrimaryWithSecondaryCell column={column} value={value} row={row} />;
  }

  return <>{formatCellValue(value, column.format ?? undefined)}</>;
}

function PillCell({ column, value }: { column: TableColumn; value: unknown }) {
  const key = typeof value === 'string' ? value : String(value ?? '');
  const variant = (column.pillVariants?.[key] ?? 'default') as
    | 'default'
    | 'secondary'
    | 'destructive'
    | 'outline'
    | 'success'
    | 'warning'
    | 'muted';
  return (
    <Badge variant={variant} className="rounded-full px-2.5 py-0.5">
      {humanizePillLabel(key)}
    </Badge>
  );
}

function humanizePillLabel(value: string): string {
  if (!value) return '—';
  return humanizeFieldName(value);
}

function AvatarLabelCell({
  column,
  value,
  row,
}: {
  column: TableColumn;
  value: unknown;
  row: Record<string, unknown>;
}) {
  const raw = typeof value === 'string' ? value : String(value ?? '');
  const tooltipValue = column.tooltipField
    ? row[column.tooltipField]
    : undefined;
  const display = displayNameFromValue(raw);
  const initials = initialsFromValue(display);
  const colorClass = colorClassForKey(raw);
  return (
    <div
      className="flex items-center gap-2"
      title={typeof tooltipValue === 'string' ? tooltipValue : raw}
    >
      <span
        className={cn(
          'flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold uppercase tracking-wide text-white',
          colorClass
        )}
        aria-hidden
      >
        {initials}
      </span>
      <span className="truncate">{display}</span>
    </div>
  );
}

function displayNameFromValue(raw: string): string {
  if (!raw) return '—';
  const local = raw.includes('@') ? raw.split('@')[0] : raw;
  return local
    .split(/[._-]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

function initialsFromValue(display: string): string {
  const parts = display.split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return `${parts[0][0]}${parts[parts.length - 1][0]}`.toUpperCase();
}

const AVATAR_COLORS = [
  'bg-violet-500',
  'bg-emerald-500',
  'bg-amber-500',
  'bg-sky-500',
  'bg-rose-500',
  'bg-cyan-500',
  'bg-indigo-500',
];

function colorClassForKey(key: string): string {
  let hash = 0;
  for (let i = 0; i < key.length; i += 1) {
    hash = (hash * 31 + key.charCodeAt(i)) | 0;
  }
  return AVATAR_COLORS[Math.abs(hash) % AVATAR_COLORS.length];
}

function BarIndicatorCell({
  column,
  value,
}: {
  column: TableColumn;
  value: unknown;
}) {
  const levels = column.levels ?? [];
  const key = typeof value === 'string' ? value : String(value ?? '');
  const idx = levels.indexOf(key);
  const total = levels.length;
  const filled = idx >= 0 ? idx + 1 : 0;
  return (
    <div className="flex items-center gap-2">
      <div className="flex items-end gap-0.5" aria-hidden>
        {Array.from({ length: Math.max(total, 1) }).map((_, i) => {
          const isFilled = i < filled;
          const heights = ['h-1.5', 'h-2', 'h-2.5', 'h-3'];
          const heightClass = heights[Math.min(i, heights.length - 1)];
          return (
            <span
              key={i}
              className={cn(
                'w-1 rounded-sm',
                heightClass,
                isFilled ? 'bg-foreground' : 'bg-muted'
              )}
            />
          );
        })}
      </div>
      <span className="text-sm">{humanizePillLabel(key)}</span>
    </div>
  );
}

function PrimaryWithSecondaryCell({
  column,
  value,
  row,
}: {
  column: TableColumn;
  value: unknown;
  row: Record<string, unknown>;
}) {
  const secondary = column.secondaryField
    ? formatCellValue(row[column.secondaryField])
    : undefined;
  const linkRaw = column.linkField ? row[column.linkField] : undefined;
  const link = typeof linkRaw === 'string' && linkRaw ? linkRaw : undefined;
  const primary = formatCellValue(value, column.format ?? undefined);
  return (
    <div className="flex flex-col">
      <div className="flex items-center gap-1.5">
        <span className="font-medium text-foreground">{primary}</span>
        {link && (
          <a
            href={link}
            target="_blank"
            rel="noreferrer noopener"
            onClick={(event) => event.stopPropagation()}
            className="text-muted-foreground hover:text-foreground"
            aria-label="Open link"
          >
            <ExternalLink className="h-3.5 w-3.5" />
          </a>
        )}
      </div>
      {secondary && (
        <span className="text-xs text-muted-foreground">{secondary}</span>
      )}
    </div>
  );
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

  const fallbackField = columns.find(
    (candidate) => candidate !== column.chart?.x
  );
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
  return (
    <ArrowUp className="h-3.5 w-3.5 opacity-0 transition-opacity group-hover/h:opacity-40" />
  );
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
