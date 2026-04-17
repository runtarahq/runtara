import { useState } from 'react';
import { Activity, HardDrive, Pencil, Trash2, Loader2 } from 'lucide-react';
import { EnrichedConnection } from '@/features/connections/types';
import { Button } from '@/shared/components/ui/button.tsx';
import { Link } from 'react-router';
import { ModalDialog } from '@/shared/components/next-dialog';
import {
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { EntityTile } from '@/shared/components/entity-tile';

interface ConnectionCardProps {
  connection: EnrichedConnection;
  onDelete: (id: string) => void;
  loading?: boolean;
}

function formatNumber(num: number): string {
  if (num >= 1000000) {
    return `${(num / 1000000).toFixed(1)}M`;
  }
  if (num >= 1000) {
    return `${(num / 1000).toFixed(1)}K`;
  }
  return num.toString();
}

export function ConnectionCard({
  connection,
  onDelete,
  loading,
}: ConnectionCardProps) {
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const { id, title, connectionType, rateLimitStats, isDefaultFileStorage } =
    connection;

  const handleDelete = () => {
    onDelete(id);
    setShowDeleteConfirm(false);
  };

  const integrationLabel = connectionType?.displayName || 'Connection';

  const metadata: React.ReactNode[] = [];
  if (isDefaultFileStorage) {
    metadata.push(
      <span
        key="default-storage"
        className="inline-flex items-center gap-1 text-emerald-600 dark:text-emerald-400"
      >
        <HardDrive className="h-3 w-3" />
        Default storage
      </span>
    );
  }
  if (connectionType?.category) {
    metadata.push(connectionType.category);
  }
  if (rateLimitStats) {
    const statsText =
      rateLimitStats.rateLimitedCount > 0
        ? `${formatNumber(rateLimitStats.totalRequests)} requests (${formatNumber(rateLimitStats.rateLimitedCount)} limited) last 24h`
        : `${formatNumber(rateLimitStats.totalRequests)} requests last 24h`;
    metadata.push(
      <span key="requests" className="inline-flex items-center gap-1">
        <Activity className="h-3 w-3" />
        {statsText}
      </span>
    );
  }

  return (
    <>
      <EntityTile
        kicker={integrationLabel}
        title={title}
        metadata={metadata}
        actions={
          <>
            <Link to={`/connections/${id}`}>
              <Button
                variant="ghost"
                size="icon"
                className="p-2 h-auto w-auto text-slate-400 hover:text-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/30 dark:hover:text-blue-400 rounded-lg transition-colors"
                title="Edit connection"
              >
                <Pencil className="w-4 h-4" />
              </Button>
            </Link>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setShowDeleteConfirm(true)}
              className="p-2 h-auto w-auto text-slate-400 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/30 dark:hover:text-red-400 rounded-lg transition-colors"
              title="Delete connection"
              disabled={loading}
            >
              {loading ? (
                <Loader2 className="w-4 h-4 animate-spin" />
              ) : (
                <Trash2 className="w-4 h-4" />
              )}
            </Button>
          </>
        }
      />

      <ModalDialog
        open={showDeleteConfirm}
        onClose={() => setShowDeleteConfirm(false)}
      >
        <DialogHeader>
          <DialogTitle>Delete Connection</DialogTitle>
          <DialogDescription>
            Are you sure you want to delete the connection "{title}"?
          </DialogDescription>
        </DialogHeader>
        <div className="py-2">
          This action cannot be undone and may affect any workflows using this
          connection.
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
            onClick={handleDelete}
            disabled={loading}
          >
            {loading ? 'Deleting...' : 'Delete Connection'}
          </Button>
        </DialogFooter>
      </ModalDialog>
    </>
  );
}
