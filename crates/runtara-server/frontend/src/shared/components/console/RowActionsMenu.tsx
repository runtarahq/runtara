import { Link } from 'react-router';
import { MoreHorizontal, Pencil, Trash2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import { Spinner } from '@/shared/components/ui/spinner';
import { useHasPermission } from '@/shared/stores/authStore.ts';

export interface RowActionsMenuProps {
  /** Names the row in the trigger's accessible label, e.g. the entity title. */
  label: string;
  /** Route of the row's edit page. */
  editTo: string;
  editPermission: string;
  onDelete: () => void;
  deletePermission: string;
  /** The row's delete is in flight: the trigger shows a spinner. */
  deleting?: boolean;
}

/**
 * Edit and delete folded into one "⋯" per row, so a list's rows stay quiet
 * and the row's own status is what stands out. Renders nothing when the
 * caller may do neither.
 */
export function RowActionsMenu({
  label,
  editTo,
  editPermission,
  onDelete,
  deletePermission,
  deleting = false,
}: RowActionsMenuProps) {
  const canEdit = useHasPermission(editPermission);
  const canDelete = useHasPermission(deletePermission);
  if (!canEdit && !canDelete) return null;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="secondary"
          size="icon-sm"
          aria-label={`More actions for ${label}`}
          disabled={deleting}
        >
          {deleting ? <Spinner className="size-4" /> : <MoreHorizontal />}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-36">
        {canEdit && (
          <DropdownMenuItem asChild>
            <Link to={editTo}>
              <Pencil className="size-4" />
              Edit
            </Link>
          </DropdownMenuItem>
        )}
        {canEdit && canDelete && <DropdownMenuSeparator />}
        {canDelete && (
          <DropdownMenuItem
            className="text-destructive focus:bg-destructive/10 focus:text-destructive"
            onSelect={onDelete}
          >
            <Trash2 className="size-4" />
            Delete
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
