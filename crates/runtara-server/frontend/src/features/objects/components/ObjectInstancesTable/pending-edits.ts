/**
 * Inline-edit bookkeeping the grid keeps while a row has unsaved changes.
 * Saves are deferred (queued behind timeouts), so this state can outlive the
 * row it belongs to unless it is pruned when rows are deleted.
 */
export interface PendingEditState {
  /** Unsaved property values keyed by row id. */
  dirtyRows: Map<string, Record<string, unknown>>;
  /** Row that most recently held focus; the next flush targets it. */
  lastFocusedRowId: string | null;
  /** Cell currently in edit mode, as `${rowId}-${columnName}`. */
  editingCellId: string | null;
}

/**
 * Drop pending edit state for rows that are being deleted, so no deferred
 * save can later flush against a row that no longer exists. State for rows
 * outside `deletedIds` is preserved untouched.
 */
export function dropEditsForRows(
  state: PendingEditState,
  deletedIds: string[]
): PendingEditState {
  const deleted = new Set(deletedIds);

  const dirtyRows = new Map(state.dirtyRows);
  deleted.forEach((id) => dirtyRows.delete(id));

  const lastFocusedRowId =
    state.lastFocusedRowId && deleted.has(state.lastFocusedRowId)
      ? null
      : state.lastFocusedRowId;

  // Row ids are fixed-length, so despite the dashes inside a UUID the
  // `${rowId}-` prefix test matches exactly one row's cells.
  const editingCellId =
    state.editingCellId &&
    deletedIds.some((id) => state.editingCellId!.startsWith(`${id}-`))
      ? null
      : state.editingCellId;

  return { dirtyRows, lastFocusedRowId, editingCellId };
}
