import { Loader2, RefreshCw } from 'lucide-react';

import { Button } from '@/shared/components/ui/button';

export function ReconnectPromptNotice({
  provider,
  changedFieldLabels,
  isReconnecting,
  onReconnectNow,
  onLater,
}: {
  provider: string;
  changedFieldLabels: readonly string[];
  isReconnecting: boolean;
  onReconnectNow: () => void;
  onLater: () => void;
}) {
  const reason =
    changedFieldLabels.length > 0
      ? `Because ${changedFieldLabels.join(', ')} changed, the stored authorization was reset.`
      : 'Because the credentials changed, the stored authorization was reset.';
  return (
    <div
      className="rounded-lg border border-blue-300 bg-blue-50 p-4 text-blue-950 dark:border-blue-700 dark:bg-blue-950/30 dark:text-blue-100"
      role="alert"
    >
      <div className="flex items-start gap-3">
        <RefreshCw className="mt-0.5 h-5 w-5 shrink-0" />
        <div className="min-w-0 flex-1 space-y-2">
          <div>
            <p className="font-medium">Saved — reconnection needed</p>
            <p className="text-sm">
              Your changes are saved. {reason} Workflows using this connection
              will fail until you reconnect with {provider}.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              size="sm"
              onClick={onReconnectNow}
              disabled={isReconnecting}
            >
              {isReconnecting ? (
                <Loader2 className="mr-1.5 h-4 w-4 animate-spin" />
              ) : (
                <RefreshCw className="mr-1.5 h-4 w-4" />
              )}
              Reconnect now
            </Button>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={onLater}
              disabled={isReconnecting}
            >
              Later
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
