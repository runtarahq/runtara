import React from 'react';
import { Dialog, DialogContent } from '@/shared/components/ui/dialog.tsx';

interface SimpleDialogProps {
  children: React.ReactNode;
  open: boolean;
  onClose: () => void;
}

/**
 * A simple dialog wrapper that renders children in a dialog.
 * Use this for custom dialog content without predefined structure.
 *
 * For confirmation dialogs with confirm/cancel buttons, use ConfirmationDialog.
 * For form dialogs with create/update buttons, use DialogBase.
 */
function SimpleDialog(props: SimpleDialogProps) {
  const { children, open = false, onClose } = props;

  if (!open) {
    return null;
  }

  return (
    <Dialog open onOpenChange={onClose}>
      <DialogContent>{children}</DialogContent>
    </Dialog>
  );
}

// Backwards compatibility alias - prefer using SimpleDialog directly
export { SimpleDialog as ModalDialog };
