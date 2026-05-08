import { useCallback, useMemo, useState, memo, useRef, useEffect } from 'react';
import { RowSelectionState, SortingState } from '@tanstack/react-table';
import { Button } from '@/shared/components/ui/button';
import {
  Download,
  Filter,
  Loader2,
  Pencil,
  Plus,
  Trash2,
  Upload,
} from 'lucide-react';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/shared/components/ui/collapsible';
import { DataTable } from '@/shared/components/table';
import { Instance, Schema, Condition } from '@/generated/RuntaraRuntimeApi';
import {
  useBulkCreateObjectInstances,
  useBulkDeleteObjectInstances,
  useBulkUpdateObjectInstances,
  useObjectInstanceDtos,
  useUpdateObjectInstanceDto,
  useCreateObjectInstanceDto,
  useExportCsv,
} from '@/features/objects/hooks/useObjectRecords.ts';
import type {
  BulkConflictMode,
  BulkCreateResult,
  BulkValidationMode,
} from '../../queries';
import { objectInstancesColumns } from './ObjectInstancesColumns';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/shared/components/ui/alert-dialog';
import { FilterComposer } from '../FilterComposer';
import { ImportCsvDialog } from '../ImportCsvDialog';
import { BulkEditDialog } from './BulkEditDialog';
import { BulkInsertDialog } from './BulkInsertDialog';
import { toast } from 'sonner';
import { Alert, AlertDescription } from '@/shared/components/ui/alert';

const AddRowButton = memo(({ onClick }: { onClick: () => void }) => {
  const [isHovering, setIsHovering] = useState(false);

  return (
    <button
      type="button"
      className={`w-full text-center py-3 border-t border-slate-200 transition-colors cursor-pointer dark:border-slate-700 ${
        isHovering
          ? 'bg-slate-100/80 dark:bg-slate-800/50'
          : 'bg-slate-50/50 dark:bg-slate-800/20'
      }`}
      onMouseEnter={() => setIsHovering(true)}
      onMouseLeave={() => setIsHovering(false)}
      onClick={onClick}
    >
      <span
        className={`text-xs font-medium ${
          isHovering
            ? 'text-slate-700 dark:text-slate-200'
            : 'text-slate-400 dark:text-slate-500'
        }`}
      >
        {isHovering ? '+ Click to add a new row' : '+ Add row'}
      </span>
    </button>
  );
});

AddRowButton.displayName = 'AddRowButton';

interface ObjectInstanceDtosTableProps {
  objectSchemaDto: Schema;
}

