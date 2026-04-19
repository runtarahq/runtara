import { useQueryClient } from '@tanstack/react-query';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';
import { getFolders, renameFolder, deleteFolder } from '../queries';

/**
 * Represents a parsed folder with its path and display information
 */
export interface FolderInfo {
  /** Full path with leading and trailing slashes, e.g., "/Sales/Shopify/" */
  path: string;
  /** Display name (last segment of path), e.g., "Shopify" */
  name: string;
  /** Parent path, e.g., "/Sales/" or "/" for root-level folders */
  parentPath: string;
  /** Depth level (1 = root-level, 2 = nested, etc.) */
  depth: number;
}

/**
 * Parse folder paths into structured folder info
 */
function parseFolderPaths(paths: string[]): FolderInfo[] {
  return paths
    .filter((path) => path && path !== '/')
    .map((path) => {
      // Remove leading/trailing slashes and split
      const segments = path.replace(/^\/|\/$/g, '').split('/');
      const name = segments[segments.length - 1] || '';
      const parentPath =
        segments.length > 1 ? '/' + segments.slice(0, -1).join('/') + '/' : '/';

      return {
        path,
        name,
        parentPath,
        depth: segments.length,
      };
    })
    .sort((a, b) => a.path.localeCompare(b.path));
}

/**
 * Get root-level folders (depth 1)
 */
function getRootFolders(folders: FolderInfo[]): FolderInfo[] {
  return folders.filter((f) => f.depth === 1);
}

/**
 * Get child folders of a given path
 */
export function getChildFolders(
  folders: FolderInfo[],
  parentPath: string
): FolderInfo[] {
  return folders.filter((f) => f.parentPath === parentPath);
}

/**
 * Hook to fetch all folders
 */
export function useFolders() {
  const result = useCustomQuery({
    queryKey: queryKeys.workflows.folders(),
    queryFn: getFolders,
    staleTime: 0, // Always consider stale so invalidation triggers refetch
    refetchOnMount: true, // Refetch when component mounts
  });

  // Transform the data after fetching
  const data = result.data as { folders: string[] } | undefined;
  const transformedData = data
    ? {
        raw: data.folders,
        parsed: parseFolderPaths(data.folders),
        root: getRootFolders(parseFolderPaths(data.folders)),
      }
    : undefined;

  return {
    ...result,
    data: transformedData,
  };
}

/**
 * Hook to rename a folder
 */
export function useRenameFolder() {
  const queryClient = useQueryClient();

  return useCustomMutation({
    mutationFn: (
      token: string,
      params: { currentPath: string; newPath: string }
    ) => renameFolder(token, params),
    onSuccess: () => {
      // Invalidate both workflows and folders
      queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all });
      queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.folders(),
      });
    },
  });
}

/**
 * Hook to delete a folder (moves all workflows to root)
 */
export function useDeleteFolder() {
  const queryClient = useQueryClient();

  return useCustomMutation({
    mutationFn: (token: string, folderPath: string) =>
      deleteFolder(token, folderPath),
    onSuccess: () => {
      // Invalidate both workflows and folders
      queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all });
      queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.folders(),
      });
    },
  });
}

/**
 * Extract folder name from path
 */
export function getFolderName(path: string): string {
  const segments = path.replace(/^\/|\/$/g, '').split('/');
  return segments[segments.length - 1] || '';
}
