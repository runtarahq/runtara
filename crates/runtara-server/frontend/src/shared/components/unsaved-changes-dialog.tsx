import { useCallback, useRef, useEffect } from 'react';
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogCancel,
  AlertDialogAction,
} from '@/shared/components/ui/alert-dialog';

type Props = {
  open: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  title?: string;
  description?: string;
  confirmLabel?: string;
  cancelLabel?: string;
};

export function UnsavedChangesDialog({
  open,
  onConfirm,
  onCancel,
  title = 'Unsaved Changes',
  description = 'You have unsaved changes that will be lost. Are you sure you want to continue?',
  confirmLabel = 'Discard Changes',
  cancelLabel = 'Cancel',
}: Props) {
  // Track whether user has interacted with the dialog
  // This prevents automatic close events from dismissing the dialog
  const isHandlingInteraction = useRef(false);

  // Reset interaction state when dialog opens
  useEffect(() => {
    if (open) {
      isHandlingInteraction.current = false;
    }
  }, [open]);

  // Handle dialog close attempts (Escape key, overlay click, etc.)
  // Only allow close if it's from user interaction with our buttons
  const handleOpenChange = useCallback(
    (isOpen: boolean) => {
      if (!isOpen && !isHandlingInteraction.current) {
        // Dialog is trying to close without button interaction
        // This happens when Escape is pressed - treat it as cancel
        onCancel();
      }
    },
    [onCancel]
  );

  // Mark that user is interacting via button before the click event completes
  // This ensures the flag is set before onOpenChange fires
  const handleButtonPointerDown = useCallback(() => {
    isHandlingInteraction.current = true;
  }, []);

  const handleConfirmClick = useCallback(() => {
    onConfirm();
  }, [onConfirm]);

  const handleCancelClick = useCallback(() => {
    onCancel();
  }, [onCancel]);

  return (
    <AlertDialog open={open} onOpenChange={handleOpenChange}>
      <AlertDialogContent className="p-6">
        <AlertDialogHeader className="pb-4">
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription className="pt-2">
            {description}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className="pt-4">
          <AlertDialogCancel
            onClick={handleCancelClick}
            onPointerDown={handleButtonPointerDown}
          >
            {cancelLabel}
          </AlertDialogCancel>
          <AlertDialogAction
            onClick={handleConfirmClick}
            onPointerDown={handleButtonPointerDown}
            className="bg-orange-600 hover:bg-orange-700"
          >
            {confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
