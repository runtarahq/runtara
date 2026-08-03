/**
 * Folder-aware navigation helpers for the workflows list and create pages.
 *
 * The list page keeps the current folder in the `folder` search param; these
 * helpers carry it through the create flow so a workflow created from inside
 * a folder lands in that folder instead of silently at the root.
 */

/** Href for the create page, carrying the current folder context. */
export function createWorkflowHref(currentFolderPath: string): string {
  return currentFolderPath === '/'
    ? '/workflows/create'
    : `/workflows/create?folder=${encodeURIComponent(currentFolderPath)}`;
}

/** Href back to the workflows list, restoring the folder the user came from. */
export function workflowsListHref(folderPath: string): string {
  return folderPath === '/'
    ? '/workflows'
    : `/workflows?folder=${encodeURIComponent(folderPath)}`;
}

/**
 * Sanitize a `folder` search param into a usable folder path.
 *
 * Accepts only '/'-wrapped paths without empty segments (the same shape the
 * backend's path validation enforces); anything else falls back to the root.
 */
export function normalizeFolderParam(raw: string | null): string {
  if (!raw || raw === '/') return '/';
  if (!raw.startsWith('/') || !raw.endsWith('/') || raw.includes('//')) {
    return '/';
  }
  return raw;
}
