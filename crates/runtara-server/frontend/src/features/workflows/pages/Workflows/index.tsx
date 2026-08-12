import { useState, useMemo, useCallback, useEffect, useRef } from 'react';
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
import { useDebounce } from '@/shared/hooks';
import {
  useFolders,
  useFolderWorkflowCounts,
  useRenameFolder,
  useDeleteFolder,
  getChildFolders,
} from '../../hooks/useFolders';
import { createWorkflowHref } from '../../folder-nav';
import {
  readListUrlState,
  writeListUrlState,
} from '../../components/WorkflowsGrid/list-url-state';

export function Workflows() {
  usePageTitle('Workflows');
  const [searchParams, setSearchParams] = useSearchParams();

  // Folder navigation state - derived from URL search params for proper browser history support
  const currentFolderPath = searchParams.get('folder') || '/';

  // The URL owns the query and the listing reads it from there. The box keeps
  // its own copy so typing stays responsive, and only the settled value is
  // written — a history entry per keystroke would make Back useless.
  const urlSearchTerm = readListUrlState(searchParams).search;
  const [searchInput, setSearchInput] = useState(urlSearchTerm);
  const debouncedInput = useDebounce(searchInput, 300);
  // Whitespace is no query at all, and the URL says so by holding nothing.
  const settledSearchTerm = debouncedInput.trim() ? debouncedInput : '';
  // The last value the two sides agreed on, so each sync below can tell its own
  // write from a change that came from elsewhere — a back/forward, or a link
  // opened with a query already on it — instead of the two overwriting in a loop.
  const syncedSearchTerm = useRef(urlSearchTerm);

  // Box → URL.
  useEffect(() => {
    // The debounce still trailing the box means this fired for some other
    // reason — `setSearchParams` is a new function after every navigation — and
    // the settled value is the one from before that navigation. Writing it back
    // is how a Back taken mid-debounce used to bounce straight forward again.
    if (debouncedInput !== searchInput) return;
    if (settledSearchTerm === syncedSearchTerm.current) return;
    syncedSearchTerm.current = settledSearchTerm;
    setSearchParams(
      (prev) => writeListUrlState(prev, { search: settledSearchTerm }),
      { replace: true }
    );
  }, [debouncedInput, searchInput, settledSearchTerm, setSearchParams]);

  // URL → box, for the queries this page didn't type: a back/forward, or a link
  // opened with one already on it.
  useEffect(() => {
    if (urlSearchTerm === syncedSearchTerm.current) return;
    syncedSearchTerm.current = urlSearchTerm;
    setSearchInput(urlSearchTerm);
  }, [urlSearchTerm]);

  // Dialog state
  const [renameFolderTarget, setRenameFolderTarget] = useState<string | null>(
    null
  );
  const [deleteFolderTarget, setDeleteFolderTarget] = useState<string | null>(
    null
  );

  // Folder data. Its error also gates "New workflow": it is the cheapest signal
  // on this page that the runtime API is reachable, and the grid renders its own
  // error state for the workflow list itself. Gating on a full workflow listing
  // would mean paging the entire tenant just to decide whether a button is
  // clickable.
  const { data: foldersData, isError } = useFolders();

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
      setSearchParams((prev) => {
        const next = new URLSearchParams(prev);
        if (path === '/') next.delete('folder');
        else next.set('folder', path);
        // Another folder is another listing, so the page index goes with it.
        return writeListUrlState(next, { page: 0 });
      });
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
              return writeListUrlState(next, { page: 0 });
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
          value={searchInput}
          onChange={setSearchInput}
          placeholder="Search workflows…"
          className="w-56"
        />
      }
      actions={
        <Can permission="workflow:create">
          <Link to={createWorkflowHref(currentFolderPath)}>
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
        searchTerm={urlSearchTerm}
        folderPath={currentFolderPath}
        showMoveAction={true}
        folders={childFolders}
        folderWorkflowCounts={folderWorkflowCounts}
        onClearSearch={() => setSearchInput('')}
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
