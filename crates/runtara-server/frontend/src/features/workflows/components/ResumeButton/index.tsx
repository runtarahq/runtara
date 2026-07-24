import { useState } from 'react';
import { SkipForward } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { resumeInstance } from '@/features/workflows/queries';
import { toast } from 'sonner';
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

export function ResumeButton(props: Props) {
  const {
    instanceId,
    variant = 'default',
    size = 'default',
    className = '',
  } = props;
  const token = useToken();
  const [isLoading, setIsLoading] = useState(false);

  const handleClick = async () => {
    if (!token) return;

    setIsLoading(true);
    try {
      await resumeInstance(token, instanceId);
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
    <Button
      size={size}
      variant={variant}
      onClick={handleClick}
      disabled={isLoading}
      className={className}
      title="Resume from last checkpoint"
    >
      <SkipForward size={16} className={size === 'icon' ? '' : 'mr-2'} />
      {size !== 'icon' && 'Resume'}
    </Button>
  );
}
