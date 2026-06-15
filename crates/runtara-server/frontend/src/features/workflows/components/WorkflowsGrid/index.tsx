import { ReactNode, useCallback, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { toast } from 'sonner';
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  Folder,
  Pencil,
  Trash2,
} from 'lucide-react';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import {
  useCustomMutation,
  useCustomQuery,
  isEntitlementDenial,
} from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys.ts';
import { queryClient } from '@/main.tsx';
import {
  cloneWorkflow,
  getWorkflowsInFolder,
  moveWorkflowToFolder,
  removeWorkflow,
  scheduleWorkflow,
} from '@/features/workflows/queries';
import { WorkflowCard } from '../WorkflowCard';
import { Icons } from '@/shared/components/icons.tsx';
import { Button } from '@/shared/components/ui/button.tsx';
import { Can } from '@/shared/components/Can';
import { cn } from '@/lib/utils.ts';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  ConsoleTableShell,
  TablePagination,
  TableStatusFooter,
} from '@/shared/components/console';
import { WorkflowExecuteDialog } from '@/features/workflows/components/WorkflowExecuteDialog';
import { MoveToFolderDialog } from '../FolderDialogs';
import { ConfirmationDialog } from '@/shared/components/confirmation-dialog';
import { parseSchema } from '@/features/workflows/utils/schema';
import { useFolders } from '../../hooks/useFolders';

const DEFAULT_PAGE_SIZE = 10;

interface WorkflowFolderItem {
  name: string;
  path: string;
}

type SortColumn = 'name' | 'updated';
type SortDir = 'asc' | 'desc';

interface WorkflowsGridProps {
  searchTerm: string;
  /** Pinned console toolbar (breadcrumb + search + actions) from the page. */
  toolbar?: ReactNode;
  /** Current folder path filter (undefined = show all, "/" = root only) */
  folderPath?: string;
  /** Whether to show the move to folder action */
  showMoveAction?: boolean;
  /** Child folders to render as the first rows of the table */
  folders?: WorkflowFolderItem[];
  folderWorkflowCounts?: Record<string, number>;
  onFolderNavigate?: (path: string) => void;
  onFolderRename?: (path: string) => void;
  onFolderDelete?: (path: string) => void;
}

/** Clickable, sortable console table header cell. */
function SortableHead({
  label,
  active,
  dir,
  onClick,
  className,
}: {
  label: string;
  active: boolean;
  dir: SortDir;
  onClick: () => void;
  className?: string;
}) {
  return (
    <TableHead
      className={cn('cursor-pointer select-none', className)}
      onClick={onClick}
      aria-sort={active ? (dir === 'asc' ? 'ascending' : 'descending') : 'none'}
    >
      <span className="inline-flex items-center gap-1">
        {label}
        {active ? (
          dir === 'asc' ? (
            <ArrowUp className="h-3.5 w-3.5 text-primary" />
          ) : (
            <ArrowDown className="h-3.5 w-3.5 text-primary" />
          )
        ) : (
          <ArrowUpDown className="h-3.5 w-3.5 text-muted-foreground/40" />
        )}
      </span>
    </TableHead>
  );
}

