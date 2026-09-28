import { useState } from 'react';
import { SkipForward } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { WithTooltip } from '@/shared/components/ui/tooltip.tsx';
import { resumeInstance } from '@/features/workflows/queries';
import { toast } from 'sonner';
import { useToken } from '@/shared/hooks';
import { useQueryClient } from '@tanstack/react-query';
import { refreshRunViews } from '../refreshRunViews';
import { isOidcAuth } from '@/shared/config/runtimeConfig';

type Props = {
  instanceId: string;
  variant?: 'primary' | 'secondary' | 'secondaryDestructive' | 'destructive';
  size?: 'default' | 'sm' | 'lg' | 'icon';
  className?: string;
};

export function ResumeButton(props: Props) {
  const {
    instanceId,
    variant = 'primary',
    size = 'default',
    className = '',
  } = props;
  const token = useToken();
  const queryClient = useQueryClient();
  const [isLoading, setIsLoading] = useState(false);

  const handleClick = async () => {
    // Local and trusted-header auth modes send no bearer token; only OIDC needs one.
    if (isOidcAuth && !token) {
      toast.error(
        'Your session has expired. Sign in again to resume this run.'
      );
      return;
    }

    setIsLoading(true);
    try {
      await resumeInstance(token, instanceId);
      await refreshRunViews(queryClient);
      toast.success('Execution resumed from last checkpoint');
    } catch (error) {
      console.error('Error resuming instance:', error);
      toast.error(
        'Failed to resume execution. The instance may have no checkpoint to resume from.'
      );
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <WithTooltip label="Resume from last checkpoint">
      <Button
        size={size}
        variant={variant}
        onClick={handleClick}
        disabled={isLoading}
        className={className}
        aria-label="Resume from last checkpoint"
      >
        <SkipForward size={16} className={size === 'icon' ? '' : 'mr-2'} />
        {size !== 'icon' && 'Resume'}
      </Button>
    </WithTooltip>
  );
}
