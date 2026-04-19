import { useState } from 'react';
import { CirclePlay, RotateCcw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { replayWorkflow } from '@/features/workflows/queries';
import { useToast } from '@/shared/hooks/useToast';
import { useToken } from '@/shared/hooks';
import {
  shouldShowRetryButton,
  getRetryDelay,
  parseStructuredError,
} from '@/shared/utils/structured-error';

type Props = {
  instanceId: string;
  /** Optional error string to enable smart retry logic */
  error?: string | null;
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

export function ReplayButton(props: Props) {
  const {
    instanceId,
    error,
    variant = 'default',
    size = 'default',
    className = '',
  } = props;
  const token = useToken();
  const { toast } = useToast();
  const [isLoading, setIsLoading] = useState(false);

  // Check if error is transient for smart retry logic
  const isTransient = shouldShowRetryButton(error);
  const structuredError = parseStructuredError(error || '');
  const retryDelay = getRetryDelay(error || '');

  const handleClick = async () => {
    if (!token) return;

    setIsLoading(true);
    try {
      await replayWorkflow(token, instanceId);
      toast({
        title: 'Success',
        description: isTransient
          ? 'Workflow retry has been scheduled'
          : 'Workflow has been scheduled for replay',
      });
    } catch (error) {
      console.error('Error replaying workflow:', error);
      toast({
        title: 'Error',
        description: 'Failed to replay workflow',
        variant: 'destructive',
      });
    } finally {
      setIsLoading(false);
    }
  };

  // Determine button label and tooltip
  const buttonLabel = size !== 'icon' ? (isTransient ? 'Retry' : 'Replay') : '';
  const buttonIcon = isTransient ? RotateCcw : CirclePlay;
  const ButtonIcon = buttonIcon;

  let tooltipText = isTransient ? 'Retry (transient error)' : 'Replay';
  if (structuredError && retryDelay) {
    const delaySec = Math.round(retryDelay / 1000);
    tooltipText += ` - Suggested delay: ${delaySec}s`;
  }

  return (
    <Button
      size={size}
      variant={variant}
      onClick={handleClick}
      disabled={isLoading}
      className={className}
      title={tooltipText}
    >
      <ButtonIcon size={16} className={size === 'icon' ? '' : 'mr-2'} />
      {buttonLabel}
    </Button>
  );
}
