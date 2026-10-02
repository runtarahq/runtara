import { useState } from 'react';
import { Trash2 } from 'lucide-react';
import { useQueryClient } from '@tanstack/react-query';
import type { SavedOperationView } from '@/generated/RuntaraRuntimeApi';
import { useToken } from '@/shared/hooks';
import { Can } from '@/shared/components/Can';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from '@/shared/components/ui/dialog';
import { message, operationsRequest } from '../queries';

export function DeleteQueue({
  queue,
  icon = false,
  onDeleted,
}: {
  queue: SavedOperationView;
  icon?: boolean;
  onDeleted?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [target, setTarget] = useState(queue);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const token = useToken();
  const client = useQueryClient();
  async function remove() {
    setBusy(true);
    setError('');
    try {
      await operationsRequest(
        token,
        `operations/views/${encodeURIComponent(target.id)}?revision=${target.revision}`,
        'DELETE'
      );
      setOpen(false);
      await client.invalidateQueries({ queryKey: ['operations'] });
      onDeleted?.();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Can permission="workflow:update">
      <WithTooltip label="Delete queue">
        <Button
          variant="secondary"
          size={icon ? 'icon' : 'default'}
          bordered={!icon}
          className={
            icon ? 'h-8 w-8 text-muted-foreground hover:text-destructive' : ''
          }
          aria-label={`Delete queue ${queue.configuration.name}`}
          onClick={() => {
            setError('');
            setTarget(queue);
            setOpen(true);
          }}
        >
          <Trash2 className="size-4" />
          {!icon && 'Delete queue'}
        </Button>
      </WithTooltip>
      <Dialog
        open={open}
        onOpenChange={(value) => {
          if (!busy) setOpen(value);
        }}
      >
        <DialogContent>
          <DialogTitle>Delete queue “{target.configuration.name}”?</DialogTitle>
          <DialogDescription>
            This removes the saved queue configuration. Workflows, runs, and
            pending requests remain available.
          </DialogDescription>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              variant="secondary"
              bordered
              disabled={busy}
              onClick={() => setOpen(false)}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={busy}
              onClick={() => void remove()}
            >
              {busy ? 'Deleting…' : 'Delete queue'}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </Can>
  );
}
