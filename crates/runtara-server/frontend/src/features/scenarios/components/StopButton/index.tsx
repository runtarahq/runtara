import { useState } from 'react';
import { Square } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { stopInstance } from '@/features/scenarios/queries';
import { useToast } from '@/shared/hooks/useToast';
import { useToken } from '@/shared/hooks';

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

export function StopButton(props: Props) {
  const {
    instanceId,
    variant = 'default',
    size = 'default',
    className = '',
  } = props;
  const token = useToken();
  const { toast } = useToast();
  const [isLoading, setIsLoading] = useState(false);

  const handleClick = async () => {
    if (!token) return;

    setIsLoading(true);
    try {
      await stopInstance(token, instanceId);
      toast({
        title: 'Success',
        description: 'Scenario instance has been stopped',
      });
    } catch (error) {
      console.error('Error stopping instance:', error);
      toast({
        title: 'Error',
        description: 'Failed to stop scenario instance',
        variant: 'destructive',
      });
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Button
      size={size}
      variant={variant}
      onClick={handleClick}
      disabled={isLoading}
      className={className}
      title="Stop"
    >
      <Square size={16} className={size === 'icon' ? '' : 'mr-2'} />
      {size !== 'icon' && 'Stop'}
    </Button>
  );
}
