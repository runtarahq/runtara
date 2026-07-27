import { useQueries, useQueryClient } from '@tanstack/react-query';
import { useAuth } from 'react-oidc-context';
import { queryKeys } from '@/shared/queries/query-keys';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';
import { isOidcAuth } from '@/shared/config/runtimeConfig';
import {
  getFolders,
  getFolderWorkflowCount,
  renameFolder,
  deleteFolder,
} from '../queries';

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
          prefix.length > 1 ? '/' + prefix.slice(0, -1).join('/') + '/' : '/';

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
 * Hook to fetch the workflow count for each of the given folders.
 *
 * One count query per folder, asked of the server with `recursive: true`. This
 * is deliberately not derived from a page of workflows on the client: any such
 * page is capped, so once a tenant holds more workflows than the cap the tally
 * silently undercounts (folders holding workflows read "0 workflows"). Counting
 * recursively also means a folder whose workflows all live in subfolders
 * reports what it actually contains instead of 0.
 *
 * Keyed under `workflows.folders()`, so the existing
 * `invalidateQueries({ queryKey: queryKeys.workflows.all })` in the folder and
 * workflow mutations refreshes these counts too.
 */
export function useFolderWorkflowCounts(
  folderPaths: string[]
): Record<string, number> {
  const auth = useAuth();
  const token = auth.user?.access_token;

  return useQueries({
    queries: folderPaths.map((path) => ({
      queryKey: queryKeys.workflows.folderCount(path),
      queryFn: () => getFolderWorkflowCount(token as string, path),
      enabled: !!token || !isOidcAuth,
      refetchOnWindowFocus: false,
    })),
    // `combine` keeps the returned record referentially stable between renders.
    combine: (results) => {
      const counts: Record<string, number> = {};
      folderPaths.forEach((path, index) => {
        const count = results[index]?.data;
        if (typeof count === 'number') {
          counts[path] = count;
        }
      });
      return counts;
    },
  });
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
