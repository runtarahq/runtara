import {
  ArrowDown,
  ArrowUp,
  ChevronLeft,
  ChevronRight,
  ChevronsUpDown,
  Search,
} from 'lucide-react';
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
} from '../../types';
import { formatCellValue, humanizeFieldName } from '../../utils';

type TableData = {
  columns?: Array<{
    key: string;
    label?: string;
    format?: string | null;
  }>;
  rows?: Array<Record<string, unknown>>;
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
};

export function TableBlock({
  block,
  result,
  search,
  sort,
  onPageChange,
  onSearchChange,
  onSortChange,
}: TableBlockProps) {
  const data = (result.data ?? {}) as TableData;
  const rows = data.rows ?? [];
  const configuredColumns = block.table?.columns ?? [];
  const columns =
    data.columns && data.columns.length > 0
      ? data.columns
      : configuredColumns.map((column) => ({
          key: column.field,
          label: column.label ?? humanizeFieldName(column.field),
          format: column.format,
        }));
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
              return (
                <TableHead
                  key={column.key}
                  aria-sort={getAriaSort(sortDirection)}
                  className="whitespace-nowrap"
                >
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 text-left text-xs font-semibold uppercase tracking-wide text-muted-foreground transition-colors hover:text-foreground"
                    onClick={() =>
                      onSortChange(nextSortForColumn(column.key, sort))
                    }
                  >
                    <span>{column.label ?? humanizeFieldName(column.key)}</span>
                    <SortIcon direction={sortDirection} />
                  </button>
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
            rows.map((row, rowIndex) => (
              <TableRow key={String(row.id ?? rowIndex)}>
                {columns.map((column) => (
                  <TableCell key={column.key}>
                    {formatCellValue(
                      row[column.key],
                      column.format ?? undefined
                    )}
                  </TableCell>
                ))}
              </TableRow>
            ))
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
