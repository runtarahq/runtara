import { useState } from 'react';
import { Loader2, Pause } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { pauseInstance } from '@/features/workflows/queries';
import { useToast } from '@/shared/hooks/useToast';
import { useToken } from '@/shared/hooks';
import { ModalDialog } from '@/shared/components/next-dialog';
import {
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';

type Props = {
  instanceId: string;
  variant?:
    | 'default'
    | 'outline'
    | 'secondary'
    | 'ghost'
    | 'link'
    | 'destructive';
  size?: 'default' | 'sm' | 'lg' | 'icon';
  className?: string;
};

/**
 * Pause a running workflow instance (suspends it at the next checkpoint).
 * The server only pauses instances in the 'running' state, so only render
 * this for running instances. Paused instances can be resumed via Resume.
 */
export function PauseButton(props: Props) {
  const {
    instanceId,
    variant = 'default',
    size = 'default',
    className = '',
  } = props;
  const token = useToken();
  const { toast } = useToast();
  const [isLoading, setIsLoading] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);

  const handleConfirm = async () => {
    if (!token) return;

    setConfirmOpen(false);
    setIsLoading(true);
    try {
      await pauseInstance(token, instanceId);
      toast({
        title: 'Success',
        description: 'Workflow instance has been paused',
      });
    } catch (error) {
      console.error('Error pausing instance:', error);
      toast({
        title: 'Error',
        description:
          error instanceof Error && error.message
            ? error.message
            : 'Failed to pause workflow instance',
        variant: 'destructive',
      });
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <>
      <Button
        size={size}
        variant={variant}
        onClick={() => setConfirmOpen(true)}
        disabled={isLoading}
        className={className}
        title="Pause"
      >
        {isLoading ? (
          <Loader2 size={16} className={size === 'icon' ? '' : 'mr-2'} />
        ) : (
          <Pause size={16} className={size === 'icon' ? '' : 'mr-2'} />
        )}
        {size !== 'icon' && 'Pause'}
      </Button>

      <ModalDialog open={confirmOpen} onClose={() => setConfirmOpen(false)}>
        <DialogHeader>
          <DialogTitle>Pause Execution</DialogTitle>
          <DialogDescription>
            Pause this running workflow instance? It will suspend at the next
            checkpoint and can be resumed later.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="gap-2 sm:gap-0">
          <DialogClose asChild>
            <Button type="button" variant="outline">
              Cancel
            </Button>
          </DialogClose>
          <Button type="button" onClick={handleConfirm} disabled={isLoading}>
            {isLoading ? 'Pausing...' : 'Pause Instance'}
          </Button>
        </DialogFooter>
      </ModalDialog>
    </>
  );
}
