import React from 'react';
import {
  ColumnDef,
  flexRender,
  getCoreRowModel,
  getExpandedRowModel,
  getSortedRowModel,
  OnChangeFn,
  PaginationState,
  Row,
  RowSelectionState,
  SortingState,
  useReactTable,
} from '@tanstack/react-table';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table.tsx';
import { SkeletonTable } from './skeleton-table.tsx';
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  ChevronFirst,
  ChevronLast,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import { cn } from '@/lib/utils.ts';

interface DataTableProps<TData, TValue> {
  columns: ColumnDef<TData, TValue>[];
  data: TData[];
  pagination?: {
    pageIndex: number;
    pageSize: number;
    pageCount?: number;
    totalCount?: number;
    onPageChange?: (page: number) => void;
    onPageSizeChange?: (pageSize: number) => void;
  };
  totalPages?: number;
  setPagination?: OnChangeFn<PaginationState>;
  shouldRenderPagination?: boolean;
  isFetching?: boolean;
  getRowCanExpand?: (row: Row<TData>) => boolean;
  SubComponent?: React.ComponentType<{ row: Row<TData> }>;
  isNested?: boolean;
  initialState?: {
    sorting?: {
      id: string;
      desc: boolean;
    }[];
  };
  sorting?: SortingState;
  onSortingChange?: OnChangeFn<SortingState>;
  manualSorting?: boolean;
  /**
   * Console look: sticky table header, no inner scroll wrapper (the table is
   * meant to live inside a ConsoleTableShell scroll body), refined sort icons
   * and a primary-tinted selected row.
   */
  stickyHeader?: boolean;
  enableRowSelection?: boolean;
  rowSelection?: RowSelectionState;
  onRowSelectionChange?: (selection: RowSelectionState) => void;
  getRowId?: (originalRow: TData, index: number) => string;
  getRowClassName?: (row: Row<TData>) => string;
  afterTableSlot?: React.ReactNode;
  beforePaginationSlot?: React.ReactNode;
}

const DEFAULT_COLUMN_WIDTH = 150;
const PAGE_SIZE_OPTIONS = [10, 20, 50, 100];