export function ObjectInstanceDtosTable({
  objectSchemaDto,
}: ObjectInstanceDtosTableProps) {
  const [isFilterOpen, setIsFilterOpen] = useState(false);
  const [filterCondition, setFilterCondition] = useState<Condition | null>(
    null
  );
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState(20);
  const [sorting, setSorting] = useState<SortingState>([]);
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
  const [showBulkDeleteDialog, setShowBulkDeleteDialog] = useState(false);
  const [showBulkEditDialog, setShowBulkEditDialog] = useState(false);
  const [showBulkInsertDialog, setShowBulkInsertDialog] = useState(false);
  const [pendingRecords, setPendingRecords] = useState<Instance[]>([]);
  const [animatingRowId, setAnimatingRowId] = useState<string | null>(null);

  // Track which row has unsaved changes
  const [dirtyRows, setDirtyRows] = useState<Map<string, any>>(new Map());
  const lastFocusedRowRef = useRef<string | null>(null);

  // Track which cell is currently being edited (rowId-columnId)
  const [editingCellId, setEditingCellId] = useState<string | null>(null);
  const editingCellIdRef = useRef<string | null>(null);

  useEffect(() => {
    editingCellIdRef.current = editingCellId;
  }, [editingCellId]);

  // Convert TanStack SortingState to API format
  // Strip leading underscore from system column IDs (_createdAt -> createdAt)
  const sortBy =
    sorting.length > 0
      ? sorting.map((s) => (s.id.startsWith('_') ? s.id.slice(1) : s.id))
      : undefined;
  const sortOrder =
    sorting.length > 0
      ? sorting.map((s) => (s.desc ? 'desc' : 'asc'))
      : undefined;

  // Check if schema has a name for filtering/sorting
  const hasFilteringOrSorting = filterCondition || sortBy || sortOrder;
  const isMissingSchemaName = !objectSchemaDto.name && hasFilteringOrSorting;

  const { data, isLoading, isError, error } = useObjectInstanceDtos(
    objectSchemaDto.id,
    objectSchemaDto.name || undefined,
    filterCondition,
    page,
    pageSize,
    sortBy,
    sortOrder
  );

  // Display error toast when query fails
  useEffect(() => {
    if (isError && error) {
      const errorMessage =
        (error as any)?.message || 'Failed to fetch records. Please try again.';
      toast.error(errorMessage);
    }
  }, [isError, error]);

  const records = useMemo(() => {
    const content = data?.content || [];

    // Apply dirty changes to display for existing records
    const contentWithDirtyData = content.map((record) => {
      const dirtyData = dirtyRows.get(record.id || '');
      if (dirtyData) {
        return {
          ...record,
          properties: {
            ...record.properties,
            ...dirtyData,
          },
        };
      }
      return record;
    });

    // Apply dirty changes to pending records as well
    const pendingWithDirtyData = pendingRecords.map((record) => {
      const dirtyData = dirtyRows.get(record.id || '');
      if (dirtyData) {
        return {
          ...record,
          properties: {
            ...record.properties,
            ...dirtyData,
          },
        };
      }
      return record;
    });

    return [...contentWithDirtyData, ...pendingWithDirtyData];
  }, [data?.content, pendingRecords, dirtyRows]);

  const totalPages =
    data?.totalPages && data.totalPages > 0
      ? data.totalPages
      : data?.totalElements && pageSize > 0
        ? Math.ceil(data.totalElements / pageSize)
        : records.length > 0
          ? Math.ceil(records.length / pageSize)
          : 1;
  const totalElements = data?.totalElements || records.length || 0;
  const currentPage = page;

  const [showImportDialog, setShowImportDialog] = useState(false);

  const bulkDeleteMutation = useBulkDeleteObjectInstances();
  const bulkUpdateMutation = useBulkUpdateObjectInstances();
  const bulkCreateMutation = useBulkCreateObjectInstances();

  const handleBulkInsert = useCallback(
    async (
      instances: unknown[],
      onConflict: BulkConflictMode,
      onError: BulkValidationMode,
      conflictColumns: string[]
    ): Promise<BulkCreateResult | undefined> => {
      try {
        const result = await bulkCreateMutation.mutateAsync({
          schemaId: objectSchemaDto.id || '',
          instances,
          options: { onConflict, onError, conflictColumns },
        });
        if (result.errors.length === 0) {
          toast.success(
            `Inserted ${result.createdCount}, skipped ${result.skippedCount}`
          );
        } else {
          toast.warning(
            `Inserted ${result.createdCount}, skipped ${result.skippedCount} (${result.errors.length} error${result.errors.length === 1 ? '' : 's'})`
          );
        }
        return result;
      } catch (error) {
        const message =
          (error as any)?.response?.data?.error ||
          (error as any)?.response?.data?.message ||
          (error as Error)?.message ||
          'Failed to insert records';
        toast.error('Unable to insert records', { description: message });
        return undefined;
      }
    },
    [bulkCreateMutation, objectSchemaDto.id]
  );
  const updateRecord = useUpdateObjectInstanceDto();
  const createRecord = useCreateObjectInstanceDto();
  const exportCsvMutation = useExportCsv();

  const handleExportCsv = useCallback(async () => {
    if (!objectSchemaDto.name) {
      toast.error('Schema name is required for export');
      return;
    }

    try {
      const blob = await exportCsvMutation.mutateAsync({
        schemaName: objectSchemaDto.name,
        data: {
          condition: filterCondition || undefined,
          sortBy,
          sortOrder,
          includeSystemColumns: true,
        },
      });

      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `${objectSchemaDto.name}.csv`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      toast.success('CSV exported successfully');
    } catch {
      // Error toast handled by useCustomMutation
    }
  }, [
    objectSchemaDto.name,
    filterCondition,
    sortBy,
    sortOrder,
    exportCsvMutation,
  ]);

  const handleRowAnimationTrigger = useCallback((rowId: string) => {
    setAnimatingRowId(rowId);
    setTimeout(() => {
      setAnimatingRowId(null);
    }, 1000);
  }, []);

  const dirtyRowsRef = useRef(dirtyRows);

  useEffect(() => {
    dirtyRowsRef.current = dirtyRows;
  }, [dirtyRows]);

  const saveDirtyRow = useCallback(
    async (rowId: string) => {
      const dirtyData = dirtyRowsRef.current.get(rowId);
      if (!dirtyData) {
        return;
      }

      if (rowId.startsWith('PENDING_')) {
        // Save pending record
        try {
          const newInstance = await createRecord.mutateAsync({
            schemaId: objectSchemaDto.id || '',
            data: {
              properties: dirtyData,
              schemaId: objectSchemaDto.id || '',
            },
          });
          setPendingRecords((prev) => prev.filter((r) => r.id !== rowId));
          setDirtyRows((prev) => {
            const updated = new Map(prev);
            updated.delete(rowId);
            return updated;
          });
          // Trigger animation with the server-returned ID
          if (newInstance.id) {
            handleRowAnimationTrigger(newInstance.id);
          }
        } catch {
          toast.error('Failed to create record');
        }
      } else {
        // Save existing record
        try {
          await updateRecord.mutateAsync({
            schemaId: objectSchemaDto.id || '',
            instanceId: rowId,
            data: {
              properties: dirtyData,
            },
          });
          setDirtyRows((prev) => {
            const updated = new Map(prev);
            updated.delete(rowId);
            return updated;
          });
          handleRowAnimationTrigger(rowId);
        } catch {
          toast.error('Failed to update record');
        }
      }
    },
    [createRecord, updateRecord, objectSchemaDto.id, handleRowAnimationTrigger]
  );

  const handleUpdate = useCallback((instanceId: string, data: any) => {
    const propertyEntries = Object.entries(data.properties || {});
    const hasChanges = propertyEntries.length > 0;

    if (instanceId.startsWith('PENDING_')) {
      // Update pending record immediately in state
      setPendingRecords((prev) => {
        let didUpdate = false;
        const next = prev.map((rec) => {
          if (rec.id !== instanceId) return rec;
          const nextProps = { ...rec.properties };
          propertyEntries.forEach(([key, value]) => {
            if (nextProps[key] !== value) {
              didUpdate = true;
              nextProps[key] = value;
            }
          });
          return didUpdate
            ? {
                ...rec,
                properties: nextProps,
              }
            : rec;
        });
        return didUpdate ? next : prev;
      });
    }

    if (hasChanges) {
      // Track as dirty for later save
      setDirtyRows((prev) => {
        const updated = new Map(prev);
        const existing = updated.get(instanceId) || {};
        const nextRow = { ...existing };
        let shouldUpdate = false;

        propertyEntries.forEach(([key, value]) => {
          if (nextRow[key] !== value) {
            shouldUpdate = true;
            nextRow[key] = value;
          }
        });

        if (!shouldUpdate) {
          return prev;
        }

        updated.set(instanceId, nextRow);
        // Update ref immediately so saveDirtyRow can see it
        dirtyRowsRef.current = updated;
        return updated;
      });
    }
  }, []);

  // Save when clicking outside the table
  const tableRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (!lastFocusedRowRef.current) return;

      const target = e.target as HTMLElement;
      const clickedInTable = target.closest('[data-table-container]');

      if (!clickedInTable) {
        // Capture ref value before the async delay
        const rowId = lastFocusedRowRef.current;
        // Small delay to let any pending handleSave complete
        setTimeout(() => {
          if (rowId) saveDirtyRow(rowId);
        }, 100);
      }
    };

    document.addEventListener('mousedown', handleClickOutside);

    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
    };
  }, [saveDirtyRow]);

  const handleAddRow = useCallback(() => {
    // Save current row before adding new one
    if (lastFocusedRowRef.current) {
      saveDirtyRow(lastFocusedRowRef.current);
    }

    const newId = `PENDING_${Date.now()}`;
    setPendingRecords((prev) => [
      ...prev,
      {
        id: newId,
        properties: {},
        tenantId: '',
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
      } as Instance,
    ]);
  }, [saveDirtyRow]);

  const handleFilterChange = (condition: Condition | null) => {
    setFilterCondition(condition);
    setPage(0);
  };

  const handlePageChange = (newPage: number) => {
    setPage(newPage);
  };

  const handlePageSizeChange = (newPageSize: number) => {
    setPageSize(newPageSize);
    setPage(0);
  };

  const handleSortingChange = useCallback(
    (updater: SortingState | ((old: SortingState) => SortingState)) => {
      const newSorting =
        typeof updater === 'function' ? updater(sorting) : updater;
      setSorting(newSorting);
      setPage(0); // Reset to first page when sorting changes
    },
    [sorting]
  );

  const handleBulkDelete = useCallback(async () => {
    const selectedIds = Object.keys(rowSelection).filter(
      (key) => rowSelection[key]
    );
    if (selectedIds.length === 0) return;

    try {
      await bulkDeleteMutation.mutateAsync({
        schemaId: objectSchemaDto.id || '',
        instanceIds: selectedIds,
      });
      setRowSelection({});
      setShowBulkDeleteDialog(false);
    } catch (error) {
      const message =
        (error as any)?.response?.data?.message ||
        (error as Error)?.message ||
        'Failed to delete records';
      toast.error('Unable to delete selected records', {
        description: message,
      });
    }
  }, [bulkDeleteMutation, objectSchemaDto.id, rowSelection]);

  const handleBulkEdit = useCallback(
    async (properties: Record<string, unknown>) => {
      const selectedIds = Object.keys(rowSelection).filter(
        (key) => rowSelection[key]
      );
      if (selectedIds.length === 0 || Object.keys(properties).length === 0) {
        return;
      }
      try {
        const updated = await bulkUpdateMutation.mutateAsync({
          schemaId: objectSchemaDto.id || '',
          instanceIds: selectedIds,
          properties,
        });
        toast.success(`Updated ${updated} record${updated === 1 ? '' : 's'}`);
        setRowSelection({});
        setShowBulkEditDialog(false);
      } catch (error) {
        const message =
          (error as any)?.response?.data?.error ||
          (error as any)?.response?.data?.message ||
          (error as Error)?.message ||
          'Failed to update records';
        toast.error('Unable to update selected records', {
          description: message,
        });
      }
    },
    [bulkUpdateMutation, objectSchemaDto.id, rowSelection]
  );

  const selectedCount = Object.keys(rowSelection).filter(
    (key) => rowSelection[key]
  ).length;

  const handleCellFocus = useCallback(
    (rowId: string) => {
      // If switching to a different row, save the previous row first
      if (lastFocusedRowRef.current && lastFocusedRowRef.current !== rowId) {
        saveDirtyRow(lastFocusedRowRef.current);
      }

      lastFocusedRowRef.current = rowId;
    },
    [saveDirtyRow]
  );

  const columns = useMemo(
    () =>
      objectInstancesColumns({
        objectSchemaDto,
        onUpdate: handleUpdate,
        enableSelection: true,
        onCellFocus: handleCellFocus,
        editingCellId,
        setEditingCellId,
      }),
    [objectSchemaDto, handleUpdate, handleCellFocus, editingCellId]
  );

  // Convert schema columns to filter schema definition format
  const filterSchemaDefinition = useMemo(() => {
    const schemaColumns = objectSchemaDto.columns || [];
    const definition: Record<string, { name?: string; dataType?: string }> = {};

    // Add system fields
    definition['id'] = { name: 'ID', dataType: 'STRING' };
    definition['createdAt'] = { name: 'Created At', dataType: 'DATE' };
    definition['updatedAt'] = { name: 'Updated At', dataType: 'DATE' };

    // Add schema columns
    schemaColumns.forEach((col) => {
      definition[col.name] = {
        name: col.name,
        dataType: col.type?.toUpperCase() || 'STRING',
      };
    });

    return definition;
  }, [objectSchemaDto.columns]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (
        (e.key === 'Delete' || e.key === 'Backspace') &&
        Object.keys(rowSelection).length > 0
      ) {
        if (
          document.activeElement?.tagName === 'INPUT' ||
          document.activeElement?.tagName === 'TEXTAREA'
        ) {
          return;
        }
        setShowBulkDeleteDialog(true);
      }
    },
    [rowSelection]
  );

  return (
    <div
      ref={tableRef}
      data-table-container
      onKeyDown={handleKeyDown}
      tabIndex={0}
      className="outline-none"
    >
      <style>{`
        @keyframes row-flash-success {
          0% { background-color: rgba(34, 197, 94, 0); }
          50% { background-color: rgba(34, 197, 94, 0.2); }
          100% { background-color: rgba(34, 197, 94, 0); }
        }
        .row-animating {
          animation: row-flash-success 1s ease-in-out;
        }
      `}</style>
      <Collapsible
        open={isFilterOpen}
        onOpenChange={setIsFilterOpen}
        className="mb-6"
      >
        <div className="flex justify-between items-center ml-3">
          <div className="flex gap-2">
            <CollapsibleTrigger asChild>
              <Button variant="outline" size="sm">
                <Filter className="h-4 w-4 mr-2" />
                {isFilterOpen ? 'Hide Filters' : 'Show Filters'}
              </Button>
            </CollapsibleTrigger>
            {objectSchemaDto.name && (
              <>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handleExportCsv}
                  disabled={exportCsvMutation.isPending}
                >
                  {exportCsvMutation.isPending ? (
                    <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  ) : (
                    <Download className="h-4 w-4 mr-2" />
                  )}
                  Export CSV
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setShowImportDialog(true)}
                >
                  <Upload className="h-4 w-4 mr-2" />
                  Import CSV
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setShowBulkInsertDialog(true)}
                >
                  <Plus className="h-4 w-4 mr-2" />
                  Bulk Insert
                </Button>
              </>
            )}
          </div>
          {selectedCount > 0 && (
            <div className="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => setShowBulkEditDialog(true)}
              >
                <Pencil className="h-4 w-4 mr-2" />
                Edit {selectedCount} selected
              </Button>
              <Button
                variant="destructive"
                size="sm"
                onClick={() => setShowBulkDeleteDialog(true)}
              >
                <Trash2 className="h-4 w-4 mr-2" />
                Delete {selectedCount} selected
              </Button>
            </div>
          )}
        </div>

        <CollapsibleContent>
          <div className="m-3 p-4 border rounded-lg bg-muted/30">
            <FilterComposer
              value={filterCondition}
              onChange={handleFilterChange}
              schemaDefinition={filterSchemaDefinition}
            />
          </div>
        </CollapsibleContent>
      </Collapsible>

      {isMissingSchemaName && (
        <Alert variant="destructive" className="mb-4 mx-3">
          <AlertDescription>
            This schema does not have a name defined. Filtering and sorting are
            not available. Please contact your administrator to add a name to
            this schema.
          </AlertDescription>
        </Alert>
      )}

      <div className="bg-white rounded-xl border border-slate-200/80 shadow-sm overflow-hidden dark:bg-card dark:border-slate-700/50">
        <DataTable
          columns={columns}
          data={records}
          isFetching={isLoading}
          shouldRenderPagination={true}
          pagination={{
            pageIndex: currentPage,
            pageSize: pageSize,
            pageCount: totalPages,
            totalCount: totalElements,
            onPageChange: handlePageChange,
            onPageSizeChange: handlePageSizeChange,
          }}
          sorting={sorting}
          onSortingChange={handleSortingChange}
          manualSorting={true}
          enableRowSelection={true}
          rowSelection={rowSelection}
          onRowSelectionChange={setRowSelection}
          getRowId={(row) => row.id || ''}
          getRowClassName={(row) =>
            row.original.id === animatingRowId ? 'row-animating' : ''
          }
          beforePaginationSlot={<AddRowButton onClick={handleAddRow} />}
        />
      </div>

      <BulkEditDialog
        open={showBulkEditDialog}
        onOpenChange={setShowBulkEditDialog}
        selectedCount={selectedCount}
        schema={objectSchemaDto}
        onSubmit={handleBulkEdit}
        isSubmitting={bulkUpdateMutation.isPending}
      />

      <BulkInsertDialog
        open={showBulkInsertDialog}
        onOpenChange={setShowBulkInsertDialog}
        schema={objectSchemaDto}
        onSubmit={handleBulkInsert}
        isSubmitting={bulkCreateMutation.isPending}
      />

      <AlertDialog
        open={showBulkDeleteDialog}
        onOpenChange={setShowBulkDeleteDialog}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Confirm Bulk Delete</AlertDialogTitle>
            <AlertDialogDescription>
              Are you sure you want to delete {selectedCount} selected record
              {selectedCount !== 1 ? 's' : ''}? This action cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={handleBulkDelete}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <ImportCsvDialog
        open={showImportDialog}
        onOpenChange={setShowImportDialog}
        objectSchemaDto={objectSchemaDto}
      />
    </div>
  );
}
