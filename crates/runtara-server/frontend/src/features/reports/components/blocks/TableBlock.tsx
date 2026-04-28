import { ChevronLeft, ChevronRight } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import { ReportBlockDefinition, ReportBlockResult } from '../../types';
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
  onPageChange: (offset: number, size: number) => void;
};

export function TableBlock({ block, result, onPageChange }: TableBlockProps) {
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

  if (columns.length === 0) {
    return (
      <div className="rounded-lg border bg-background p-6 text-sm text-muted-foreground">
        This table has no configured columns.
      </div>
    );
  }

  return (
    <div className="rounded-lg border bg-background">
      <Table>
        <TableHeader>
          <TableRow>
            {columns.map((column) => (
              <TableHead key={column.key}>
                {column.label ?? humanizeFieldName(column.key)}
              </TableHead>
            ))}
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
      <div className="flex items-center justify-between border-t px-4 py-3 text-sm text-muted-foreground">
        <span>
          {page.totalCount !== undefined
            ? `${page.offset + 1}-${Math.min(page.offset + page.size, page.totalCount)} of ${page.totalCount}`
            : `Offset ${page.offset}`}
        </span>
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
  );
}
