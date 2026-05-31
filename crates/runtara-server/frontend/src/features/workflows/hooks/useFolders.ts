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
 * Parse folder paths into structured folder info.
 *
 * Every ancestor is materialized, not just the literal paths the API returns:
 * e.g. if the only path is "/Demo/Test/", an intermediate "/Demo/" folder is
 * synthesized so it still shows up at its level and stays navigable. Without
 * this, a nested folder whose parent has no direct workflows would be
 * unreachable (the parent never appears as a row to click into).
 */
function parseFolderPaths(paths: readonly string[]): FolderInfo[] {
  const byPath = new Map<string, FolderInfo>();

  paths
    .filter((path) => path && path !== '/')
    .forEach((path) => {
      const segments = path
        .replace(/^\/|\/$/g, '')
        .split('/')
        .filter(Boolean);

      // Walk each prefix so ancestors are included (and de-duplicated).
      for (let depth = 1; depth <= segments.length; depth++) {
        const prefix = segments.slice(0, depth);
        const fullPath = '/' + prefix.join('/') + '/';
        if (byPath.has(fullPath)) continue;

        const parentPath =
          prefix.length > 1
            ? '/' + prefix.slice(0, -1).join('/') + '/'
            : '/';

        byPath.set(fullPath, {
          path: fullPath,
          name: prefix[prefix.length - 1] || '',
          parentPath,
          depth: prefix.length,
        });
      }
    });

  return Array.from(byPath.values()).sort((a, b) =>
    a.path.localeCompare(b.path)
  );
}

/**
 * Get root-level folders (depth 1)
 */
function getRootFolders(folders: FolderInfo[]): FolderInfo[] {
  return folders.filter((f) => f.depth === 1);
}

function getFolderPaths(data: unknown): string[] {
  if (!data || typeof data !== 'object') return [];

  const folders = (data as { folders?: unknown }).folders;
  if (!Array.isArray(folders)) return [];

  return folders.filter(
    (folder): folder is string => typeof folder === 'string'
  );
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

  const folderPaths = getFolderPaths(result.data);
  const parsedFolders = parseFolderPaths(folderPaths);
  const transformedData =
    result.data !== undefined
      ? {
          raw: folderPaths,
          parsed: parsedFolders,
          root: getRootFolders(parsedFolders),
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
