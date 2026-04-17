import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog.tsx';
import { Button } from '@/shared/components/ui/button.tsx';
import React, { useEffect } from 'react';

const DEFAULT_TITLE = 'Are you absolutely sure?';
const DEFAULT_DESCRIPTION = 'This action will delete this entity.';

interface ConfirmationDialogProps {
  children?: React.ReactNode;
  open: boolean;
  title?: string;
  description?: string;
  loading?: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

/**
 * A confirmation dialog component for destructive or important actions.
 * Use this for delete confirmations, clone confirmations, etc.
 *
 * For simple dialogs that just wrap content, use SimpleDialog from next-dialog.
 * For form dialogs with create/update buttons, use DialogBase from next-dialog.
 */
export function ConfirmationDialog(props: ConfirmationDialogProps) {
  const {
    children = null,
    open = false,
    title = DEFAULT_TITLE,
    description = DEFAULT_DESCRIPTION,
    loading = false,
    onClose,
    onConfirm,
  } = props;

  // Clean up pointer-events style when component unmounts
  useEffect(() => {
    return () => {
      document.body.style.removeProperty('pointer-events');
    };
  }, []);

  // Clean up pointer-events style when modal is closed
  useEffect(() => {
    if (!open) {
      document.body.style.removeProperty('pointer-events');
    }
  }, [open]);

  return (
    <Dialog open={open} onOpenChange={onClose}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        {loading && 'Loading..'}
        {children && !loading && children}
        <DialogFooter>
          <Button type="button" variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="button" onClick={onConfirm}>
            Confirm
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// Backwards compatibility alias - prefer using ConfirmationDialog directly
// ModalDialog is not used externally; kept as local alias only
