/**
 * Shared layout for the value cell of a mapping row.
 *
 * Every row in the mapping table ends with the same two things: a mode toggle
 * and a delete button. The delete button has its own table column, so it lines
 * up for free. The toggle does not — it lives inside the value cell, which is
 * why keeping it in one vertical column takes these two constants.
 *
 * Both are exported from here rather than written inline at each call site
 * because there are six such sites across two row components, and a row that
 * forgets one is a row whose toggle steps out of line. Historically each one
 * did forget, in a different way.
 */

/** Side of ModeToggleButton, in px. Owned here; the button imports it. */
export const TOGGLE_SIZE_PX = 36;

/** Gap between the value control and the toggle, in px (the row's `gap-2`). */
export const TOGGLE_GAP_PX = 8;

/** Tailwind side class for ModeToggleButton — `size-9` is 36px. */
export const TOGGLE_SIZE_CLASS = 'size-9';

/**
 * Value cell class.
 *
 * TableCell unconditionally emits `[&:has([role=checkbox])]:pr-0`, meant for a
 * selection column. A boolean value control is a checkbox too, so it trips the
 * same selector and that one row loses 12px of right padding — which moves its
 * toggle 12px past every other row's. `!` because the has-variant selector
 * outranks a plain `px-3`.
 */
export const VALUE_CELL_CLASS = '[&:has([role=checkbox])]:!pr-3';

/**
 * Reserves the toggle's slot for controls that render no toggle of their own
 * (arrays, objects, files, composites — all of which open a separate editor).
 *
 * Without it those controls run the full width of the cell and end 44px past
 * every other row's control. 2.75rem = TOGGLE_SIZE_PX + TOGGLE_GAP_PX; the
 * literal is spelled out because Tailwind needs a static class string, and
 * `value-cell-layout.test.ts` fails if the arithmetic stops matching.
 */
export const TOGGLE_GUTTER_CLASS = 'mr-11 w-[calc(100%-2.75rem)]';
