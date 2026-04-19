import { useCallback, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { toast } from 'sonner';
import {
  ChevronFirst,
  ChevronLast,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
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
import { WorkflowExecuteDialog } from '@/features/workflows/components/WorkflowExecuteDialog';
import { MoveToFolderDialog } from '../FolderDialogs';
import { ConfirmationDialog } from '@/shared/components/confirmation-dialog';
import { parseSchema } from '@/features/workflows/utils/schema';
import { useFolders } from '../../hooks/useFolders';

const DEFAULT_PAGE_SIZE = 10;

interface WorkflowsGridProps {
  searchTerm: string;
  sortBy: 'updated' | 'name';
  /** Current folder path filter (undefined = show all, "/" = root only) */
  folderPath?: string;
  /** Whether to show the move to folder action */
  showMoveAction?: boolean;
}

export function WorkflowsGrid({
  searchTerm,
  sortBy,
  folderPath,
  showMoveAction = false,
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
  const isFirstPage = response?.data?.first ?? true;
  const isLastPage = response?.data?.last ?? true;
  // Server handles both folder and search filtering via query parameters
  // Client-side: sort only
  const filteredWorkflows = useMemo(() => {
    return [...workflows].sort((a, b) => {
      if (sortBy === 'name') {
        return (a.name || '').localeCompare(b.name || '');
      }

      const timeA = a.updated ? new Date(a.updated).getTime() : 0;
      const timeB = b.updated ? new Date(b.updated).getTime() : 0;
      return timeB - timeA;
    });
  }, [workflows, sortBy]);

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
        const apiError =
          error?.response?.data?.error ||
          error?.response?.data?.message ||
          error?.message;
        setExecuteError(apiError || 'Failed to start workflow');
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

  if (isFetching) {
    return (
      <div className="space-y-3">
        {[...Array(8)].map((_, i) => (
          <div
            key={i}
            className="flex items-center gap-4 rounded-xl bg-muted/20 p-4 animate-pulse"
          >
            <div className="h-10 w-10 rounded-full bg-muted/70" />
            <div className="flex-1 space-y-2">
              <div className="h-4 w-40 rounded bg-muted/70" />
              <div className="h-3 w-60 rounded bg-muted/70" />
            </div>
            <div className="flex gap-2">
              <div className="h-8 w-20 rounded-full bg-muted/70" />
              <div className="h-8 w-8 rounded-full bg-muted/70" />
              <div className="h-8 w-8 rounded-full bg-muted/70" />
            </div>
          </div>
        ))}
      </div>
    );
  }

  if (isError) {
    const err = error as any;
    const isNetworkError =
      err?.message?.includes('fetch') ||
      err?.code === 'ERR_NETWORK' ||
      !err?.response;

    const status = err?.response?.status;
    const message = err?.response?.data?.message || err?.message;

    return (
      <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-10 text-center">
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
            <p className="text-xs font-mono text-destructive break-words">
              {error.message || 'Unknown error'}
            </p>
          </div>
        )}
      </div>
    );
  }

  if (!filteredWorkflows || filteredWorkflows.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-10 text-center">
        <Icons.inbox className="mb-4 h-10 w-10 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No workflows yet
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Create your first workflow to kick off an automation flow.
        </p>
        <Link to="/workflows/create" className="mt-4">
          <Button className="rounded-full px-5">
            <Icons.add className="mr-2 h-4 w-4" />
            Create workflow
          </Button>
        </Link>
      </div>
    );
  }

  // Calculate display values for pagination
  const startRow = totalElements === 0 ? 0 : (page - 1) * pageSize + 1;
  const endRow = Math.min(page * pageSize, totalElements);

  return (
    <>
      <div className="min-w-0 w-full space-y-2 sm:space-y-3">
        {filteredWorkflows.map((workflow: WorkflowDto, index: number) => (
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
            className="animate-in fade-in-slide-up"
            style={{ animationDelay: `${index * 100}ms` }}
          />
        ))}
      </div>

      {/* Pagination Controls */}
      {totalElements > 0 && (
        <div className="mt-6 px-2 py-4 border-t border-slate-200 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between dark:border-slate-700">
          <div className="flex flex-wrap items-center gap-3 text-sm text-slate-600 dark:text-slate-400">
            <span>
              Showing {startRow}-{endRow} of {totalElements.toLocaleString()}
            </span>
            <div className="flex items-center gap-2">
              <span className="text-sm text-slate-500 dark:text-slate-400">
                Page size:
              </span>
              <select
                className="h-8 rounded-md border border-slate-200 bg-white px-2.5 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300"
                value={pageSize}
                onChange={(e) => {
                  setPageSize(Number(e.target.value));
                  setPage(1); // Reset to first page when changing page size
                }}
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
              Page {page} of {totalPages.toLocaleString()}
            </span>
            <div className="flex items-center gap-1">
              <button
                className="p-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-700"
                disabled={isFirstPage}
                onClick={() => setPage(1)}
                title="First page"
              >
                <ChevronFirst size={16} />
              </button>
              <button
                className="p-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-700"
                disabled={isFirstPage}
                onClick={() => setPage((p) => Math.max(1, p - 1))}
                title="Previous page"
              >
                <ChevronLeft size={16} />
              </button>
              <button
                className="p-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-700"
                disabled={isLastPage}
                onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
                title="Next page"
              >
                <ChevronRight size={16} />
              </button>
              <button
                className="p-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded transition-colors disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-700"
                disabled={isLastPage}
                onClick={() => setPage(totalPages)}
                title="Last page"
              >
                <ChevronLast size={16} />
              </button>
            </div>
          </div>
        </div>
      )}

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
