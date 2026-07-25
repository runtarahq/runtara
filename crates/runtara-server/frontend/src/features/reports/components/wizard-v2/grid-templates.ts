/**
 * Shared grid-column templates for the report wizard-v2 editor rows.
 *
 * Each const is a complete Tailwind utility class (including any responsive
 * prefix) so the Tailwind scanner picks the literal up from this file.
 * Do not build these by interpolating a prefix at runtime — the scanner
 * only sees full literals.
 */

/** Two flexible fields + two 120px numeric columns + trailing actions. */
export const GRID_COLS_TWO_FIELDS_TWO_120 =
  'grid-cols-[1fr_1fr_120px_120px_minmax(0,auto)]';

/** Two flexible fields + trailing actions. */
export const GRID_COLS_TWO_FIELDS_ACTIONS =
  'grid-cols-[1fr_1fr_minmax(0,auto)]';

/** Two flexible fields + 100px column + trailing actions. */
export const GRID_COLS_TWO_FIELDS_100_ACTIONS =
  'grid-cols-[1fr_1fr_100px_minmax(0,auto)]';

/** Two overflow-safe fields + auto-sized actions column. */
export const GRID_COLS_TWO_MINMAX_AUTO =
  'grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]';

/** `sm:` variant of {@link GRID_COLS_TWO_MINMAX_AUTO}. */
export const SM_GRID_COLS_TWO_MINMAX_AUTO =
  'sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]';

/** Three overflow-safe fields + auto-sized actions column. */
export const GRID_COLS_THREE_MINMAX_AUTO =
  'grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_auto]';
