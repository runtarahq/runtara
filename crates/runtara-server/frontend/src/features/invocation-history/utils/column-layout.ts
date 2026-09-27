/**
 * Responsive layout of the Invocation History table. The console table sizes
 * to its content (`min-w-max`), so every column costs its full width; the
 * lower-priority ones hide below a viewport width so the table, with the
 * sidebar expanded, fits without horizontal scroll and keeps Actions on
 * screen. Execution, Started, Status and Actions always show.
 *
 * Budget with the sidebar expanded (viewport − 240px): 1280 → 1040px,
 * 1440 → 1200px, 1600 → 1360px, 1800 → 1560px.
 */
export type InvocationColumnId =
  | 'workflowId'
  | 'createdAt'
  | 'completedAt'
  | 'status'
  | 'parentInstanceId'
  | 'executionDurationSeconds'
  | 'version'
  | 'actions';

/** Smallest viewport width (px) at which a column shows; absent = always. */
export const COLUMN_MIN_VIEWPORT: Partial<Record<InvocationColumnId, number>> =
  {
    executionDurationSeconds: 1280,
    parentInstanceId: 1440,
    completedAt: 1600,
    version: 1800,
  };

// Literal class strings so Tailwind generates them.
const HIDE_BELOW: Record<number, string> = {
  1280: 'hidden xl:table-cell',
  1440: 'hidden min-[1440px]:table-cell',
  1600: 'hidden min-[1600px]:table-cell',
  1800: 'hidden min-[1800px]:table-cell',
};

/** Header/cell classes that hide a column below its minimum viewport. */
export function responsiveColumnClass(columnId: InvocationColumnId): string {
  const minViewport = COLUMN_MIN_VIEWPORT[columnId];
  return minViewport ? (HIDE_BELOW[minViewport] ?? '') : '';
}

/** Whether a column shows at a viewport width. */
export function isColumnVisibleAt(
  columnId: InvocationColumnId,
  viewportWidth: number
): boolean {
  const minViewport = COLUMN_MIN_VIEWPORT[columnId];
  return minViewport === undefined || viewportWidth >= minViewport;
}

/** First eight characters of a run id, for compact cells (full id on hover). */
export function shortRunId(instanceId: string): string {
  return instanceId.length > 8 ? `${instanceId.slice(0, 8)}…` : instanceId;
}
