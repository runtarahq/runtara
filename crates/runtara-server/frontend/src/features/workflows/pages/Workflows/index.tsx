import { useState, useMemo, useCallback } from 'react';
import { Link, useSearchParams } from 'react-router';
import { PlusIcon, Search, X } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/shared/components/ui/button.tsx';
import { Input } from '@/shared/components/ui/input.tsx';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { WorkflowsGrid } from '../../components/WorkflowsGrid';
import { FolderCard } from '../../components/FolderCard';
import { FolderBreadcrumb } from '../../components/FolderBreadcrumb';
import {
  RenameFolderDialog,
  DeleteFolderDialog,
} from '../../components/FolderDialogs';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { TilesPage, TileList } from '@/shared/components/tiles-page';
import { useCustomQuery, useDebounce } from '@/shared/hooks';
import { queryKeys } from '@/shared/queries/query-keys';
import { getWorkflows } from '@/features/workflows/queries';
import {
  useFolders,
  useRenameFolder,
  useDeleteFolder,
  getChildFolders,
} from '../../hooks/useFolders';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';

export function Workflows() {
  usePageTitle('Workflows');
  const [searchParams, setSearchParams] = useSearchParams();
  const [searchTerm, setSearchTerm] = useState('');
  const debouncedSearchTerm = useDebounce(searchTerm, 300);
  const [sortBy, setSortBy] = useState<'updated' | 'name'>('updated');

  // Folder navigation state - derived from URL search params for proper browser history support
  const currentFolderPath = searchParams.get('folder') || '/';

  // Dialog state
  const [renameFolderTarget, setRenameFolderTarget] = useState<string | null>(
    null
  );
  const [deleteFolderTarget, setDeleteFolderTarget] = useState<string | null>(
    null
  );

  const { isError } = useCustomQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: getWorkflows,
  });

  // Folder data
  const { data: foldersData } = useFolders();

  // Mutations for folder operations
  const renameFolderMutation = useRenameFolder();
  const deleteFolderMutation = useDeleteFolder();

  // Get child folders for current path
  const childFolders = useMemo(() => {
    if (!foldersData?.parsed) return [];
    return getChildFolders(foldersData.parsed, currentFolderPath);
  }, [foldersData?.parsed, currentFolderPath]);

  // Get workflows data for folder counts (uses recursive to get all workflows)
  const { data: workflowsResponse } = useCustomQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: getWorkflows,
  });
  const workflows = useMemo(
    () => (workflowsResponse?.data?.content || []) as WorkflowDto[],
    [workflowsResponse?.data?.content]
  );

  // Count workflows per folder
  const folderWorkflowCounts = useMemo(() => {
    const counts: Record<string, number> = {};
    workflows.forEach((workflow) => {
      const folderPath = (workflow as any).path || '/';
      counts[folderPath] = (counts[folderPath] || 0) + 1;
    });
    return counts;
  }, [workflows]);

  // Folder navigation - updates URL to enable browser back/forward navigation
  const handleFolderNavigate = useCallback(
    (path: string) => {
      if (path === '/') {
        // Remove folder param when navigating to root
        setSearchParams((prev) => {
          const next = new URLSearchParams(prev);
          next.delete('folder');
          return next;
        });
      } else {
        setSearchParams((prev) => {
          const next = new URLSearchParams(prev);
          next.set('folder', path);
          return next;
        });
      }
    },
    [setSearchParams]
  );

  // Rename folder
  const handleRenameFolder = useCallback(
    async (currentPath: string, newName: string) => {
      const segments = currentPath.replace(/^\/|\/$/g, '').split('/');
      segments[segments.length - 1] = newName;
      const newPath = '/' + segments.join('/') + '/';

      try {
        await renameFolderMutation.mutateAsync({ currentPath, newPath });
        toast.success('Folder renamed successfully');
        setRenameFolderTarget(null);
        // If we were inside the renamed folder, update current path in URL
        if (currentFolderPath === currentPath) {
          setSearchParams(
            (prev) => {
              const next = new URLSearchParams(prev);
              next.set('folder', newPath);
              return next;
            },
            { replace: true }
          );
        }
      } catch (error: any) {
        toast.error(error?.message || 'Failed to rename folder');
      }
    },
    [renameFolderMutation, currentFolderPath, setSearchParams]
  );

  // Delete folder
  const handleDeleteFolder = useCallback(
    async (path: string) => {
      try {
        await deleteFolderMutation.mutateAsync(path);
        toast.success('Folder deleted successfully');
        setDeleteFolderTarget(null);
        // If we were inside the deleted folder, go back to root
        if (currentFolderPath.startsWith(path)) {
          setSearchParams(
            (prev) => {
              const next = new URLSearchParams(prev);
              next.delete('folder');
              return next;
            },
            { replace: true }
          );
        }
      } catch (error: any) {
        toast.error(error?.message || 'Failed to delete folder');
      }
    },
    [deleteFolderMutation, currentFolderPath, setSearchParams]
  );

  const isAtRoot = currentFolderPath === '/';

  return (
    <TilesPage
      kicker="Workflows"
      title="Build and iterate automation flows"
      action={
        <Link to="/workflows/create" className="w-full sm:w-auto">
          <Button
            className="h-10 w-full sm:w-auto sm:px-4 shadow-sm shadow-blue-600/20 dark:shadow-blue-900/30"
            disabled={isError}
          >
            <PlusIcon className="mr-2 h-4 w-4" />
            New workflow
          </Button>
        </Link>
      }
      toolbar={
        <div className="space-y-4">
          {/* Breadcrumb navigation */}
          {!isAtRoot && (
            <FolderBreadcrumb
              currentPath={currentFolderPath}
              folders={foldersData?.raw || []}
              onNavigate={handleFolderNavigate}
            />
          )}

          {/* Search and sort */}
          <div className="flex w-full flex-col gap-4 sm:flex-row sm:items-center">
            <div className="relative w-full sm:flex-[2]">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-400" />
              <Input
                value={searchTerm}
                onChange={(event) => setSearchTerm(event.target.value)}
                placeholder="Search workflows..."
                className="h-10 w-full rounded-lg border-slate-200 bg-white pl-10 pr-4 text-sm placeholder:text-slate-400 focus:border-blue-300 focus:ring-2 focus:ring-blue-100 dark:bg-slate-900 dark:border-slate-700 dark:focus:border-blue-600 dark:focus:ring-blue-900/30"
              />
              {searchTerm && (
                <Button
                  variant="ghost"
                  size="icon"
                  className="absolute right-1 top-1/2 -translate-y-1/2 h-8 w-8"
                  onClick={() => setSearchTerm('')}
                >
                  <X className="h-4 w-4" />
                </Button>
              )}
            </div>
            <Select
              value={sortBy}
              onValueChange={(value) => setSortBy(value as 'updated' | 'name')}
            >
              <SelectTrigger className="h-10 w-full sm:flex-1 rounded-lg border-slate-200 bg-white px-3.5 text-sm text-slate-600 hover:bg-slate-50 dark:bg-slate-900 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800">
                <SelectValue placeholder="Sort by" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="updated">Sort by: Updated</SelectItem>
                <SelectItem value="name">Sort by: Name</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>
      }
    >
      <TileList>
        {/* Folder cards - only show when at root or in a folder with children */}
        {isAtRoot && childFolders.length > 0 && (
          <div className="space-y-2 mb-6">
            {childFolders.map((folder, index) => (
              <FolderCard
                key={folder.path}
                name={folder.name}
                path={folder.path}
                workflowCount={folderWorkflowCounts[folder.path] || 0}
                onOpen={handleFolderNavigate}
                onRename={(path) => setRenameFolderTarget(path)}
                onDelete={(path) => setDeleteFolderTarget(path)}
                className="animate-in fade-in-slide-up"
                style={{ animationDelay: `${index * 50}ms` }}
              />
            ))}
          </div>
        )}

        {/* Workflows grid */}
        <WorkflowsGrid
          searchTerm={debouncedSearchTerm}
          sortBy={sortBy}
          folderPath={currentFolderPath}
          showMoveAction={true}
        />
      </TileList>

      {/* Rename Folder Dialog */}
      <RenameFolderDialog
        open={!!renameFolderTarget}
        onOpenChange={(open) => !open && setRenameFolderTarget(null)}
        onConfirm={handleRenameFolder}
        folderPath={renameFolderTarget || '/'}
        isLoading={renameFolderMutation.isPending}
      />

      {/* Delete Folder Dialog */}
      <DeleteFolderDialog
        open={!!deleteFolderTarget}
        onOpenChange={(open) => !open && setDeleteFolderTarget(null)}
        onConfirm={handleDeleteFolder}
        folderPath={deleteFolderTarget || '/'}
        workflowCount={
          deleteFolderTarget ? folderWorkflowCounts[deleteFolderTarget] || 0 : 0
        }
        isLoading={deleteFolderMutation.isPending}
      />
    </TilesPage>
  );
}
