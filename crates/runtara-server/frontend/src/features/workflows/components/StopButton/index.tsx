import { useState } from 'react';
import { Square } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { WithTooltip } from '@/shared/components/ui/tooltip.tsx';
import { stopInstance } from '@/features/workflows/queries';
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

export function StopButton(props: Props) {
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
      toast.error('Your session has expired. Sign in again to stop this run.');
      return;
    }

    setIsLoading(true);
    try {
      await stopInstance(token, instanceId);
      await refreshRunViews(queryClient);
      toast.success('Workflow instance has been stopped');
    } catch (error) {
      console.error('Error stopping instance:', error);
      toast.error('Failed to stop workflow instance');
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <WithTooltip label="Stop">
      <Button
        size={size}
        variant={variant}
        onClick={handleClick}
        disabled={isLoading}
        className={className}
        aria-label="Stop"
      >
        <Square size={16} className={size === 'icon' ? '' : 'mr-2'} />
        {size !== 'icon' && 'Stop'}
      </Button>
    </WithTooltip>
  );
}
