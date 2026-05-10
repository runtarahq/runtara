import { memo, useCallback, useMemo, useState } from 'react';
import {
  ArrowDown,
  ArrowUp,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Loader2,
  Pencil,
  Play,
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
import { Checkbox } from '@/shared/components/ui/checkbox';
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
  ReportEditorConfig,
  ReportOrderBy,
  ReportTableActionConfig,
  ReportTableColumn,
  ReportWorkflowActionConfig,
} from '../../types';
import {
  formatCellValue,
  humanizeFieldName,
  isWorkflowActionDisabled,
  isWorkflowActionVisible,
} from '../../utils';
import { FieldEditor } from './editable/FieldEditor';
import { useReportWriteback } from './editable/useReportWriteback';
import { useReportWorkflowAction } from './useReportWorkflowAction';

type TableColumn = {
  key: string;
  label?: string;
  displayField?: string;
  format?: string | null;
  type?: 'value' | 'chart' | 'workflow_button';
  chart?: ReportTableColumn['chart'];
  secondaryField?: string;
  linkField?: string;
  tooltipField?: string;
  pillVariants?: ReportTableColumn['pillVariants'];
  levels?: string[];
  align?: 'left' | 'right' | 'center';
  editable?: boolean;
  editor?: ReportEditorConfig;
  workflowAction?: ReportWorkflowActionConfig;
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
  missing?: boolean;
  unsatisfiedFilter?: string;
  message?: string;
};

type WritebackContext = { schemaId: string; instanceId: string };

type TableRowEntry = {
  row: Record<string, unknown> | unknown[];
  rowKey: string;
  rowObject: Record<string, unknown>;
};

type TableBlockProps = {
  reportId: string;
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  sort: ReportOrderBy[];
  filters: Record<string, unknown>;
  blockFilters: Record<string, unknown>;
  onPageChange: (offset: number, size: number) => void;
  onSortChange: (sort: ReportOrderBy[]) => void;
  onRowClick?: (row: Record<string, unknown>) => void;
  onCellClick?: (cell: Record<string, unknown>) => boolean;
  onRefresh?: () => void | Promise<void>;
};

const EMPTY_ROWS: NonNullable<TableData['rows']> = [];
const EMPTY_CONFIGURED_COLUMNS: ReportTableColumn[] = [];
const EMPTY_TABLE_ACTIONS: ReportTableActionConfig[] = [];
const EMPTY_DIAGNOSTICS: NonNullable<TableData['diagnostics']> = [];

