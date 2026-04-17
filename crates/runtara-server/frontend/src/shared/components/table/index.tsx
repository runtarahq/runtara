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
import { Icons } from '@/shared/components/icons.tsx';
import { SkeletonTable } from './skeleton-table.tsx';
import {
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
  enableRowSelection?: boolean;
  rowSelection?: RowSelectionState;
  onRowSelectionChange?: (selection: RowSelectionState) => void;
  getRowId?: (originalRow: TData, index: number) => string;
  getRowClassName?: (row: Row<TData>) => string;
  afterTableSlot?: React.ReactNode;
  beforePaginationSlot?: React.ReactNode;
}

const DEFAULT_COLUMN_WIDTH = 150;

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
        isNested ? 'w-full' : 'max-w-full overflow-hidden'
      )}
    >
      <div className="w-full">
        <Table variant={isNested ? 'nested' : 'default'}>
          <TableHeader>
            {table.getHeaderGroups().map((headerGroup) => (
              <TableRow key={headerGroup.id}>
                {headerGroup.headers.map((header) => {
                  const styles =
                    header.getSize() !== DEFAULT_COLUMN_WIDTH
                      ? { width: `${header.getSize()}px` }
                      : {};

                  const canSort = header.column.getCanSort();

                  return (
                    <TableHead
                      key={header.id}
                      style={styles}
                      className={cn(
                        (header.column.columnDef.meta as any)?.headerClassName
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
                          <div className="flex items-center gap-1">
                            {flexRender(
                              header.column.columnDef.header,
                              header.getContext()
                            )}
                            {canSort &&
                              (header.column.getIsSorted() === 'asc' ? (
                                <Icons.chevronUp className="w-4 h-4" />
                              ) : header.column.getIsSorted() === 'desc' ? (
                                <Icons.chevronDown className="w-4 h-4" />
                              ) : (
                                <Icons.chevronsUpDown className="w-4 h-4 opacity-50" />
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
                    {row.getVisibleCells().map((cell) => (
                      <TableCell
                        key={cell.id}
                        className={cn(
                          (cell.column.columnDef.meta as any)?.cellClassName
                        )}
                      >
                        {flexRender(
                          cell.column.columnDef.cell,
                          cell.getContext()
                        )}
                      </TableCell>
                    ))}
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
      {shouldRenderPagination && (
        <div className="px-5 py-4 border-t border-slate-200 bg-slate-50/30 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between dark:border-slate-700 dark:bg-slate-800/30">
          <div className="flex flex-wrap items-center gap-3 text-sm text-slate-600 dark:text-slate-400">
            <span>
              Rows {startRow}-{endRow} of {totalRowCount.toLocaleString()}
            </span>
            <div className="flex items-center gap-2">
              <span className="text-sm text-slate-500 dark:text-slate-400">
                Page size:
              </span>
              <select
                className="h-8 rounded-md border border-slate-200 bg-white px-2.5 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300"
                value={pagination.pageSize}
                onChange={(event) =>
                  pagination.onPageSizeChange?.(Number(event.target.value))
                }
              >
                {[10, 20, 50, 100].map((size) => (
                  <option key={size} value={size}>
                    {size} / page
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="flex items-center gap-3">
            <span className="text-sm text-slate-600 dark:text-slate-400">
              Page {table.getState().pagination.pageIndex + 1} of{' '}
              {table.getPageCount().toLocaleString()}
            </span>
            <div className="flex items-center gap-1">
              <button
                className="p-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-700"
                disabled={!table.getCanPreviousPage()}
                onClick={() => table.firstPage()}
              >
                <ChevronFirst size={16} />
              </button>
              <button
                className="p-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-700"
                disabled={!table.getCanPreviousPage()}
                onClick={() => table.previousPage()}
              >
                <ChevronLeft size={16} />
              </button>
              <button
                className="p-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-700"
                disabled={!table.getCanNextPage()}
                onClick={() => table.nextPage()}
              >
                <ChevronRight size={16} />
              </button>
              <button
                className="p-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-700"
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
