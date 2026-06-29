import { useFormContext, useWatch } from 'react-hook-form';
import { Link } from 'react-router';
import { Gauge, ShieldOff, ExternalLink } from 'lucide-react';
import { FormSection } from './FormSection';
import { TextInput } from '@/shared/components/text-input';
import { CheckboxInput } from '@/shared/components/checkbox-input';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import {
  RateLimitConfigDto,
  RateLimitStatusDto,
} from '@/generated/RuntaraRuntimeApi';
import { rateLimitBadgeFromStatus } from '@/shared/lib/rate-limit-status';

type RateLimitSectionProps = {
  defaultConfig?: RateLimitConfigDto | null;
  /** Live rate-limit status for this connection (edit mode), if loaded. */
  liveStatus?: RateLimitStatusDto | null;
};

/** Conservative fallback used by "Set a safe limit" when no provider default exists. */
const SAFE_FLOOR: RateLimitConfigDto = {
  requestsPerSecond: 10,
  burstSize: 20,
  retryOnLimit: true,
  maxRetries: 3,
  maxWaitMs: 60000,
};

export function RateLimitSection({
  defaultConfig,
  liveStatus,
}: RateLimitSectionProps) {
  const { setValue } = useFormContext();
  const rateLimitEnabled = useWatch({ name: 'rateLimitEnabled' });

  const liveBadge = liveStatus ? rateLimitBadgeFromStatus(liveStatus) : null;

  const applySafeLimit = () => {
    const cfg = defaultConfig ?? SAFE_FLOOR;
    setValue('rateLimitEnabled', true, { shouldDirty: true });
    setValue('requestsPerSecond', cfg.requestsPerSecond, { shouldDirty: true });
    setValue('burstSize', cfg.burstSize, { shouldDirty: true });
    setValue('maxRetries', cfg.maxRetries, { shouldDirty: true });
    setValue('maxWaitMs', cfg.maxWaitMs, { shouldDirty: true });
    setValue('retryOnLimit', cfg.retryOnLimit, { shouldDirty: true });
  };

  return (
    <FormSection title="Rate Limiting" icon={Gauge} optional>
      <div className="space-y-4">
        {/* Live protection state (edit mode) */}
        {liveBadge && (
          <div className="flex items-center gap-2">
            <Badge variant={liveBadge.variant} title={liveBadge.description}>
              {liveBadge.label}
            </Badge>
            <span className="text-xs text-muted-foreground">
              {liveBadge.description}
            </span>
          </div>
        )}

        <CheckboxInput
          name="rateLimitEnabled"
          label={
            defaultConfig ? 'Override default rate limits' : 'Enable rate limiting'
          }
        />

        {!rateLimitEnabled && defaultConfig && (
          <div className="text-xs text-slate-500 bg-slate-50 rounded-md px-3 py-2 dark:bg-slate-800/50 dark:text-slate-400">
            Using defaults: {defaultConfig.requestsPerSecond} req/s, burst{' '}
            {defaultConfig.burstSize},{' '}
            {defaultConfig.retryOnLimit
              ? `auto-retry enabled (max ${defaultConfig.maxRetries} retries, ${defaultConfig.maxWaitMs}ms wait)`
              : 'no auto-retry'}
          </div>
        )}

        {!rateLimitEnabled && !defaultConfig && (
          <div className="flex flex-col gap-2 rounded-md border border-warning/30 bg-warning/10 px-3 py-2.5">
            <div className="flex items-start gap-2 text-xs text-warning">
              <ShieldOff className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span>
                <span className="font-medium">No rate limiting is applied.</span>{' '}
                Requests to this connection are unlimited — bursts can trigger
                provider 429s, key bans, or runaway cost. Enable a limit to
                protect this connection.
              </span>
            </div>
            <div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={applySafeLimit}
              >
                Set a safe limit
              </Button>
            </div>
          </div>
        )}

        {rateLimitEnabled && (
          <div className="grid grid-cols-2 gap-4">
            <TextInput
              name="requestsPerSecond"
              label="Requests per second"
              type="number"
              placeholder={defaultConfig?.requestsPerSecond?.toString()}
              description="Token refill rate"
            />
            <TextInput
              name="burstSize"
              label="Burst size"
              type="number"
              placeholder={defaultConfig?.burstSize?.toString()}
              description="Maximum token capacity"
            />
            <TextInput
              name="maxRetries"
              label="Max retries"
              type="number"
              placeholder={defaultConfig?.maxRetries?.toString()}
              description="Maximum retry attempts"
            />
            <TextInput
              name="maxWaitMs"
              label="Max wait (ms)"
              type="number"
              placeholder={defaultConfig?.maxWaitMs?.toString()}
              description="Maximum cumulative wait time"
            />
            <div className="col-span-2">
              <CheckboxInput
                name="retryOnLimit"
                label="Auto-retry on rate limit"
              />
            </div>
          </div>
        )}

        {/* Cross-link to the live activity dashboard */}
        <Link
          to="/analytics/rate-limits"
          className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
        >
          <ExternalLink className="h-3 w-3" />
          View live rate-limit activity
        </Link>
      </div>
    </FormSection>
  );
}