export function DataTable<TData, TValue>({
  columns,
  data = [] as TData[],
  pagination = { pageIndex: 0, pageSize: 10 },
  totalPages,
  setPagination,
  shouldRenderPagination = true,
  isFetching = false,
  getRowCanExpand = () => false,
  SubComponent,
  isNested = false,
  initialState,
  sorting: controlledSorting,
  onSortingChange,
  manualSorting = false,
  stickyHeader = false,
  enableRowSelection = false,
  rowSelection = {},
  onRowSelectionChange,
  getRowId,
  getRowClassName,
  afterTableSlot,
  beforePaginationSlot,
}: DataTableProps<TData, TValue>) {
  const table = useReactTable({
    data,
    columns,
    defaultColumn: {
      enableColumnFilter: false,
    },
    state: {
      pagination: {
        pageIndex: pagination.pageIndex,
        pageSize: pagination.pageSize,
      },
      sorting:
        controlledSorting ??
        (initialState?.sorting ? initialState.sorting : []),
      rowSelection: enableRowSelection ? rowSelection : {},
    },
    enableSorting: true,
    manualSorting,
    onSortingChange,
    enableRowSelection,
    manualPagination: true,
    rowCount: pagination.totalCount || data.length,
    pageCount: pagination.pageCount || totalPages || 0,
    onRowSelectionChange: enableRowSelection
      ? (updatedSelection) => {
          const newSelection =
            typeof updatedSelection === 'function'
              ? updatedSelection(rowSelection)
              : updatedSelection;
          onRowSelectionChange?.(newSelection);
        }
      : undefined,
    onPaginationChange: (updatedPagination) => {
      // Handle both function and direct object cases for Updater<PaginationState>
      const newPagination =
        typeof updatedPagination === 'function'
          ? updatedPagination({
              pageIndex: pagination.pageIndex,
              pageSize: pagination.pageSize,
            })
          : updatedPagination;

      if (
        pagination.onPageChange &&
        newPagination.pageIndex !== pagination.pageIndex
      ) {
        pagination.onPageChange(newPagination.pageIndex);
      }
      if (
        pagination.onPageSizeChange &&
        newPagination.pageSize !== pagination.pageSize
      ) {
        pagination.onPageSizeChange(newPagination.pageSize);
      }
      if (setPagination) {
        setPagination(updatedPagination);
      }
    },
    getRowCanExpand,
    getRowId,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getExpandedRowModel: getExpandedRowModel(),
  });

  const totalRowCount =
    pagination.totalCount !== undefined
      ? pagination.totalCount
      : data.length || 0;
  const startRow =
    totalRowCount === 0 || data.length === 0
      ? 0
      : pagination.pageIndex * pagination.pageSize + 1;
  const endRow =
    totalRowCount === 0 || data.length === 0
      ? 0
      : Math.min(startRow + data.length - 1, totalRowCount);

  // Only show skeleton during initial load, not during refetch when we have data
  if (isFetching && data.length === 0) {
    return <SkeletonTable />;
  }

  return (
    <div
      className={cn(
        'flex flex-1 flex-col',
        isNested
          ? 'w-full'
          : stickyHeader
            ? 'w-full'
            : 'max-w-full overflow-hidden'
      )}
    >
      <div className="w-full">
        <Table
          variant={isNested ? 'nested' : stickyHeader ? 'console' : 'default'}
          className={stickyHeader ? 'min-w-max' : undefined}
        >
          <TableHeader>
            {table.getHeaderGroups().map((headerGroup) => (
              <TableRow key={headerGroup.id}>
                {headerGroup.headers.map((header) => {
                  const styles =
                    header.getSize() !== DEFAULT_COLUMN_WIDTH
                      ? { width: `${header.getSize()}px` }
                      : {};

                  const canSort = header.column.getCanSort();
                  const meta = header.column.columnDef.meta as any;
                  const alignRight = meta?.align === 'right';
                  const sorted = header.column.getIsSorted();

                  return (
                    <TableHead
                      key={header.id}
                      style={styles}
                      className={cn(
                        alignRight && 'text-right',
                        meta?.headerClassName
                      )}
                    >
                      {header.isPlaceholder ? null : (
                        <div
                          className={
                            canSort ? 'cursor-pointer select-none' : ''
                          }
                          onClick={
                            canSort
                              ? () => {
                                  const currentSort =
                                    header.column.getIsSorted();
                                  // Cycle: false -> desc -> asc -> desc
                                  const nextDesc =
                                    currentSort === 'desc' ? false : true;
                                  onSortingChange?.([
                                    { id: header.column.id, desc: nextDesc },
                                  ]);
                                }
                              : undefined
                          }
                        >
                          <div
                            className={cn(
                              'flex items-center gap-1',
                              alignRight && 'justify-end'
                            )}
                          >
                            {flexRender(
                              header.column.columnDef.header,
                              header.getContext()
                            )}
                            {canSort &&
                              (sorted === 'asc' ? (
                                <ArrowUp className="h-3.5 w-3.5 text-primary" />
                              ) : sorted === 'desc' ? (
                                <ArrowDown className="h-3.5 w-3.5 text-primary" />
                              ) : (
                                <ArrowUpDown className="h-3.5 w-3.5 text-muted-foreground/40" />
                              ))}
                          </div>
                        </div>
                      )}
                    </TableHead>
                  );
                })}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {table.getRowModel().rows?.length ? (
              table.getRowModel().rows.map((row) => (
                <React.Fragment key={row.id}>
                  <TableRow
                    data-state={row.getIsSelected() && 'selected'}
                    className={getRowClassName?.(row)}
                  >
                    {row.getVisibleCells().map((cell) => {
                      const cellMeta = cell.column.columnDef.meta as any;
                      return (
                        <TableCell
                          key={cell.id}
                          className={cn(
                            cellMeta?.align === 'right' && 'text-right',
                            cellMeta?.mono && 'font-mono text-[12.5px]',
                            cellMeta?.cellClassName
                          )}
                        >
                          {flexRender(
                            cell.column.columnDef.cell,
                            cell.getContext()
                          )}
                        </TableCell>
                      );
                    })}
                  </TableRow>
                  {row.getIsExpanded() && (
                    <tr className="hover:bg-transparent">
                      <td
                        colSpan={row.getAllCells().length}
                        className="!p-0 !border-0"
                      >
                        <div className="relative">
                          <div className="absolute inset-0 bg-muted/30 border-t" />
                          <div className="relative p-4">
                            <div className="overflow-x-auto -mx-4 px-4">
                              {SubComponent && <SubComponent row={row} />}
                            </div>
                          </div>
                        </div>
                      </td>
                    </tr>
                  )}
                </React.Fragment>
              ))
            ) : (
              <TableRow>
                <TableCell
                  colSpan={columns.length}
                  className="h-24 text-center"
                >
                  No results.
                </TableCell>
              </TableRow>
            )}
            {afterTableSlot}
          </TableBody>
        </Table>
      </div>
      {beforePaginationSlot}
      {shouldRenderPagination && totalRowCount > PAGE_SIZE_OPTIONS[0] && (
        <div className="px-3 py-2.5 border-t bg-muted/30 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex flex-wrap items-center gap-3 text-sm text-muted-foreground">
            <span>
              Rows {startRow}-{endRow} of {totalRowCount.toLocaleString()}
            </span>
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">
                Page size:
              </span>
              <select
                className="h-8 rounded-md border bg-background px-2.5 text-sm text-foreground"
                value={pagination.pageSize}
                onChange={(event) =>
                  pagination.onPageSizeChange?.(Number(event.target.value))
                }
              >
                {PAGE_SIZE_OPTIONS.map((size) => (
                  <option key={size} value={size}>
                    {size} / page
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="flex items-center gap-3">
            <span className="text-sm text-muted-foreground">
              Page {table.getState().pagination.pageIndex + 1} of{' '}
              {table.getPageCount().toLocaleString()}
            </span>
            <div className="flex items-center gap-1">
              <button
                className="p-1.5 text-muted-foreground hover:text-foreground hover:bg-muted rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-foreground"
                disabled={!table.getCanPreviousPage()}
                onClick={() => table.firstPage()}
              >
                <ChevronFirst size={16} />
              </button>
              <button
                className="p-1.5 text-muted-foreground hover:text-foreground hover:bg-muted rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-foreground"
                disabled={!table.getCanPreviousPage()}
                onClick={() => table.previousPage()}
              >
                <ChevronLeft size={16} />
              </button>
              <button
                className="p-1.5 text-muted-foreground hover:text-foreground hover:bg-muted rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-foreground"
                disabled={!table.getCanNextPage()}
                onClick={() => table.nextPage()}
              >
                <ChevronRight size={16} />
              </button>
              <button
                className="p-1.5 text-muted-foreground hover:text-foreground hover:bg-muted rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-foreground"
                disabled={!table.getCanNextPage()}
                onClick={() => table.lastPage()}
              >
                <ChevronLast size={16} />
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
