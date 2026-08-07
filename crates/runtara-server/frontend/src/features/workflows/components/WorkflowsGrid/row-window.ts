/**
 * Pagination math for a listing whose rows are folders first, then workflows.
 *
 * The two halves arrive differently: the folder endpoint returns the whole child
 * list in one response, while workflows are paginated server-side. Paging them
 * independently is what made the same folders reappear above every page, so the
 * grid treats them as a single row set and slices both from one page index.
 */

export interface RowWindow {
  /** Index of this page's first folder row within the folder list. */
  folderStart: number;
  /** How many folder rows this page holds. */
  folderTake: number;
  /** Index of this page's first workflow row within the workflow set. */
  workflowOffset: number;
  /** How many workflow rows this page holds, once folders have taken theirs. */
  workflowLimit: number;
}

/** Split one page of the combined listing into its folder and workflow parts. */
export function folderWorkflowWindow(
  folderCount: number,
  page: number,
  pageSize: number
): RowWindow {
  const rowOffset = Math.max(0, page) * pageSize;
  const folderStart = Math.min(rowOffset, folderCount);
  const folderTake = Math.min(folderCount - folderStart, pageSize);

  return {
    folderStart,
    folderTake,
    workflowOffset: Math.max(0, rowOffset - folderCount),
    workflowLimit: pageSize - folderTake,
  };
}

export interface WorkflowServerSlice {
  /** 0-based page to request from the workflow listing API. */
  page: number;
  /** Rows to drop from the front of that page. */
  skip: number;
  /** Rows to keep after dropping `skip` of them. */
  take: number;
}

/**
 * Translate an arbitrary `[offset, offset + limit)` workflow window into the
 * page-based listing API.
 *
 * The API only offsets by whole pages, so a window that starts mid-page — which
 * is every window once a folder block that is not a multiple of `pageSize` sits
 * in front of it — spans two server pages. `skip + take > pageSize` is the caller's
 * signal that it has to fetch the following page too.
 */
export function workflowServerSlice(
  offset: number,
  limit: number,
  pageSize: number
): WorkflowServerSlice {
  const page = Math.floor(offset / pageSize);

  return { page, skip: offset - page * pageSize, take: limit };
}

/** Pages needed for `folderCount` folder rows followed by `workflowCount` workflows. */
export function rowPageCount(
  folderCount: number,
  workflowCount: number,
  pageSize: number
): number {
  return Math.max(1, Math.ceil((folderCount + workflowCount) / pageSize));
}
