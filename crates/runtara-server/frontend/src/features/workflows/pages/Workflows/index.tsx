import { useState, useMemo, useCallback } from 'react';
import { Link, useSearchParams } from 'react-router';
import { PlusIcon } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/shared/components/ui/button.tsx';
import { Can } from '@/shared/components/Can';
import {
  Breadcrumb,
  ConsoleToolbar,
  ToolbarSearch,
  type BreadcrumbItem,
} from '@/shared/components/console';
import { WorkflowsGrid } from '../../components/WorkflowsGrid';
import {
  RenameFolderDialog,
  DeleteFolderDialog,
} from '../../components/FolderDialogs';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useCustomQuery, useDebounce } from '@/shared/hooks';
import { queryKeys } from '@/shared/queries/query-keys';
import { getWorkflows } from '@/features/workflows/queries';
import {
  useFolders,
  useFolderWorkflowCounts,
  useRenameFolder,
  useDeleteFolder,
  getChildFolders,
} from '../../hooks/useFolders';

export function Workflows() {
  usePageTitle('Workflows');
  const [searchParams, setSearchParams] = useSearchParams();
  const [searchTerm, setSearchTerm] = useState('');
  const debouncedSearchTerm = useDebounce(searchTerm, 300);

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

  const childFolderPaths = useMemo(
    () => childFolders.map((folder) => folder.path),
    [childFolders]
  );

  // Counts come from the server, one recursive count per visible folder.
  const folderWorkflowCounts = useFolderWorkflowCounts(childFolderPaths);

  // Folder navigation - updates URL to enable browser back/forward navigation
  const handleFolderNavigate = useCallback(
    (path: string) => {
      if (path === '/') {
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

  // Breadcrumb path: Workflows / <folder> / <subfolder> …
  const breadcrumbItems = useMemo<BreadcrumbItem[]>(() => {
    const items: BreadcrumbItem[] = [
      { label: 'Workflows', onClick: () => handleFolderNavigate('/') },
    ];
    if (currentFolderPath && currentFolderPath !== '/') {
      const segments = currentFolderPath
        .replace(/^\/|\/$/g, '')
        .split('/')
        .filter(Boolean);
      let acc = '';
      segments.forEach((segment) => {
        acc += '/' + segment;
        const path = acc + '/';
        items.push({
          label: segment,
          onClick: () => handleFolderNavigate(path),
        });
      });
    }
    return items;
  }, [currentFolderPath, handleFolderNavigate]);

  const toolbar = (
    <ConsoleToolbar
      left={<Breadcrumb items={breadcrumbItems} />}
      search={
        <ToolbarSearch
          value={searchTerm}
          onChange={setSearchTerm}
          placeholder="Search workflows…"
          className="w-56"
        />
      }
      actions={
        <Can permission="workflow:create">
          <Link to="/workflows/create">
            <Button disabled={isError}>
              <PlusIcon className="mr-2 size-4" />
              New workflow
            </Button>
          </Link>
        </Can>
      }
    />
  );

  return (
    <>
      <WorkflowsGrid
        toolbar={toolbar}
        searchTerm={debouncedSearchTerm}
        folderPath={currentFolderPath}
        showMoveAction={true}
        folders={childFolders}
        folderWorkflowCounts={folderWorkflowCounts}
        onFolderNavigate={handleFolderNavigate}
        onFolderRename={setRenameFolderTarget}
        onFolderDelete={setDeleteFolderTarget}
      />

      <RenameFolderDialog
        open={!!renameFolderTarget}
        onOpenChange={(open) => !open && setRenameFolderTarget(null)}
        onConfirm={handleRenameFolder}
        folderPath={renameFolderTarget || '/'}
        isLoading={renameFolderMutation.isPending}
      />

      <DeleteFolderDialog
        open={!!deleteFolderTarget}
        onOpenChange={(open) => !open && setDeleteFolderTarget(null)}
        onConfirm={handleDeleteFolder}
        folderPath={deleteFolderTarget || '/'}
        workflowCount={
          deleteFolderTarget
            ? folderWorkflowCounts[deleteFolderTarget]
            : undefined
        }
        isLoading={deleteFolderMutation.isPending}
      />
    </>
  );
}
