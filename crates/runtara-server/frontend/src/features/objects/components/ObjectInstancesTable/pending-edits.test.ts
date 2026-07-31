import { describe, expect, it } from 'vitest';

import { dropEditsForRows, type PendingEditState } from './pending-edits';

const ROW_A = '3f8a2c1d-9b4e-4f6a-8c2d-1e5b7a9c3d0f';
const ROW_B = '7c2e5b8a-1d4f-4a9c-b6e3-0f8d2a5c7e1b';

function state(overrides: Partial<PendingEditState> = {}): PendingEditState {
  return {
    dirtyRows: new Map([
      [ROW_A, { name: 'edited' }],
      [ROW_B, { qty: 3 }],
    ]),
    lastFocusedRowId: ROW_A,
    editingCellId: `${ROW_A}-name`,
    ...overrides,
  };
}

describe('dropEditsForRows', () => {
  it('drops dirty entries only for the deleted rows', () => {
    const result = dropEditsForRows(state(), [ROW_A]);

    expect(result.dirtyRows.has(ROW_A)).toBe(false);
    expect(result.dirtyRows.get(ROW_B)).toEqual({ qty: 3 });
  });

  it('does not mutate the input map', () => {
    const input = state();
    dropEditsForRows(input, [ROW_A]);

    expect(input.dirtyRows.has(ROW_A)).toBe(true);
  });

  it('clears the focused row only when it was deleted', () => {
    expect(dropEditsForRows(state(), [ROW_A]).lastFocusedRowId).toBeNull();
    expect(dropEditsForRows(state(), [ROW_B]).lastFocusedRowId).toBe(ROW_A);
  });

  it('clears the editing cell only when its row was deleted', () => {
    // A cell id is `${rowId}-${columnName}`; the row id itself contains
    // dashes, so this pins the prefix match to the full row id.
    expect(dropEditsForRows(state(), [ROW_A]).editingCellId).toBeNull();
    expect(dropEditsForRows(state(), [ROW_B]).editingCellId).toBe(
      `${ROW_A}-name`
    );
  });

  it('handles several deleted rows at once', () => {
    const result = dropEditsForRows(state(), [ROW_A, ROW_B]);

    expect(result.dirtyRows.size).toBe(0);
    expect(result.lastFocusedRowId).toBeNull();
    expect(result.editingCellId).toBeNull();
  });

  it('leaves empty state untouched', () => {
    const result = dropEditsForRows(
      { dirtyRows: new Map(), lastFocusedRowId: null, editingCellId: null },
      [ROW_A]
    );

    expect(result.dirtyRows.size).toBe(0);
    expect(result.lastFocusedRowId).toBeNull();
    expect(result.editingCellId).toBeNull();
  });
});
