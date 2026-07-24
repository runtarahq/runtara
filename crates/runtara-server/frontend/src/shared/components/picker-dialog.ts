/**
 * Shared geometry for picker dialogs (step, capability, variable, and
 * connection pickers). Design decision: every picker is a 500px-wide dialog
 * with a 400px scrollable list and 200px item truncation so all pickers read
 * as one control. Owner: shared picker components — change the values here,
 * never per-modal.
 */
export const PICKER_DIALOG_WIDTH = 'sm:max-w-[500px]';
export const PICKER_LIST_MAX_HEIGHT = 'max-h-[400px]';
export const PICKER_TRUNCATE_MAX_WIDTH = 'max-w-[200px]';