export function TableBlock({
  reportId,
  block,
  result,
  sort,
  filters,
  blockFilters,
  onPageChange,
  onSortChange,
  onRowClick,
  onCellClick,
  onRefresh,
}: TableBlockProps) {
  const writeback = useReportWriteback(reportId);
  const [selectedRowKeys, setSelectedRowKeys] = useState<Set<string>>(
    () => new Set()
  );
  const workflowAction = useReportWorkflowAction({
    onCompleted: async () => {
      setSelectedRowKeys(new Set());
      await onRefresh?.();
    },
  });
  const [editingCell, setEditingCell] = useState<{
    rowKey: string;
    field: string;
  } | null>(null);
  const data = (result.data ?? {}) as TableData;
  const rows = data.rows ?? EMPTY_ROWS;
  const configuredColumns = block.table?.columns ?? EMPTY_CONFIGURED_COLUMNS;
  const columns = useMemo(
    () => normalizeColumns(data.columns, configuredColumns),
    [data.columns, configuredColumns]
  );
  const page = data.page ?? { offset: 0, size: 50, hasNextPage: false };
  const rowEntries = useMemo<TableRowEntry[]>(
    () =>
      rows.map((row, rowIndex) => ({
        row,
        rowKey: getRowKey(row, page.offset + rowIndex),
        rowObject: getRowObject(row, columns),
      })),
    [columns, page.offset, rows]
  );
  const tableActions = block.table?.actions ?? EMPTY_TABLE_ACTIONS;
  const selectable = Boolean(
    block.table?.selectable || tableActions.length > 0
  );
  const selectedRows = useMemo(
    () =>
      rowEntries
        .filter((entry) => selectedRowKeys.has(entry.rowKey))
        .map((entry) => entry.rowObject),
    [rowEntries, selectedRowKeys]
  );
  const allRowsSelected =
    rowEntries.length > 0 &&
    rowEntries.every((entry) => selectedRowKeys.has(entry.rowKey));
  const someRowsSelected =
    rowEntries.some((entry) => selectedRowKeys.has(entry.rowKey)) &&
    !allRowsSelected;
  const pageSizeOptions = useMemo(
    () => getPageSizeOptions(block, page.size),
    [block, page.size]
  );
  const diagnostics = data.diagnostics ?? EMPTY_DIAGNOSTICS;
  const writebackMutate = writeback.mutate;
  const writebackPending = writeback.isPending;

  const toggleAllRows = useCallback(
    (checked: boolean | 'indeterminate') => {
      setSelectedRowKeys((current) => {
        const next = new Set(current);
        for (const entry of rowEntries) {
          if (checked === true) {
            next.add(entry.rowKey);
          } else {
            next.delete(entry.rowKey);
          }
        }
        return next;
      });
    },
    [rowEntries]
  );

  const toggleRowSelected = useCallback(
    (rowKey: string, checked: boolean | 'indeterminate') => {
      setSelectedRowKeys((current) => {
        const next = new Set(current);
        if (checked === true) {
          next.add(rowKey);
        } else {
          next.delete(rowKey);
        }
        return next;
      });
    },
    []
  );

  const editCell = useCallback((rowKey: string, field: string) => {
    setEditingCell({ rowKey, field });
  }, []);

  const commitCell = useCallback(
    (
      writebackContext: WritebackContext | null,
      field: string,
      value: unknown,
      refreshAfterCommit: boolean
    ) => {
      if (writebackContext) {
        writebackMutate(
          {
            schemaId: writebackContext.schemaId,
            instanceId: writebackContext.instanceId,
            field,
            value,
          },
          {
            onSuccess: () => {
              if (refreshAfterCommit) {
                void onRefresh?.();
              }
            },
          }
        );
      }
      setEditingCell(null);
    },
    [onRefresh, writebackMutate]
  );

  const cancelCell = useCallback(() => setEditingCell(null), []);

  const showPagination =
    page.hasNextPage ||
    page.offset > 0 ||
    (typeof page.totalCount === 'number' && page.totalCount > page.size);

  if (data.missing && data.unsatisfiedFilter) {
    return (
      <div className="rounded-lg border border-dashed bg-muted/20 p-6 text-sm text-muted-foreground">
        {data.message ??
          `Required filter '${data.unsatisfiedFilter}' is not set.`}
      </div>
    );
  }

  if (columns.length === 0) {
    return (
      <div className="rounded-lg border bg-background p-6 text-sm text-muted-foreground">
        This table has no configured columns.
      </div>
    );
  }

  return (
    <div className="overflow-hidden rounded-lg border bg-background">
      {tableActions.length > 0 && (
        <TableActionsToolbar
          blockId={block.id}
          actions={tableActions}
          selectedRows={selectedRows}
          workflowAction={workflowAction}
        />
      )}
      <Table>
        <TableHeader>
          <TableRow className="group/header bg-muted/30 hover:bg-muted/30">
            {selectable && (
              <TableHead className="h-10 w-10">
                <Checkbox
                  aria-label="Select all rows"
                  checked={
                    allRowsSelected
                      ? true
                      : someRowsSelected
                        ? 'indeterminate'
                        : false
                  }
                  disabled={rowEntries.length === 0}
                  onCheckedChange={toggleAllRows}
                />
              </TableHead>
            )}
            {columns.map((column) => {
              const sortDirection = getColumnSortDirection(column.key, sort);
              const isSortable = !isNonSortableColumn(column);
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
                colSpan={columns.length + (selectable ? 1 : 0)}
                className="py-12 text-center text-sm text-muted-foreground"
              >
                No rows match the current filters.
              </TableCell>
            </TableRow>
          ) : (
            rowEntries.map(({ row, rowKey, rowObject }) => {
              return (
                <MemoizedTableBodyRow
                  key={rowKey}
                  reportId={reportId}
                  blockId={block.id}
                  row={row}
                  rowKey={rowKey}
                  rowObject={rowObject}
                  columns={columns}
                  selectable={selectable}
                  selected={selectedRowKeys.has(rowKey)}
                  editingField={
                    editingCell?.rowKey === rowKey ? editingCell.field : null
                  }
                  filters={filters}
                  blockFilters={blockFilters}
                  writebackPending={writebackPending}
                  onToggleSelected={toggleRowSelected}
                  onEditCell={editCell}
                  onCommitCell={commitCell}
                  onCancelCell={cancelCell}
                  onRowClick={onRowClick}
                  onCellClick={onCellClick}
                  onRunWorkflow={workflowAction.run}
                  isWorkflowRunning={workflowAction.isRunning}
                />
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

type TableBodyRowProps = {
  reportId: string;
  blockId: string;
  row: Record<string, unknown> | unknown[];
  rowKey: string;
  rowObject: Record<string, unknown>;
  columns: TableColumn[];
  selectable: boolean;
  selected: boolean;
  editingField: string | null;
  filters: Record<string, unknown>;
  blockFilters: Record<string, unknown>;
  writebackPending: boolean;
  onToggleSelected: (
    rowKey: string,
    checked: boolean | 'indeterminate'
  ) => void;
  onEditCell: (rowKey: string, field: string) => void;
  onCommitCell: (
    writebackContext: WritebackContext | null,
    field: string,
    value: unknown,
    refreshAfterCommit: boolean
  ) => void;
  onCancelCell: () => void;
  onRowClick?: (row: Record<string, unknown>) => void;
  onCellClick?: (cell: Record<string, unknown>) => boolean;
  onRunWorkflow: (args: {
    key: string;
    action: ReportWorkflowActionConfig;
    row?: Record<string, unknown>;
    value?: unknown;
    fallbackField?: string;
    selectedRows?: Record<string, unknown>[];
  }) => void | Promise<void>;
  isWorkflowRunning: (key: string) => boolean;
};

const MemoizedTableBodyRow = memo(TableBodyRow, areTableBodyRowPropsEqual);

function TableBodyRow({
  reportId,
  blockId,
  row,
  rowKey,
  rowObject,
  columns,
  selectable,
  selected,
  editingField,
  filters,
  blockFilters,
  writebackPending,
  onToggleSelected,
  onEditCell,
  onCommitCell,
  onCancelCell,
  onRowClick,
  onCellClick,
  onRunWorkflow,
  isWorkflowRunning,
}: TableBodyRowProps) {
  return (
    <TableRow
      className={
        onRowClick
          ? 'cursor-pointer transition-colors hover:bg-muted/40'
          : undefined
      }
      onClick={() => onRowClick?.(rowObject)}
    >
      {selectable && (
        <TableCell className="w-10 py-3 align-top">
          <Checkbox
            aria-label="Select row"
            checked={selected}
            onClick={(event) => event.stopPropagation()}
            onCheckedChange={(checked) => onToggleSelected(rowKey, checked)}
          />
        </TableCell>
      )}
      {columns.map((column, columnIndex) => {
        const value = getCellValue(row, column, columnIndex);
        const displayValue = getCellDisplayValue(rowObject, column, value);
        const writebackContext = getWritebackContext(column, rowObject);
        const workflowActionKey = `${blockId}:${rowKey}:${column.key}`;
        const isWorkflowActionRunning =
          column.workflowAction !== undefined &&
          isWorkflowRunning(workflowActionKey);
        const shouldRenderWorkflowAction =
          column.workflowAction !== undefined &&
          isWorkflowActionVisible(column.workflowAction, rowObject);
        const isWorkflowDisabled =
          column.workflowAction !== undefined &&
          isWorkflowActionDisabled(column.workflowAction, rowObject);
        const isEditing = editingField === column.key;

        return (
          <TableCell
            key={column.key}
            className={cn(
              'group/cell relative py-3 align-top',
              column.align === 'right' && 'text-right tabular-nums',
              writebackContext && !isEditing && 'pr-8'
            )}
            onClick={(event) => {
              if (isEditing) {
                event.stopPropagation();
                return;
              }
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
            {isEditing ? (
              <div onClick={(event) => event.stopPropagation()}>
                <FieldEditor
                  value={value}
                  displayValue={displayValue}
                  format={column.format}
                  pillVariants={column.pillVariants}
                  editor={column.editor}
                  lookupContext={{
                    reportId,
                    blockId,
                    field: column.key,
                    filters,
                    blockFilters,
                  }}
                  busy={writebackPending}
                  onCommit={(next) =>
                    onCommitCell(
                      writebackContext,
                      column.key,
                      next,
                      shouldRefreshAfterWriteback(column)
                    )
                  }
                  onCancel={onCancelCell}
                />
              </div>
            ) : (
              <>
                {column.workflowAction && shouldRenderWorkflowAction ? (
                  <WorkflowActionButton
                    action={column.workflowAction}
                    labelFallback="Run"
                    running={isWorkflowActionRunning}
                    disabled={isWorkflowDisabled}
                    value={value}
                    row={rowObject}
                    fallbackField={column.key}
                    actionKey={workflowActionKey}
                    onRun={onRunWorkflow}
                  />
                ) : column.workflowAction ? null : (
                  <TableCellValue
                    column={column}
                    value={value}
                    displayValue={displayValue}
                    row={rowObject}
                  />
                )}
                {writebackContext && (
                  <button
                    type="button"
                    aria-label="Edit cell"
                    className="absolute right-2 top-1/2 -translate-y-1/2 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-muted hover:text-foreground group-hover/cell:opacity-100"
                    onClick={(event) => {
                      event.stopPropagation();
                      onEditCell(rowKey, column.key);
                    }}
                  >
                    <Pencil className="h-3 w-3" />
                  </button>
                )}
              </>
            )}
          </TableCell>
        );
      })}
    </TableRow>
  );
}

function areTableBodyRowPropsEqual(
  previous: TableBodyRowProps,
  next: TableBodyRowProps
): boolean {
  const editingStateEqual =
    previous.editingField === next.editingField &&
    (previous.editingField === null ||
      (previous.writebackPending === next.writebackPending &&
        previous.filters === next.filters &&
        previous.blockFilters === next.blockFilters));

  return (
    previous.reportId === next.reportId &&
    previous.blockId === next.blockId &&
    previous.row === next.row &&
    previous.rowObject === next.rowObject &&
    previous.columns === next.columns &&
    previous.selectable === next.selectable &&
    previous.selected === next.selected &&
    editingStateEqual &&
    Boolean(previous.onRowClick) === Boolean(next.onRowClick) &&
    Boolean(previous.onCellClick) === Boolean(next.onCellClick) &&
    previous.isWorkflowRunning === next.isWorkflowRunning
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
      displayField: configured?.displayField ?? merged.displayField,
      secondaryField: configured?.secondaryField ?? merged.secondaryField,
      linkField: configured?.linkField ?? merged.linkField,
      tooltipField: configured?.tooltipField ?? merged.tooltipField,
      pillVariants: configured?.pillVariants ?? merged.pillVariants,
      levels: configured?.levels ?? merged.levels,
      align: configured?.align ?? merged.align ?? defaultAlign(merged.format),
      editable: configured?.editable ?? merged.editable,
      editor: configured?.editor ?? merged.editor,
      workflowAction: configured?.workflowAction ?? merged.workflowAction,
    };
  });
}

function isNonSortableColumn(column: TableColumn): boolean {
  return column.type === 'chart' || isWorkflowButtonColumn(column);
}

function isWorkflowButtonColumn(column: TableColumn): boolean {
  return (
    column.type === 'workflow_button' || column.workflowAction !== undefined
  );
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

function getCellDisplayValue(
  row: Record<string, unknown>,
  column: TableColumn,
  value: unknown
) {
  if (!column.displayField) return value;
  const displayValue = row[column.displayField];
  if (displayValue === null || displayValue === undefined) return value;
  if (typeof displayValue === 'string' && displayValue.trim().length === 0) {
    return value;
  }
  return displayValue;
}

function getWritebackContext(
  column: TableColumn,
  rowObject: Record<string, unknown>
): WritebackContext | null {
  if (!column.editable) return null;
  if (column.type === 'chart' || isWorkflowButtonColumn(column)) return null;
  const id = rowObject.id;
  const schemaId = rowObject.schemaId;
  if (typeof id !== 'string' || typeof schemaId !== 'string') return null;
  return { schemaId, instanceId: id };
}

function shouldRefreshAfterWriteback(column: TableColumn): boolean {
  return Boolean(column.displayField || column.editor?.kind === 'lookup');
}

function TableActionsToolbar({
  blockId,
  actions,
  selectedRows,
  workflowAction,
}: {
  blockId: string;
  actions: ReportTableActionConfig[];
  selectedRows: Record<string, unknown>[];
  workflowAction: ReturnType<typeof useReportWorkflowAction>;
}) {
  const selectedCount = selectedRows.length;

  return (
    <div className="report-print-hidden flex flex-col gap-3 border-b bg-muted/20 px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
      <span className="text-sm text-muted-foreground">
        {selectedCount === 1
          ? '1 row selected'
          : `${selectedCount} rows selected`}
      </span>
      <div className="flex flex-wrap gap-2">
        {actions.map((action) => {
          const actionKey = `${blockId}:table-action:${action.id}`;
          const running = workflowAction.isRunning(actionKey);
          return (
            <WorkflowActionButton
              key={action.id}
              action={action.workflowAction}
              labelFallback={action.label ?? 'Run'}
              running={running}
              disabled={selectedCount === 0}
              value={selectedRows}
              row={{}}
              selectedRows={selectedRows}
              fallbackField={action.id}
              actionKey={actionKey}
              onRun={workflowAction.run}
            />
          );
        })}
      </div>
    </div>
  );
}

function WorkflowActionButton({
  action,
  labelFallback,
  running,
  disabled,
  value,
  row,
  selectedRows,
  fallbackField,
  actionKey,
  onRun,
}: {
  action: ReportWorkflowActionConfig;
  labelFallback: string;
  running: boolean;
  disabled: boolean;
  value: unknown;
  row: Record<string, unknown>;
  selectedRows?: Record<string, unknown>[];
  fallbackField: string;
  actionKey: string;
  onRun: (args: {
    key: string;
    action: ReportWorkflowActionConfig;
    row?: Record<string, unknown>;
    value?: unknown;
    fallbackField?: string;
    selectedRows?: Record<string, unknown>[];
  }) => void | Promise<void>;
}) {
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      className="h-8 max-w-full gap-1.5"
      disabled={running || disabled}
      onClick={(event) => {
        event.stopPropagation();
        if (disabled) return;
        void onRun({
          key: actionKey,
          action,
          row,
          value,
          fallbackField,
          selectedRows,
        });
      }}
    >
      {running ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      ) : (
        <Play className="h-3.5 w-3.5" />
      )}
      <span className="truncate">
        {running
          ? (action.runningLabel ?? 'Running...')
          : (action.label ?? labelFallback)}
      </span>
    </Button>
  );
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
  displayValue,
  row,
}: {
  column: TableColumn;
  value: unknown;
  displayValue?: unknown;
  row: Record<string, unknown>;
}) {
  if (column.type === 'chart') {
    return <InlineTableChart column={column} value={value} />;
  }

  if (column.format === 'pill') {
    return <PillCell column={column} value={displayValue ?? value} />;
  }

  if (column.format === 'avatar_label') {
    return (
      <AvatarLabelCell
        column={column}
        value={displayValue ?? value}
        row={row}
      />
    );
  }

  if (column.format === 'bar_indicator') {
    return <BarIndicatorCell column={column} value={displayValue ?? value} />;
  }

  if (column.secondaryField || column.linkField) {
    return (
      <PrimaryWithSecondaryCell
        column={column}
        value={displayValue ?? value}
        row={row}
      />
    );
  }

  return (
    <>{formatCellValue(displayValue ?? value, column.format ?? undefined)}</>
  );
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
