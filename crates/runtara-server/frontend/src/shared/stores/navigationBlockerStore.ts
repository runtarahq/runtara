import { create } from 'zustand';

interface BlockerState {
  // Whether navigation should be blocked
  shouldBlock: boolean;
  // Callback to execute when user confirms navigation
  onConfirmNavigation: (() => void) | null;
  // The blocker's proceed function from useBlocker
  proceedFn: (() => void) | null;
  // The blocker's reset function from useBlocker
  resetFn: (() => void) | null;
  // Whether the confirmation dialog is open
  isDialogOpen: boolean;

  // Actions
  setBlocker: (shouldBlock: boolean, onConfirm?: () => void) => void;
  setBlockerFunctions: (proceed: () => void, reset: () => void) => void;
  openDialog: () => void;
  closeDialog: () => void;
  confirmNavigation: () => void;
  cancelNavigation: () => void;
  reset: () => void;
}

export const useNavigationBlockerStore = create<BlockerState>((set, get) => ({
  shouldBlock: false,
  onConfirmNavigation: null,
  proceedFn: null,
  resetFn: null,
  isDialogOpen: false,

  setBlocker: (shouldBlock, onConfirm) => {
    set({
      shouldBlock,
      onConfirmNavigation: onConfirm || null,
    });
  },

  setBlockerFunctions: (proceed, reset) => {
    set({
      proceedFn: proceed,
      resetFn: reset,
    });
  },

  openDialog: () => {
    set({ isDialogOpen: true });
  },

  closeDialog: () => {
    set({ isDialogOpen: false });
  },

  confirmNavigation: () => {
    const { onConfirmNavigation, proceedFn } = get();
    // Execute the cleanup callback if provided
    if (onConfirmNavigation) {
      onConfirmNavigation();
    }
    // Proceed with navigation
    if (proceedFn) {
      proceedFn();
    }
    set({
      isDialogOpen: false,
      proceedFn: null,
      resetFn: null,
    });
  },

  cancelNavigation: () => {
    const { resetFn } = get();
    // Cancel navigation
    if (resetFn) {
      resetFn();
    }
    set({
      isDialogOpen: false,
      proceedFn: null,
      resetFn: null,
    });
  },

  reset: () => {
    set({
      shouldBlock: false,
      onConfirmNavigation: null,
      proceedFn: null,
      resetFn: null,
      isDialogOpen: false,
    });
  },
}));
