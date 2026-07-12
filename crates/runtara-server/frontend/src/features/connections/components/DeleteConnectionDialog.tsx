import { Loader2 } from 'lucide-react';

import { buttonVariants } from '@/shared/components/ui/button';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/shared/components/ui/alert-dialog';

type DeleteConnectionDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** OAuth authorization-code types have a provider grant that is revoked. */
  isOAuth: boolean;
  isDeleting: boolean;
  onConfirm: () => void;
};

/**
 * Confirmation for connection deletion. Deletion is irreversible and, for OAuth
 * types, revokes the provider grant first (best effort) — so the copy names the
 * consequence and the action is never a single unguarded click.
 */
export function DeleteConnectionDialog({
  open,
  onOpenChange,
  title,
  isOAuth,
  isDeleting,
  onConfirm,
}: DeleteConnectionDialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete &ldquo;{title}&rdquo;?</AlertDialogTitle>
          <AlertDialogDescription>
            This permanently deletes the connection and cannot be undone.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <ul className="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
          {isOAuth && (
            <li>The provider&rsquo;s access grant will be revoked first (best effort).</li>
          )}
          <li>
            Stored credentials and tokens are removed and cannot be recovered.
          </li>
          <li>Workflows using this connection will fail until updated.</li>
        </ul>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={isDeleting}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            className={buttonVariants({ variant: 'destructive' })}
            onClick={(event) => {
              // Keep the dialog mounted through the delete request so its
              // pending state shows; the caller closes it on success.
              event.preventDefault();
              onConfirm();
            }}
            disabled={isDeleting}
          >
            {isDeleting ? (
              <>
                <Loader2 className="w-4 h-4 mr-1.5 animate-spin" />
                Deleting…
              </>
            ) : (
              'Delete connection'
            )}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
