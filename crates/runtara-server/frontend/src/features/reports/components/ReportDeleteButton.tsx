import { useState } from 'react';
import { useNavigate } from 'react-router';
import { Loader2, Trash2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { ModalDialog } from '@/shared/components/next-dialog';
import {
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { useDeleteReport } from '../hooks/useReports';

type ReportDeleteButtonProps = {
  reportId: string;
  reportName: string;
  className?: string;
};

export function ReportDeleteButton({
  reportId,
  reportName,
  className,
}: ReportDeleteButtonProps) {
  const navigate = useNavigate();
  const deleteReport = useDeleteReport();
  const [open, setOpen] = useState(false);

  const handleDelete = async () => {
    try {
      await deleteReport.mutateAsync(reportId);
      setOpen(false);
      navigate('/reports');
    } catch {
      // Global mutation error handling shows the failure toast.
    }
  };

  return (
    <>
      <Button
        type="button"
        variant="destructive"
        className={className}
        disabled={deleteReport.isPending}
        onClick={() => setOpen(true)}
      >
        {deleteReport.isPending ? (
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        ) : (
          <Trash2 className="mr-2 h-4 w-4" />
        )}
        Delete
      </Button>

      <ModalDialog open={open} onClose={() => setOpen(false)}>
        <DialogHeader>
          <DialogTitle>Delete report</DialogTitle>
          <DialogDescription>
            Permanently delete "{reportName}"?
          </DialogDescription>
        </DialogHeader>
        <div className="py-2 text-sm text-muted-foreground">
          This removes the report definition, semantic datasets, blocks, and
          saved layout. This action cannot be undone.
        </div>
        <DialogFooter className="gap-2 sm:gap-0">
          <DialogClose asChild>
            <Button type="button" variant="outline">
              Cancel
            </Button>
          </DialogClose>
          <Button
            type="button"
            variant="destructive"
            disabled={deleteReport.isPending}
            onClick={() => void handleDelete()}
          >
            {deleteReport.isPending ? 'Deleting...' : 'Delete report'}
          </Button>
        </DialogFooter>
      </ModalDialog>
    </>
  );
}