export function WorkflowsGrid({
  searchTerm,
  toolbar,
  folderPath,
  showMoveAction = false,
  folders = [],
  folderWorkflowCounts = {},
  onFolderNavigate,
  onFolderRename,
  onFolderDelete,
}: WorkflowsGridProps) {
  const navigate = useNavigate();
  const [pendingAction, setPendingAction] = useState<{
    id: string;
    type: 'schedule' | 'clone' | 'delete' | 'move';
  } | null>(null);
  const [executeTarget, setExecuteTarget] = useState<WorkflowDto | null>(null);
  const [executeError, setExecuteError] = useState<string | null>(null);
  const [moveTarget, setMoveTarget] = useState<WorkflowDto | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<WorkflowDto | null>(null);

  // Client-side sort over the current page, toggled from the column headers.
  const [sort, setSort] = useState<{ column: SortColumn; dir: SortDir }>({
    column: 'updated',
    dir: 'desc',
  });
  const toggleSort = useCallback((column: SortColumn) => {
    setSort((prev) =>
      prev.column === column
        ? { column, dir: prev.dir === 'asc' ? 'desc' : 'asc' }
        : { column, dir: column === 'name' ? 'asc' : 'desc' }
    );
  }, []);

  // Pagination state (API uses 1-based pages)
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);

  // Reset to first page when folder or search changes
  useEffect(() => {
    setPage(1);
  }, [folderPath, searchTerm]);

  // Fetch folders for move dialog
  const { data: foldersData } = useFolders();

  const {
    data: response,
    isFetching,
    isError,
    error,
  } = useCustomQuery({
    queryKey: [
      ...queryKeys.workflows.inFolder(folderPath ?? '/', false),
      page,
      pageSize,
      searchTerm,
    ],
    queryFn: (token: string) =>
      getWorkflowsInFolder(token, {
        path: folderPath,
        recursive: false,
        page,
        pageSize,
        search: searchTerm?.trim() || undefined,
      }),
  });

  // Extract workflows array and pagination info from paginated response
  const workflows = useMemo(
    () => (response?.data?.content || []) as WorkflowDto[],
    [response?.data?.content]
  );
  const totalElements = response?.data?.totalElements ?? 0;
  const totalPages = response?.data?.totalPages ?? 1;
  // Server handles both folder and search filtering via query parameters
  // Client-side: sort only
  const filteredWorkflows = useMemo(() => {
    return [...workflows].sort((a, b) => {
      let cmp: number;
      if (sort.column === 'name') {
        cmp = (a.name || '').localeCompare(b.name || '');
      } else {
        const timeA = a.updated ? new Date(a.updated).getTime() : 0;
        const timeB = b.updated ? new Date(b.updated).getTime() : 0;
        cmp = timeA - timeB;
      }
      return sort.dir === 'asc' ? cmp : -cmp;
    });
  }, [workflows, sort]);

  const removeMutation = useCustomMutation({
    mutationFn: removeWorkflow,
    onSuccess: () => {
      toast.info('Workflow has been removed');
      queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all });
    },
  });

  const scheduleMutation = useCustomMutation({
    mutationFn: (
      token: string,
      params: { workflowId: string; inputs?: Record<string, any> }
    ) => scheduleWorkflow(token, params.workflowId, params.inputs),
    onSuccess: () => {
      toast.info('Workflow has been scheduled');
      queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.allInstances(),
      });
    },
  });

  const cloneMutation = useCustomMutation({
    mutationFn: cloneWorkflow,
    onSuccess: () => {
      toast.success('Workflow has been cloned successfully');
      queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all });
    },
  });

  const moveMutation = useCustomMutation({
    mutationFn: (token: string, params: { workflowId: string; path: string }) =>
      moveWorkflowToFolder(token, params),
    onSuccess: () => {
      toast.success('Workflow has been moved');
      queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all });
      queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.folders(),
      });
    },
  });

  const handleUpdate = useCallback(
    (workflow: WorkflowDto) => {
      navigate(`/workflows/${workflow.id}`);
    },
    [navigate]
  );

  const handleDelete = useCallback((workflow: WorkflowDto) => {
    if (!workflow.id) return;
    setDeleteTarget(workflow);
  }, []);

  const handleConfirmDelete = useCallback(() => {
    if (!deleteTarget?.id) return;
    setPendingAction({ id: deleteTarget.id, type: 'delete' });
    removeMutation.mutate(deleteTarget.id, {
      onSettled: () => {
        setPendingAction(null);
        setDeleteTarget(null);
      },
    });
  }, [deleteTarget, removeMutation]);

  const handleSchedule = useCallback(
    (workflow: WorkflowDto) => {
      if (!workflow.id) return;
      setExecuteError(null);
      const rawSchema =
        (workflow as any).inputSchema ?? (workflow as any).input_schema ?? {};
      const hasInputs = parseSchema(rawSchema).length > 0;

      if (!hasInputs) {
        setPendingAction({ id: workflow.id, type: 'schedule' });
        scheduleMutation.mutate(
          { workflowId: workflow.id, inputs: {} },
          {
            onSettled: () => setPendingAction(null),
            onError: (error: any) => {
              // Entitlement-shaped 403s are already surfaced by the shared
              // useCustomMutation handler (handleEntitlementDenial) with a
              // proper message — don't toast them again here, or the user
              // sees the denial twice (SYN-433: entitlement double-popup).
              if (isEntitlementDenial(error)) return;
              const apiError =
                error?.response?.data?.error ||
                error?.response?.data?.message ||
                error?.message;
              toast.error(apiError || 'Failed to start workflow');
            },
          }
        );
        return;
      }

      setExecuteTarget(workflow);
    },
    [scheduleMutation]
  );

  const handleExecute = useCallback(
    async (inputs: Record<string, any>) => {
      if (!executeTarget?.id) return;

      setPendingAction({ id: executeTarget.id, type: 'schedule' });
      try {
        await scheduleMutation.mutateAsync({
          workflowId: executeTarget.id,
          inputs,
        });
        setExecuteTarget(null);
      } catch (error: any) {
        // Entitlement denials already get a proper toast from the shared
        // mutation handler; don't also show the raw summary inline
        // (SYN-433: entitlement double-popup).
        if (!isEntitlementDenial(error)) {
          const apiError =
            error?.response?.data?.error ||
            error?.response?.data?.message ||
            error?.message;
          setExecuteError(apiError || 'Failed to start workflow');
        }
      } finally {
        setPendingAction(null);
      }
    },
    [executeTarget, scheduleMutation]
  );

  const handleClone = useCallback(
    (workflow: WorkflowDto) => {
      if (!workflow.id) return;
      setPendingAction({ id: workflow.id, type: 'clone' });
      cloneMutation.mutate(workflow.id, {
        onSettled: () => setPendingAction(null),
      });
    },
    [cloneMutation]
  );

  const handleChat = useCallback(
    (workflow: WorkflowDto) => {
      navigate(`/workflows/${workflow.id}/chat`);
    },
    [navigate]
  );

  const handleMoveToFolder = useCallback((workflow: WorkflowDto) => {
    setMoveTarget(workflow);
  }, []);

  const handleConfirmMove = useCallback(
    (targetPath: string) => {
      if (!moveTarget?.id) return;
      setPendingAction({ id: moveTarget.id, type: 'move' });
      moveMutation.mutate(
        { workflowId: moveTarget.id, path: targetPath },
        {
          onSettled: () => {
            setPendingAction(null);
            setMoveTarget(null);
          },
        }
      );
    },
    [moveTarget, moveMutation]
  );

  const hasFolders = folders.length > 0;
  const hasWorkflows = filteredWorkflows.length > 0;
  const hasContent = hasFolders || hasWorkflows;

  // Pagination display values
  const startRow = totalElements === 0 ? 0 : (page - 1) * pageSize + 1;
  const endRow = Math.min(page * pageSize, totalElements);

  let body: ReactNode;
  if (isFetching) {
    body = (
      <div className="divide-y divide-border/50">
        {[...Array(8)].map((_, i) => (
          <div key={i} className="flex items-center gap-4 px-5 py-3.5">
            <div className="h-4 w-40 animate-pulse rounded bg-muted/60" />
            <div className="h-4 w-64 animate-pulse rounded bg-muted/60" />
            <div className="ml-auto h-4 w-24 animate-pulse rounded bg-muted/60" />
          </div>
        ))}
      </div>
    );
  } else if (isError) {
    const err = error as any;
    const isNetworkError =
      err?.message?.includes('fetch') ||
      err?.code === 'ERR_NETWORK' ||
      !err?.response;
    const status = err?.response?.status;
    const message = err?.response?.data?.message || err?.message;
    body = (
      <div className="flex h-full flex-col items-center justify-center px-6 py-10 text-center">
        <Icons.warning className="mb-4 h-10 w-10 text-destructive" />
        <p className="text-base font-semibold text-foreground">
          {isNetworkError
            ? 'Unable to connect to backend'
            : `An error occurred (Status: ${status || 'N/A'})`}
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          {isNetworkError
            ? 'Please check your network connection and try again.'
            : message ||
              'There was a problem loading workflows. Please try again.'}
        </p>
        {import.meta.env.DEV && error && (
          <div className="mt-4 max-w-md rounded-lg bg-destructive/10 p-3 text-left">
            <p className="break-words font-mono text-xs text-destructive">
              {error.message || 'Unknown error'}
            </p>
          </div>
        )}
      </div>
    );
  } else if (!hasContent) {
    body = (
      <div className="flex h-full flex-col items-center justify-center px-6 py-10 text-center">
        <Icons.inbox className="mb-4 h-10 w-10 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No workflows yet
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Create your first workflow to kick off an automation flow.
        </p>
        <Link to="/workflows/create" className="mt-4">
          <Button>
            <Icons.add className="mr-2 h-4 w-4" />
            Create workflow
          </Button>
        </Link>
      </div>
    );
  } else {
    body = (
      <Table variant="console" className="table-fixed">
        <TableHeader>
          <TableRow>
            <SortableHead
              label="Name"
              active={sort.column === 'name'}
              dir={sort.dir}
              onClick={() => toggleSort('name')}
            />
            <TableHead>Description</TableHead>
            <SortableHead
              label="Updated"
              className="w-44"
              active={sort.column === 'updated'}
              dir={sort.dir}
              onClick={() => toggleSort('updated')}
            />
            <TableHead className="w-48" />
          </TableRow>
        </TableHeader>
        <TableBody>
          {folders.map((folder) => {
            const count = folderWorkflowCounts[folder.path] || 0;
            return (
              <TableRow
                key={`folder-${folder.path}`}
                className="group cursor-pointer"
                onClick={() => onFolderNavigate?.(folder.path)}
              >
                <TableCell className="font-medium text-foreground">
                  <span className="flex items-center gap-2">
                    <Folder className="h-4 w-4 shrink-0 text-amber-500 dark:text-amber-400" />
                    <span className="truncate">{folder.name}</span>
                  </span>
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {count} workflow{count !== 1 ? 's' : ''}
                </TableCell>
                <TableCell className="text-muted-foreground">—</TableCell>
                <TableCell
                  className="text-right"
                  onClick={(e) => e.stopPropagation()}
                >
                  <div className="flex items-center justify-end gap-1 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
                    <Can permission="workflow:folder_rename">
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 text-muted-foreground"
                      title="Rename folder"
                      onClick={() => onFolderRename?.(folder.path)}
                    >
                      <Pencil className="h-4 w-4" />
                    </Button>
                    </Can>
                    <Can permission="workflow:delete">
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      title="Delete folder"
                      onClick={() => onFolderDelete?.(folder.path)}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                    </Can>
                  </div>
                </TableCell>
              </TableRow>
            );
          })}
          {filteredWorkflows.map((workflow: WorkflowDto) => (
            <WorkflowCard
              key={workflow.id}
              workflow={workflow}
              onUpdate={handleUpdate}
              onDelete={handleDelete}
              onSchedule={handleSchedule}
              onClone={handleClone}
              onChat={handleChat}
              onMoveToFolder={handleMoveToFolder}
              showMoveAction={showMoveAction}
              pendingActionId={pendingAction?.id}
              pendingActionType={pendingAction?.type}
            />
          ))}
          {!hasWorkflows && (
            <TableRow className="hover:bg-transparent">
              <TableCell
                colSpan={4}
                className="py-6 text-center text-sm text-muted-foreground"
              >
                No workflows in this folder yet.
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>
    );
  }

  return (
    <>
      <ConsoleTableShell
        toolbar={toolbar}
        footer={
          hasContent && !isFetching && !isError ? (
            <TableStatusFooter
              left={`Rows ${startRow}–${endRow} of ${totalElements.toLocaleString()}${
                hasFolders
                  ? ` · ${folders.length} folder${folders.length === 1 ? '' : 's'}`
                  : ''
              }`}
              right={
                <TablePagination
                  pageIndex={page - 1}
                  pageSize={pageSize}
                  pageCount={totalPages}
                  onPageChange={(p) => setPage(p + 1)}
                  onPageSizeChange={(size) => {
                    setPageSize(size);
                    setPage(1);
                  }}
                />
              }
            />
          ) : undefined
        }
      >
        {body}
      </ConsoleTableShell>

      <WorkflowExecuteDialog
        open={!!executeTarget}
        onOpenChange={(open) => {
          setExecuteError(null);
          if (!open) {
            setExecuteTarget(null);
          }
        }}
        workflowName={executeTarget?.name}
        inputSchema={
          executeTarget
            ? ((executeTarget as any).inputSchema ??
              (executeTarget as any).input_schema ??
              {})
            : {}
        }
        onExecute={handleExecute}
        isSubmitting={scheduleMutation.isPending}
        serverError={executeError}
      />
      <MoveToFolderDialog
        open={!!moveTarget}
        onOpenChange={(open) => {
          if (!open) {
            setMoveTarget(null);
          }
        }}
        workflowName={moveTarget?.name || ''}
        currentPath={(moveTarget as any)?.path || '/'}
        folders={foldersData?.parsed || []}
        onConfirm={handleConfirmMove}
        isLoading={moveMutation.isPending}
      />
      <ConfirmationDialog
        open={!!deleteTarget}
        description={`This action will delete the workflow "${deleteTarget?.name}".`}
        onConfirm={handleConfirmDelete}
        onClose={() => setDeleteTarget(null)}
      />
    </>
  );
}
