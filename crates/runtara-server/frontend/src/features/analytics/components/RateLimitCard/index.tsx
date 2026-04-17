import { Card } from '@/shared/components/ui/card';
import { Badge } from '@/shared/components/ui/badge';
import {
  AlertTriangle,
  Link,
  ChevronRight,
  Activity,
  Clock,
} from 'lucide-react';
import type { RateLimitStatusDto } from '@/generated/RuntaraRuntimeApi';

interface RateLimitCardProps {
  rateLimitStatus: RateLimitStatusDto;
  onClick?: () => void;
  selected?: boolean;
}

function getCapacityColor(capacityPercent: number | null | undefined): string {
  if (capacityPercent === null || capacityPercent === undefined) {
    return 'bg-muted';
  }
  if (capacityPercent > 50) {
    return 'bg-green-500';
  }
  if (capacityPercent >= 20) {
    return 'bg-yellow-500';
  }
  return 'bg-red-500';
}

function getCapacityTextColor(
  capacityPercent: number | null | undefined
): string {
  if (capacityPercent === null || capacityPercent === undefined) {
    return 'text-muted-foreground';
  }
  if (capacityPercent > 50) {
    return 'text-green-600 dark:text-green-400';
  }
  if (capacityPercent >= 20) {
    return 'text-yellow-600 dark:text-yellow-400';
  }
  return 'text-red-600 dark:text-red-400';
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

export function RateLimitCard({
  rateLimitStatus,
  onClick,
  selected,
}: RateLimitCardProps) {
  const {
    connectionTitle,
    integrationId,
    config,
    state,
    metrics,
    periodStats,
  } = rateLimitStatus;

  const hasConfig = config !== null && config !== undefined;
  const isRedisAvailable = state.available;
  const capacityPercent = metrics.capacityPercent;
  const isRateLimited = metrics.isRateLimited;

  const showLearnedLimitWarning =
    hasConfig &&
    state.learnedLimit !== null &&
    state.learnedLimit !== undefined &&
    state.learnedLimit !== config.burstSize;

  return (
    <Card
      className={`rounded-xl border bg-card p-4 shadow-none transition-all ${
        onClick ? 'cursor-pointer hover:border-primary/50' : ''
      } ${selected ? 'border-primary ring-1 ring-primary/20' : 'border-border/40'}`}
      onClick={onClick}
    >
      <div className="space-y-4">
        {/* Header */}
        <div className="flex items-start justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <Link className="h-4 w-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0">
              <h3 className="text-sm font-semibold truncate">
                {connectionTitle}
              </h3>
              {integrationId && (
                <p className="text-xs text-muted-foreground truncate">
                  {integrationId}
                </p>
              )}
            </div>
          </div>
          <div className="flex items-center gap-2">
            <Badge
              variant={isRateLimited ? 'destructive' : 'success'}
              className="shrink-0"
            >
              {isRateLimited ? 'Rate Limited' : 'OK'}
            </Badge>
            {onClick && (
              <ChevronRight
                className={`h-4 w-4 text-muted-foreground transition-transform ${selected ? 'rotate-90' : ''}`}
              />
            )}
          </div>
        </div>

        {/* Redis Unavailable Warning */}
        {!isRedisAvailable && (
          <div className="flex items-center gap-2 rounded-md bg-yellow-500/10 p-2 text-yellow-600 dark:text-yellow-400">
            <AlertTriangle className="h-4 w-4 shrink-0" />
            <span className="text-xs font-medium">Redis Unavailable</span>
          </div>
        )}

        {/* No Rate Limit Configured */}
        {!hasConfig && (
          <div className="text-sm text-muted-foreground">
            No rate limit configured
          </div>
        )}

        {/* Capacity Progress */}
        {hasConfig && (
          <div className="space-y-2">
            <div className="flex items-center justify-between text-xs">
              <span className="text-muted-foreground">Capacity</span>
              <span className={getCapacityTextColor(capacityPercent)}>
                {capacityPercent !== null && capacityPercent !== undefined
                  ? `${capacityPercent.toFixed(1)}%`
                  : 'N/A'}
              </span>
            </div>
            <div className="relative h-2 w-full overflow-hidden rounded-full bg-muted">
              <div
                className={`h-full transition-all ${getCapacityColor(capacityPercent)}`}
                style={{ width: `${capacityPercent ?? 0}%` }}
              />
            </div>

            {/* Current Tokens */}
            {state.currentTokens !== null &&
              state.currentTokens !== undefined && (
                <div className="flex items-center justify-between text-xs text-muted-foreground">
                  <span>Tokens</span>
                  <span>
                    {state.currentTokens.toFixed(1)} / {config.burstSize}
                  </span>
                </div>
              )}
          </div>
        )}

        {/* Period Stats */}
        {periodStats && (
          <div className="flex gap-4 text-xs">
            <div className="flex items-center gap-1.5">
              <Activity className="h-3.5 w-3.5 text-muted-foreground" />
              <span className="text-muted-foreground">Requests:</span>
              <span className="font-medium">
                {formatNumber(periodStats.totalRequests)}
              </span>
            </div>
            {periodStats.rateLimitedCount > 0 && (
              <div className="flex items-center gap-1.5">
                <Clock className="h-3.5 w-3.5 text-muted-foreground" />
                <span className="text-muted-foreground">Limited:</span>
                <span className="font-medium text-yellow-600 dark:text-yellow-400">
                  {formatNumber(periodStats.rateLimitedCount)}
                </span>
              </div>
            )}
          </div>
        )}

        {/* Config Details */}
        {hasConfig && (
          <div className="grid grid-cols-2 gap-2 text-xs">
            <div>
              <span className="text-muted-foreground">Rate</span>
              <p className="font-medium">{config.requestsPerSecond} req/s</p>
            </div>
            <div>
              <span className="text-muted-foreground">Burst</span>
              <p className="font-medium">{config.burstSize}</p>
            </div>
            {config.retryOnLimit && (
              <div>
                <span className="text-muted-foreground">Max Retries</span>
                <p className="font-medium">{config.maxRetries}</p>
              </div>
            )}
            {config.retryOnLimit && (
              <div>
                <span className="text-muted-foreground">Max Wait</span>
                <p className="font-medium">{config.maxWaitMs}ms</p>
              </div>
            )}
          </div>
        )}

        {/* Learned Limit Warning */}
        {showLearnedLimitWarning && (
          <div className="flex items-center gap-2 rounded-md bg-blue-500/10 p-2 text-blue-600 dark:text-blue-400">
            <AlertTriangle className="h-4 w-4 shrink-0" />
            <span className="text-xs">
              API reports limit: {state.learnedLimit}
            </span>
          </div>
        )}

        {/* Retry After */}
        {isRateLimited &&
          metrics.retryAfterMs !== null &&
          metrics.retryAfterMs !== undefined && (
            <div className="text-xs text-muted-foreground">
              Available in {metrics.retryAfterMs}ms
            </div>
          )}
      </div>
    </Card>
  );
}

export function RateLimitCardSkeleton() {
  return (
    <Card className="rounded-xl border border-border/40 bg-card p-4 shadow-none">
      <div className="space-y-4">
        <div className="flex items-start justify-between gap-2">
          <div className="flex items-center gap-2">
            <div className="h-4 w-4 rounded bg-muted animate-pulse" />
            <div className="space-y-1">
              <div className="h-4 w-32 rounded bg-muted animate-pulse" />
              <div className="h-3 w-24 rounded bg-muted animate-pulse" />
            </div>
          </div>
          <div className="h-5 w-16 rounded bg-muted animate-pulse" />
        </div>
        <div className="space-y-2">
          <div className="flex justify-between">
            <div className="h-3 w-16 rounded bg-muted animate-pulse" />
            <div className="h-3 w-12 rounded bg-muted animate-pulse" />
          </div>
          <div className="h-2 w-full rounded bg-muted animate-pulse" />
        </div>
        <div className="grid grid-cols-2 gap-2">
          <div className="space-y-1">
            <div className="h-3 w-10 rounded bg-muted animate-pulse" />
            <div className="h-4 w-16 rounded bg-muted animate-pulse" />
          </div>
          <div className="space-y-1">
            <div className="h-3 w-10 rounded bg-muted animate-pulse" />
            <div className="h-4 w-12 rounded bg-muted animate-pulse" />
          </div>
        </div>
      </div>
    </Card>
  );
}
