import { Instance } from '@/generated/RuntaraRuntimeApi';

/** Prefix that marks a client-only draft row not yet persisted to the server. */
export const DRAFT_ID_PREFIX = 'PENDING_';

/** Whether a row id belongs to an unsaved draft rather than a server record. */
export function isDraftRow(id: string | null | undefined): boolean {
  return !!id && id.startsWith(DRAFT_ID_PREFIX);
}

/**
 * Build the client-side draft that "+ Add row" inserts. The draft carries no
 * timestamps — those belong to the server record it may become; fabricating
 * them would present the draft as data that already exists.
 */
export function makeDraftInstance(id: string): Instance {
  return {
    id,
    properties: {},
    tenantId: '',
    createdAt: '',
    updatedAt: '',
  };
}

/**
 * Footer totals for the grid, derived from the server page alone. Draft rows
 * are appended client-side and must never count: the totals describe records
 * that exist, and a draft does not exist until it is committed.
 */
export function computeRecordTotals(
  page:
    | { content?: unknown[]; totalPages?: number; totalElements?: number }
    | undefined,
  pageSize: number
): { totalPages: number; totalElements: number } {
  const serverRowCount = page?.content?.length ?? 0;
  const totalElements = page?.totalElements || serverRowCount;
  const totalPages =
    page?.totalPages && page.totalPages > 0
      ? page.totalPages
      : totalElements > 0 && pageSize > 0
        ? Math.ceil(totalElements / pageSize)
        : 1;
  return { totalPages, totalElements };
}
