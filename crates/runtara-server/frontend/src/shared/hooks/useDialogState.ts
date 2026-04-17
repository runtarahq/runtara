import { useState, useCallback } from 'react';

/**
 * Custom hook for managing dialog/modal open state
 * Reduces boilerplate for common dialog state patterns
 *
 * @param initialState - Initial open state (default: false)
 * @returns Dialog state and control functions
 *
 * @example
 * const deleteDialog = useDialogState();
 * // deleteDialog.isOpen - current state
 * // deleteDialog.open() - opens the dialog
 * // deleteDialog.close() - closes the dialog
 * // deleteDialog.toggle() - toggles the dialog
 * // deleteDialog.setIsOpen(bool) - sets specific state
 */
export function useDialogState(initialState = false) {
  const [isOpen, setIsOpen] = useState(initialState);

  const open = useCallback(() => setIsOpen(true), []);
  const close = useCallback(() => setIsOpen(false), []);
  const toggle = useCallback(() => setIsOpen((prev) => !prev), []);

  return {
    isOpen,
    setIsOpen,
    open,
    close,
    toggle,
  };
}
