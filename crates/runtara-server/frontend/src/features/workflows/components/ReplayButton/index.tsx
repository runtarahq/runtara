import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { CirclePlay, RotateCcw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';
import { WithTooltip } from '@/shared/components/ui/tooltip.tsx';
import { replayWorkflow } from '@/features/workflows/queries';
import { toast } from 'sonner';
import { useToken } from '@/shared/hooks';
import { queryKeys } from '@/shared/queries/query-keys';
import { isOidcAuth } from '@/shared/config/runtimeConfig';
import {
  shouldShowRetryButton,
  getRetryDelay,
  parseStructuredError,
} from '@/shared/utils/structured-error';

type ReplayWorkflowResult = {
  workflowId?: string;
};

type Props = {
  instanceId: string;
  /** Optional error string to enable smart retry logic */
  error?: string | null;
  variant?: 'primary' | 'secondary' | 'secondaryDestructive' | 'destructive';
  size?: 'default' | 'sm' | 'lg' | 'icon';
  className?: string;
};

export function ReplayButton(props: Props) {
  const {
    instanceId,
    error,
    variant = 'primary',
    size = 'default',
    className = '',
  } = props;
  const token = useToken();
  const queryClient = useQueryClient();
  const [isLoading, setIsLoading] = useState(false);

  // Check if error is transient for smart retry logic
  const isTransient = shouldShowRetryButton(error);
  const structuredError = parseStructuredError(error || '');
  const retryDelay = getRetryDelay(error || '');

  const handleClick = async () => {
    if (isOidcAuth && !token) return;

    setIsLoading(true);
    try {
      const result = (await replayWorkflow(
        token,
        instanceId
      )) as ReplayWorkflowResult;
      const replayedWorkflowId = result?.workflowId;

      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: queryKeys.executions.lists(),
        }),
        replayedWorkflowId
          ? queryClient.invalidateQueries({
              queryKey: queryKeys.workflows.instances(replayedWorkflowId),
            })
          : queryClient.invalidateQueries({
              queryKey: queryKeys.workflows.details(),
            }),
      ]);

      toast.success(
        isTransient
          ? 'Workflow retry has been scheduled'
          : 'Workflow has been scheduled for replay'
      );
    } catch (error) {
      console.error('Error replaying workflow:', error);
      toast.error('Failed to replay workflow');
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
    <WithTooltip label={tooltipText}>
      <Button
        size={size}
        variant={variant}
        onClick={handleClick}
        disabled={isLoading}
        className={className}
        aria-label={tooltipText}
      >
        <ButtonIcon size={16} className={size === 'icon' ? '' : 'mr-2'} />
        {buttonLabel}
      </Button>
    </WithTooltip>
  );
}
