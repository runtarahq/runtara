import {
  memo,
  useCallback,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from 'react';
import {
  Activity,
  ArrowDown,
  ArrowRight,
  ArrowUp,
  ChevronLeft,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  Eye,
  ExternalLink,
  FileText,
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
  Tooltip as RechartsTooltip,
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
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/shared/components/ui/tooltip';
import { cn } from '@/lib/utils';
import {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportEditorConfig,
  ReportInteractionAction,
  ReportOrderBy,
  ReportTableActionConfig,
  ReportTableColumn,
  ReportTableInteractionButtonConfig,
  ReportWorkflowActionConfig,
} from '../../types';
import {
  formatCellValue,
  getReportRowValue,
  humanizeFieldName,
  isWorkflowActionDisabled,
  isWorkflowActionVisible,
  matchesReportRowCondition,
  renderDisplayTemplate,
  truncateCellText,
} from '../../utils';
import { FieldEditor } from './editable/FieldEditor';
import { useReportWriteback } from './editable/useReportWriteback';
import {
  ReportWorkflowActionPhase,
  ReportWorkflowActionResult,
  useReportWorkflowAction,
} from './useReportWorkflowAction';

type TableColumn = {
  key: string;
  label?: string | null;
  displayField?: string | null;
  displayTemplate?: string | null;
  format?: string | null;
  type?: 'value' | 'chart' | 'workflow_button' | 'interaction_buttons' | null;
  chart?: ReportTableColumn['chart'];
  secondaryField?: string | null;
  linkField?: string | null;
  tooltipField?: string | null;
  pillVariants?: ReportTableColumn['pillVariants'];
  levels?: string[] | null;
  align?: 'left' | 'right' | 'center' | string | null;
  maxChars?: number | null;
  editable?: boolean | null;
  editor?: ReportEditorConfig | null;
  workflowAction?: ReportWorkflowActionConfig | null;
  interactionButtons?: ReportTableInteractionButtonConfig[];
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
  activeViewId?: string | null;
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  sort: ReportOrderBy[];
  filters: Record<string, unknown>;
  blockFilters: Record<string, unknown>;
  onPageChange: (offset: number, size: number) => void;
  onSortChange: (sort: ReportOrderBy[]) => void;
  onRowClick?: (row: Record<string, unknown>) => void;
  onCellClick?: (cell: Record<string, unknown>) => boolean;
  onInteractionButtonClick?: (
    actions: ReportInteractionAction[],
    row: Record<string, unknown>
  ) => boolean;
  onRefresh?: (
    result?: ReportWorkflowActionResult,
    action?: ReportWorkflowActionConfig
  ) => void | Promise<void>;
};

const EMPTY_ROWS: NonNullable<TableData['rows']> = [];
const EMPTY_CONFIGURED_COLUMNS: ReportTableColumn[] = [];
const EMPTY_TABLE_ACTIONS: ReportTableActionConfig[] = [];
const EMPTY_DIAGNOSTICS: NonNullable<TableData['diagnostics']> = [];

export function TableBlock({
  reportId,
  activeViewId,
  block,
  result,
  sort,
  filters,
  blockFilters,
  onPageChange,
  onSortChange,
  onRowClick,
  onCellClick,
  onInteractionButtonClick,
  onRefresh,
}: TableBlockProps) {
  const writeback = useReportWriteback(reportId);
  const [selectedRowKeys, setSelectedRowKeys] = useState<Set<string>>(
    () => new Set()
  );
  const workflowAction = useReportWorkflowAction({
    onCompleted: async (result, action) => {
      setSelectedRowKeys(new Set());
      await onRefresh?.(result, action);
    },
    report: {
      reportId,
      blockId: block.id,
      viewId: activeViewId,
      filters,
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
  const totalPages = getTotalPages(page);
  const currentPage =
    totalPages && totalPages > 0
      ? Math.floor(page.offset / Math.max(page.size, 1)) + 1
      : undefined;
  const lastPageOffset =
    totalPages && totalPages > 0 ? (totalPages - 1) * page.size : undefined;
  const columnLayouts = useMemo(
    () => columns.map((column, idx) => inferColumnLayout(column, rows, idx)),
    [columns, rows]
  );
  // Every column carries a concrete width, so a trailing filler column always
  // absorbs leftover space (keeping columns at their natural size instead of
  // stretching them) and collapses to zero when the table needs to scroll.
  const hasFlexibleFillerColumn = true;
  const tableClassName = cn('table-fixed');
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
      value: unknown
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
              void onRefresh?.();
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
    <TooltipProvider delayDuration={150} skipDelayDuration={0}>
      <div className="overflow-x-auto rounded-lg border bg-card shadow-sm">
        {tableActions.length > 0 && (
          <TableActionsToolbar
            blockId={block.id}
            actions={tableActions}
            selectedRows={selectedRows}
            workflowAction={workflowAction}
          />
        )}
        <Table className={tableClassName || undefined}>
          <colgroup>
            {selectable && (
              <col
                className="report-print-hidden"
                style={{ width: '2.5rem' }}
              />
            )}
            {columns.map((column, idx) => (
              <col key={column.key} style={columnLayouts[idx]?.style} />
            ))}
            {hasFlexibleFillerColumn && <col aria-hidden="true" />}
          </colgroup>
          <TableHeader className="sticky top-0 z-10 bg-card shadow-[inset_0_-1px_0_0_hsl(var(--border))]">
            <TableRow className="group/header border-b-0 bg-muted/30 hover:bg-muted/30">
              {selectable && (
                <TableHead className="report-print-hidden h-9 w-10">
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
              {columns.map((column, idx) => {
                const sortDirection = getColumnSortDirection(column.key, sort);
                const isSortable = !isNonSortableColumn(column);
                const layout = columnLayouts[idx];
                const effectiveAlign = column.align ?? undefined;
                return (
                  <TableHead
                    key={column.key}
                    aria-sort={getAriaSort(sortDirection)}
                    style={layout?.style}
                    className={cn(
                      'h-9 whitespace-nowrap',
                      isActionColumn(column) && 'report-print-hidden',
                      effectiveAlign === 'right' && 'text-right'
                    )}
                  >
                    {isSortable ? (
                      <button
                        type="button"
                        className={cn(
                          'group/h flex w-full items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground transition-colors hover:text-foreground',
                          effectiveAlign === 'right' && 'justify-end'
                        )}
                        onClick={() =>
                          onSortChange(nextSortForColumn(column.key, sort))
                        }
                      >
                        <span className="truncate">
                          {column.label ?? humanizeFieldName(column.key)}
                        </span>
                        <SortIcon direction={sortDirection} />
                      </button>
                    ) : (
                      <span
                        className={cn(
                          'block truncate text-xs font-semibold uppercase tracking-wide text-muted-foreground',
                          effectiveAlign === 'right'
                            ? 'text-right'
                            : 'text-left'
                        )}
                      >
                        {column.label ?? humanizeFieldName(column.key)}
                      </span>
                    )}
                  </TableHead>
                );
              })}
              {hasFlexibleFillerColumn && (
                <TableHead aria-hidden="true" className="h-9 p-0" />
              )}
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={
                    columns.length +
                    (selectable ? 1 : 0) +
                    (hasFlexibleFillerColumn ? 1 : 0)
                  }
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
                    columnLayouts={columnLayouts}
                    hasFlexibleFillerColumn={hasFlexibleFillerColumn}
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
                    onInteractionButtonClick={onInteractionButtonClick}
                    onRunWorkflow={workflowAction.run}
                    workflowActionPhase={workflowAction.phase}
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
                {lastPageOffset !== undefined ? (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page.offset <= 0}
                    onClick={() => onPageChange(0, page.size)}
                  >
                    <ChevronsLeft className="mr-1 h-4 w-4" />
                    First
                  </Button>
                ) : null}
                <Button
                  variant="outline"
                  size="sm"
                  disabled={page.offset <= 0}
                  onClick={() =>
                    onPageChange(
                      Math.max(0, page.offset - page.size),
                      page.size
                    )
                  }
                >
                  <ChevronLeft className="mr-1 h-4 w-4" />
                  Previous
                </Button>
                {currentPage !== undefined && totalPages !== undefined ? (
                  <span className="whitespace-nowrap px-1 text-xs text-muted-foreground">
                    Page {currentPage} of {totalPages}
                  </span>
                ) : null}
                <Button
                  variant="outline"
                  size="sm"
                  disabled={!page.hasNextPage}
                  onClick={() =>
                    onPageChange(page.offset + page.size, page.size)
                  }
                >
                  Next
                  <ChevronRight className="ml-1 h-4 w-4" />
                </Button>
                {lastPageOffset !== undefined ? (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={page.offset >= lastPageOffset}
                    onClick={() => onPageChange(lastPageOffset, page.size)}
                  >
                    Last
                    <ChevronsRight className="ml-1 h-4 w-4" />
                  </Button>
                ) : null}
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
    </TooltipProvider>
  );
}

type TableBodyRowProps = {
  reportId: string;
  blockId: string;
  row: Record<string, unknown> | unknown[];
  rowKey: string;
  rowObject: Record<string, unknown>;
  columns: TableColumn[];
  columnLayouts: ColumnLayout[];
  hasFlexibleFillerColumn: boolean;
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
  onInteractionButtonClick?: (
    actions: ReportInteractionAction[],
    row: Record<string, unknown>
  ) => boolean;
  onRunWorkflow: (args: {
    key: string;
    action: ReportWorkflowActionConfig;
    row?: Record<string, unknown>;
    value?: unknown;
    fallbackField?: string;
    selectedRows?: Record<string, unknown>[];
  }) => void | Promise<void>;
  workflowActionPhase: (key: string) => ReportWorkflowActionPhase | undefined;
};

const MemoizedTableBodyRow = memo(TableBodyRow, areTableBodyRowPropsEqual);

function TableBodyRow({
  reportId,
  blockId,
  row,
  rowKey,
  rowObject,
  columns,
  columnLayouts,
  hasFlexibleFillerColumn,
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
  onInteractionButtonClick,
  onRunWorkflow,
  workflowActionPhase,
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
        <TableCell className="report-print-hidden w-10 py-2 align-middle">
          <Checkbox
            aria-label="Select row"
            checked={selected}
            onClick={(event) => event.stopPropagation()}
            onCheckedChange={(checked) => onToggleSelected(rowKey, checked)}
          />
        </TableCell>
      )}
      {columns.map((column, columnIndex) => {
        const layout = columnLayouts[columnIndex];
        const value = getCellValue(row, column, columnIndex);
        const displayValue = getCellDisplayValue(rowObject, column, value);
        const writebackContext = getWritebackContext(column, rowObject);
        const workflowActionKey = `${blockId}:${rowKey}:${column.key}`;
        const workflowPhase =
          column.workflowAction != null &&
          workflowActionPhase(workflowActionKey);
        const shouldRenderWorkflowAction =
          column.workflowAction != null &&
          isWorkflowActionVisible(column.workflowAction, rowObject);
        const isWorkflowDisabled =
          column.workflowAction != null &&
          isWorkflowActionDisabled(column.workflowAction, rowObject);
        const isEditing = editingField === column.key;
        const effectiveAlign = column.align ?? undefined;

        return (
          <TableCell
            key={column.key}
            style={layout?.style}
            className={cn(
              'group/cell relative py-2 align-middle',
              effectiveAlign === 'right' && 'text-right tabular-nums',
              isActionColumn(column) && 'report-print-hidden',
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
                    phase={workflowPhase || undefined}
                    disabled={isWorkflowDisabled}
                    value={value}
                    row={rowObject}
                    fallbackField={column.key}
                    actionKey={workflowActionKey}
                    onRun={onRunWorkflow}
                  />
                ) : isInteractionButtonsColumn(column) ? (
                  <InteractionButtonsCell
                    buttons={column.interactionButtons ?? []}
                    row={rowObject}
                    onRun={onInteractionButtonClick}
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
                    className="report-print-hidden absolute right-2 top-1/2 -translate-y-1/2 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-muted hover:text-foreground group-hover/cell:opacity-100"
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
      {hasFlexibleFillerColumn && (
        <TableCell aria-hidden="true" className="p-0" />
      )}
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
    previous.columnLayouts === next.columnLayouts &&
    previous.hasFlexibleFillerColumn === next.hasFlexibleFillerColumn &&
    previous.selectable === next.selectable &&
    previous.selected === next.selected &&
    editingStateEqual &&
    Boolean(previous.onRowClick) === Boolean(next.onRowClick) &&
    Boolean(previous.onCellClick) === Boolean(next.onCellClick) &&
    Boolean(previous.onInteractionButtonClick) ===
      Boolean(next.onInteractionButtonClick) &&
    previous.workflowActionPhase === next.workflowActionPhase
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
      displayTemplate: configured?.displayTemplate ?? merged.displayTemplate,
      secondaryField: configured?.secondaryField ?? merged.secondaryField,
      linkField: configured?.linkField ?? merged.linkField,
      tooltipField: configured?.tooltipField ?? merged.tooltipField,
      pillVariants: configured?.pillVariants ?? merged.pillVariants,
      levels: configured?.levels ?? merged.levels,
      align: configured?.align ?? merged.align ?? defaultAlign(merged.format),
      maxChars: configured?.maxChars ?? merged.maxChars,
      editable: configured?.editable ?? merged.editable,
      editor: configured?.editor ?? merged.editor,
      workflowAction: configured?.workflowAction ?? merged.workflowAction,
      interactionButtons:
        configured?.interactionButtons ?? merged.interactionButtons,
    };
  });
}

function isNonSortableColumn(column: TableColumn): boolean {
  return column.type === 'chart' || isActionColumn(column);
}

function isWorkflowButtonColumn(column: TableColumn): boolean {
  return (
    column.type === 'workflow_button' || column.workflowAction !== undefined
  );
}

function isInteractionButtonsColumn(column: TableColumn): boolean {
  return (
    column.type === 'interaction_buttons' ||
    (column.interactionButtons?.length ?? 0) > 0
  );
}

function isActionColumn(column: TableColumn): boolean {
  return isWorkflowButtonColumn(column) || isInteractionButtonsColumn(column);
}

function hasPositiveMaxChars(
  maxChars: number | null | undefined
): maxChars is number {
  return (
    typeof maxChars === 'number' && Number.isFinite(maxChars) && maxChars > 0
  );
}

function getColumnWidthStyle(column: TableColumn): CSSProperties | undefined {
  const configuredMaxChars = column.maxChars;
  if (!hasPositiveMaxChars(configuredMaxChars)) return undefined;

  const label = column.label ?? humanizeFieldName(column.key);
  const maxChars = Math.trunc(configuredMaxChars);
  const contentChars = maxChars + 3;
  const headerChars = Array.from(label).length + 4;
  const widthChars = Math.max(contentChars, headerChars, 6);
  const width = `calc(${widthChars}ch + 1rem)`;
  return { width, maxWidth: width };
}

const SAMPLE_LIMIT = 100;

const FORMAT_WIDTHS: Record<string, number> = {
  date: 116,
  datetime: 184,
  bytes: 110,
  percent: 96,
  currency: 128,
  currency_compact: 110,
  number: 104,
  number_compact: 96,
  decimal: 110,
  bar_indicator: 140,
};

type ColumnLayout = {
  style?: CSSProperties;
};

// Width bounds for inferred text columns, expressed in `ch`. The lower bound
// keeps short columns from collapsing; the upper bound keeps a single long
// column (descriptions, AI rationales) from monopolizing the table — the rest
// of the value is reachable via the hover tooltip on the truncated cell.
const MIN_TEXT_CH = 9;
const MAX_TEXT_CH = 30;

// Every column resolves to a concrete width. Critically, no column is left
// auto/flex: with `table-layout: fixed` + the table primitive's
// `min-w-max`, an auto column with `white-space: nowrap` content expands to
// its full intrinsic width, and several such columns blow the table up to
// thousands of px wide (the "only one column visible, rest scrolled off"
// regression). Bounded widths + the trailing filler col (which has no
// content, so it contributes 0 to max-content) keep the table predictable:
// it fills the container when there's slack and scrolls when there isn't.
function inferColumnLayout(
  column: TableColumn,
  rows: NonNullable<TableData['rows']>,
  columnIndex: number
): ColumnLayout {
  // Explicit author config wins.
  if (hasPositiveMaxChars(column.maxChars)) {
    return { style: getColumnWidthStyle(column) };
  }

  if (isActionColumn(column)) {
    return { style: { width: '160px' } };
  }

  if (column.type === 'chart') {
    return { style: { width: '160px' } };
  }

  const labelLen = Array.from(
    column.label ?? humanizeFieldName(column.key)
  ).length;
  // Uppercase text-xs + tracking-wide glyphs run ~8px, and the header also
  // holds the sort caret and cell padding. Every width below floors on this
  // so a fixed-format column can never truncate its own header
  // ("CREDIT SC…" over a 120px number column).
  const headerPx = Math.ceil(labelLen * 8) + 48;

  const formatName = (column.format ?? '').split(':', 1)[0];

  // Pills and avatar cells render decorated content, so their width comes
  // from the decorated sample (humanized label / derived display name), not
  // from raw value length.
  if (formatName === 'pill') {
    const sample = sampleColumnValues(rows, column, columnIndex, SAMPLE_LIMIT);
    const longest = sample.reduce(
      (acc, value) => Math.max(acc, humanizeFieldName(value).length),
      0
    );
    const pillPx = clampPx(longest * 6.5 + 58, 96, 200);
    return { style: { width: `${Math.max(pillPx, headerPx)}px` } };
  }

  if (formatName === 'avatar_label') {
    const sample = sampleColumnValues(rows, column, columnIndex, SAMPLE_LIMIT);
    const longest = sample.reduce(
      (acc, value) => Math.max(acc, displayNameFromValue(value).length),
      0
    );
    const avatarPx = clampPx(longest * 7 + 64, 140, 240);
    return { style: { width: `${Math.max(avatarPx, headerPx)}px` } };
  }

  const formatWidth = FORMAT_WIDTHS[formatName];
  if (formatWidth) {
    return { style: { width: `${Math.max(formatWidth, headerPx)}px` } };
  }

  const sample = sampleColumnValues(rows, column, columnIndex, SAMPLE_LIMIT);
  const maxLen = sample.reduce((acc, value) => Math.max(acc, value.length), 0);

  // Header glyphs render uppercase + tracking-wide + semibold next to a sort
  // caret, so budget ~1.1× per glyph plus padding/icon allowance — otherwise
  // a header like "DTI" truncates to "D..." in a column sized for its data.
  const headerChars = Math.ceil(labelLen * 1.1) + 8;
  // Data side: longest sampled value plus slack for the ellipsis. An empty
  // column (maxLen 0) sizes purely to its header rather than collapsing.
  const dataChars = maxLen > 0 ? maxLen + 3 : 0;

  const widthChars = Math.min(
    MAX_TEXT_CH,
    Math.max(headerChars, dataChars, MIN_TEXT_CH)
  );
  return { style: { width: `${widthChars}ch` } };
}

function clampPx(value: number, min: number, max: number): number {
  return Math.round(Math.min(Math.max(value, min), max));
}

function sampleColumnValues(
  rows: NonNullable<TableData['rows']>,
  column: TableColumn,
  columnIndex: number,
  limit: number
): string[] {
  const result: string[] = [];
  const count = Math.min(rows.length, limit);
  for (let i = 0; i < count; i += 1) {
    const value = getCellValue(rows[i], column, columnIndex);
    if (value === null || value === undefined) {
      result.push('');
      continue;
    }
    if (typeof value === 'string') {
      result.push(value);
    } else if (typeof value === 'number' || typeof value === 'boolean') {
      result.push(String(value));
    } else {
      try {
        result.push(JSON.stringify(value));
      } catch {
        result.push(String(value));
      }
    }
  }
  return result;
}

function defaultAlign(format?: string | null): TableColumn['align'] {
  if (!format) return undefined;
  const formatName = format.split(':', 1)[0];
  if (
    formatName === 'currency' ||
    formatName === 'currency_compact' ||
    formatName === 'number' ||
    formatName === 'number_compact' ||
    formatName === 'decimal' ||
    formatName === 'percent' ||
    formatName === 'bytes'
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
  if (column.displayTemplate) {
    const displayValue = renderDisplayTemplate(row, column.displayTemplate);
    if (displayValue.trim().length > 0) return displayValue;
  }
  if (column.displayField) {
    const displayValue = getReportRowValue(row, column.displayField);
    if (displayValue === null || displayValue === undefined) return value;
    if (typeof displayValue === 'string' && displayValue.trim().length === 0) {
      return value;
    }
    return displayValue;
  }
  return value;
}

function getWritebackContext(
  column: TableColumn,
  rowObject: Record<string, unknown>
): WritebackContext | null {
  if (!column.editable) return null;
  if (column.type === 'chart' || isActionColumn(column)) return null;
  const id = rowObject.id;
  const schemaId = rowObject.schemaId;
  if (typeof id !== 'string' || typeof schemaId !== 'string') return null;
  return { schemaId, instanceId: id };
}

function shouldRefreshAfterWriteback(column: TableColumn): boolean {
  return Boolean(
    column.displayField ||
      column.displayTemplate ||
      column.editor?.kind === 'lookup'
  );
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
          const phase = workflowAction.phase(actionKey);
          return (
            <WorkflowActionButton
              key={action.id}
              action={action.workflowAction}
              labelFallback={action.label ?? 'Run'}
              phase={phase}
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
  phase,
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
  phase?: ReportWorkflowActionPhase;
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
      className="report-print-hidden h-8 max-w-full gap-1.5"
      disabled={Boolean(phase) || disabled}
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
      {phase ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      ) : (
        <Play className="h-3.5 w-3.5" />
      )}
      <span className="truncate">
        {phase
          ? workflowActionPhaseLabel(action, phase)
          : (action.label ?? labelFallback)}
      </span>
    </Button>
  );
}

function workflowActionPhaseLabel(
  action: ReportWorkflowActionConfig,
  phase: ReportWorkflowActionPhase
): string {
  if (phase === 'submitting') return 'Starting...';
  if (phase === 'refreshing') return 'Updating report...';
  return action.runningLabel ?? 'Running...';
}

function InteractionButtonsCell({
  buttons,
  row,
  onRun,
}: {
  buttons: ReportTableInteractionButtonConfig[];
  row: Record<string, unknown>;
  onRun?: (
    actions: ReportInteractionAction[],
    row: Record<string, unknown>
  ) => boolean;
}) {
  const visibleButtons = buttons.filter((button) =>
    isInteractionButtonVisible(button, row)
  );

  if (visibleButtons.length === 0) {
    return <EmptyCellPlaceholder />;
  }

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {visibleButtons.map((button) => {
        const disabled = isInteractionButtonDisabled(button, row);
        const Icon = iconForInteractionButton(button.icon);
        const label = button.label ?? humanizeFieldName(button.id);

        return (
          <Button
            key={button.id}
            type="button"
            variant="outline"
            size="sm"
            className="h-8 max-w-full gap-1.5 px-2.5"
            disabled={disabled}
            onClick={(event) => {
              event.stopPropagation();
              if (disabled) return;
              onRun?.(button.actions ?? [], row);
            }}
          >
            <Icon className="h-3.5 w-3.5" />
            <span className="truncate">{label}</span>
          </Button>
        );
      })}
    </div>
  );
}

function isInteractionButtonVisible(
  button: ReportTableInteractionButtonConfig,
  row: Record<string, unknown>
): boolean {
  if (
    button.visibleWhen &&
    !matchesReportRowCondition(button.visibleWhen, row)
  ) {
    return false;
  }
  if (button.hiddenWhen && matchesReportRowCondition(button.hiddenWhen, row)) {
    return false;
  }
  return true;
}

function isInteractionButtonDisabled(
  button: ReportTableInteractionButtonConfig,
  row: Record<string, unknown>
): boolean {
  return button.disabledWhen
    ? matchesReportRowCondition(button.disabledWhen, row)
    : false;
}

function iconForInteractionButton(
  icon: ReportTableInteractionButtonConfig['icon']
) {
  switch (icon) {
    case 'file_text':
      return FileText;
    case 'activity':
      return Activity;
    case 'arrow_right':
      return ArrowRight;
    case 'eye':
    default:
      return Eye;
  }
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
    <TruncatedCellText
      text={formatCellValue(displayValue ?? value, column.format ?? undefined)}
      maxChars={column.maxChars}
    />
  );
}

function TruncatedCellText({
  text,
  maxChars,
  className,
}: {
  text: string;
  maxChars?: number | null;
  className?: string;
}) {
  const display = truncateCellText(text, maxChars);
  return (
    <OverflowText
      text={display.text}
      fullText={display.title ?? display.text}
      className={className}
    />
  );
}

function OverflowText({
  text,
  fullText,
  className,
}: {
  text: string;
  fullText: string;
  className?: string;
}) {
  const spanRef = useRef<HTMLSpanElement>(null);
  const [overflowing, setOverflowing] = useState(false);

  const measure = useCallback(() => {
    const el = spanRef.current;
    if (!el) return;
    setOverflowing(el.scrollWidth - el.clientWidth > 1);
  }, []);

  if (!text || text.length === 0) {
    return <EmptyCellPlaceholder className={className} />;
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          ref={spanRef}
          className={cn('block min-w-0 truncate', className)}
          aria-label={overflowing ? fullText : undefined}
          onPointerEnter={measure}
          onFocus={measure}
        >
          {text}
        </span>
      </TooltipTrigger>
      {overflowing && (
        <TooltipContent
          side="top"
          align="start"
          className="max-w-md whitespace-pre-line break-words text-xs"
        >
          {fullText}
        </TooltipContent>
      )}
    </Tooltip>
  );
}

function EmptyCellPlaceholder({ className }: { className?: string }) {
  return (
    <span
      aria-label="No value"
      className={cn('text-muted-foreground/60', className)}
    >
      —
    </span>
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
    ? getReportRowValue(row, column.tooltipField)
    : undefined;
  const display = displayNameFromValue(raw);
  const displayText = truncateCellText(display, column.maxChars);
  const initials = initialsFromValue(display);
  const colorClass = colorClassForKey(raw);
  const fullText =
    typeof tooltipValue === 'string'
      ? tooltipValue
      : (displayText.title ?? raw);
  return (
    <div className="flex min-w-0 items-center gap-2">
      <span
        className={cn(
          'flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold uppercase tracking-wide',
          colorClass
        )}
        aria-hidden
      >
        {initials}
      </span>
      <OverflowText text={displayText.text} fullText={fullText} />
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

// Soft tint + dark ink in both themes: initials stay readable where a white
// glyph on a mid-500 fill (amber, cyan) dropped below comfortable contrast.
const AVATAR_COLORS = [
  'bg-violet-100 text-violet-700 dark:bg-violet-500/25 dark:text-violet-300',
  'bg-emerald-100 text-emerald-700 dark:bg-emerald-500/25 dark:text-emerald-300',
  'bg-amber-100 text-amber-700 dark:bg-amber-500/25 dark:text-amber-300',
  'bg-sky-100 text-sky-700 dark:bg-sky-500/25 dark:text-sky-300',
  'bg-rose-100 text-rose-700 dark:bg-rose-500/25 dark:text-rose-300',
  'bg-cyan-100 text-cyan-700 dark:bg-cyan-500/25 dark:text-cyan-300',
  'bg-indigo-100 text-indigo-700 dark:bg-indigo-500/25 dark:text-indigo-300',
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
    <div className="flex min-w-0 items-center gap-2">
      <div className="flex shrink-0 items-end gap-0.5" aria-hidden>
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
      <TruncatedCellText
        text={humanizePillLabel(key)}
        maxChars={column.maxChars}
        className="text-sm"
      />
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
    ? formatCellValue(getReportRowValue(row, column.secondaryField))
    : undefined;
  const linkRaw = column.linkField
    ? getReportRowValue(row, column.linkField)
    : undefined;
  const link = typeof linkRaw === 'string' && linkRaw ? linkRaw : undefined;
  const primary = formatCellValue(value, column.format ?? undefined);
  return (
    <div className="flex min-w-0 flex-col">
      <div className="flex min-w-0 items-center gap-1.5">
        <TruncatedCellText
          text={primary}
          maxChars={column.maxChars}
          className="font-medium text-foreground"
        />
        {link && (
          <a
            href={link}
            target="_blank"
            rel="noreferrer noopener"
            onClick={(event) => event.stopPropagation()}
            className="shrink-0 text-muted-foreground hover:text-foreground"
            aria-label="Open link"
          >
            <ExternalLink className="h-3.5 w-3.5" />
          </a>
        )}
      </div>
      {secondary && (
        <OverflowText
          text={secondary}
          fullText={secondary}
          className="text-xs text-muted-foreground"
        />
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

  // Scatter is a full-block visualization; a bubble cloud in a ~44px sparkline
  // cell is unreadable, so render nothing rather than silently falling through
  // to the line-chart default branch below.
  if (
    !chart ||
    rows.length === 0 ||
    series.length === 0 ||
    chart.kind === 'scatter'
  ) {
    return <EmptyCellPlaceholder />;
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
            <RechartsTooltip
              cursor={false}
              contentStyle={{ fontSize: 12 }}
              labelStyle={{ fontSize: 12 }}
            />
            <Bar
              dataKey={seriesField}
              fill={INLINE_CHART_COLOR}
              radius={[3, 3, 0, 0]}
              isAnimationActive={false}
            />
          </BarChart>
        ) : chart.kind === 'area' ? (
          <AreaChart data={chartRows}>
            <RechartsTooltip
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
              isAnimationActive={false}
            />
          </AreaChart>
        ) : (
          <LineChart data={chartRows}>
            <RechartsTooltip
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
              isAnimationActive={false}
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

function getTotalPages(page: NonNullable<TableData['page']>) {
  if (typeof page.totalCount !== 'number') return undefined;
  if (page.totalCount <= 0) return 0;
  return Math.ceil(page.totalCount / Math.max(page.size, 1));
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
